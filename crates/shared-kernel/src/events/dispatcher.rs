//! Port of `OutboxDispatcher` and `OutboxDispatcher<TContext>`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use sqlx::Row;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::AppConfig;
use crate::clock::Clock;
use crate::concurrency::ModuleWorkQueue;
use crate::diagnostics::ModuleMeter;
use crate::events::{EventDecoder, EventSubscriptions, IntegrationEvent, ModuleDatabases, inbox};
use crate::hosting::{BoxFuture, HostedService};
use crate::persistence::{DbTransaction, ModuleDatabase};

/// Rows looked at per poll.
pub const BATCH_SIZE: usize = 50;

/// The wait after each failed delivery. The sixth failure dead-letters the row.
pub const RETRY_DELAYS: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(120),
    Duration::from_secs(600),
];

/// The longest `LastError` kept, as `varchar(2000)` allows.
pub const MAX_ERROR_LENGTH: usize = 2000;

/// `Outbox:*` configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutboxOptions {
    /// `Outbox:Enabled`, default `true`. Off, the hosted loop returns at once
    /// but [`OutboxDispatcher::process_batch`] still works.
    pub enabled: bool,
    /// `Outbox:PollIntervalSeconds`, default `1`. Read once at startup.
    pub poll_interval: Duration,
}

impl OutboxOptions {
    /// Reads the section, with upstream's defaults.
    #[must_use]
    pub fn from_config(config: &AppConfig) -> Self {
        let enabled = config
            .get_string("Outbox:Enabled")
            .and_then(|value| value.parse::<bool>().ok())
            .unwrap_or(true);
        let seconds = config
            .get_string("Outbox:PollIntervalSeconds")
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            .unwrap_or(1.0);

        Self {
            enabled,
            poll_interval: Duration::from_secs_f64(seconds),
        }
    }
}

struct Inner {
    name: String,
    database: ModuleDatabase,
    decoders: BTreeMap<&'static str, EventDecoder>,
    subscriptions: EventSubscriptions,
    databases: ModuleDatabases,
    queue: Arc<ModuleWorkQueue>,
    meter: Arc<ModuleMeter>,
    clock: Arc<dyn Clock>,
    options: OutboxOptions,
}

/// Polls one module's outbox and delivers what it finds.
#[derive(Clone)]
pub struct OutboxDispatcher(Arc<Inner>);

impl std::fmt::Debug for OutboxDispatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OutboxDispatcher")
            .field("name", &self.0.name)
            .field("schema", &self.0.database.schema())
            .finish_non_exhaustive()
    }
}

/// Everything a dispatcher reads from, gathered once.
#[derive(Debug, Clone)]
pub struct DispatcherParts {
    /// The publishing module's database, whose outbox this polls.
    pub database: ModuleDatabase,
    /// Every module's subscriptions.
    pub subscriptions: EventSubscriptions,
    /// Every module's database, for the handlers' inboxes.
    pub databases: ModuleDatabases,
    /// The module's work queue, which every batch runs through.
    pub queue: Arc<ModuleWorkQueue>,
    /// The module's meter.
    pub meter: Arc<ModuleMeter>,
    /// The clock.
    pub clock: Arc<dyn Clock>,
    /// `Outbox:*`.
    pub options: OutboxOptions,
}

/// One row the dispatcher picked up.
struct OutboxRow {
    id: Uuid,
    event_type: String,
    payload: String,
    attempts: i32,
}

