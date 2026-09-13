//! Album, artist, track, and playlist repositories.

use async_trait::async_trait;
use shared_persistence::api_models::{
    AlbumApiModel, ArtistApiModel, PlaylistApiModel, TrackApiModel,
};
use shared_persistence::entities::{Album, Artist, Playlist, Track};
use shared_persistence::repositories::{
    AlbumRepository, ArtistRepository, PlaylistRepository, Repository, RepositoryResult,
    TrackRepository,
};
use sqlx::SqlitePool;

use super::common::{self, tables};
use crate::rows::{self, TRACK_MODEL_SELECT};

/// sqlx implementation of [`AlbumRepository`].
#[derive(Debug, Clone)]
pub struct SqliteAlbumRepository {
    pool: SqlitePool,
}

impl SqliteAlbumRepository {
    /// Binds the repository to a pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Repository<Album> for SqliteAlbumRepository {
    async fn entity_exists(&self, id: i32) -> RepositoryResult<bool> {
        common::exists(&self.pool, tables::ALBUM, id).await
    }

    async fn get_all(&self) -> RepositoryResult<Vec<Album>> {
        let rows = sqlx::query(r#"SELECT "Id", "Title", "ArtistId" FROM "Album" ORDER BY "Id""#)
            .fetch_all(&self.pool)
            .await
            .map_err(common::database)?;

        rows.iter()
            .map(rows::album)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }

    async fn add(&self, entity: Album) -> RepositoryResult<Album> {
        let row = sqlx::query(
            r#"INSERT INTO "Album" ("Title", "ArtistId") VALUES (?, ?)
               RETURNING "Id", "Title", "ArtistId""#,
        )
        .bind(&entity.title)
        .bind(entity.artist_id)
        .fetch_one(&self.pool)
        .await
        .map_err(common::database)?;

        rows::album(&row).map_err(common::database)
    }

    async fn update(&self, entity: Album) -> RepositoryResult<bool> {
        let updated =
            sqlx::query(r#"UPDATE "Album" SET "Title" = ?, "ArtistId" = ? WHERE "Id" = ?"#)
                .bind(&entity.title)
                .bind(entity.artist_id)
                .bind(entity.id)
                .execute(&self.pool)
                .await
                .map_err(common::database)?;

        Ok(updated.rows_affected() > 0)
    }

    async fn delete(&self, id: i32) -> RepositoryResult<bool> {
        common::delete_by_id(&self.pool, tables::ALBUM, id).await
    }
}

#[async_trait]
impl AlbumRepository for SqliteAlbumRepository {
    /// Albums by one artist, each carrying the artist's name.
    ///
    /// The C# query `Include`s the artist, and the conversion reads its name,
    /// so `ArtistName` is populated here even though the plain collection
    /// endpoint leaves it null. The nested `Artist` object stays null and
    /// `Tracks` stays empty, because the conversion copies neither.
    async fn get_by_artist_id(&self, id: i32) -> RepositoryResult<Vec<AlbumApiModel>> {
        let rows = sqlx::query(
            r#"SELECT a."Id", a."Title", a."ArtistId", ar."Name" AS "ArtistName"
               FROM "Album" a
               LEFT JOIN "Artist" ar ON ar."Id" = a."ArtistId"
               WHERE a."ArtistId" = ?
               ORDER BY a."Id""#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        let mut albums = Vec::with_capacity(rows.len());
        for row in &rows {
            let album = rows::album(row).map_err(common::database)?;
            albums.push(AlbumApiModel {
                id: album.id,
                title: album.title,
                artist_name: rows::text(row, "ArtistName").map_err(common::database)?,
                artist_id: album.artist_id,
                artist: None,
                tracks: Vec::new(),
            });
        }

        Ok(albums)
    }

    /// The album with its artist and tracks.
    ///
    /// Two statements rather than one, which is the shape the original reaches
    /// for with `AsSplitQuery()` — its comment calls it "important on SQLite to
    /// avoid cartesian explosion", since a single join would repeat the album
    /// row once per track.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<AlbumApiModel>> {
        let Some(header) = sqlx::query(
            r#"SELECT a."Id", a."Title", a."ArtistId", ar."Name" AS "ArtistName"
               FROM "Album" a
               LEFT JOIN "Artist" ar ON ar."Id" = a."ArtistId"
               WHERE a."Id" = ?"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(common::database)?
        else {
            return Ok(None);
        };

        let album = rows::album(&header).map_err(common::database)?;
        let artist_name = rows::text(&header, "ArtistName").map_err(common::database)?;

        let track_rows = sqlx::query(&format!(
            r#"{TRACK_MODEL_SELECT} WHERE t."AlbumId" = ? ORDER BY t."Id""#
        ))
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        let tracks = track_rows
            .iter()
            .map(rows::track_model)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)?;

        Ok(Some(AlbumApiModel {
            id: album.id,
            title: album.title.clone(),
            artist_name: artist_name.clone(),
            artist_id: album.artist_id,
            // The nested artist carries only its key and name; its own album
            // collection stays empty, as the original leaves it.
            artist: album.artist_id.map(|artist_id| ArtistApiModel {
                id: artist_id,
                name: artist_name,
                albums: Vec::new(),
            }),
            tracks,
        }))
    }
}

/// sqlx implementation of [`ArtistRepository`].
#[derive(Debug, Clone)]
pub struct SqliteArtistRepository {
    pool: SqlitePool,
}

impl SqliteArtistRepository {
    /// Binds the repository to a pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Repository<Artist> for SqliteArtistRepository {
    async fn entity_exists(&self, id: i32) -> RepositoryResult<bool> {
        common::exists(&self.pool, tables::ARTIST, id).await
    }

    async fn get_all(&self) -> RepositoryResult<Vec<Artist>> {
        let rows = sqlx::query(r#"SELECT "Id", "Name" FROM "Artist" ORDER BY "Id""#)
            .fetch_all(&self.pool)
            .await
            .map_err(common::database)?;

        rows.iter()
            .map(rows::artist)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }

    async fn add(&self, entity: Artist) -> RepositoryResult<Artist> {
        let row = sqlx::query(r#"INSERT INTO "Artist" ("Name") VALUES (?) RETURNING "Id", "Name""#)
            .bind(&entity.name)
            .fetch_one(&self.pool)
            .await
            .map_err(common::database)?;

        rows::artist(&row).map_err(common::database)
    }

    async fn update(&self, entity: Artist) -> RepositoryResult<bool> {
        let updated = sqlx::query(r#"UPDATE "Artist" SET "Name" = ? WHERE "Id" = ?"#)
            .bind(&entity.name)
            .bind(entity.id)
            .execute(&self.pool)
            .await
            .map_err(common::database)?;

        Ok(updated.rows_affected() > 0)
    }

    async fn delete(&self, id: i32) -> RepositoryResult<bool> {
        common::delete_by_id(&self.pool, tables::ARTIST, id).await
    }
}

#[async_trait]
impl ArtistRepository for SqliteArtistRepository {
    /// The artist, their albums, and every album's tracks.
    ///
    /// Three statements, again following the original's split-query shape. The
    /// tracks come back in one pass and are grouped by album here rather than
    /// queried per album, which keeps this O(1) in statements instead of O(n).
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<ArtistApiModel>> {
        let Some(header) = sqlx::query(r#"SELECT "Id", "Name" FROM "Artist" WHERE "Id" = ?"#)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(common::database)?
        else {
            return Ok(None);
        };

        let artist = rows::artist(&header).map_err(common::database)?;

        let album_rows = sqlx::query(
            r#"SELECT "Id", "Title", "ArtistId" FROM "Album" WHERE "ArtistId" = ? ORDER BY "Id""#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        let track_rows = sqlx::query(&format!(
            r#"{TRACK_MODEL_SELECT}
               INNER JOIN "Album" owner ON owner."Id" = t."AlbumId"
               WHERE owner."ArtistId" = ?
               ORDER BY t."Id""#
        ))
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        let tracks = track_rows
            .iter()
            .map(rows::track_model)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)?;

        let mut albums = Vec::with_capacity(album_rows.len());
        for row in &album_rows {
            let album = rows::album(row).map_err(common::database)?;

            albums.push(AlbumApiModel {
                id: album.id,
                title: album.title.clone(),
                // Denormalized from the artist being loaded, as the original does.
                artist_name: artist.name.clone(),
                artist_id: album.artist_id,
                // Deliberately null: this album is already inside its artist.
                artist: None,
                tracks: tracks
                    .iter()
                    .filter(|track| track.album_id == Some(album.id))
                    .cloned()
                    .collect(),
            });
        }

        Ok(Some(ArtistApiModel {
            id: artist.id,
            name: artist.name,
            albums,
        }))
    }
}

/// sqlx implementation of [`TrackRepository`].
#[derive(Debug, Clone)]
pub struct SqliteTrackRepository {
    pool: SqlitePool,
}

impl SqliteTrackRepository {
    /// Binds the repository to a pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Runs one of the by-foreign-key track queries.
    ///
    /// All six take a single key and differ only in how they reach it, so they
    /// share one projection and one `ORDER BY`.
    async fn tracks_where(&self, clause: &str, id: i32) -> RepositoryResult<Vec<Track>> {
        let statement = format!("{TRACK_COLUMNS} {clause} ORDER BY t.\"Id\"");

        let rows = sqlx::query(&statement)
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(common::database)?;

        rows.iter()
            .map(rows::track)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }
}

/// The plain track projection, without the denormalized join columns.
const TRACK_COLUMNS: &str = r#"
    SELECT t."Id", t."Name", t."AlbumId", t."MediaTypeId", t."GenreId", t."Composer",
           t."Milliseconds", t."Bytes", t."UnitPrice"
    FROM "Track" t
"#;

#[async_trait]
impl Repository<Track> for SqliteTrackRepository {
    async fn entity_exists(&self, id: i32) -> RepositoryResult<bool> {
        common::exists(&self.pool, tables::TRACK, id).await
    }

    async fn get_all(&self) -> RepositoryResult<Vec<Track>> {
        // Unbounded, as in the original: all 3,503 rows.
        let rows = sqlx::query(&format!("{TRACK_COLUMNS} ORDER BY t.\"Id\""))
            .fetch_all(&self.pool)
            .await
            .map_err(common::database)?;

        rows.iter()
            .map(rows::track)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }

    async fn add(&self, entity: Track) -> RepositoryResult<Track> {
        let row = sqlx::query(
            r#"INSERT INTO "Track"
                   ("Name", "AlbumId", "MediaTypeId", "GenreId", "Composer",
                    "Milliseconds", "Bytes", "UnitPrice")
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)
               RETURNING "Id", "Name", "AlbumId", "MediaTypeId", "GenreId", "Composer",
                         "Milliseconds", "Bytes", "UnitPrice""#,
        )
        .bind(&entity.name)
        .bind(entity.album_id)
        .bind(entity.media_type_id)
        .bind(entity.genre_id)
        .bind(&entity.composer)
        .bind(entity.milliseconds)
        .bind(entity.bytes)
        .bind(crate::rows::money(entity.unit_price))
        .fetch_one(&self.pool)
        .await
        .map_err(common::database)?;

        rows::track(&row).map_err(common::database)
    }

    async fn update(&self, entity: Track) -> RepositoryResult<bool> {
        let updated = sqlx::query(
            r#"UPDATE "Track" SET "Name" = ?, "AlbumId" = ?, "MediaTypeId" = ?, "GenreId" = ?,
                                  "Composer" = ?, "Milliseconds" = ?, "Bytes" = ?, "UnitPrice" = ?
               WHERE "Id" = ?"#,
        )
        .bind(&entity.name)
        .bind(entity.album_id)
        .bind(entity.media_type_id)
        .bind(entity.genre_id)
        .bind(&entity.composer)
        .bind(entity.milliseconds)
        .bind(entity.bytes)
        .bind(crate::rows::money(entity.unit_price))
        .bind(entity.id)
        .execute(&self.pool)
        .await
        .map_err(common::database)?;

        Ok(updated.rows_affected() > 0)
    }

    async fn delete(&self, id: i32) -> RepositoryResult<bool> {
        common::delete_by_id(&self.pool, tables::TRACK, id).await
    }
}

#[async_trait]
impl TrackRepository for SqliteTrackRepository {
    async fn get_by_album_id(&self, id: i32) -> RepositoryResult<Vec<Track>> {
        self.tracks_where(r#"WHERE t."AlbumId" = ?"#, id).await
    }

    async fn get_by_genre_id(&self, id: i32) -> RepositoryResult<Vec<Track>> {
        self.tracks_where(r#"WHERE t."GenreId" = ?"#, id).await
    }

    async fn get_by_media_type_id(&self, id: i32) -> RepositoryResult<Vec<Track>> {
        self.tracks_where(r#"WHERE t."MediaTypeId" = ?"#, id).await
    }

    /// Tracks sold on one invoice.
    ///
    /// An `EXISTS` subquery rather than a join, matching the original's
    /// `Tracks.Where(t => t.InvoiceLines.Any(l => l.InvoiceId == id))` — and
    /// avoiding the duplicate rows a join would produce when a track appears on
    /// the same invoice twice.
    async fn get_by_invoice_id(&self, id: i32) -> RepositoryResult<Vec<Track>> {
        self.tracks_where(
            r#"WHERE EXISTS (SELECT 1 FROM "InvoiceLine" il
                             WHERE il."TrackId" = t."Id" AND il."InvoiceId" = ?)"#,
            id,
        )
        .await
    }

    async fn get_by_playlist_id(&self, id: i32) -> RepositoryResult<Vec<Track>> {
        self.tracks_where(
            r#"INNER JOIN "PlaylistTrack" pt ON pt."TrackId" = t."Id" WHERE pt."PlaylistId" = ?"#,
            id,
        )
        .await
    }

    async fn get_by_artist_id(&self, id: i32) -> RepositoryResult<Vec<Track>> {
        self.tracks_where(
            r#"INNER JOIN "Album" al ON al."Id" = t."AlbumId" WHERE al."ArtistId" = ?"#,
            id,
        )
        .await
    }

    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<TrackApiModel>> {
        let row = sqlx::query(&format!(r#"{TRACK_MODEL_SELECT} WHERE t."Id" = ?"#))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(common::database)?;

        row.as_ref()
            .map(rows::track_model)
            .transpose()
            .map_err(common::database)
    }
}

/// sqlx implementation of [`PlaylistRepository`].
#[derive(Debug, Clone)]
pub struct SqlitePlaylistRepository {
    pool: SqlitePool,
}

impl SqlitePlaylistRepository {
    /// Binds the repository to a pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Repository<Playlist> for SqlitePlaylistRepository {
    async fn entity_exists(&self, id: i32) -> RepositoryResult<bool> {
        common::exists(&self.pool, tables::PLAYLIST, id).await
    }

    async fn get_all(&self) -> RepositoryResult<Vec<Playlist>> {
        let rows = sqlx::query(r#"SELECT "Id", "Name" FROM "Playlist" ORDER BY "Id""#)
            .fetch_all(&self.pool)
            .await
            .map_err(common::database)?;

        rows.iter()
            .map(rows::playlist)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }

    async fn add(&self, entity: Playlist) -> RepositoryResult<Playlist> {
        let row =
            sqlx::query(r#"INSERT INTO "Playlist" ("Name") VALUES (?) RETURNING "Id", "Name""#)
                .bind(&entity.name)
                .fetch_one(&self.pool)
                .await
                .map_err(common::database)?;

        rows::playlist(&row).map_err(common::database)
    }

    async fn update(&self, entity: Playlist) -> RepositoryResult<bool> {
        let updated = sqlx::query(r#"UPDATE "Playlist" SET "Name" = ? WHERE "Id" = ?"#)
            .bind(&entity.name)
            .bind(entity.id)
            .execute(&self.pool)
            .await
            .map_err(common::database)?;

        Ok(updated.rows_affected() > 0)
    }

    async fn delete(&self, id: i32) -> RepositoryResult<bool> {
        common::delete_by_id(&self.pool, tables::PLAYLIST, id).await
    }
}

#[async_trait]
impl PlaylistRepository for SqlitePlaylistRepository {
    async fn get_by_track_id(&self, id: i32) -> RepositoryResult<Vec<Playlist>> {
        let rows = sqlx::query(
            r#"SELECT p."Id", p."Name"
               FROM "PlaylistTrack" pt
               INNER JOIN "Playlist" p ON p."Id" = pt."PlaylistId"
               WHERE pt."TrackId" = ?
               ORDER BY p."Id""#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        rows.iter()
            .map(rows::playlist)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }

    /// The playlist and its tracks.
    ///
    /// Two lean statements, which is what the original deliberately does here
    /// rather than materializing an entity graph. Tracks are ordered by key,
    /// also as the original does — the only `GetById` that orders explicitly.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<PlaylistApiModel>> {
        let Some(header) = sqlx::query(r#"SELECT "Id", "Name" FROM "Playlist" WHERE "Id" = ?"#)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(common::database)?
        else {
            return Ok(None);
        };

        let playlist = rows::playlist(&header).map_err(common::database)?;

        let track_rows = sqlx::query(&format!(
            r#"{TRACK_MODEL_SELECT}
               INNER JOIN "PlaylistTrack" pt ON pt."TrackId" = t."Id"
               WHERE pt."PlaylistId" = ?
               ORDER BY t."Id""#
        ))
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        let tracks = track_rows
            .iter()
            .map(rows::track_model)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)?;

        Ok(Some(PlaylistApiModel {
            id: playlist.id,
            name: playlist.name,
            tracks,
        }))
    }
}
