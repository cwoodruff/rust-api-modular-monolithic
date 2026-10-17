//! Catalog's services, ported from `Catalog.Module/Services`.
//!
//! Each reads its module's own tables through its own pool — there is no
//! repository layer, upstream's "EF Core is the repository" — and caches the
//! result cache-aside for twenty minutes under upstream's keys and tags. A
//! lookup that finds nothing is not cached.
//!
//! Collections come back in the table's physical order (`ORDER BY ctid`),
//! because upstream's list queries have no `ORDER BY` and PostgreSQL answers
//! them in heap order. The seeded tracks are not stored in `Id` order, and an
//! updated row moves to the end of its table, so ordering by `Id` would answer
//! differently from the original — the parity harness caught exactly that.
//! By-id projections keep the explicit order upstream's queries give them.

use std::sync::Arc;

use async_trait::async_trait;
use rust_decimal::Decimal;
use shared_kernel::DataResult;
use shared_kernel::caching::{CacheEntryOptions, CacheFacade, CacheKeyComposer};
use shared_kernel::persistence::DbPool;
use sqlx::FromRow;

use crate::models::{
    AlbumApiModel, ArtistApiModel, PlaylistApiModel, TrackApiModel, TrackSalesApiModel,
};

/// The cache module segment.
const MODULE: &str = "catalog";
/// The schema version every key carries.
const VERSION: &str = "v1";

const ALBUM_TAGS: [&str; 2] = ["catalog:album", "catalog:album:by-id"];
const ARTIST_TAGS: [&str; 2] = ["catalog:artist", "catalog:artist:by-id"];
const TRACK_TAGS: [&str; 2] = ["catalog:track", "catalog:track:by-id"];
const PLAYLIST_TAGS: [&str; 2] = ["catalog:playlist", "catalog:playlist:by-id"];

/// What every Catalog service holds: the pool, the module's cache, the key composer.
#[derive(Debug, Clone)]
pub(crate) struct Store {
    pub pool: DbPool,
    pub cache: Arc<CacheFacade>,
    pub keys: Arc<CacheKeyComposer>,
}

impl Store {
    /// Reads `entity:discriminator` through the cache.
    async fn cached<T, F, Fut>(
        &self,
        entity: &str,
        discriminator: &str,
        tags: [&str; 2],
        load: F,
    ) -> DataResult<Option<T>>
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = DataResult<Option<T>>> + Send,
    {
        let key = self.keys.compose(MODULE, entity, VERSION, discriminator);
        self.cache
            .try_get_or_add(&key, load, Some(CacheEntryOptions::for_service(tags)))
            .await
    }

