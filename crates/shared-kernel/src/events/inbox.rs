//! Port of `InboxGuard`: apply an event at most once per handler.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::events::HandlerError;
use crate::hosting::BoxFuture;
use crate::persistence::{DbConnection, DbPool};

/// Runs `handle` inside a transaction that first records `(event_id, handler_name)`.
///
/// 1. `BEGIN` on the handler module's pool.
/// 2. `INSERT … ON CONFLICT DO NOTHING` into `<schema>."InboxMessage"`.
/// 3. No row inserted: the event was handled before. Roll back, return `false`.
/// 4. Otherwise run the handler on the same transaction and commit, so its
///    writes and the inbox row land together; return `true`.
///
/// A handler failure rolls the inbox row back with everything else, so the
/// next delivery runs it again.
///
/// # Errors
///
/// The handler's failure, or the database's.
pub async fn run<F>(
    pool: &DbPool,
    schema: &str,
    event_id: Uuid,
    handler_name: &str,
    now: DateTime<Utc>,
    handle: F,
) -> Result<bool, HandlerError>
where
    F: for<'c> FnOnce(&'c mut DbConnection) -> BoxFuture<'c, Result<(), HandlerError>>,
{
    let mut transaction = pool.begin().await?;

    let inserted = sqlx::query(&format!(
        r#"INSERT INTO "{schema}"."InboxMessage" ("EventId", "HandlerName", "ProcessedAt") VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"#
    ))
    .bind(event_id)
    .bind(handler_name)
    .bind(now)
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    if inserted == 0 {
        transaction.rollback().await?;
        return Ok(false);
    }

    handle(&mut transaction).await?;
    transaction.commit().await?;

    Ok(true)
}
