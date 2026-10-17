//! Catalog's endpoint handlers, ported from `Catalog.Module/Endpoints/*Handlers`.
//!
//! Named functions rather than closures, as upstream moved them, so each can be
//! called directly with a fake service (`HandlerTests`).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use shared_kernel::ApiError;
use shared_kernel::data::found;

use crate::CatalogState;
use crate::models::{
    AlbumApiModel, ArtistApiModel, PlaylistApiModel, TrackApiModel, TrackSalesApiModel,
};

/// What every handler here returns.
pub(crate) type Answer<T> = Result<Json<T>, ApiError>;

type Catalog = State<Arc<CatalogState>>;

pub(crate) async fn get_album_by_id(
    State(catalog): Catalog,
    Path(id): Path<i32>,
) -> Answer<AlbumApiModel> {
    found(catalog.albums.get_album_by_id(id).await?)
}

pub(crate) async fn get_all_albums(State(catalog): Catalog) -> Answer<Vec<AlbumApiModel>> {
    Ok(Json(catalog.albums.get_all_albums().await?))
}

pub(crate) async fn get_albums_by_artist_id(
    State(catalog): Catalog,
    Path(id): Path<i32>,
) -> Answer<Vec<AlbumApiModel>> {
    Ok(Json(catalog.albums.get_albums_by_artist_id(id).await?))
}

pub(crate) async fn get_artist_by_id(
    State(catalog): Catalog,
    Path(id): Path<i32>,
) -> Answer<ArtistApiModel> {
    found(catalog.artists.get_artist_by_id(id).await?)
}

pub(crate) async fn get_all_artists(State(catalog): Catalog) -> Answer<Vec<ArtistApiModel>> {
    Ok(Json(catalog.artists.get_all_artists().await?))
}

pub(crate) async fn get_track_by_id(
    State(catalog): Catalog,
    Path(id): Path<i32>,
) -> Answer<TrackApiModel> {
    found(catalog.tracks.get_track_by_id(id).await?)
}

pub(crate) async fn get_track_sales(
    State(catalog): Catalog,
    Path(id): Path<i32>,
) -> Answer<TrackSalesApiModel> {
    found(catalog.tracks.get_track_sales(id).await?)
}

pub(crate) async fn get_all_tracks(State(catalog): Catalog) -> Answer<Vec<TrackApiModel>> {
    Ok(Json(catalog.tracks.get_all_tracks().await?))
}

pub(crate) async fn get_tracks_by_artist_id(
    State(catalog): Catalog,
    Path(id): Path<i32>,
) -> Answer<Vec<TrackApiModel>> {
    Ok(Json(catalog.tracks.get_tracks_by_artist_id(id).await?))
}

pub(crate) async fn get_tracks_by_playlist_id(
    State(catalog): Catalog,
    Path(id): Path<i32>,
) -> Answer<Vec<TrackApiModel>> {
    Ok(Json(catalog.tracks.get_tracks_by_playlist_id(id).await?))
}

pub(crate) async fn get_tracks_by_album_id(
    State(catalog): Catalog,
    Path(id): Path<i32>,
) -> Answer<Vec<TrackApiModel>> {
    Ok(Json(catalog.tracks.get_tracks_by_album_id(id).await?))
}

pub(crate) async fn get_tracks_by_genre_id(
    State(catalog): Catalog,
    Path(id): Path<i32>,
) -> Answer<Vec<TrackApiModel>> {
    Ok(Json(catalog.tracks.get_tracks_by_genre_id(id).await?))
}

pub(crate) async fn get_tracks_by_media_type_id(
    State(catalog): Catalog,
    Path(id): Path<i32>,
) -> Answer<Vec<TrackApiModel>> {
    Ok(Json(catalog.tracks.get_tracks_by_media_type_id(id).await?))
}

pub(crate) async fn get_playlist_by_id(
    State(catalog): Catalog,
    Path(id): Path<i32>,
) -> Answer<PlaylistApiModel> {
    found(catalog.playlists.get_playlist_by_id(id).await?)
}

