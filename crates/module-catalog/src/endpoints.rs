//! Catalog's routes, ported from `Catalog.Module/Endpoints`.
//!
//! One group at `/api/catalog`, carrying the module's own rate limit and
//! metrics. Every data route requires `catalog.read` and `tenant.scoped`;
//! health and data-health stay anonymous.

use std::sync::Arc;

use identity_contracts::{permissions, policies};
use shared_kernel::endpoints::Endpoints;
use shared_kernel::health;

use crate::{CatalogState, NAME, PREFIX, TAG, handlers};

/// Every data route's policies.
const READ: &[&str] = &[permissions::CATALOG_READ, policies::TENANT_SCOPED];

/// What a by-id route can answer.
const BY_ID: &[u16] = &[200, 401, 403, 404, 429];
/// What a collection route can answer. No 404: upstream stopped declaring it.
const LIST: &[u16] = &[200, 401, 403, 429];

pub(crate) fn map(endpoints: &mut Endpoints, state: &Arc<CatalogState>) {
    let mut group = endpoints.map_group(PREFIX, TAG, Arc::clone(state));
    group
        .require_rate_limiting(state.rate_limiter.clone())
        .add_module_metrics(Arc::clone(&state.meter));

    health::map_health_endpoints(&mut group, NAME, Arc::new(state.database.clone()));

    // Albums
    group
        .map_get("/albums/{id:int}", handlers::get_album_by_id)
        .with_name("CatalogGetAlbumById")
        .require_authorization(READ)
        .produces(BY_ID);
    group
        .map_get("/albums/", handlers::get_all_albums)
        .with_name("GetAllAlbums")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_get("/albums/artist/{id:int}", handlers::get_albums_by_artist_id)
        .with_name("GetAlbumsByArtistId")
        .require_authorization(READ)
        .produces(LIST);

    // Artists
    group
        .map_get("/artists/{id:int}", handlers::get_artist_by_id)
        .with_name("CatalogGetArtistById")
        .require_authorization(READ)
        .produces(BY_ID);
    group
        .map_get("/artists/", handlers::get_all_artists)
        .with_name("GetAllArtists")
        .require_authorization(READ)
        .produces(LIST);

    // Tracks
    group
        .map_get("/tracks/{id:int}", handlers::get_track_by_id)
        .with_name("CatalogGetTrackById")
        .require_authorization(READ)
        .produces(BY_ID);
    group
        .map_get("/tracks/{id:int}/sales", handlers::get_track_sales)
        .with_name("CatalogGetTrackSales")
        .with_description(
            "Units sold, counted from InvoiceFinalized events. Eventually consistent: an invoice finalized moments ago may not be counted yet.",
        )
        .require_authorization(READ)
        .produces(BY_ID);
    group
        .map_get("/tracks/", handlers::get_all_tracks)
        .with_name("GetAllTracks")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_get("/tracks/artist/{id:int}", handlers::get_tracks_by_artist_id)
        .with_name("GetTracksByArtistId")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_get(
            "/tracks/playlist/{id:int}",
            handlers::get_tracks_by_playlist_id,
        )
        .with_name("GetTracksByPlaylistId")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_get("/tracks/album/{id:int}", handlers::get_tracks_by_album_id)
        .with_name("GetTracksByAlbumId")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_get("/tracks/genre/{id:int}", handlers::get_tracks_by_genre_id)
        .with_name("GetTracksByGenreId")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_get(
            "/tracks/mediatype/{id:int}",
            handlers::get_tracks_by_media_type_id,
        )
        .with_name("GetTracksByMediaTypeId")
        .require_authorization(READ)
        .produces(LIST);

    // Playlists
    group
        .map_get("/playlists/{id:int}", handlers::get_playlist_by_id)
        .with_name("CatalogGetPlaylistById")
        .require_authorization(READ)
        .produces(BY_ID);
    group
        .map_get("/playlists/", handlers::get_all_playlists)
        .with_name("GetAllPlaylists")
        .require_authorization(READ)
        .produces(LIST);
}
