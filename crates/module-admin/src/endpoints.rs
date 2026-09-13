//! The Administration module's routes, ported from its five endpoint classes.
//!
//! This is the one module that stacks **three** policies. Reads require
//! `role.admin`, `administration.read` and `tenant.scoped`; writes swap the
//! second for `administration.write`. All three must pass, and which set a
//! route carries is visible in its handler's signature — [`Reader`] or
//! [`Writer`] — rather than in a call at the top of the body that a new
//! endpoint could be written without.
//!
//! It also carries the only writes in the application. Nine other services
//! expose create and update methods that no route reaches.

use axum::extract::{Path, State};
use axum::http::{HeaderName, StatusCode, header};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use shared_kernel::data::found;
use shared_kernel::guards::{AdministrationRead, AdministrationWrite};
use shared_kernel::{ApiError, Authorized, JsonBody};
use shared_persistence::AppState;
use shared_persistence::api_models::{
    CustomerApiModel, EmployeeApiModel, GenreApiModel, MediaTypeApiModel,
};

use crate::services;

/// What a handler serving a body returns.
type Answer<T> = Result<Json<T>, ApiError>;

/// A caller who has passed the three read policies.
type Reader = Authorized<AdministrationRead>;

/// A caller who has passed the three write policies.
type Writer = Authorized<AdministrationWrite>;

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
    _caller: Reader,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<CustomerApiModel> {
    found(services::customer_by_id(&state, id).await?)
}

async fn all_customers(
    _caller: Reader,
    State(state): State<AppState>,
) -> Answer<Vec<CustomerApiModel>> {
    Ok(Json(services::all_customers(&state).await?))
}

async fn customers_by_support_rep(
    _caller: Reader,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<Vec<CustomerApiModel>> {
    Ok(Json(services::customers_by_support_rep(&state, id).await?))
}

// ---------------------------------------------------------------------------
// Employees
// ---------------------------------------------------------------------------

async fn employee_by_id(
    _caller: Reader,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<EmployeeApiModel> {
    found(services::employee_by_id(&state, id).await?)
}

async fn all_employees(
    _caller: Reader,
    State(state): State<AppState>,
) -> Answer<Vec<EmployeeApiModel>> {
    Ok(Json(services::all_employees(&state).await?))
}

async fn direct_reports(
    _caller: Reader,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<Vec<EmployeeApiModel>> {
    Ok(Json(services::direct_reports(&state, id).await?))
}

async fn reports_to(
    _caller: Reader,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<EmployeeApiModel> {
    found(services::reports_to(&state, id).await?)
}

// ---------------------------------------------------------------------------
// Media types
// ---------------------------------------------------------------------------

async fn media_type_by_id(
    _caller: Reader,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<MediaTypeApiModel> {
    found(services::media_type_by_id(&state, id).await?)
}

async fn all_media_types(
    _caller: Reader,
    State(state): State<AppState>,
) -> Answer<Vec<MediaTypeApiModel>> {
    Ok(Json(services::all_media_types(&state).await?))
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

/// The lowercase echo `PUT` answers with.
///
/// Note the casing against the `POST`, which answers a PascalCase model: one
/// comes from an anonymous object and the other from an API model, and the
/// host's naming policy leaves both alone.
#[derive(Debug, Clone, serde::Serialize)]
struct GenreEcho {
    id: i32,
    name: Option<String>,
}

async fn genre_by_id(
    _caller: Reader,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<GenreApiModel> {
    found(services::genre_by_id(&state, id).await?)
}

async fn all_genres(_caller: Reader, State(state): State<AppState>) -> Answer<Vec<GenreApiModel>> {
    Ok(Json(services::all_genres(&state).await?))
}

/// `POST /api/admin/genres` — 201 with a `Location` header and the new model.
async fn create_genre(
    _caller: Writer,
    State(state): State<AppState>,
    JsonBody(request): JsonBody<GenreRequest>,
) -> Result<(StatusCode, [(HeaderName, String); 1], Json<GenreApiModel>), ApiError> {
    let created = services::create_genre(&state, request.name).await?;
    let location = format!("/api/admin/genres/{}", created.id);

    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(created),
    ))
}

/// `PUT /api/admin/genres/{id}` — 200 with a lowercase echo of the change.
async fn update_genre(
    _caller: Writer,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    JsonBody(request): JsonBody<GenreRequest>,
) -> Answer<GenreEcho> {
    let name = request.name.clone();

    if services::update_genre(&state, id, request.name).await? {
        Ok(Json(GenreEcho { id, name }))
    } else {
        Err(ApiError::NotFound)
    }
}

/// `DELETE /api/admin/genres/{id}` — 204, or 404 when there is nothing to drop.
async fn delete_genre(
    _caller: Writer,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Result<StatusCode, ApiError> {
    if services::delete_genre(&state, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}