    /// The same for a list, which is never absent.
    async fn cached_list<T, F, Fut>(
        &self,
        entity: &str,
        discriminator: &str,
        tags: [&str; 2],
        load: F,
    ) -> DataResult<Vec<T>>
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = DataResult<Vec<T>>> + Send,
    {
        Ok(self
            .cached(entity, discriminator, tags, || async {
                load().await.map(Some)
            })
            .await?
            .unwrap_or_default())
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct TrackRow {
    #[sqlx(rename = "Id")]
    id: i32,
    #[sqlx(rename = "Name")]
    name: Option<String>,
    #[sqlx(rename = "AlbumId")]
    album_id: Option<i32>,
    #[sqlx(rename = "MediaTypeId")]
    media_type_id: Option<i32>,
    #[sqlx(rename = "GenreId")]
    genre_id: Option<i32>,
    #[sqlx(rename = "Composer")]
    composer: Option<String>,
    #[sqlx(rename = "Milliseconds")]
    milliseconds: Option<i32>,
    #[sqlx(rename = "Bytes")]
    bytes: Option<i32>,
    #[sqlx(rename = "UnitPrice")]
    unit_price: Option<Decimal>,
    #[sqlx(rename = "AlbumName", default)]
    album_name: Option<String>,
}

impl From<TrackRow> for TrackApiModel {
    fn from(row: TrackRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            album_id: row.album_id,
            media_type_id: row.media_type_id,
            genre_id: row.genre_id,
            composer: row.composer,
            milliseconds: row.milliseconds,
            bytes: row.bytes,
            unit_price: row.unit_price,
            album: None,
            playlists: Vec::new(),
            album_name: row.album_name,
        }
    }
}

#[derive(FromRow)]
struct AlbumRow {
    #[sqlx(rename = "Id")]
    id: i32,
    #[sqlx(rename = "Title")]
    title: Option<String>,
    #[sqlx(rename = "ArtistId")]
    artist_id: Option<i32>,
    #[sqlx(rename = "ArtistName", default)]
    artist_name: Option<String>,
}

impl From<AlbumRow> for AlbumApiModel {
    fn from(row: AlbumRow) -> Self {
        Self {
            id: row.id,
            title: row.title,
            artist_name: row.artist_name,
            artist_id: row.artist_id,
            artist: None,
            tracks: Vec::new(),
        }
    }
}

#[derive(FromRow)]
struct NamedRow {
    #[sqlx(rename = "Id")]
    id: i32,
    #[sqlx(rename = "Name")]
    name: Option<String>,
}

/// Scalars only: what `ToApiModel()` copies.
const TRACK_COLUMNS: &str = r#"SELECT t."Id", t."Name", t."AlbumId", t."MediaTypeId", t."GenreId", t."Composer",
       t."Milliseconds", t."Bytes", t."UnitPrice" FROM catalog."Track" t"#;

/// Scalars plus the album's title, as the by-id projections fill `AlbumName`.
const TRACK_WITH_ALBUM: &str = r#"SELECT t."Id", t."Name", t."AlbumId", t."MediaTypeId", t."GenreId", t."Composer",
       t."Milliseconds", t."Bytes", t."UnitPrice", al."Title" AS "AlbumName"
  FROM catalog."Track" t LEFT JOIN catalog."Album" al ON al."Id" = t."AlbumId""#;

async fn tracks(pool: &DbPool, sql: &str, id: Option<i32>) -> DataResult<Vec<TrackApiModel>> {
    let mut query = sqlx::query_as::<_, TrackRow>(sql);
    if let Some(id) = id {
        query = query.bind(id);
    }
    Ok(query
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

// ---------------------------------------------------------------------------
// Albums
// ---------------------------------------------------------------------------

/// Port of `IAlbumService`.
#[async_trait]
pub(crate) trait AlbumService: Send + Sync {
    async fn get_album_by_id(&self, id: i32) -> DataResult<Option<AlbumApiModel>>;
    async fn get_all_albums(&self) -> DataResult<Vec<AlbumApiModel>>;
    async fn get_albums_by_artist_id(&self, id: i32) -> DataResult<Vec<AlbumApiModel>>;
}

/// Port of `AlbumService`.
#[derive(Debug, Clone)]
pub(crate) struct Albums(pub Store);

#[async_trait]
impl AlbumService for Albums {
    async fn get_album_by_id(&self, id: i32) -> DataResult<Option<AlbumApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached("album", &format!("by-id:{id}"), ALBUM_TAGS, || async {
                let Some(row) = sqlx::query_as::<_, AlbumRow>(
                    r#"SELECT a."Id", a."Title", a."ArtistId", ar."Name" AS "ArtistName"
                         FROM catalog."Album" a LEFT JOIN catalog."Artist" ar ON ar."Id" = a."ArtistId"
                        WHERE a."Id" = $1"#,
                )
                .bind(id)
                .fetch_optional(pool)
                .await?
                else {
                    return Ok(None);
                };

                let tracks = tracks(pool, &format!(r#"{TRACK_WITH_ALBUM} WHERE t."AlbumId" = $1 ORDER BY t."Id""#), Some(id)).await?;
                let artist = row.artist_id.map(|artist_id| ArtistApiModel {
                    id: artist_id,
                    name: row.artist_name.clone(),
                    albums: Vec::new(),
                });

                Ok(Some(AlbumApiModel {
                    artist,
                    tracks,
                    ..row.into()
                }))
            })
            .await
    }

    async fn get_all_albums(&self) -> DataResult<Vec<AlbumApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list("album", "all", ALBUM_TAGS, || async {
                // No Include upstream, so the artist's name stays null here.
                let rows = sqlx::query_as::<_, AlbumRow>(
                    r#"SELECT "Id", "Title", "ArtistId" FROM catalog."Album" ORDER BY ctid"#,
                )
                .fetch_all(pool)
                .await?;
                Ok(rows.into_iter().map(Into::into).collect())
            })
            .await
    }

    async fn get_albums_by_artist_id(&self, id: i32) -> DataResult<Vec<AlbumApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list("album", &format!("by-artist:{id}"), ALBUM_TAGS, || async {
                // This route `Include`s the artist, so its name is filled.
                let rows = sqlx::query_as::<_, AlbumRow>(
                    r#"SELECT a."Id", a."Title", a."ArtistId", ar."Name" AS "ArtistName"
                         FROM catalog."Album" a LEFT JOIN catalog."Artist" ar ON ar."Id" = a."ArtistId"
                        WHERE a."ArtistId" = $1 ORDER BY a.ctid"#,
                )
                .bind(id)
                .fetch_all(pool)
                .await?;
                Ok(rows.into_iter().map(Into::into).collect())
            })
            .await
    }
}

// ---------------------------------------------------------------------------
// Artists
// ---------------------------------------------------------------------------

/// Port of `IArtistService`.
#[async_trait]
pub(crate) trait ArtistService: Send + Sync {
    async fn get_artist_by_id(&self, id: i32) -> DataResult<Option<ArtistApiModel>>;
    async fn get_all_artists(&self) -> DataResult<Vec<ArtistApiModel>>;
}

/// Port of `ArtistService`.
#[derive(Debug, Clone)]
pub(crate) struct Artists(pub Store);

#[async_trait]
impl ArtistService for Artists {
    async fn get_artist_by_id(&self, id: i32) -> DataResult<Option<ArtistApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached("artist", &format!("by-id:{id}"), ARTIST_TAGS, || async {
                let Some(artist) = sqlx::query_as::<_, NamedRow>(r#"SELECT "Id", "Name" FROM catalog."Artist" WHERE "Id" = $1"#)
                    .bind(id)
                    .fetch_optional(pool)
                    .await?
                else {
                    return Ok(None);
                };

                let albums = sqlx::query_as::<_, AlbumRow>(
                    r#"SELECT "Id", "Title", "ArtistId" FROM catalog."Album" WHERE "ArtistId" = $1 ORDER BY "Id""#,
                )
                .bind(id)
                .fetch_all(pool)
                .await?;

                let album_tracks = tracks(
                    pool,
                    &format!(
                        r#"{TRACK_WITH_ALBUM} WHERE al."ArtistId" = $1 ORDER BY t."Id""#
                    ),
                    Some(id),
                )
                .await?;

                let albums = albums
                    .into_iter()
                    .map(|row| {
                        let album_id = row.id;
                        AlbumApiModel {
                            artist_name: artist.name.clone(),
                            tracks: album_tracks
                                .iter()
                                .filter(|track| track.album_id == Some(album_id))
                                .cloned()
                                .collect(),
                            ..row.into()
                        }
                    })
                    .collect();

                Ok(Some(ArtistApiModel {
                    id: artist.id,
                    name: artist.name,
                    albums,
                }))
            })
            .await
    }

    async fn get_all_artists(&self) -> DataResult<Vec<ArtistApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list("artist", "all", ARTIST_TAGS, || async {
                let rows = sqlx::query_as::<_, NamedRow>(
                    r#"SELECT "Id", "Name" FROM catalog."Artist" ORDER BY ctid"#,
                )
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|row| ArtistApiModel {
                        id: row.id,
                        name: row.name,
                        albums: Vec::new(),
                    })
                    .collect())
            })
            .await
    }
}

