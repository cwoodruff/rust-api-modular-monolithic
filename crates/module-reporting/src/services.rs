//! Port of `IReportingService` and `ReportingService`.
//!
//! Reads go through the views, as the read-only login, and nothing is cached.
//! The two view queries are the expensive ones: each holds a slot of the
//! module's gate, so at most `Concurrency:Reporting:MaxConcurrentExpensive`
//! run at once (ADR-0013).

use async_trait::async_trait;
use shared_kernel::DataResult;
use shared_kernel::concurrency::ModuleGate;
use shared_kernel::persistence::DbPool;

use crate::models::{IntegrityFinding, InvoiceLineWithNamesRow, SalesByGenreRow};

/// Port of `IReportingService`.
#[async_trait]
pub(crate) trait ReportingService: Send + Sync {
    async fn sales_by_genre(&self) -> DataResult<Vec<SalesByGenreRow>>;
    async fn invoice_lines(&self, invoice_id: i32) -> DataResult<Vec<InvoiceLineWithNamesRow>>;
    async fn open_findings(&self) -> DataResult<Vec<IntegrityFinding>>;
}

/// Port of `ReportingService`.
#[derive(Debug, Clone)]
pub(crate) struct Reports {
    pub pool: DbPool,
    pub gate: ModuleGate,
}

#[async_trait]
impl ReportingService for Reports {
    async fn sales_by_genre(&self) -> DataResult<Vec<SalesByGenreRow>> {
        let _slot = self.gate.enter().await;

        Ok(sqlx::query_as::<_, SalesByGenreRow>(
            r#"SELECT "GenreId", "GenreName", "TrackCount", "UnitsSold" FROM reporting.sales_by_genre ORDER BY "UnitsSold" DESC, "GenreName" ASC"#,
        )
        .fetch_all(&self.pool)
        .await?)
    }

    async fn invoice_lines(&self, invoice_id: i32) -> DataResult<Vec<InvoiceLineWithNamesRow>> {
        let _slot = self.gate.enter().await;

        Ok(sqlx::query_as::<_, InvoiceLineWithNamesRow>(
            r#"SELECT "InvoiceLineId", "InvoiceId", "TrackId", "TrackName", "CustomerId", "CustomerName", "UnitPrice", "Quantity"
                 FROM reporting.invoice_lines_with_names WHERE "InvoiceId" = $1 ORDER BY "InvoiceLineId""#,
        )
        .bind(invoice_id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// No gate: one indexed table of Reporting's own.
    async fn open_findings(&self) -> DataResult<Vec<IntegrityFinding>> {
        Ok(sqlx::query_as::<_, IntegrityFinding>(
            r#"SELECT "Id", "CheckName", "SourceSchema", "SourceTable", "SourceId", "MissingReference", "DetectedAt", "ResolvedAt"
                 FROM reporting."IntegrityFinding" WHERE "ResolvedAt" IS NULL ORDER BY "CheckName", "SourceId""#,
        )
        .fetch_all(&self.pool)
        .await?)
    }
}
