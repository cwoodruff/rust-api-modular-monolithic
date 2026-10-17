//! Per-module metrics and health checks, ported from `SharedKernel.Diagnostics`.
//!
//! Upstream gives each module a `System.Diagnostics.Metrics` meter named
//! `ModularMonolith.<Module>`, tags every measurement `module=<Module>`, and
//! wires no exporter. [`ModuleMeter`] keeps the same instrument names and tags
//! in process, readable by tests and by anything a host chooses to export them
//! to. [`HealthCheck`] is the per-module database check `/healthz` reports.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

use crate::hosting::BoxFuture;
use crate::persistence::ModuleDatabase;

/// Every meter's name starts with this.
pub const METER_NAME_PREFIX: &str = "ModularMonolith.";

/// The tag every measurement carries.
pub const MODULE_TAG: &str = "module";

/// The tag request counts carry.
pub const STATUS_CODE_TAG: &str = "status_code";

/// Requests handled by the module's endpoints, by status code.
pub const REQUESTS: &str = "modmono.http.requests";
/// Cache lookups served from the module's cache.
pub const CACHE_HITS: &str = "modmono.cache.hits";
/// Cache lookups that had to load the value.
pub const CACHE_MISSES: &str = "modmono.cache.misses";
/// Events added to the module's outbox (counted at publish, before the transaction commits).
pub const OUTBOX_PUBLISHED: &str = "modmono.outbox.published";
/// Outbox events delivered to every handler.
pub const OUTBOX_DISPATCHED: &str = "modmono.outbox.dispatched";
/// Failed deliveries scheduled for a retry.
pub const OUTBOX_RETRIED: &str = "modmono.outbox.retried";
/// Events dead-lettered after the last retry.
pub const OUTBOX_DEAD_LETTERED: &str = "modmono.outbox.dead_lettered";
/// Work items waiting in the module's bounded queue.
pub const WORK_QUEUE_DEPTH: &str = "modmono.work_queue.depth";

/// One reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measurement {
    /// The instrument, such as [`REQUESTS`].
    pub instrument: &'static str,
    /// Every tag, `module` first.
    pub tags: Vec<(String, String)>,
    /// The counter's total, or the gauge's current value.
    pub value: u64,
}

type Counters = BTreeMap<(&'static str, Option<u16>), Arc<AtomicU64>>;

/// One module's meter. Port of `ModuleMeter`.
#[derive(Default)]
pub struct ModuleMeter {
    module: String,
    counters: Mutex<Counters>,
    queue_depth: OnceLock<Arc<AtomicUsize>>,
}

impl std::fmt::Debug for ModuleMeter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModuleMeter")
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}

impl ModuleMeter {
    /// The meter for `module`.
    #[must_use]
    pub fn new(module: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            ..Self::default()
        }
    }

    /// The module this meter belongs to.
    #[must_use]
    pub fn module(&self) -> &str {
        &self.module
    }

    /// The meter's name: `ModularMonolith.<Module>`.
    #[must_use]
    pub fn name(&self) -> String {
        format!("{METER_NAME_PREFIX}{}", self.module)
    }

    fn add(&self, instrument: &'static str, status: Option<u16>) {
        let counter = {
            let mut counters = self
                .counters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(counters.entry((instrument, status)).or_default())
        };

        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts a request that finished with `status_code`.
    pub fn request(&self, status_code: u16) {
        self.add(REQUESTS, Some(status_code));
    }

    /// Counts a cache hit.
    pub fn cache_hit(&self) {
        self.add(CACHE_HITS, None);
    }

    /// Counts a cache miss.
    pub fn cache_miss(&self) {
        self.add(CACHE_MISSES, None);
    }

    /// Counts an event added to the outbox.
    pub fn published(&self) {
        self.add(OUTBOX_PUBLISHED, None);
    }

    /// Counts an event delivered to every handler.
    pub fn dispatched(&self) {
        self.add(OUTBOX_DISPATCHED, None);
    }

    /// Counts a failed delivery scheduled for a retry.
    pub fn retried(&self) {
        self.add(OUTBOX_RETRIED, None);
    }

    /// Counts a dead-lettered event.
    pub fn dead_lettered(&self) {
        self.add(OUTBOX_DEAD_LETTERED, None);
    }

    /// Reports `depth` as the work-queue gauge. Only the first call counts.
    pub fn observe_queue_depth(&self, depth: Arc<AtomicUsize>) {
        let _ = self.queue_depth.set(depth);
    }

    /// A counter's total, summed over status codes.
    #[must_use]
    pub fn total(&self, instrument: &str) -> u64 {
        self.counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|((name, _), _)| *name == instrument)
            .map(|(_, counter)| counter.load(Ordering::Relaxed))
            .sum()
    }

    /// How many requests finished with `status_code`.
    #[must_use]
    pub fn requests_with_status(&self, status_code: u16) -> u64 {
        self.counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(REQUESTS, Some(status_code)))
            .map_or(0, |counter| counter.load(Ordering::Relaxed))
    }

    /// Every reading, tagged as upstream tags them.
    #[must_use]
    pub fn measurements(&self) -> Vec<Measurement> {
        let module_tag = (MODULE_TAG.to_owned(), self.module.clone());

        let mut readings: Vec<Measurement> = self
            .counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|((instrument, status), counter)| {
                let mut tags = vec![module_tag.clone()];
                if let Some(status) = status {
                    tags.push((STATUS_CODE_TAG.to_owned(), status.to_string()));
                }
                Measurement {
                    instrument,
                    tags,
                    value: counter.load(Ordering::Relaxed),
                }
            })
            .collect();

        if let Some(depth) = self.queue_depth.get() {
            readings.push(Measurement {
                instrument: WORK_QUEUE_DEPTH,
                tags: vec![module_tag],
                value: depth.load(Ordering::Relaxed) as u64,
            });
        }

        readings
    }
}

