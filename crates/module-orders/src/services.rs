//! Orders' services, ported from `Orders.Module/Services`.
//!
//! Each reads its module's own tables through its own pool — no repository
//! layer — and caches reads cache-aside for twenty minutes under upstream's
//! keys and tags. Finalization writes the invoice and its outbox row in one
//! transaction, so `InvoiceFinalized` exists exactly when the status changed.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orders_contracts::events::{InvoiceFinalized, InvoiceFinalizedLine};
use rust_decimal::Decimal;
use shared_kernel::DataResult;
use shared_kernel::caching::{CacheEntryOptions, CacheFacade, CacheKeyComposer};
use shared_kernel::clock::Clock;
use shared_kernel::events::EventPublisher;
use shared_kernel::persistence::DbPool;
use sqlx::FromRow;
use uuid::Uuid;

use crate::models::{DeadLetter, InvoiceApiModel, InvoiceLineApiModel, InvoiceStatus};

/// The cache module segment.
const MODULE: &str = "orders";
/// The schema version every key carries.
const VERSION: &str = "v1";

const INVOICE_TAGS: [&str; 2] = ["orders:invoice", "orders:invoice:by-id"];
const INVOICE_LINE_TAGS: [&str; 2] = ["orders:invoiceline", "orders:invoiceline:by-id"];

/// What every Orders service holds: the pool, the module's cache, the key composer.
#[derive(Debug, Clone)]
pub(crate) struct Store {
    pub pool: DbPool,
    pub cache: Arc<CacheFacade>,
    pub keys: Arc<CacheKeyComposer>,
}

impl Store {
    /// Reads `entity:discriminator` through the cache.
    async fn cached<T, F, Fut>(
        &self,
        entity: &str,
        discriminator: &str,
        tags: [&str; 2],
        load: F,
    ) -> DataResult<Option<T>>
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = DataResult<Option<T>>> + Send,
    {
        let key = self.keys.compose(MODULE, entity, VERSION, discriminator);
        self.cache
            .try_get_or_add(&key, load, Some(CacheEntryOptions::for_service(tags)))
            .await
    }

