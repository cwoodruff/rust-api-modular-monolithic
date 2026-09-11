//! The service layer, ported from `Music.Modules.Services`.
//!
//! Every read follows the same cache-aside shape the four C# services share:
//! compose a key, read through the cache, fall back to the repository. The
//! twenty-minute lifetime is hard-coded in each of them — it ignores both
//! `Caching:DefaultTTLSeconds` and the per-module values under
//! `Caching:PerModule`, which are never read by anything.
//!
//! The C# classes exist so the container can inject a repository, a cache, a
//! key composer and a validator. Here those all live in the shared state, so
//! these are functions over it rather than structs.

use shared_kernel::caching::CacheEntryOptions;
use shared_kernel::caching::tags::music;
use shared_persistence::AppState;
use shared_persistence::api_models::{
    AlbumApiModel, ArtistApiModel, PlaylistApiModel, TrackApiModel,
};
use shared_persistence::convert::convert_all;
use shared_persistence::repositories::RepositoryResult;

/// The cache module segment these services compose keys under.
const MODULE: &str = "music";

/// The schema version every key carries.
const VERSION: &str = "v1";

/// Tags an album entry carries.
///
/// Two of them, as in the original — though only the first is ever passed to
/// an invalidation call.
const ALBUM_TAGS: [&str; 2] = [music::ALBUM, "music:album:by-id"];
const ARTIST_TAGS: [&str; 2] = [music::ARTIST, "music:artist:by-id"];
const TRACK_TAGS: [&str; 2] = [music::TRACK, "music:track:by-id"];
const PLAYLIST_TAGS: [&str; 2] = [music::PLAYLIST, "music:playlist:by-id"];

fn entry_options(tags: [&str; 2]) -> CacheEntryOptions {
    CacheEntryOptions::for_service(tags)
}

// ---------------------------------------------------------------------------
// Albums
// ---------------------------------------------------------------------------