/// Counts every response a module's endpoint produced, by final status.
///
/// Port of `AddModuleMetrics`, an endpoint filter: it is applied to each of a
/// module's endpoints *inside* authorization, so a 401, 403, 429 or routing 404
/// is not counted — upstream's filters never see those either.
pub async fn record_request(meter: Arc<ModuleMeter>, request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    meter.request(response.status().as_u16());
    response
}

/// How a health check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum HealthStatus {
    /// Working.
    Healthy,
    /// Working, but not fully.
    Degraded,
    /// Not working.
    Unhealthy,
}

impl HealthStatus {
    /// The name `/healthz` reports.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "Healthy",
            Self::Degraded => "Degraded",
            Self::Unhealthy => "Unhealthy",
        }
    }
}

/// One check's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthCheckResult {
    /// The outcome.
    pub status: HealthStatus,
    /// A sentence saying why.
    pub description: String,
}

/// Something `/healthz` asks. Port of `IHealthCheck`.
pub trait HealthCheck: Send + Sync {
    /// Runs the check.
    fn check(&self) -> BoxFuture<'_, HealthCheckResult>;
}

/// A check with the name and tags `/healthz` reports it under.
#[derive(Clone)]
pub struct HealthCheckRegistration {
    /// The module name.
    pub name: String,
    /// `[<Module>, "database"]` for a module's database.
    pub tags: Vec<String>,
    /// The check.
    pub check: Arc<dyn HealthCheck>,
}

impl std::fmt::Debug for HealthCheckRegistration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HealthCheckRegistration")
            .field("name", &self.name)
            .field("tags", &self.tags)
            .finish_non_exhaustive()
    }
}

/// Whether a module's database answers. Port of `ModuleDbContextHealthCheck<T>`.
#[derive(Debug, Clone)]
pub struct DatabaseHealthCheck {
    database: ModuleDatabase,
}

impl DatabaseHealthCheck {
    /// A check on `database`.
    #[must_use]
    pub fn new(database: ModuleDatabase) -> Self {
        Self { database }
    }

    /// The registration upstream's `AddModuleDbContextHealthCheck` makes.
    #[must_use]
    pub fn registration(database: &ModuleDatabase) -> HealthCheckRegistration {
        HealthCheckRegistration {
            name: database.module().to_owned(),
            tags: vec![database.module().to_owned(), "database".to_owned()],
            check: Arc::new(Self::new(database.clone())),
        }
    }
}

impl HealthCheck for DatabaseHealthCheck {
    fn check(&self) -> BoxFuture<'_, HealthCheckResult> {
        Box::pin(async move {
            let name = self.database.context_name();

            if self.database.can_connect().await {
                HealthCheckResult {
                    status: HealthStatus::Healthy,
                    description: format!("{name} reachable"),
                }
            } else {
                HealthCheckResult {
                    status: HealthStatus::Unhealthy,
                    description: format!("{name} cannot connect"),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn every_measurement_is_tagged_with_its_module() {
        let meter = ModuleMeter::new("Catalog");
        meter.request(200);
        meter.request(200);
        meter.request(404);
        meter.cache_hit();

        assert_eq!(meter.name(), "ModularMonolith.Catalog");
        assert_eq!(meter.requests_with_status(200), 2);
        assert_eq!(meter.total(REQUESTS), 3);

        for reading in meter.measurements() {
            assert_eq!(reading.tags[0], ("module".to_owned(), "Catalog".to_owned()));
        }
    }

    #[test]
    fn the_queue_gauge_reads_the_live_count() {
        let meter = ModuleMeter::new("Orders");
        let depth = Arc::new(AtomicUsize::new(3));
        meter.observe_queue_depth(Arc::clone(&depth));

        let gauge = meter
            .measurements()
            .into_iter()
            .find(|reading| reading.instrument == WORK_QUEUE_DEPTH)
            .expect("the gauge should report");

        assert_eq!(gauge.value, 3);
    }
}