    /// The same for a list, which is never absent.
    async fn cached_list<T, F, Fut>(
        &self,
        entity: &str,
        discriminator: &str,
        tags: [&str; 2],
        load: F,
    ) -> DataResult<Vec<T>>
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = DataResult<Vec<T>>> + Send,
    {
        Ok(self
            .cached(entity, discriminator, tags, || async {
                load().await.map(Some)
            })
            .await?
            .unwrap_or_default())
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct InvoiceRow {
    #[sqlx(rename = "Id")]
    id: i32,
    #[sqlx(rename = "CustomerId")]
    customer_id: Option<i32>,
    #[sqlx(rename = "InvoiceDate")]
    invoice_date: DateTime<Utc>,
    #[sqlx(rename = "BillingAddress")]
    billing_address: Option<String>,
    #[sqlx(rename = "BillingCity")]
    billing_city: Option<String>,
    #[sqlx(rename = "BillingState")]
    billing_state: Option<String>,
    #[sqlx(rename = "BillingCountry")]
    billing_country: Option<String>,
    #[sqlx(rename = "BillingPostalCode")]
    billing_postal_code: Option<String>,
    #[sqlx(rename = "Total")]
    total: Decimal,
    #[sqlx(rename = "Status")]
    status: String,
}

impl From<InvoiceRow> for InvoiceApiModel {
    fn from(row: InvoiceRow) -> Self {
        Self {
            id: row.id,
            customer_id: row.customer_id,
            invoice_date: row.invoice_date,
            billing_address: row.billing_address,
            billing_city: row.billing_city,
            billing_state: row.billing_state,
            billing_country: row.billing_country,
            billing_postal_code: row.billing_postal_code,
            total: row.total,
            status: row.status,
            invoice_lines: Vec::new(),
        }
    }
}

#[derive(FromRow)]
struct InvoiceLineRow {
    #[sqlx(rename = "Id")]
    id: i32,
    #[sqlx(rename = "InvoiceId")]
    invoice_id: Option<i32>,
    #[sqlx(rename = "TrackId")]
    track_id: Option<i32>,
    #[sqlx(rename = "UnitPrice")]
    unit_price: Option<Decimal>,
    #[sqlx(rename = "Quantity")]
    quantity: Option<i32>,
}

impl From<InvoiceLineRow> for InvoiceLineApiModel {
    fn from(row: InvoiceLineRow) -> Self {
        Self {
            id: row.id,
            invoice_id: row.invoice_id,
            track_id: row.track_id,
            unit_price: row.unit_price,
            quantity: row.quantity,
            invoice: None,
        }
    }
}

const INVOICE_COLUMNS: &str = r#"SELECT "Id", "CustomerId", "InvoiceDate", "BillingAddress", "BillingCity", "BillingState",
       "BillingCountry", "BillingPostalCode", "Total", "Status" FROM orders."Invoice""#;

const LINE_COLUMNS: &str =
    r#"SELECT "Id", "InvoiceId", "TrackId", "UnitPrice", "Quantity" FROM orders."InvoiceLine""#;

async fn invoices(pool: &DbPool, sql: &str, id: Option<i32>) -> DataResult<Vec<InvoiceApiModel>> {
    let mut query = sqlx::query_as::<_, InvoiceRow>(sql);
    if let Some(id) = id {
        query = query.bind(id);
    }
    Ok(query
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

async fn lines(pool: &DbPool, sql: &str, id: Option<i32>) -> DataResult<Vec<InvoiceLineApiModel>> {
    let mut query = sqlx::query_as::<_, InvoiceLineRow>(sql);
    if let Some(id) = id {
        query = query.bind(id);
    }
    Ok(query
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

// ---------------------------------------------------------------------------
// Invoices
// ---------------------------------------------------------------------------

/// What finalizing an invoice came to. Port of `FinalizeInvoiceResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinalizeOutcome {
    /// Final now; `InvoiceFinalized` is in the outbox.
    Finalized,
    /// No invoice by that id.
    NotFound,
    /// It was already final; nothing was published.
    AlreadyFinalized,
}

/// Port of `IInvoiceService`.
#[async_trait]
pub(crate) trait InvoiceService: Send + Sync {
    async fn get_invoice_by_id(&self, id: i32) -> DataResult<Option<InvoiceApiModel>>;
    async fn get_all_invoices(&self) -> DataResult<Vec<InvoiceApiModel>>;
    async fn get_invoices_by_customer_id(&self, id: i32) -> DataResult<Vec<InvoiceApiModel>>;
    async fn finalize_invoice(&self, id: i32) -> DataResult<FinalizeOutcome>;
}

/// Port of `InvoiceService`.
#[derive(Debug, Clone)]
pub(crate) struct Invoices {
    pub store: Store,
    pub publisher: EventPublisher,
    pub clock: Arc<dyn Clock>,
}

#[async_trait]
impl InvoiceService for Invoices {
    async fn get_invoice_by_id(&self, id: i32) -> DataResult<Option<InvoiceApiModel>> {
        let pool = &self.store.pool;
        self.store
            .cached("invoice", &format!("by-id:{id}"), INVOICE_TAGS, || async {
                let Some(invoice) = invoices(
                    pool,
                    &format!(r#"{INVOICE_COLUMNS} WHERE "Id" = $1"#),
                    Some(id),
                )
                .await?
                .into_iter()
                .next() else {
                    return Ok(None);
                };

                let invoice_lines = lines(
                    pool,
                    &format!(r#"{LINE_COLUMNS} WHERE "InvoiceId" = $1 ORDER BY "Id""#),
                    Some(id),
                )
                .await?;

                Ok(Some(InvoiceApiModel {
                    invoice_lines,
                    ..invoice
                }))
            })
            .await
    }

    async fn get_all_invoices(&self) -> DataResult<Vec<InvoiceApiModel>> {
        let pool = &self.store.pool;
        self.store
            .cached_list("invoice", "all", INVOICE_TAGS, || async {
                invoices(pool, &format!(r#"{INVOICE_COLUMNS} ORDER BY "Id""#), None).await
            })
            .await
    }

    async fn get_invoices_by_customer_id(&self, id: i32) -> DataResult<Vec<InvoiceApiModel>> {
        let pool = &self.store.pool;
        self.store
            .cached_list(
                "invoice",
                &format!("by-customer:{id}"),
                INVOICE_TAGS,
                || async {
                    invoices(
                        pool,
                        &format!(r#"{INVOICE_COLUMNS} WHERE "CustomerId" = $1 ORDER BY "Id""#),
                        Some(id),
                    )
                    .await
                },
            )
            .await
    }

    /// Port of `FinalizeInvoiceAsync`.
    ///
    /// The status change and the outbox row commit together or not at all.
    /// The row lock taken here also makes two concurrent finalizations of one
    /// invoice publish once, where upstream relies on its status check alone.
    async fn finalize_invoice(&self, id: i32) -> DataResult<FinalizeOutcome> {
        let mut transaction = self.store.pool.begin().await?;

        let Some(invoice) = sqlx::query_as::<_, InvoiceRow>(&format!(
            r#"{INVOICE_COLUMNS} WHERE "Id" = $1 FOR UPDATE"#
        ))
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Ok(FinalizeOutcome::NotFound);
        };

        if invoice.status == InvoiceStatus::Finalized.as_str() {
            return Ok(FinalizeOutcome::AlreadyFinalized);
        }

        let invoice_lines = sqlx::query_as::<_, InvoiceLineRow>(&format!(
            r#"{LINE_COLUMNS} WHERE "InvoiceId" = $1 ORDER BY "Id""#
        ))
        .bind(id)
        .fetch_all(&mut *transaction)
        .await?;

        sqlx::query(r#"UPDATE orders."Invoice" SET "Status" = $2 WHERE "Id" = $1"#)
            .bind(id)
            .bind(InvoiceStatus::Finalized.as_str())
            .execute(&mut *transaction)
            .await?;

        let event = InvoiceFinalized {
            event_id: Uuid::new_v4(),
            occurred_at: self.clock.now(),
            invoice_id: invoice.id,
            customer_id: invoice.customer_id,
            invoice_date: invoice.invoice_date,
            total: invoice.total,
            lines: invoice_lines
                .iter()
                .filter_map(|line| {
                    line.track_id.map(|track_id| InvoiceFinalizedLine {
                        track_id,
                        quantity: line.quantity.unwrap_or(0),
                        unit_price: line.unit_price.unwrap_or(Decimal::ZERO),
                    })
                })
                .collect(),
        };

        self.publisher.publish(&event, &mut transaction).await?;
        transaction.commit().await?;

        self.store.cache.remove_by_tag(INVOICE_TAGS[0]).await;
        self.store
            .cache
            .remove(
                &self
                    .store
                    .keys
                    .compose(MODULE, "invoice", VERSION, &format!("by-id:{id}")),
            )
            .await;

        Ok(FinalizeOutcome::Finalized)
    }
}

// ---------------------------------------------------------------------------
// Invoice lines
// ---------------------------------------------------------------------------

/// Port of `IInvoiceLineService`.
#[async_trait]
pub(crate) trait InvoiceLineService: Send + Sync {
    async fn get_invoice_line_by_id(&self, id: i32) -> DataResult<Option<InvoiceLineApiModel>>;
    async fn get_all_invoice_lines(&self) -> DataResult<Vec<InvoiceLineApiModel>>;
    async fn get_invoice_lines_by_invoice_id(
        &self,
        id: i32,
    ) -> DataResult<Vec<InvoiceLineApiModel>>;
    async fn get_invoice_lines_by_track_id(&self, id: i32) -> DataResult<Vec<InvoiceLineApiModel>>;
}

/// Port of `InvoiceLineService`.
#[derive(Debug, Clone)]
pub(crate) struct InvoiceLines(pub Store);

impl InvoiceLines {
    async fn list(
        &self,
        discriminator: String,
        column: &'static str,
        id: i32,
    ) -> DataResult<Vec<InvoiceLineApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list(
                "invoiceline",
                &discriminator,
                INVOICE_LINE_TAGS,
                || async move {
                    lines(
                        pool,
                        &format!(r#"{LINE_COLUMNS} WHERE "{column}" = $1 ORDER BY "Id""#),
                        Some(id),
                    )
                    .await
                },
            )
            .await
    }
}

#[async_trait]
impl InvoiceLineService for InvoiceLines {
    async fn get_invoice_line_by_id(&self, id: i32) -> DataResult<Option<InvoiceLineApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached(
                "invoiceline",
                &format!("by-id:{id}"),
                INVOICE_LINE_TAGS,
                || async {
                    Ok(lines(
                        pool,
                        &format!(r#"{LINE_COLUMNS} WHERE "Id" = $1"#),
                        Some(id),
                    )
                    .await?
                    .into_iter()
                    .next())
                },
            )
            .await
    }

    async fn get_all_invoice_lines(&self) -> DataResult<Vec<InvoiceLineApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list("invoiceline", "all", INVOICE_LINE_TAGS, || async {
                lines(pool, &format!(r#"{LINE_COLUMNS} ORDER BY "Id""#), None).await
            })
            .await
    }

    async fn get_invoice_lines_by_invoice_id(
        &self,
        id: i32,
    ) -> DataResult<Vec<InvoiceLineApiModel>> {
        self.list(format!("by-invoice:{id}"), "InvoiceId", id).await
    }

    async fn get_invoice_lines_by_track_id(&self, id: i32) -> DataResult<Vec<InvoiceLineApiModel>> {
        self.list(format!("by-track:{id}"), "TrackId", id).await
    }
}

// ---------------------------------------------------------------------------
// Dead letters
// ---------------------------------------------------------------------------

/// Port of `DeadLetterService`, behind a trait so its handlers can be faked.
#[async_trait]
pub(crate) trait DeadLetterService: Send + Sync {
    /// Every dead-lettered message, oldest dead letter first.
    async fn list(&self) -> DataResult<Vec<DeadLetter>>;
    /// Sends one back for delivery. `false` when no dead letter has that id.
    async fn retry(&self, id: Uuid) -> DataResult<bool>;
}

/// Reads and resets `orders."OutboxMessage"` dead letters.
#[derive(Debug, Clone)]
pub(crate) struct DeadLetters {
    pub pool: DbPool,
    pub clock: Arc<dyn Clock>,
}

#[derive(FromRow)]
struct DeadLetterRow {
    #[sqlx(rename = "Id")]
    id: Uuid,
    #[sqlx(rename = "EventType")]
    event_type: String,
    #[sqlx(rename = "OccurredAt")]
    occurred_at: DateTime<Utc>,
    #[sqlx(rename = "Attempts")]
    attempts: i32,
    #[sqlx(rename = "LastError")]
    last_error: Option<String>,
    #[sqlx(rename = "DeadLetteredAt")]
    dead_lettered_at: DateTime<Utc>,
}

#[async_trait]
impl DeadLetterService for DeadLetters {
    async fn list(&self) -> DataResult<Vec<DeadLetter>> {
        let rows = sqlx::query_as::<_, DeadLetterRow>(
            r#"SELECT "Id", "EventType", "OccurredAt", "Attempts", "LastError", "DeadLetteredAt"
                 FROM orders."OutboxMessage" WHERE "DeadLetteredAt" IS NOT NULL ORDER BY "DeadLetteredAt""#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| DeadLetter {
                id: row.id,
                event_type: row.event_type,
                occurred_at: row.occurred_at,
                attempts: row.attempts,
                last_error: row.last_error,
                dead_lettered_at: row.dead_lettered_at,
            })
            .collect())
    }

    /// Resets the attempts so the dispatcher delivers it on its next poll.
    /// `LastError` is kept, as upstream keeps it.
    async fn retry(&self, id: Uuid) -> DataResult<bool> {
        let updated = sqlx::query(
            r#"UPDATE orders."OutboxMessage" SET "Attempts" = 0, "DeadLetteredAt" = NULL, "NextAttemptAt" = $2
                WHERE "Id" = $1 AND "DeadLetteredAt" IS NOT NULL"#,
        )
        .bind(id)
        .bind(self.clock.now())
        .execute(&self.pool)
        .await?;

        Ok(updated.rows_affected() > 0)
    }
}
