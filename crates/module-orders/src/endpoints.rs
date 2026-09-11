//! The Orders module's routes, ported from its two endpoint classes.
//!
//! Every data endpoint stacks `orders.read` and `tenant.scoped`; the health
//! endpoints stay anonymous. Collection routes are registered under both the
//! slashed and unslashed spellings, for the reason the Music module's
//! endpoints explain.

use axum::Router;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use shared_kernel::Principal;
use shared_kernel::auth::{policies, read_scoped};
use shared_kernel::data::{collection_response, item_response};
use shared_persistence::AppState;

use crate::services;

/// The shape `GET /invoice-lines/{id}` returns.
///
/// The C# repository hands back an *entity*, and the service passes it
/// straight through, so the response carries the entity's navigation
/// properties — `Invoice` and `Track`, both always null, because nothing
/// loaded them. The entities in this port are row shapes with no navigation,
/// so the two members are added back here, which is the only place they exist.
/// Caught by diffing against the running service.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct InvoiceLineEntityResponse {
    #[serde(flatten)]
    line: shared_persistence::entities::InvoiceLine,
    invoice: Option<()>,
    track: Option<()>,
}

impl From<shared_persistence::entities::InvoiceLine> for InvoiceLineEntityResponse {
    fn from(line: shared_persistence::entities::InvoiceLine) -> Self {
        Self {
            line,
            invoice: None,
            track: None,
        }
    }
}

/// Checks the two policies every data endpoint here carries.
///
/// Returns the refusal to send, or `None` when the caller may proceed.
fn refuse(principal: &Principal) -> Option<Response> {
    principal
        .authorize(&read_scoped(policies::ORDERS_READ))
        .err()
        .map(axum::response::IntoResponse::into_response)
}

/// Adds a collection route under both the slashed and unslashed spellings.
fn collection<H, T>(router: Router<AppState>, path: &str, handler: H) -> Router<AppState>
where
    H: axum::handler::Handler<T, AppState> + Clone,
    T: 'static,
{
    router
        .route(path, get(handler.clone()))
        .route(&format!("{path}/"), get(handler))
}

/// The Orders module's routes.
pub(crate) fn routes() -> Router<AppState> {
    let router = Router::new()
        .route("/invoices/{id}", get(invoice_by_id))
        .route("/invoices/customer/{id}", get(invoices_by_customer))
        .route("/invoice-lines/{id}", get(invoice_line_by_id))
        .route("/invoice-lines/invoice/{id}", get(invoice_lines_by_invoice))
        .route("/invoice-lines/track/{id}", get(invoice_lines_by_track));

    let router = collection(router, "/invoices", all_invoices);

    collection(router, "/invoice-lines", all_invoice_lines)
}

async fn invoice_by_id(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    item_response(services::invoice_by_id(&state, id).await)
}

async fn all_invoices(principal: Principal, State(state): State<AppState>) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::all_invoices(&state).await)
}

async fn invoices_by_customer(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::invoices_by_customer(&state, id).await)
}

/// The one endpoint that serves an entity shape rather than an API model.
async fn invoice_line_by_id(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    item_response(
        services::invoice_line_by_id(&state, id)
            .await
            .map(|line| line.map(InvoiceLineEntityResponse::from)),
    )
}

async fn all_invoice_lines(principal: Principal, State(state): State<AppState>) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::all_invoice_lines(&state).await)
}

async fn invoice_lines_by_invoice(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::invoice_lines_by_invoice(&state, id).await)
}

async fn invoice_lines_by_track(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::invoice_lines_by_track(&state, id).await)
}
