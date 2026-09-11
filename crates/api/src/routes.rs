//! The root endpoint and the OpenAPI document.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use shared_kernel::build_info;
use shared_kernel::health::{HEALTHY, HealthResponse, ROOT_MODULE};
use shared_persistence::AppState;

/// Port of the host's `MapGet("/")`.
///
/// Reports the same three members every health endpoint does, under the module
/// name `root`, and adds the operational metadata in Development and Demo.
pub fn root() -> Router<AppState> {
    Router::new().route(
        "/",
        get(|State(state): State<AppState>| async move {
            Json(HealthResponse::new(ROOT_MODULE, HEALTHY).gated(&state))
        }),
    )
}

/// The Swagger routes, mounted only where the original mounts them.
///
/// `UseSwagger()` and `UseSwaggerUI()` sit behind
/// `BuildInfoProvider.ShouldExposeOperationalMetadata`, so in Production the
/// paths simply do not exist and answer 404.
///
/// The document grows as the modules land. The interactive UI is deferred to
/// Phase 9 with the rest of the packaging work; what matters for parity now is
/// that the route exists in Development and Demo and is absent elsewhere.
pub fn swagger(state: &AppState) -> Router<AppState> {
    if !state.exposes_operational_metadata() {
        return Router::new();
    }

    Router::new()
        .route("/swagger/v1/swagger.json", get(openapi_document))
        .route("/swagger", get(swagger_index))
}

async fn openapi_document(State(state): State<AppState>) -> Response {
    Json(crate::openapi::document(&state)).into_response()
}

async fn swagger_index() -> Response {
    // The original serves Swagger UI here. Until Phase 9 wires that up, point
    // at the document rather than pretending the page exists.
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain")],
        format!(
            "Modular Monolith API {}\n\nOpenAPI document: /swagger/v1/swagger.json\n",
            build_info::version()
        ),
    )
        .into_response()
}
