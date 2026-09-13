//! The Music module's routes, ported from its four endpoint classes.
//!
//! Every data endpoint stacks two policies — `music.read` and `tenant.scoped` —
//! and both must pass. The health endpoints stay anonymous.
//!
//! # How the guard is applied
//!
//! Each handler takes an [`Authorized<MusicRead>`]. That argument is the check:
//! the extractor runs both requirements before the handler body starts, and
//! there is no way to write a handler here that reads data without one, because
//! `services::*` needs the state and the route needs the extractor to build the
//! handler at all. The earlier shape opened every body with a `refuse(...)`
//! call, where an omission compiled cleanly and served the data.
//!
//! # Trailing slashes
//!
//! The original registers its collection routes with a trailing slash
//! (`albums/`) and its by-id routes without (`/albums/{id}`). ASP.NET routing
//! treats a trailing slash as insignificant, so both `/api/music/albums` and
//! `/api/music/albums/` answer. axum does not, so each collection route is
//! registered under both spellings rather than normalizing the path globally —
//! a global rewrite would also affect routes the original leaves alone.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use shared_kernel::data::found;
use shared_kernel::guards::MusicRead;
use shared_kernel::{ApiError, Authorized};
use shared_persistence::AppState;
use shared_persistence::api_models::{
    AlbumApiModel, ArtistApiModel, PlaylistApiModel, TrackApiModel,
};

use crate::services;

/// What every handler here returns.
type Answer<T> = Result<Json<T>, ApiError>;

/// The caller, once both policies have passed.
type Caller = Authorized<MusicRead>;

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
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<AlbumApiModel> {
    found(services::album_by_id(&state, id).await?)
}

async fn all_albums(_caller: Caller, State(state): State<AppState>) -> Answer<Vec<AlbumApiModel>> {
    Ok(Json(services::all_albums(&state).await?))
}

async fn albums_by_artist(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<Vec<AlbumApiModel>> {
    Ok(Json(services::albums_by_artist(&state, id).await?))
}

// ---------------------------------------------------------------------------
// Artists
// ---------------------------------------------------------------------------

async fn artist_by_id(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<ArtistApiModel> {
    found(services::artist_by_id(&state, id).await?)
}

async fn all_artists(
    _caller: Caller,
    State(state): State<AppState>,
) -> Answer<Vec<ArtistApiModel>> {
    Ok(Json(services::all_artists(&state).await?))
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

async fn playlist_by_id(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<PlaylistApiModel> {
    found(services::playlist_by_id(&state, id).await?)
}

async fn all_playlists(
    _caller: Caller,
    State(state): State<AppState>,
) -> Answer<Vec<PlaylistApiModel>> {
    Ok(Json(services::all_playlists(&state).await?))
}

// ---------------------------------------------------------------------------
// Tracks
// ---------------------------------------------------------------------------

async fn track_by_id(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<TrackApiModel> {
    found(services::track_by_id(&state, id).await?)
}

async fn all_tracks(_caller: Caller, State(state): State<AppState>) -> Answer<Vec<TrackApiModel>> {
    Ok(Json(services::all_tracks(&state).await?))
}

async fn tracks_by_album(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<Vec<TrackApiModel>> {
    Ok(Json(services::tracks_by_album(&state, id).await?))
}

async fn tracks_by_artist(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<Vec<TrackApiModel>> {
    Ok(Json(services::tracks_by_artist(&state, id).await?))
}

async fn tracks_by_genre(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<Vec<TrackApiModel>> {
    Ok(Json(services::tracks_by_genre(&state, id).await?))
}

async fn tracks_by_media_type(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<Vec<TrackApiModel>> {
    Ok(Json(services::tracks_by_media_type(&state, id).await?))
}

async fn tracks_by_playlist(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<Vec<TrackApiModel>> {
    Ok(Json(services::tracks_by_playlist(&state, id).await?))
}

async fn tracks_by_invoice(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<Vec<TrackApiModel>> {
    Ok(Json(services::tracks_by_invoice(&state, id).await?))
}
