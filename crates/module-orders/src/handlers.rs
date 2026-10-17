//! Orders' endpoint handlers, ported from `Orders.Module/Endpoints/*Handlers`.
//!
//! Named functions rather than closures, so each can be called directly with a
//! fake service (`HandlerTests`).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use shared_kernel::ApiError;
use shared_kernel::data::found;
use uuid::Uuid;

use crate::OrdersState;
use crate::models::{
    DeadLetter, InvoiceApiModel, InvoiceFinalizeAccepted, InvoiceLineApiModel, InvoiceStatus,
};
use crate::services::FinalizeOutcome;

/// What every read handler here returns.
pub(crate) type Answer<T> = Result<Json<T>, ApiError>;

type Orders = State<Arc<OrdersState>>;

pub(crate) async fn get_invoice_by_id(
    State(orders): Orders,
    Path(id): Path<i32>,
) -> Answer<InvoiceApiModel> {
    found(orders.invoices.get_invoice_by_id(id).await?)
}

pub(crate) async fn get_all_invoices(State(orders): Orders) -> Answer<Vec<InvoiceApiModel>> {
    Ok(Json(orders.invoices.get_all_invoices().await?))
}

pub(crate) async fn get_invoices_by_customer_id(
    State(orders): Orders,
    Path(id): Path<i32>,
) -> Answer<Vec<InvoiceApiModel>> {
    Ok(Json(orders.invoices.get_invoices_by_customer_id(id).await?))
}

/// Port of `InvoiceHandlers.FinalizeInvoice`: 202 with the invoice's
/// `Location`, 404 for a missing invoice, 409 for one already final.
pub(crate) async fn finalize_invoice(
    State(orders): Orders,
    Path(id): Path<i32>,
) -> Result<Response, ApiError> {
    match orders.invoices.finalize_invoice(id).await? {
        FinalizeOutcome::Finalized => Ok((
            StatusCode::ACCEPTED,
            [(header::LOCATION, format!("/api/orders/invoices/{id}"))],
            Json(InvoiceFinalizeAccepted {
                invoice_id: id,
                status: InvoiceStatus::Finalized.as_str().to_owned(),
                sales_counters_update: "eventual".to_owned(),
            }),
        )
            .into_response()),
        FinalizeOutcome::NotFound => Err(ApiError::NotFound),
        FinalizeOutcome::AlreadyFinalized => Err(ApiError::Conflict),
    }
}

pub(crate) async fn get_invoice_line_by_id(
    State(orders): Orders,
    Path(id): Path<i32>,
) -> Answer<InvoiceLineApiModel> {
    found(orders.invoice_lines.get_invoice_line_by_id(id).await?)
}

pub(crate) async fn get_all_invoice_lines(
    State(orders): Orders,
) -> Answer<Vec<InvoiceLineApiModel>> {
    Ok(Json(orders.invoice_lines.get_all_invoice_lines().await?))
}

pub(crate) async fn get_invoice_lines_by_invoice_id(
    State(orders): Orders,
    Path(id): Path<i32>,
) -> Answer<Vec<InvoiceLineApiModel>> {
    Ok(Json(
        orders
            .invoice_lines
            .get_invoice_lines_by_invoice_id(id)
            .await?,
    ))
}

pub(crate) async fn get_invoice_lines_by_track_id(
    State(orders): Orders,
    Path(id): Path<i32>,
) -> Answer<Vec<InvoiceLineApiModel>> {
    Ok(Json(
        orders
            .invoice_lines
            .get_invoice_lines_by_track_id(id)
            .await?,
    ))
}

pub(crate) async fn list_dead_letters(State(orders): Orders) -> Answer<Vec<DeadLetter>> {
    Ok(Json(orders.dead_letters.list().await?))
}

/// 204 once reset, 404 when no dead letter has that id.
pub(crate) async fn retry_dead_letter(
    State(orders): Orders,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    if orders.dead_letters.retry(id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use async_trait::async_trait;
    use shared_kernel::DataResult;

    use super::*;
    use crate::services::{DeadLetterService, InvoiceLineService, InvoiceService};

    /// Invoice 1 is a draft, invoice 2 is already final, nothing else exists.
    struct Fake;

    #[async_trait]
    impl InvoiceService for Fake {
        async fn get_invoice_by_id(&self, id: i32) -> DataResult<Option<InvoiceApiModel>> {
            Ok((id == 1).then(|| InvoiceApiModel {
                id: 1,
                ..InvoiceApiModel::default()
            }))
        }
        async fn get_all_invoices(&self) -> DataResult<Vec<InvoiceApiModel>> {
            Ok(Vec::new())
        }
        async fn get_invoices_by_customer_id(&self, id: i32) -> DataResult<Vec<InvoiceApiModel>> {
            Ok(vec![InvoiceApiModel {
                id: 1,
                customer_id: Some(id),
                ..InvoiceApiModel::default()
            }])
        }
        async fn finalize_invoice(&self, id: i32) -> DataResult<FinalizeOutcome> {
            Ok(match id {
                1 => FinalizeOutcome::Finalized,
                2 => FinalizeOutcome::AlreadyFinalized,
                _ => FinalizeOutcome::NotFound,
            })
        }
    }

    #[async_trait]
    impl InvoiceLineService for Fake {
        async fn get_invoice_line_by_id(&self, _: i32) -> DataResult<Option<InvoiceLineApiModel>> {
            Ok(None)
        }
        async fn get_all_invoice_lines(&self) -> DataResult<Vec<InvoiceLineApiModel>> {
            Ok(Vec::new())
        }
        async fn get_invoice_lines_by_invoice_id(
            &self,
            _: i32,
        ) -> DataResult<Vec<InvoiceLineApiModel>> {
            Ok(Vec::new())
        }
        async fn get_invoice_lines_by_track_id(
            &self,
            _: i32,
        ) -> DataResult<Vec<InvoiceLineApiModel>> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl DeadLetterService for Fake {
        async fn list(&self) -> DataResult<Vec<DeadLetter>> {
            Ok(Vec::new())
        }
        async fn retry(&self, id: Uuid) -> DataResult<bool> {
            Ok(id == Uuid::nil())
        }
    }

    fn orders() -> Orders {
        let fake = Arc::new(Fake);
        State(Arc::new(OrdersState::from_services(
            fake.clone(),
            fake.clone(),
            fake,
        )))
    }

    #[tokio::test]
    async fn finalize_maps_each_outcome_to_its_result() {
        let accepted = finalize_invoice(orders(), Path(1))
            .await
            .expect("a draft finalizes");
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);
        assert_eq!(
            accepted.headers()[header::LOCATION],
            "/api/orders/invoices/1"
        );

        assert!(matches!(
            finalize_invoice(orders(), Path(2)).await,
            Err(ApiError::Conflict)
        ));
        assert!(matches!(
            finalize_invoice(orders(), Path(3)).await,
            Err(ApiError::NotFound)
        ));
    }

    #[tokio::test]
    async fn get_invoices_by_customer_id_passes_the_customer_through() {
        let Json(invoices) = get_invoices_by_customer_id(orders(), Path(7))
            .await
            .unwrap();

        assert_eq!(invoices[0].customer_id, Some(7));
    }

    #[tokio::test]
    async fn retrying_an_unknown_dead_letter_is_not_found() {
        assert_eq!(
            retry_dead_letter(orders(), Path(Uuid::nil()))
                .await
                .unwrap(),
            StatusCode::NO_CONTENT
        );
        assert!(matches!(
            retry_dead_letter(orders(), Path(Uuid::new_v4())).await,
            Err(ApiError::NotFound)
        ));
    }
}