// ---------------------------------------------------------------------------
// Tracks
// ---------------------------------------------------------------------------

/// Port of `ITrackService`.
#[async_trait]
pub(crate) trait TrackService: Send + Sync {
    async fn get_track_by_id(&self, id: i32) -> DataResult<Option<TrackApiModel>>;
    async fn get_all_tracks(&self) -> DataResult<Vec<TrackApiModel>>;
    async fn get_tracks_by_artist_id(&self, id: i32) -> DataResult<Vec<TrackApiModel>>;
    async fn get_tracks_by_playlist_id(&self, id: i32) -> DataResult<Vec<TrackApiModel>>;
    async fn get_tracks_by_album_id(&self, id: i32) -> DataResult<Vec<TrackApiModel>>;
    async fn get_tracks_by_genre_id(&self, id: i32) -> DataResult<Vec<TrackApiModel>>;
    async fn get_tracks_by_media_type_id(&self, id: i32) -> DataResult<Vec<TrackApiModel>>;
    async fn get_track_sales(&self, id: i32) -> DataResult<Option<TrackSalesApiModel>>;
}

/// Port of `TrackService`.
#[derive(Debug, Clone)]
pub(crate) struct Tracks(pub Store);

impl Tracks {
    async fn list(
        &self,
        discriminator: String,
        clause: &'static str,
        id: i32,
    ) -> DataResult<Vec<TrackApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list("track", &discriminator, TRACK_TAGS, || async move {
                tracks(
                    pool,
                    &format!(r#"{TRACK_COLUMNS} {clause} ORDER BY t.ctid"#),
                    Some(id),
                )
                .await
            })
            .await
    }
}

#[async_trait]
impl TrackService for Tracks {
    async fn get_track_by_id(&self, id: i32) -> DataResult<Option<TrackApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached("track", &format!("by-id:{id}"), TRACK_TAGS, || async {
                let found = tracks(
                    pool,
                    &format!(r#"{TRACK_WITH_ALBUM} WHERE t."Id" = $1"#),
                    Some(id),
                )
                .await?;
                Ok(found.into_iter().next())
            })
            .await
    }

