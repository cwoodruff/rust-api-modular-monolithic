//! The Administration module's routes, ported from its five endpoint classes.
//!
//! This is the one module that stacks **three** policies. Reads require
//! `role.admin`, `administration.read` and `tenant.scoped`; writes swap the
//! second for `administration.write`. All three must pass.
//!
//! It also carries the only writes in the application. Nine other services
//! expose create and update methods that no route reaches.

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use shared_kernel::auth::{administration_read, administration_write};
use shared_kernel::data::{collection_response, item_response};
use shared_kernel::{JsonBody, Principal};
use shared_persistence::AppState;

use crate::services;

/// Checks the three policies a read endpoint carries.
fn refuse_read(principal: &Principal) -> Option<Response> {
    principal
        .authorize(&administration_read())
        .err()
        .map(IntoResponse::into_response)
}

/// Checks the three policies a write endpoint carries.
fn refuse_write(principal: &Principal) -> Option<Response> {
    principal
        .authorize(&administration_write())
        .err()
        .map(IntoResponse::into_response)
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

/// The Administration module's routes.
pub(crate) fn routes() -> Router<AppState> {
    let router = Router::new()
        .route("/customers/{id}", get(customer_by_id))
        .route("/customers/support-rep/{id}", get(customers_by_support_rep))
        .route("/employees/{id}", get(employee_by_id))
        .route("/employees/{id}/direct-reports", get(direct_reports))
        .route("/employees/{id}/reports-to", get(reports_to))
        .route("/media-types/{id}", get(media_type_by_id))
        .route("/genres/{id}", get(genre_by_id))
        // The write surface.
        .route("/genres", post(create_genre))
        .route("/genres/{id}", put(update_genre).delete(delete_genre));

    let router = collection(router, "/customers", all_customers);
    let router = collection(router, "/employees", all_employees);
    let router = collection(router, "/media-types", all_media_types);

    collection(router, "/genres", all_genres)
}

// ---------------------------------------------------------------------------
// Customers
// ---------------------------------------------------------------------------

async fn customer_by_id(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse_read(&principal) {
        return refusal;
    }

    item_response(services::customer_by_id(&state, id).await)
}

async fn all_customers(principal: Principal, State(state): State<AppState>) -> Response {
    if let Some(refusal) = refuse_read(&principal) {
        return refusal;
    }

    collection_response(services::all_customers(&state).await)
}

async fn customers_by_support_rep(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse_read(&principal) {
        return refusal;
    }

    collection_response(services::customers_by_support_rep(&state, id).await)
}

// ---------------------------------------------------------------------------
// Employees
// ---------------------------------------------------------------------------

async fn employee_by_id(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse_read(&principal) {
        return refusal;
    }

    item_response(services::employee_by_id(&state, id).await)
}

async fn all_employees(principal: Principal, State(state): State<AppState>) -> Response {
    if let Some(refusal) = refuse_read(&principal) {
        return refusal;
    }

    collection_response(services::all_employees(&state).await)
}

async fn direct_reports(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse_read(&principal) {
        return refusal;
    }

    collection_response(services::direct_reports(&state, id).await)
}

async fn reports_to(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse_read(&principal) {
        return refusal;
    }

    item_response(services::reports_to(&state, id).await)
}

// ---------------------------------------------------------------------------
// Media types
// ---------------------------------------------------------------------------

async fn media_type_by_id(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse_read(&principal) {
        return refusal;
    }

    item_response(services::media_type_by_id(&state, id).await)
}

async fn all_media_types(principal: Principal, State(state): State<AppState>) -> Response {
    if let Some(refusal) = refuse_read(&principal) {
        return refusal;
    }

    collection_response(services::all_media_types(&state).await)
}

// ---------------------------------------------------------------------------
// Genres
// ---------------------------------------------------------------------------

/// The body both write endpoints take.
///
/// The C# records declare `string Name`, and the host leaves the naming policy
/// alone but keeps the web defaults' case-insensitive matching, so `Name` and
/// `name` both bind. The `[Required]` and `[StringLength]` attributes on those
/// records are decorative — Minimal APIs do not run data annotations, and the
/// real enforcement is the FluentValidation rule inside the service.
#[derive(Debug, Clone, Deserialize)]
struct GenreRequest {
    #[serde(alias = "name")]
    #[serde(rename = "Name")]
    name: Option<String>,
}

async fn genre_by_id(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse_read(&principal) {
        return refusal;
    }

    item_response(services::genre_by_id(&state, id).await)
}

async fn all_genres(principal: Principal, State(state): State<AppState>) -> Response {
    if let Some(refusal) = refuse_read(&principal) {
        return refusal;
    }

    collection_response(services::all_genres(&state).await)
}

/// `POST /api/admin/genres` — 201 with a `Location` header and the new model.
async fn create_genre(
    principal: Principal,
    State(state): State<AppState>,
    body: Result<JsonBody<GenreRequest>, shared_kernel::json::MalformedRequest>,
) -> Response {
    if let Some(refusal) = refuse_write(&principal) {
        return refusal;
    }

    let request = match body {
        Ok(JsonBody(request)) => request,
        Err(malformed) => return malformed.into_response(),
    };

    match services::create_genre(&state, request.name).await {
        Ok(created) => (
            StatusCode::CREATED,
            [(
                header::LOCATION,
                format!("/api/admin/genres/{}", created.id),
            )],
            Json(created),
        )
            .into_response(),
        Err(failure) => failure.into_response(),
    }
}

/// `PUT /api/admin/genres/{id}` — 200 with a lowercase echo of the change.
///
/// Note the casing: this answers `{"id":…,"name":…}` while the `POST` answers
/// a PascalCase model. One comes from an anonymous object and the other from
/// an API model, and the host's naming policy leaves both alone.
async fn update_genre(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    body: Result<JsonBody<GenreRequest>, shared_kernel::json::MalformedRequest>,
) -> Response {
    if let Some(refusal) = refuse_write(&principal) {
        return refusal;
    }

    let request = match body {
        Ok(JsonBody(request)) => request,
        Err(malformed) => return malformed.into_response(),
    };

    let name = request.name.clone();

    match services::update_genre(&state, id, request.name).await {
        Ok(true) => Json(serde_json::json!({ "id": id, "name": name })).into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(failure) => failure.into_response(),
    }
}

/// `DELETE /api/admin/genres/{id}` — 204, or 404 when there is nothing to drop.
async fn delete_genre(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse_write(&principal) {
        return refusal;
    }

    match services::delete_genre(&state, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => error.into_response(),
    }
}
