//! Shared pieces of the CRUD surface.
//!
//! The C# `BaseRepository<T>` gets its generic CRUD from EF Core, which can
//! build SQL for any mapped type. There is no such generic here, so each
//! repository writes its own statements — except the two that need nothing but
//! a table name, which live here.

use shared_persistence::repositories::{RepositoryError, RepositoryResult};
use sqlx::SqlitePool;

/// Wraps a driver error for the shared contract.
pub(crate) fn database(error: sqlx::Error) -> RepositoryError {
    RepositoryError::database(error)
}

/// Port of `BaseRepository.EntityExists`.
///
/// `table` is interpolated rather than bound because SQL has no parameter slot
/// for an identifier. Every caller passes one of the module-level constants
/// below, never anything derived from a request.
pub(crate) async fn exists(pool: &SqlitePool, table: &str, id: i32) -> RepositoryResult<bool> {
    let statement = format!(r#"SELECT EXISTS(SELECT 1 FROM "{table}" WHERE "Id" = ?)"#);

    let found: i64 = sqlx::query_scalar(&statement)
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(database)?;

    Ok(found != 0)
}

/// Port of `BaseRepository.Delete`.
///
/// Reports `false` for a row that is not there rather than treating it as an
/// error, which is what lets the Genre endpoint answer 404 instead of 500.
pub(crate) async fn delete_by_id(
    pool: &SqlitePool,
    table: &str,
    id: i32,
) -> RepositoryResult<bool> {
    if !exists(pool, table, id).await? {
        return Ok(false);
    }

    let statement = format!(r#"DELETE FROM "{table}" WHERE "Id" = ?"#);

    sqlx::query(&statement)
        .bind(id)
        .execute(pool)
        .await
        .map_err(database)?;

    Ok(true)
}

/// Table names, matching the singular names the C# `AppDbContext` maps to.
pub(crate) mod tables {
    pub(crate) const ALBUM: &str = "Album";
    pub(crate) const ARTIST: &str = "Artist";
    pub(crate) const CUSTOMER: &str = "Customer";
    pub(crate) const EMPLOYEE: &str = "Employee";
    pub(crate) const GENRE: &str = "Genre";
    pub(crate) const INVOICE: &str = "Invoice";
    pub(crate) const INVOICE_LINE: &str = "InvoiceLine";
    pub(crate) const MEDIA_TYPE: &str = "MediaType";
    pub(crate) const PLAYLIST: &str = "Playlist";
    pub(crate) const TRACK: &str = "Track";
}