pub(crate) async fn get_all_playlists(State(catalog): Catalog) -> Answer<Vec<PlaylistApiModel>> {
    Ok(Json(catalog.playlists.get_all_playlists().await?))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use async_trait::async_trait;
    use shared_kernel::DataResult;

    use super::*;
    use crate::services::{AlbumService, ArtistService, PlaylistService, TrackService};

    /// A Catalog whose every lookup answers from a fixed list.
    struct Fake;

    #[async_trait]
    impl AlbumService for Fake {
        async fn get_album_by_id(&self, id: i32) -> DataResult<Option<AlbumApiModel>> {
            Ok((id == 1).then(|| AlbumApiModel {
                id: 1,
                title: Some("For Those About To Rock".to_owned()),
                ..AlbumApiModel::default()
            }))
        }
        async fn get_all_albums(&self) -> DataResult<Vec<AlbumApiModel>> {
            Ok(Vec::new())
        }
        async fn get_albums_by_artist_id(&self, _: i32) -> DataResult<Vec<AlbumApiModel>> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl ArtistService for Fake {
        async fn get_artist_by_id(&self, _: i32) -> DataResult<Option<ArtistApiModel>> {
            Ok(None)
        }
        async fn get_all_artists(&self) -> DataResult<Vec<ArtistApiModel>> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl TrackService for Fake {
        async fn get_track_by_id(&self, _: i32) -> DataResult<Option<TrackApiModel>> {
            Ok(None)
        }
        async fn get_all_tracks(&self) -> DataResult<Vec<TrackApiModel>> {
            Ok(Vec::new())
        }
        async fn get_tracks_by_artist_id(&self, _: i32) -> DataResult<Vec<TrackApiModel>> {
            Ok(Vec::new())
        }
        async fn get_tracks_by_playlist_id(&self, _: i32) -> DataResult<Vec<TrackApiModel>> {
            Ok(Vec::new())
        }
        async fn get_tracks_by_album_id(&self, _: i32) -> DataResult<Vec<TrackApiModel>> {
            Ok(Vec::new())
        }
        async fn get_tracks_by_genre_id(&self, _: i32) -> DataResult<Vec<TrackApiModel>> {
            Ok(Vec::new())
        }
        async fn get_tracks_by_media_type_id(&self, _: i32) -> DataResult<Vec<TrackApiModel>> {
            Ok(Vec::new())
        }
        async fn get_track_sales(&self, id: i32) -> DataResult<Option<TrackSalesApiModel>> {
            Ok((id == 1).then_some(TrackSalesApiModel {
                track_id: 1,
                times_sold: 3,
                last_sold_at: None,
            }))
        }
    }

    #[async_trait]
    impl PlaylistService for Fake {
        async fn get_playlist_by_id(&self, _: i32) -> DataResult<Option<PlaylistApiModel>> {
            Ok(None)
        }
        async fn get_all_playlists(&self) -> DataResult<Vec<PlaylistApiModel>> {
            Ok(Vec::new())
        }
    }

    fn catalog() -> Catalog {
        let fake = Arc::new(Fake);
        State(Arc::new(CatalogState::from_services(
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake,
        )))
    }

    #[tokio::test]
    async fn get_album_by_id_answers_ok_or_not_found() {
        let Json(album) = get_album_by_id(catalog(), Path(1))
            .await
            .expect("album 1 exists");
        assert_eq!(album.id, 1);

        let missing = get_album_by_id(catalog(), Path(2)).await;
        assert!(matches!(missing, Err(ApiError::NotFound)));
    }

    #[tokio::test]
    async fn get_track_sales_answers_ok_or_not_found() {
        let Json(sales) = get_track_sales(catalog(), Path(1))
            .await
            .expect("track 1 exists");
        assert_eq!(sales.times_sold, 3);

        assert!(matches!(
            get_track_sales(catalog(), Path(9)).await,
            Err(ApiError::NotFound)
        ));
    }
}