    async fn get_all_tracks(&self) -> DataResult<Vec<TrackApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list("track", "all", TRACK_TAGS, || async {
                tracks(pool, &format!(r#"{TRACK_COLUMNS} ORDER BY t.ctid"#), None).await
            })
            .await
    }

    async fn get_tracks_by_artist_id(&self, id: i32) -> DataResult<Vec<TrackApiModel>> {
        self.list(
            format!("by-artist:{id}"),
            r#"INNER JOIN catalog."Album" al ON al."Id" = t."AlbumId" WHERE al."ArtistId" = $1"#,
            id,
        )
        .await
    }

    async fn get_tracks_by_playlist_id(&self, id: i32) -> DataResult<Vec<TrackApiModel>> {
        self.list(
            format!("by-playlist:{id}"),
            r#"INNER JOIN catalog."PlaylistTrack" pt ON pt."TrackId" = t."Id" WHERE pt."PlaylistId" = $1"#,
            id,
        )
        .await
    }

    async fn get_tracks_by_album_id(&self, id: i32) -> DataResult<Vec<TrackApiModel>> {
        self.list(format!("by-album:{id}"), r#"WHERE t."AlbumId" = $1"#, id)
            .await
    }

    async fn get_tracks_by_genre_id(&self, id: i32) -> DataResult<Vec<TrackApiModel>> {
        self.list(format!("by-genre:{id}"), r#"WHERE t."GenreId" = $1"#, id)
            .await
    }

    async fn get_tracks_by_media_type_id(&self, id: i32) -> DataResult<Vec<TrackApiModel>> {
        self.list(
            format!("by-mediatype:{id}"),
            r#"WHERE t."MediaTypeId" = $1"#,
            id,
        )
        .await
    }

    /// Not cached, as upstream's is not: the counter moves on every event.
    async fn get_track_sales(&self, id: i32) -> DataResult<Option<TrackSalesApiModel>> {
        let exists: bool =
            sqlx::query_scalar(r#"SELECT EXISTS (SELECT 1 FROM catalog."Track" WHERE "Id" = $1)"#)
                .bind(id)
                .fetch_one(&self.0.pool)
                .await?;
        if !exists {
            return Ok(None);
        }

        let sales: Option<(i32, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
            r#"SELECT "TimesSold", "LastSoldAt" FROM catalog."TrackSales" WHERE "TrackId" = $1"#,
        )
        .bind(id)
        .fetch_optional(&self.0.pool)
        .await?;

        let (times_sold, last_sold_at) = sales.unwrap_or((0, None));
        Ok(Some(TrackSalesApiModel {
            track_id: id,
            times_sold,
            last_sold_at,
        }))
    }
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

/// Port of `IPlaylistService`.
#[async_trait]
pub(crate) trait PlaylistService: Send + Sync {
    async fn get_playlist_by_id(&self, id: i32) -> DataResult<Option<PlaylistApiModel>>;
    async fn get_all_playlists(&self) -> DataResult<Vec<PlaylistApiModel>>;
}

/// Port of `PlaylistService`.
#[derive(Debug, Clone)]
pub(crate) struct Playlists(pub Store);

#[async_trait]
impl PlaylistService for Playlists {
    async fn get_playlist_by_id(&self, id: i32) -> DataResult<Option<PlaylistApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached("playlist", &format!("by-id:{id}"), PLAYLIST_TAGS, || async {
                let Some(playlist) = sqlx::query_as::<_, NamedRow>(r#"SELECT "Id", "Name" FROM catalog."Playlist" WHERE "Id" = $1"#)
                    .bind(id)
                    .fetch_optional(pool)
                    .await?
                else {
                    return Ok(None);
                };

                let tracks = tracks(
                    pool,
                    &format!(
                        r#"{TRACK_WITH_ALBUM} INNER JOIN catalog."PlaylistTrack" pt ON pt."TrackId" = t."Id"
                            WHERE pt."PlaylistId" = $1 ORDER BY t."Id""#
                    ),
                    Some(id),
                )
                .await?;

                Ok(Some(PlaylistApiModel {
                    id: playlist.id,
                    name: playlist.name,
                    tracks,
                }))
            })
            .await
    }

    async fn get_all_playlists(&self) -> DataResult<Vec<PlaylistApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list("playlist", "all", PLAYLIST_TAGS, || async {
                let rows = sqlx::query_as::<_, NamedRow>(
                    r#"SELECT "Id", "Name" FROM catalog."Playlist" ORDER BY ctid"#,
                )
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|row| PlaylistApiModel {
                        id: row.id,
                        name: row.name,
                        tracks: Vec::new(),
                    })
                    .collect())
            })
            .await
    }
}
