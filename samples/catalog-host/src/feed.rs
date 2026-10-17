//! Orders' outbox, read by cursor. Port of `OrdersOutboxFeed` (ADR-0017).
//!
//! In the monolith, Orders' dispatcher delivers `InvoiceFinalized` to Catalog.
//! Here Orders is not running, and taking rows from its outbox would starve the
//! monolith's own dispatcher of them — Administration still needs every one.
//! So this host only reads: it keeps a cursor of its own,
//! `catalog_host.orders_outbox_cursor`, walks the outbox in `(OccurredAt, Id)`
//! order past it, and hands each event to Catalog's handler through Catalog's
//! inbox under the monolith's handler name. An event either process applied
//! is applied once.
//!
//! - Rows newer than the settle window (`SettleSeconds`, default 5) wait: a
//!   row whose transaction committed late could otherwise land behind the
//!   cursor and be skipped.
//! - A row whose type Catalog does not handle is passed over and moves the
//!   cursor (P21).
//! - A handler failure stops the batch and leaves the cursor before the row,
//!   so the next poll tries it again. There is no backoff and no dead letter.
//! - The cursor moves in a statement of its own, after the inbox transaction:
//!   at least once, made idempotent by the inbox.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use orders_contracts::events::InvoiceFinalized;
use shared_kernel::AppConfig;
use shared_kernel::clock::Clock;
use shared_kernel::events::{EventDecoder, EventSubscriptions, IntegrationEvent, inbox};
use shared_kernel::hosting::{BoxFuture, HostedService};
use shared_kernel::persistence::ModuleDatabase;
use sqlx::Row;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Rows looked at per batch.
pub const BATCH_SIZE: i64 = 50;

/// The module whose handlers the feed delivers to.
pub const MODULE_KEY: &str = "Catalog";

/// `CatalogHost:Feed:*`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeedOptions {
    /// `Enabled`, default true.
    pub enabled: bool,
    /// `PollIntervalSeconds`, default 1.
    pub poll_interval: Duration,
    /// `SettleSeconds`, default 5.
    pub settle: Duration,
}

impl FeedOptions {
    /// Reads the section with upstream's defaults.
    #[must_use]
    pub fn from_config(config: &AppConfig) -> Self {
        let seconds = |key: &str, default: f64| {
            Duration::from_secs_f64(
                Some(config.get_or(&format!("CatalogHost:Feed:{key}"), default))
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .unwrap_or(default),
            )
        };

        Self {
            enabled: config.get_or("CatalogHost:Feed:Enabled", true),
            poll_interval: seconds("PollIntervalSeconds", 1.0),
            settle: seconds("SettleSeconds", 5.0),
        }
    }
}

/// Why a batch stopped early.
#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    /// The database refused.
    #[error("the feed's database work failed")]
    Database(#[from] sqlx::Error),
    /// A payload could not be read as its event type.
    #[error("cannot read the payload of {event_id}: {reason}")]
    Payload {
        /// The outbox row.
        event_id: Uuid,
        /// The parser's message.
        reason: String,
    },
}

struct Inner {
    database: ModuleDatabase,
    subscriptions: EventSubscriptions,
    decoders: Vec<EventDecoder>,
    clock: Arc<dyn Clock>,
    options: FeedOptions,
}

/// Reads Orders' outbox by cursor and delivers to Catalog.
#[derive(Clone)]
pub struct OrdersOutboxFeed(Arc<Inner>);

impl std::fmt::Debug for OrdersOutboxFeed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OrdersOutboxFeed")
            .field("options", &self.0.options)
            .finish_non_exhaustive()
    }
}

impl OrdersOutboxFeed {
    /// A feed over Catalog's `database`, delivering to Catalog's subscriptions.
    #[must_use]
    pub fn new(
        database: ModuleDatabase,
        subscriptions: EventSubscriptions,
        clock: Arc<dyn Clock>,
        options: FeedOptions,
    ) -> Self {
        Self(Arc::new(Inner {
            database,
            subscriptions,
            decoders: vec![EventDecoder::of::<InvoiceFinalized>()],
            clock,
            options,
        }))
    }