impl OutboxDispatcher {
    /// A dispatcher named `name`, such as `OrdersOutboxDispatcher`, for the
    /// event types in `publishes`.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        parts: DispatcherParts,
        publishes: &[EventDecoder],
    ) -> Self {
        Self(Arc::new(Inner {
            name: name.into(),
            database: parts.database,
            decoders: publishes
                .iter()
                .map(|decoder| (decoder.event_type, *decoder))
                .collect(),
            subscriptions: parts.subscriptions,
            databases: parts.databases,
            queue: parts.queue,
            meter: parts.meter,
            clock: parts.clock,
            options: parts.options,
        }))
    }

    /// The decoder list for one event type, for [`OutboxDispatcher::new`].
    #[must_use]
    pub fn publishes<E: IntegrationEvent>() -> EventDecoder {
        EventDecoder::of::<E>()
    }

    /// Delivers one batch and returns how many rows it looked at.
    ///
    /// Port of `ProcessBatchAsync`. The rows are locked with `FOR UPDATE SKIP
    /// LOCKED` until the batch commits, so a second dispatcher on the same
    /// table skips them. Each handler runs in a transaction of its own, on its
    /// own module's database, inside this one's window: a handler's commit is
    /// not atomic with the row update, and the inbox absorbs the redelivery if
    /// this commit fails.
    ///
    /// # Errors
    ///
    /// The database refusing the batch's own reads and writes. A handler's
    /// failure is not an error here: it is recorded on the row.
    pub async fn process_batch(&self) -> Result<usize, sqlx::Error> {
        let inner = &self.0;
        let schema = inner.database.schema();
        let mut transaction = inner.database.pool().begin().await?;

        let rows = sqlx::query(&format!(
            r#"SELECT "Id", "EventType", "Payload"::text AS "Payload", "Attempts" FROM "{schema}"."OutboxMessage"
               WHERE "ProcessedAt" IS NULL AND "DeadLetteredAt" IS NULL AND "NextAttemptAt" <= $1
               ORDER BY "OccurredAt", "Id" LIMIT {BATCH_SIZE} FOR UPDATE SKIP LOCKED"#
        ))
        .bind(inner.clock.now())
        .fetch_all(&mut *transaction)
        .await?;

        let batch: Vec<OutboxRow> = rows
            .iter()
            .map(|row| {
                Ok(OutboxRow {
                    id: row.try_get("Id")?,
                    event_type: row.try_get("EventType")?,
                    payload: row.try_get("Payload")?,
                    attempts: row.try_get("Attempts")?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()?;

        for message in &batch {
            let error = self.deliver(message).await;
            let now = inner.clock.now();

            match error {
                None => {
                    sqlx::query(&format!(
                        r#"UPDATE "{schema}"."OutboxMessage" SET "ProcessedAt" = $2 WHERE "Id" = $1"#
                    ))
                    .bind(message.id)
                    .bind(now)
                    .execute(&mut *transaction)
                    .await?;
                    inner.meter.dispatched();
                }
                Some(error) => {
                    self.record_failure(&mut transaction, message, &error, now)
                        .await?;
                }
            }
        }

        transaction.commit().await?;

        Ok(batch.len())
    }

    async fn record_failure(
        &self,
        transaction: &mut DbTransaction,
        message: &OutboxRow,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        let inner = &self.0;
        let schema = inner.database.schema();
        let attempts = message.attempts + 1;
        let last_error = truncate(error, MAX_ERROR_LENGTH);

        // `attempts` counts failures, so the sixth exceeds the five delays.
        let retry = usize::try_from(attempts - 1)
            .ok()
            .and_then(|index| RETRY_DELAYS.get(index));

        match retry {
            Some(delay) => {
                let next = now + TimeDelta::from_std(*delay).unwrap_or(TimeDelta::MAX);
                sqlx::query(&format!(
                    r#"UPDATE "{schema}"."OutboxMessage" SET "Attempts" = $2, "LastError" = $3, "NextAttemptAt" = $4 WHERE "Id" = $1"#
                ))
                .bind(message.id)
                .bind(attempts)
                .bind(&last_error)
                .bind(next)
                .execute(&mut **transaction)
                .await?;
                inner.meter.retried();
            }
            None => {
                sqlx::query(&format!(
                    r#"UPDATE "{schema}"."OutboxMessage" SET "Attempts" = $2, "LastError" = $3, "DeadLetteredAt" = $4 WHERE "Id" = $1"#
                ))
                .bind(message.id)
                .bind(attempts)
                .bind(&last_error)
                .bind(now)
                .execute(&mut **transaction)
                .await?;
                inner.meter.dead_lettered();
                tracing::error!(
                    event_id = 2003,
                    outbox_event_id = %message.id,
                    event_type = message.event_type,
                    attempts,
                    "Outbox message {} ({}) dead-lettered after {} failed deliveries",
                    message.id,
                    message.event_type,
                    attempts
                );
            }
        }

        Ok(())
    }

    /// Hands one row to every subscribed handler. `None` when all succeeded.
    async fn deliver(&self, message: &OutboxRow) -> Option<String> {
        let inner = &self.0;

        let Some(decoder) = inner.decoders.get(message.event_type.as_str()) else {
            return Some(format!("Unknown event type '{}'.", message.event_type));
        };

        let event = match decoder.decode(&message.payload) {
            Ok(event) => event,
            Err(reason) => return Some(format!("Cannot read payload: {reason}")),
        };

        let mut failures = Vec::new();

        for subscription in inner.subscriptions.for_event(&message.event_type) {
            let Some(database) = inner.databases.get(subscription.module_key) else {
                failures.push(format!(
                    "{}: no database is registered for module '{}'.",
                    subscription.handler_name, subscription.module_key
                ));
                continue;
            };

            let event = Arc::clone(&event);
            let handler = subscription.clone();
            let outcome = inbox::run(
                database.pool(),
                database.schema(),
                message.id,
                subscription.handler_name,
                inner.clock.now(),
                move |connection| {
                    Box::pin(async move { handler.handle(event.as_ref(), connection).await })
                },
            )
            .await;

            if let Err(error) = outcome {
                let reason = base_message(error.as_ref());
                tracing::warn!(
                    event_id = 2002,
                    handler = subscription.handler_name,
                    outbox_event_id = %message.id,
                    %reason,
                    "Handler {} failed for event {}",
                    subscription.handler_name,
                    message.id
                );
                failures.push(format!("{}: {reason}", subscription.handler_name));
            }
        }

        (!failures.is_empty()).then(|| failures.join("\n"))
    }
}

impl HostedService for OutboxDispatcher {
    fn name(&self) -> String {
        self.0.name.clone()
    }

    fn execute(&self, stopping: CancellationToken) -> BoxFuture<'static, ()> {
        let dispatcher = self.clone();

        Box::pin(async move {
            if !dispatcher.0.options.enabled {
                return;
            }

            let poll = dispatcher.0.options.poll_interval;

            while !stopping.is_cancelled() {
                let batch = dispatcher.clone();
                let handled = match dispatcher
                    .0
                    .queue
                    .run(move |_| async move { batch.process_batch().await })
                    .await
                {
                    Ok(Ok(handled)) => handled,
                    Ok(Err(error)) => {
                        tracing::error!(event_id = 2001, %error, "Outbox poll failed in {}", dispatcher.0.name);
                        0
                    }
                    Err(error) => {
                        if stopping.is_cancelled() {
                            return;
                        }
                        tracing::error!(event_id = 2001, %error, "Outbox poll failed in {}", dispatcher.0.name);
                        0
                    }
                };

                // A full batch suggests more is waiting: go again at once.
                if handled < BATCH_SIZE {
                    tokio::select! {
                        () = stopping.cancelled() => return,
                        () = tokio::time::sleep(poll) => {}
                    }
                }
            }
        })
    }
}

/// Upstream's `GetBaseException().Message`: the innermost cause.
fn base_message(error: &(dyn std::error::Error + 'static)) -> String {
    let mut current = error;
    while let Some(source) = current.source() {
        current = source;
    }
    current.to_string()
}

/// Cuts `text` to at most `limit` characters.
fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schedule_is_five_delays_long() {
        let seconds: Vec<u64> = RETRY_DELAYS.iter().map(Duration::as_secs).collect();

        assert_eq!(seconds, [1, 5, 30, 120, 600]);
    }

    #[test]
    fn a_long_error_is_cut_to_the_column() {
        assert_eq!(truncate(&"x".repeat(2500), MAX_ERROR_LENGTH).len(), 2000);
    }
}
