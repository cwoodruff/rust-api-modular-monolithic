//! The database probe backing `/data-health`.

use async_trait::async_trait;
use shared_kernel::health::DatabaseProbe;
use sqlx::SqlitePool;

/// Port of `db.Database.CanConnectAsync(ct)`.
#[derive(Debug, Clone)]
pub struct SqliteDatabaseProbe {
    pool: SqlitePool,
}

impl SqliteDatabaseProbe {
    /// Binds the probe to a pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl DatabaseProbe for SqliteDatabaseProbe {
    /// Runs the cheapest statement that proves a connection works.
    ///
    /// Failures are swallowed and reported as `false`, matching the original's
    /// bare `catch`. It does log, though — the C# handler discards the reason
    /// entirely, which leaves a degraded host with nothing to diagnose from.
    async fn can_connect(&self) -> bool {
        match sqlx::query_scalar::<_, i64>("SELECT 1")
            .fetch_one(&self.pool)
            .await
        {
            Ok(_) => true,
            Err(error) => {
                tracing::warn!(%error, "data-health probe could not reach the database");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::create_pool;
    use crate::tests::support;

    #[tokio::test]
    async fn a_working_pool_reports_connected() {
        let pool = create_pool(&support::bundled_database()).await.unwrap();

        assert!(SqliteDatabaseProbe::new(pool).can_connect().await);
    }

    #[tokio::test]
    async fn a_closed_pool_reports_degraded_rather_than_failing() {
        let pool = create_pool(&support::bundled_database()).await.unwrap();
        pool.close().await;

        assert!(
            !SqliteDatabaseProbe::new(pool).can_connect().await,
            "the probe must answer false rather than propagate the failure"
        );
    }
}
