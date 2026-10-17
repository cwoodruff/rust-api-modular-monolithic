//! Reporting's endpoint handlers, ported from `Reporting.Module/Endpoints/ReportingHandlers`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use shared_kernel::ApiError;

use crate::ReportingState;
use crate::models::{
    IntegrityFinding, IntegrityRunResult, InvoiceLineWithNamesRow, SalesByGenreRow,
};

/// What every handler here returns.
pub(crate) type Answer<T> = Result<Json<T>, ApiError>;

type Reporting = State<Arc<ReportingState>>;

pub(crate) async fn sales_by_genre(State(reporting): Reporting) -> Answer<Vec<SalesByGenreRow>> {
    Ok(Json(reporting.reports.sales_by_genre().await?))
}

/// 404 when the invoice has no lines — including when it does not exist (P23).
pub(crate) async fn invoice_lines(
    State(reporting): Reporting,
    Path(id): Path<i32>,
) -> Answer<Vec<InvoiceLineWithNamesRow>> {
    let lines = reporting.reports.invoice_lines(id).await?;
    if lines.is_empty() {
        return Err(ApiError::NotFound);
    }
    Ok(Json(lines))
}

pub(crate) async fn run_integrity_check(State(reporting): Reporting) -> Answer<IntegrityRunResult> {
    Ok(Json(reporting.integrity.run_now().await?))
}

pub(crate) async fn integrity_findings(
    State(reporting): Reporting,
) -> Answer<Vec<IntegrityFinding>> {
    Ok(Json(reporting.reports.open_findings().await?))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use async_trait::async_trait;
    use shared_kernel::DataResult;

    use super::*;
    use crate::job::IntegrityRunner;
    use crate::services::ReportingService;

    struct Fake;

    #[async_trait]
    impl ReportingService for Fake {
        async fn sales_by_genre(&self) -> DataResult<Vec<SalesByGenreRow>> {
            Ok(vec![SalesByGenreRow {
                genre_id: 1,
                genre_name: Some("Rock".to_owned()),
                track_count: 1297,
                units_sold: 2,
            }])
        }

        async fn invoice_lines(&self, invoice_id: i32) -> DataResult<Vec<InvoiceLineWithNamesRow>> {
            Ok(if invoice_id == 1 {
                vec![InvoiceLineWithNamesRow {
                    invoice_line_id: 1,
                    invoice_id: 1,
                    track_id: Some(2),
                    track_name: Some("Balls to the Wall".to_owned()),
                    customer_id: Some(2),
                    customer_name: Some("Leonie Köhler".to_owned()),
                    unit_price: None,
                    quantity: Some(1),
                }]
            } else {
                Vec::new()
            })
        }

        async fn open_findings(&self) -> DataResult<Vec<IntegrityFinding>> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl IntegrityRunner for Fake {
        async fn run_now(&self) -> DataResult<IntegrityRunResult> {
            Ok(IntegrityRunResult {
                detected: 0,
                resolved: 0,
                open: 0,
                ran_at: chrono::DateTime::UNIX_EPOCH,
            })
        }
    }

    fn reporting() -> Reporting {
        State(Arc::new(ReportingState::from_services(
            Arc::new(Fake),
            Arc::new(Fake),
        )))
    }

    #[tokio::test]
    async fn invoice_lines_answers_rows_or_not_found_when_there_are_none() {
        let Json(lines) = invoice_lines(reporting(), Path(1))
            .await
            .expect("invoice 1 has lines");
        assert_eq!(lines.len(), 1);

        assert!(matches!(
            invoice_lines(reporting(), Path(999)).await,
            Err(ApiError::NotFound)
        ));
    }

    #[tokio::test]
    async fn sales_by_genre_answers_every_row() {
        let Json(rows) = sales_by_genre(reporting()).await.expect("the view answers");

        assert_eq!(rows[0].genre_name.as_deref(), Some("Rock"));
    }

    #[tokio::test]
    async fn an_integrity_run_reports_its_counts() {
        let Json(result) = run_integrity_check(reporting())
            .await
            .expect("the run answers");

        assert_eq!(result.open, 0);
    }
}
