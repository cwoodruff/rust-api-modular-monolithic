//! Port of `IEventPublisher` and `OutboxEventPublisher`.

use std::sync::Arc;

use serde_json::Value;

use crate::clock::Clock;
use crate::diagnostics::ModuleMeter;
use crate::events::IntegrationEvent;
use crate::persistence::DbConnection;

/// Writes events into one module's outbox.
#[derive(Debug, Clone)]
pub struct EventPublisher {
    schema: &'static str,
    clock: Arc<dyn Clock>,
    meter: Arc<ModuleMeter>,
}

impl EventPublisher {
    /// A publisher writing to `<schema>."OutboxMessage"`.
    #[must_use]
    pub fn new(schema: &'static str, clock: Arc<dyn Clock>, meter: Arc<ModuleMeter>) -> Self {
        Self {
            schema,
            clock,
            meter,
        }
    }

    /// Adds `event` to the outbox on the caller's transaction.
    ///
    /// The row commits when the caller commits, and not otherwise. The
    /// `published` counter moves now, before that commit — upstream's
    /// instrument says so in its description (P22).
    ///
    /// # Errors
    ///
    /// The insert failing.
    pub async fn publish<E: IntegrationEvent>(
        &self,
        event: &E,
        connection: &mut DbConnection,
    ) -> Result<(), sqlx::Error> {
        let payload =
            serde_json::to_value(event).map_err(|error| sqlx::Error::Encode(Box::new(error)))?;

        sqlx::query(&format!(
            r#"INSERT INTO "{}"."OutboxMessage" ("Id", "EventType", "Payload", "OccurredAt", "Attempts", "NextAttemptAt")
               VALUES ($1, $2, $3, $4, 0, $5)"#,
            self.schema
        ))
        .bind(event.event_id())
        .bind(E::EVENT_TYPE)
        .bind(sqlx::types::Json::<Value>(payload))
        .bind(event.occurred_at())
        .bind(self.clock.now())
        .execute(&mut *connection)
        .await?;

        self.meter.published();

        Ok(())
    }
}
