//! The connection pool, ported from `AddDbContextPool<AppDbContext>(…, 128)`.

use std::path::Path;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

/// Pool size, mirroring the original's `poolSize: 128`.
///
/// Generous for SQLite, which serializes writes on a single writer regardless —
/// but this API is overwhelmingly reads, and those do run concurrently.
pub const MAX_POOL_CONNECTIONS: u32 = 128;

/// Opens the Chinook database.
///
/// # Journal mode
///
/// Left at the database's own setting rather than switched to WAL. Journal mode
/// is recorded in the file header, so enabling it would rewrite `chinook.db` —
/// a file this repository commits — and leave the working tree dirty after
/// merely running the app. A deployment that wants WAL should set it on its own
/// copy.
///
/// # Errors
///
/// Returns the driver error if the file is missing or cannot be opened. The
/// original would instead let SQLite create an empty database and fail later on
/// the first query; failing here names the real problem.
pub async fn create_pool(database: &Path) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename(database)
        .create_if_missing(false)
        .foreign_keys(true);

    SqlitePoolOptions::new()
        .max_connections(MAX_POOL_CONNECTIONS)
        .connect_with(options)
        .await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::tests::support;

    #[tokio::test]
    async fn the_pool_opens_the_bundled_database() {
        let pool = create_pool(&support::bundled_database())
            .await
            .expect("the bundled database should open");

        let tables: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type = 'table'")
                .fetch_one(&pool)
                .await
                .expect("the schema should be readable");

        assert!(tables >= 11, "expected the Chinook tables, found {tables}");
    }

    #[tokio::test]
    async fn opening_a_missing_database_fails_rather_than_creating_one() {
        let missing = std::env::temp_dir().join("chinook-does-not-exist.db");
        let _ = std::fs::remove_file(&missing);

        assert!(create_pool(&missing).await.is_err());
        assert!(
            !missing.exists(),
            "an empty database should not be left behind"
        );
    }
}
