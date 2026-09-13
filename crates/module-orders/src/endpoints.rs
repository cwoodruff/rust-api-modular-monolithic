//! The Orders module's routes, ported from its two endpoint classes.
//!
//! Every data endpoint stacks `orders.read` and `tenant.scoped`; the health
//! endpoints stay anonymous. Both requirements are carried by the
//! [`Authorized<OrdersRead>`] each handler takes, so a handler cannot reach the
//! service layer without them having passed — see the Music module's endpoints
//! for why that replaced a per-body check.
//!
//! Collection routes are registered under both the slashed and unslashed
//! spellings, for the reason the Music module's endpoints explain.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use shared_kernel::data::found;
use shared_kernel::guards::OrdersRead;
use shared_kernel::{ApiError, Authorized};
use shared_persistence::AppState;
use shared_persistence::api_models::{InvoiceApiModel, InvoiceLineApiModel};

use crate::services;

/// What every handler here returns.
type Answer<T> = Result<Json<T>, ApiError>;

/// The caller, once both policies have passed.
type Caller = Authorized<OrdersRead>;

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
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<InvoiceApiModel> {
    found(services::invoice_by_id(&state, id).await?)
}

async fn all_invoices(
    _caller: Caller,
    State(state): State<AppState>,
) -> Answer<Vec<InvoiceApiModel>> {
    Ok(Json(services::all_invoices(&state).await?))
}

async fn invoices_by_customer(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<Vec<InvoiceApiModel>> {
    Ok(Json(services::invoices_by_customer(&state, id).await?))
}

/// The one endpoint that serves an entity shape rather than an API model.
async fn invoice_line_by_id(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<InvoiceLineEntityResponse> {
    found(
        services::invoice_line_by_id(&state, id)
            .await?
            .map(InvoiceLineEntityResponse::from),
    )
}

async fn all_invoice_lines(
    _caller: Caller,
    State(state): State<AppState>,
) -> Answer<Vec<InvoiceLineApiModel>> {
    Ok(Json(services::all_invoice_lines(&state).await?))
}

async fn invoice_lines_by_invoice(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<Vec<InvoiceLineApiModel>> {
    Ok(Json(services::invoice_lines_by_invoice(&state, id).await?))
}

async fn invoice_lines_by_track(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<Vec<InvoiceLineApiModel>> {
    Ok(Json(services::invoice_lines_by_track(&state, id).await?))
}
