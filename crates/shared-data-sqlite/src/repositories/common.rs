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
/// Part of the repository contract, so it stays — but note that no *write* uses
/// it any more. A write reports what its own statement did; see
/// [`delete_by_id`].
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
///
/// # One statement, not two
///
/// This used to ask whether the row existed and then delete it. Two statements,
/// no transaction between them, so the answer to the first was already stale by
/// the time the second ran: a row deleted in between turned a concurrent
/// `DELETE` into a reported success that deleted nothing, and the client got a
/// 204 for a row someone else had removed. The same shape in `update` reported
/// success for a row that had just gone.
///
/// A statement already knows how many rows it touched. Asking it is one round
/// trip instead of two, and the answer describes what actually happened rather
/// than what was true a moment earlier.
pub(crate) async fn delete_by_id(
    pool: &SqlitePool,
    table: &str,
    id: i32,
) -> RepositoryResult<bool> {
    let statement = format!(r#"DELETE FROM "{table}" WHERE "Id" = ?"#);

    let deleted = sqlx::query(&statement)
        .bind(id)
        .execute(pool)
        .await
        .map_err(database)?;

    Ok(deleted.rows_affected() > 0)
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
