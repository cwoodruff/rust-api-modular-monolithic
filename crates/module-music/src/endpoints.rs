//! The Music module's routes, ported from its four endpoint classes.
//!
//! Every data endpoint stacks two policies — `music.read` and `tenant.scoped` —
//! and both must pass. The health endpoints stay anonymous.
//!
//! # Trailing slashes
//!
//! The original registers its collection routes with a trailing slash
//! (`albums/`) and its by-id routes without (`/albums/{id}`). ASP.NET routing
//! treats a trailing slash as insignificant, so both `/api/music/albums` and
//! `/api/music/albums/` answer. axum does not, so each collection route is
//! registered under both spellings rather than normalizing the path globally —
//! a global rewrite would also affect routes the original leaves alone.

use axum::Router;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use shared_kernel::Principal;
use shared_kernel::auth::{policies, read_scoped};
use shared_kernel::data::{collection_response, item_response};
use shared_persistence::AppState;

use crate::services;

/// Checks the two policies every data endpoint here carries.
///
/// Returns the refusal to send, or `None` when the caller may proceed.
fn refuse(principal: &Principal) -> Option<Response> {
    principal
        .authorize(&read_scoped(policies::MUSIC_READ))
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

/// The Music module's routes.
pub(crate) fn routes() -> Router<AppState> {
    let router = Router::new()
        .route("/albums/{id}", get(album_by_id))
        .route("/albums/artist/{id}", get(albums_by_artist))
        .route("/artists/{id}", get(artist_by_id))
        .route("/playlists/{id}", get(playlist_by_id))
        .route("/tracks/{id}", get(track_by_id))
        .route("/tracks/album/{id}", get(tracks_by_album))
        .route("/tracks/artist/{id}", get(tracks_by_artist))
        .route("/tracks/genre/{id}", get(tracks_by_genre))
        .route("/tracks/mediatype/{id}", get(tracks_by_media_type))
        .route("/tracks/playlist/{id}", get(tracks_by_playlist))
        .route("/tracks/invoice/{id}", get(tracks_by_invoice));

    let router = collection(router, "/albums", all_albums);
    let router = collection(router, "/artists", all_artists);
    let router = collection(router, "/playlists", all_playlists);

    collection(router, "/tracks", all_tracks)
}

// ---------------------------------------------------------------------------
// Albums
// ---------------------------------------------------------------------------

async fn album_by_id(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    item_response(services::album_by_id(&state, id).await)
}

async fn all_albums(principal: Principal, State(state): State<AppState>) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::all_albums(&state).await)
}

async fn albums_by_artist(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::albums_by_artist(&state, id).await)
}

// ---------------------------------------------------------------------------
// Artists
// ---------------------------------------------------------------------------

async fn artist_by_id(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    item_response(services::artist_by_id(&state, id).await)
}

async fn all_artists(principal: Principal, State(state): State<AppState>) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::all_artists(&state).await)
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

async fn playlist_by_id(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    item_response(services::playlist_by_id(&state, id).await)
}

async fn all_playlists(principal: Principal, State(state): State<AppState>) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::all_playlists(&state).await)
}

// ---------------------------------------------------------------------------
// Tracks
// ---------------------------------------------------------------------------

async fn track_by_id(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    item_response(services::track_by_id(&state, id).await)
}

async fn all_tracks(principal: Principal, State(state): State<AppState>) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::all_tracks(&state).await)
}

async fn tracks_by_album(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::tracks_by_album(&state, id).await)
}

async fn tracks_by_artist(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::tracks_by_artist(&state, id).await)
}

async fn tracks_by_genre(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::tracks_by_genre(&state, id).await)
}

async fn tracks_by_media_type(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::tracks_by_media_type(&state, id).await)
}

async fn tracks_by_playlist(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::tracks_by_playlist(&state, id).await)
}

async fn tracks_by_invoice(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Response {
    if let Some(refusal) = refuse(&principal) {
        return refusal;
    }

    collection_response(services::tracks_by_invoice(&state, id).await)
}