    async fn ensure_cursor(&self) -> Result<(), sqlx::Error> {
        sqlx::raw_sql(
            "CREATE SCHEMA IF NOT EXISTS catalog_host;
             CREATE TABLE IF NOT EXISTS catalog_host.orders_outbox_cursor (
                 id int PRIMARY KEY, occurred_at timestamptz NOT NULL, event_id uuid NOT NULL);
             INSERT INTO catalog_host.orders_outbox_cursor VALUES (1, '-infinity', '00000000-0000-0000-0000-000000000000')
             ON CONFLICT (id) DO NOTHING;",
        )
        .execute(self.0.database.pool())
        .await?;
        Ok(())
    }

    /// Delivers one batch, returning how many rows the cursor moved past.
    ///
    /// # Errors
    ///
    /// The database refusing, or a payload that cannot be read. Either leaves
    /// the cursor where the failing row is.
    pub async fn process_batch(&self) -> Result<usize, FeedError> {
        let inner = &self.0;
        let pool = inner.database.pool();
        self.ensure_cursor().await?;

        let settled = inner.clock.now()
            - TimeDelta::from_std(inner.options.settle).unwrap_or(TimeDelta::zero());

        // Compared against the cursor in SQL: its starting `-infinity` has no
        // chrono counterpart to read it into.
        let rows = sqlx::query(&format!(
            r#"SELECT o."Id", o."EventType", o."Payload"::text AS "Payload", o."OccurredAt"
                 FROM orders."OutboxMessage" o, catalog_host.orders_outbox_cursor c
                WHERE c.id = 1 AND (o."OccurredAt", o."Id") > (c.occurred_at, c.event_id) AND o."OccurredAt" <= $1
                ORDER BY o."OccurredAt", o."Id" LIMIT {BATCH_SIZE}"#
        ))
        .bind(settled)
        .fetch_all(pool)
        .await?;

        let mut moved = 0;
        let catalog = inner.subscriptions.for_module(MODULE_KEY);

        for row in &rows {
            let id: Uuid = row.try_get("Id")?;
            let event_type: String = row.try_get("EventType")?;
            let payload: String = row.try_get("Payload")?;
            let occurred_at: DateTime<Utc> = row.try_get("OccurredAt")?;

            let handlers: Vec<_> = catalog
                .iter()
                .filter(|subscription| subscription.event_type == event_type)
                .collect();
            let decoder = inner
                .decoders
                .iter()
                .find(|decoder| decoder.event_type == event_type);

            if let (false, Some(decoder)) = (handlers.is_empty(), decoder) {
                let event = decoder
                    .decode(&payload)
                    .map_err(|reason| FeedError::Payload {
                        event_id: id,
                        reason,
                    })?;

                for subscription in handlers {
                    let handler = subscription.clone();
                    let event = Arc::clone(&event);
                    let outcome = inbox::run(
                        pool,
                        inner.database.schema(),
                        id,
                        subscription.handler_name,
                        inner.clock.now(),
                        move |connection| {
                            Box::pin(
                                async move { handler.handle(event.as_ref(), connection).await },
                            )
                        },
                    )
                    .await;

                    if let Err(error) = outcome {
                        tracing::warn!(
                            event_id = 3002,
                            %error,
                            "Handler {} failed for event {id}; the feed will retry it",
                            subscription.handler_name
                        );
                        return Ok(moved);
                    }
                }
            }

            sqlx::query("UPDATE catalog_host.orders_outbox_cursor SET occurred_at = $1, event_id = $2 WHERE id = 1")
                .bind(occurred_at)
                .bind(id)
                .execute(pool)
                .await?;
            moved += 1;
        }

        Ok(moved)
    }
}

impl HostedService for OrdersOutboxFeed {
    fn name(&self) -> String {
        "OrdersOutboxFeed".to_owned()
    }

    fn execute(&self, stopping: CancellationToken) -> BoxFuture<'static, ()> {
        let feed = self.clone();

        Box::pin(async move {
            if !feed.0.options.enabled {
                return;
            }

            while !stopping.is_cancelled() {
                let moved = match feed.process_batch().await {
                    Ok(moved) => moved,
                    Err(error) => {
                        tracing::error!(event_id = 3001, %error, "Orders outbox feed poll failed");
                        0
                    }
                };

                if moved < BATCH_SIZE as usize {
                    tokio::select! {
                        () = stopping.cancelled() => return,
                        () = tokio::time::sleep(feed.0.options.poll_interval) => {}
                    }
                }
            }
        })
    }
}

/// The event type the feed decodes, for documentation and tests.
pub const DELIVERED_EVENT: &str = InvoiceFinalized::EVENT_TYPE;