/// Port of `AlbumService.GetAlbumByIdAsync`.
pub(crate) async fn album_by_id(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Option<AlbumApiModel>> {
    let key = state
        .cache_keys
        .compose(MODULE, "album", VERSION, &format!("by-id:{id}"));

    state
        .cache
        .try_get_or_add(
            &key,
            || async { state.repositories.albums.get_by_id(id).await },
            Some(entry_options(ALBUM_TAGS)),
        )
        .await
}

/// Port of `AlbumService.GetAllAlbumsAsync`.
pub(crate) async fn all_albums(state: &AppState) -> RepositoryResult<Vec<AlbumApiModel>> {
    let key = state.cache_keys.compose(MODULE, "album", VERSION, "all");

    let albums = state
        .cache
        .try_get_or_add(
            &key,
            || async {
                let entities = state.repositories.albums.get_all().await?;
                Ok(Some(convert_all(&entities)))
            },
            Some(entry_options(ALBUM_TAGS)),
        )
        .await?;

    Ok(albums.unwrap_or_default())
}

/// Port of `AlbumService.GetAlbumsByArtistIdAsync`.
pub(crate) async fn albums_by_artist(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Vec<AlbumApiModel>> {
    let key = state
        .cache_keys
        .compose(MODULE, "album", VERSION, &format!("by-artist:{id}"));

    let albums = state
        .cache
        .try_get_or_add(
            &key,
            || async {
                // Already API models: the repository surfaces the artist name
                // this route's join supplies.
                Ok(Some(state.repositories.albums.get_by_artist_id(id).await?))
            },
            Some(entry_options(ALBUM_TAGS)),
        )
        .await?;

    Ok(albums.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Artists
// ---------------------------------------------------------------------------

/// Port of `ArtistService.GetArtistByIdAsync`.
pub(crate) async fn artist_by_id(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Option<ArtistApiModel>> {
    let key = state
        .cache_keys
        .compose(MODULE, "artist", VERSION, &format!("by-id:{id}"));

    state
        .cache
        .try_get_or_add(
            &key,
            || async { state.repositories.artists.get_by_id(id).await },
            Some(entry_options(ARTIST_TAGS)),
        )
        .await
}

/// Port of `ArtistService.GetAllArtistsAsync`.
pub(crate) async fn all_artists(state: &AppState) -> RepositoryResult<Vec<ArtistApiModel>> {
    let key = state.cache_keys.compose(MODULE, "artist", VERSION, "all");

    let artists = state
        .cache
        .try_get_or_add(
            &key,
            || async {
                let entities = state.repositories.artists.get_all().await?;
                Ok(Some(convert_all(&entities)))
            },
            Some(entry_options(ARTIST_TAGS)),
        )
        .await?;

    Ok(artists.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

/// Port of `PlaylistService.GetPlaylistByIdAsync`.
pub(crate) async fn playlist_by_id(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Option<PlaylistApiModel>> {
    let key = state
        .cache_keys
        .compose(MODULE, "playlist", VERSION, &format!("by-id:{id}"));

    state
        .cache
        .try_get_or_add(
            &key,
            || async { state.repositories.playlists.get_by_id(id).await },
            Some(entry_options(PLAYLIST_TAGS)),
        )
        .await
}

/// Port of `PlaylistService.GetAllPlaylistsAsync`.
pub(crate) async fn all_playlists(state: &AppState) -> RepositoryResult<Vec<PlaylistApiModel>> {
    let key = state.cache_keys.compose(MODULE, "playlist", VERSION, "all");

    let playlists = state
        .cache
        .try_get_or_add(
            &key,
            || async {
                let entities = state.repositories.playlists.get_all().await?;
                Ok(Some(convert_all(&entities)))
            },
            Some(entry_options(PLAYLIST_TAGS)),
        )
        .await?;

    Ok(playlists.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Tracks
// ---------------------------------------------------------------------------

/// Port of `TrackService.GetTrackByIdAsync`.
pub(crate) async fn track_by_id(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Option<TrackApiModel>> {
    let key = state
        .cache_keys
        .compose(MODULE, "track", VERSION, &format!("by-id:{id}"));

    state
        .cache
        .try_get_or_add(
            &key,
            || async { state.repositories.tracks.get_by_id(id).await },
            Some(entry_options(TRACK_TAGS)),
        )
        .await
}

/// Port of `TrackService.GetAllTracksAsync`.
///
/// Unbounded: all 3,503 rows, composed and cached whole.
pub(crate) async fn all_tracks(state: &AppState) -> RepositoryResult<Vec<TrackApiModel>> {
    tracks_cached(state, "all", || async {
        state.repositories.tracks.get_all().await
    })
    .await
}

/// Port of `TrackService.GetTracksByAlbumIdAsync`.
pub(crate) async fn tracks_by_album(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Vec<TrackApiModel>> {
    tracks_cached(state, &format!("by-album:{id}"), || async {
        state.repositories.tracks.get_by_album_id(id).await
    })
    .await
}

/// Port of `TrackService.GetTracksByArtistIdAsync`.
pub(crate) async fn tracks_by_artist(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Vec<TrackApiModel>> {
    tracks_cached(state, &format!("by-artist:{id}"), || async {
        state.repositories.tracks.get_by_artist_id(id).await
    })
    .await
}

/// Port of `TrackService.GetTracksByGenreIdAsync`.
pub(crate) async fn tracks_by_genre(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Vec<TrackApiModel>> {
    tracks_cached(state, &format!("by-genre:{id}"), || async {
        state.repositories.tracks.get_by_genre_id(id).await
    })
    .await
}

/// Port of `TrackService.GetTracksByMediaTypeIdAsync`.
pub(crate) async fn tracks_by_media_type(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Vec<TrackApiModel>> {
    tracks_cached(state, &format!("by-mediatype:{id}"), || async {
        state.repositories.tracks.get_by_media_type_id(id).await
    })
    .await
}

/// Port of `TrackService.GetTracksByPlaylistIdAsync`.
pub(crate) async fn tracks_by_playlist(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Vec<TrackApiModel>> {
    tracks_cached(state, &format!("by-playlist:{id}"), || async {
        state.repositories.tracks.get_by_playlist_id(id).await
    })
    .await
}

/// Port of `TrackService.GetTracksByInvoiceIdAsync`.
pub(crate) async fn tracks_by_invoice(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Vec<TrackApiModel>> {
    tracks_cached(state, &format!("by-invoice:{id}"), || async {
        state.repositories.tracks.get_by_invoice_id(id).await
    })
    .await
}

/// The shape the seven track collection reads share.
async fn tracks_cached<F, Fut>(
    state: &AppState,
    discriminator: &str,
    fetch: F,
) -> RepositoryResult<Vec<TrackApiModel>>
where
    F: FnOnce() -> Fut + Send,
    Fut: Future<Output = RepositoryResult<Vec<shared_persistence::entities::Track>>> + Send,
{
    let key = state
        .cache_keys
        .compose(MODULE, "track", VERSION, discriminator);

    let tracks = state
        .cache
        .try_get_or_add(
            &key,
            || async {
                let entities = fetch().await?;
                Ok(Some(convert_all(&entities)))
            },
            Some(entry_options(TRACK_TAGS)),
        )
        .await?;

    Ok(tracks.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use shared_kernel::caching::CacheKeyComposer;

    /// The discriminators the original composes, pinned so a rename cannot
    /// silently split one cache entry into two.
    #[test]
    fn the_cache_keys_match_the_originals() {
        let composer = CacheKeyComposer::from_parts("mmapi", "test");

        assert_eq!(
            composer
                .compose(MODULE, "album", VERSION, "by-id:1")
                .to_string(),
            "test:mmapi:music:album:v1::::by-id:1"
        );
        assert_eq!(
            composer
                .compose(MODULE, "track", VERSION, "by-mediatype:2")
                .to_string(),
            "test:mmapi:music:track:v1::::by-mediatype:2"
        );
        assert_eq!(
            composer
                .compose(MODULE, "playlist", VERSION, "all")
                .to_string(),
            "test:mmapi:music:playlist:v1::::all"
        );
    }

    #[test]
    fn every_entity_carries_its_two_tags() {
        assert_eq!(ALBUM_TAGS, ["music:album", "music:album:by-id"]);
        assert_eq!(ARTIST_TAGS, ["music:artist", "music:artist:by-id"]);
        assert_eq!(TRACK_TAGS, ["music:track", "music:track:by-id"]);
        assert_eq!(PLAYLIST_TAGS, ["music:playlist", "music:playlist:by-id"]);
    }

    #[test]
    fn entries_use_the_twenty_minute_lifetime_the_services_hard_code() {
        let options = entry_options(ALBUM_TAGS);

        assert_eq!(
            options.absolute_expiration_relative_to_now,
            Some(std::time::Duration::from_secs(1200))
        );
    }
}
