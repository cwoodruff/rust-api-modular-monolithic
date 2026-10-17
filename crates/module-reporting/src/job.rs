//! Port of `IntegrityCheckJob`: the orphan check for the four cross-module
//! references that lost their foreign keys (ADR-0015).
//!
//! It runs daily at `Reporting:Integrity:RunAtUtc` (default 02:00 UTC) and on
//! demand through `POST /api/reporting/integrity/run`. Both go through the
//! Reporting work queue, so two runs never overlap. It records findings and
//! resolves the ones whose orphan has gone; it never repairs anything.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, NaiveTime, TimeDelta, Utc};
use shared_kernel::clock::Clock;
use shared_kernel::concurrency::ModuleWorkQueue;
use shared_kernel::hosting::{BoxFuture, HostedService};
use shared_kernel::persistence::DbPool;
use shared_kernel::{AppConfig, DataError, DataResult};
use tokio_util::sync::CancellationToken;

use crate::models::IntegrityRunResult;
use crate::sql::ORPHAN_CHECKS;

/// The hour the job runs when `Reporting:Integrity:RunAtUtc` is unset.
pub(crate) const DEFAULT_RUN_AT_UTC: NaiveTime = match NaiveTime::from_hms_opt(2, 0, 0) {
    Some(time) => time,
    None => NaiveTime::MIN,
};

/// What the on-demand endpoint needs: a way to run the checks now.
#[async_trait]
pub(crate) trait IntegrityRunner: Send + Sync {
    async fn run_now(&self) -> DataResult<IntegrityRunResult>;
}

/// `Reporting:Integrity:*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IntegrityOptions {
    pub enabled: bool,
    pub run_at_utc: NaiveTime,
}

impl IntegrityOptions {
    pub(crate) fn from_config(config: &AppConfig) -> Self {
        Self {
            enabled: config.get_or("Reporting:Integrity:Enabled", true),
            run_at_utc: config
                .get_string("Reporting:Integrity:RunAtUtc")
                .and_then(|value| parse_time_of_day(&value))
                .unwrap_or(DEFAULT_RUN_AT_UTC),
        }
    }
}

/// Reads .NET's `TimeSpan` text for a time of day: `02:00`, `02:00:00`, `02:00:00.5`.
fn parse_time_of_day(value: &str) -> Option<NaiveTime> {
    let value = value.trim();
    NaiveTime::parse_from_str(value, "%H:%M:%S%.f")
        .or_else(|_| NaiveTime::parse_from_str(value, "%H:%M"))
        .ok()
}

/// Port of `UntilNext(now, runAtUtc)`: today's run time if it is still ahead,
/// otherwise tomorrow's.
pub(crate) fn until_next(now: DateTime<Utc>, run_at_utc: NaiveTime) -> TimeDelta {
    let mut next = now.date_naive().and_time(run_at_utc).and_utc();
    if next <= now {
        next += TimeDelta::days(1);
    }
    next - now
}

struct Inner {
    pool: DbPool,
    queue: Arc<ModuleWorkQueue>,
    clock: Arc<dyn Clock>,
    options: IntegrityOptions,
}

/// The job. Cheap to clone.
#[derive(Clone)]
pub(crate) struct IntegrityCheckJob(Arc<Inner>);

impl std::fmt::Debug for IntegrityCheckJob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IntegrityCheckJob")
            .field("options", &self.0.options)
            .finish_non_exhaustive()
    }
}

impl IntegrityCheckJob {
    pub(crate) fn new(
        pool: DbPool,
        queue: Arc<ModuleWorkQueue>,
        clock: Arc<dyn Clock>,
        options: IntegrityOptions,
    ) -> Self {
        Self(Arc::new(Inner {
            pool,
            queue,
            clock,
            options,
        }))
    }

    /// Port of `RunChecksAsync`: every check's detect, then its resolve, then
    /// the open count across all checks. One `now` for the whole run.
    async fn run_checks(
        pool: &DbPool,
        now: DateTime<Utc>,
    ) -> Result<IntegrityRunResult, sqlx::Error> {
        let mut detected = 0_u64;
        let mut resolved = 0_u64;

        for check in ORPHAN_CHECKS {
            detected += sqlx::query(&check.detect_sql())
                .bind(check.name)
                .bind(check.source_schema)
                .bind(check.source_table)
                .bind(check.missing_reference_prefix())
                .bind(now)
                .execute(pool)
                .await?
                .rows_affected();

            resolved += sqlx::query(&check.resolve_sql())
                .bind(check.name)
                .bind(now)
                .execute(pool)
                .await?
                .rows_affected();
        }

        let open: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM reporting."IntegrityFinding" WHERE "ResolvedAt" IS NULL"#,
        )
        .fetch_one(pool)
        .await?;

        Ok(IntegrityRunResult {
            detected: i64::try_from(detected).unwrap_or(i64::MAX),
            resolved: i64::try_from(resolved).unwrap_or(i64::MAX),
            open,
            ran_at: now,
        })
    }
}

#[async_trait]
impl IntegrityRunner for IntegrityCheckJob {
    /// Port of `RunNowAsync`: waits its turn in the work queue, then runs.
    async fn run_now(&self) -> DataResult<IntegrityRunResult> {
        let pool = self.0.pool.clone();
        let clock = Arc::clone(&self.0.clock);

        // `now` is read when the run starts, not when it was queued.
        self.0
            .queue
            .run(move |_| async move { Self::run_checks(&pool, clock.now()).await })
            .await
            .map_err(DataError::database)?
            .map_err(DataError::from)
    }
}

impl HostedService for IntegrityCheckJob {
    fn name(&self) -> String {
        "IntegrityCheckJob".to_owned()
    }

    fn execute(&self, stopping: CancellationToken) -> BoxFuture<'static, ()> {
        let job = self.clone();

        Box::pin(async move {
            if !job.0.options.enabled {
                return;
            }

            loop {
                let wait = until_next(job.0.clock.now(), job.0.options.run_at_utc)
                    .to_std()
                    .unwrap_or(Duration::ZERO);

                tokio::select! {
                    () = stopping.cancelled() => return,
                    () = tokio::time::sleep(wait) => {}
                }

                match job.run_now().await {
                    Ok(result) => tracing::info!(
                        event_id = 4001,
                        "Integrity check: {} new findings, {} resolved, {} open",
                        result.detected,
                        result.resolved,
                        result.open
                    ),
                    Err(error) => {
                        if stopping.is_cancelled() {
                            return;
                        }
                        tracing::error!(event_id = 4002, %error, "Integrity check failed");
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use chrono::TimeZone;

    use super::*;

    #[test]
    fn the_next_run_is_today_when_still_ahead_and_tomorrow_otherwise() {
        let early = Utc.with_ymd_and_hms(2026, 10, 16, 1, 30, 0).unwrap();
        let late = Utc.with_ymd_and_hms(2026, 10, 16, 2, 0, 0).unwrap();

        assert_eq!(
            until_next(early, DEFAULT_RUN_AT_UTC),
            TimeDelta::minutes(30)
        );
        assert_eq!(until_next(late, DEFAULT_RUN_AT_UTC), TimeDelta::days(1));
    }

    #[test]
    fn the_run_time_reads_as_a_dotnet_time_span() {
        assert_eq!(parse_time_of_day("02:00:00"), Some(DEFAULT_RUN_AT_UTC));
        assert_eq!(
            parse_time_of_day("23:15"),
            NaiveTime::from_hms_opt(23, 15, 0)
        );
        assert_eq!(parse_time_of_day("soon"), None);
    }
}
