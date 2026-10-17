//! Port of `Reporting.Module`: the cross-module read model and the orphan check.
//!
//! Reporting owns schema `reporting` — its `IntegrityFinding` table and two
//! views over the Catalog, Orders and Administration schemas (ADR-0014). It
//! reads as a login in the `reporting_reader` role, which may select from those
//! three schemas and write only its own findings; it migrates as the owner.
//! The views are the coupling the upgrade accepts on purpose, and the
//! rename-detection test is what keeps it honest. Reporting has no contracts
//! crate: nothing depends on it.

mod endpoints;
mod handlers;
mod job;
mod models;
mod services;
mod sql;

use std::sync::Arc;

use shared_kernel::diagnostics::ModuleMeter;
use shared_kernel::endpoints::Endpoints;
use shared_kernel::persistence::{self, ModuleDatabase};
use shared_kernel::traffic_control::{self, RateLimiter};
use shared_kernel::{AppConfig, Module, ModuleError, Services};

use crate::job::{IntegrityCheckJob, IntegrityOptions, IntegrityRunner};
use crate::services::{ReportingService, Reports};

/// Module name. Port of `IModule.Name`, and every key the module registers under.
pub const NAME: &str = "Reporting";

/// Route prefix. Port of `MapGroup("/api/reporting")`.
pub const PREFIX: &str = "/api/reporting";

/// The schema the module owns.
pub const SCHEMA: &str = "reporting";

/// The OpenAPI tag, and the name of the module's own document.
pub const TAG: &str = "Reporting";

/// The connection string Reporting reads through: the read-only login.
pub const CONNECTION_NAME: &str = "Reporting";

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Everything a Reporting handler reads.
pub(crate) struct ReportingState {
    reports: Arc<dyn ReportingService>,
    integrity: Arc<dyn IntegrityRunner>,
    database: ModuleDatabase,
    rate_limiter: RateLimiter,
    meter: Arc<ModuleMeter>,
}

impl ReportingState {
    /// A state over fakes, for handler tests.
    #[cfg(test)]
    fn from_services(
        reports: Arc<dyn ReportingService>,
        integrity: Arc<dyn IntegrityRunner>,
    ) -> Self {
        let config =
            AppConfig::from_figment(Default::default(), shared_kernel::Environment::Development);
        Self {
            reports,
            integrity,
            database: ModuleDatabase::unconnected(NAME, SCHEMA, &MIGRATOR),
            rate_limiter: RateLimiter::from_config(&config, traffic_control::policies::REPORTING),
            meter: Arc::new(ModuleMeter::new(NAME)),
        }
    }
}

/// The Reporting module. Port of `ReportingModule.Modules`.
#[derive(Default)]
pub struct ReportingModule {
    state: Option<Arc<ReportingState>>,
}

impl std::fmt::Debug for ReportingModule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReportingModule")
            .field("registered", &self.state.is_some())
            .finish()
    }
}

impl Module for ReportingModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register_services(
        &mut self,
        services: &mut Services,
        config: &AppConfig,
    ) -> Result<(), ModuleError> {
        // Reads as the reporting login; migrates as the owner (ADR-0014).
        let database = ModuleDatabase::connect_as(
            config,
            NAME,
            SCHEMA,
            &MIGRATOR,
            CONNECTION_NAME,
            Some(persistence::CONNECTION_NAME),
        )?;
        services.add_module_database(&database);

        let meter = services.add_module_meter(NAME);
        let rate_limiter = services.add_rate_limit_policy(traffic_control::policies::REPORTING);
        let gate = services.add_module_gate(NAME);
        let queue = services.add_module_work_queue(NAME);

        let job = IntegrityCheckJob::new(
            database.pool().clone(),
            queue,
            services.clock(),
            IntegrityOptions::from_config(config),
        );
        services.add_hosted_service(Arc::new(job.clone()));

        self.state = Some(Arc::new(ReportingState {
            reports: Arc::new(Reports {
                pool: database.pool().clone(),
                gate,
            }),
            integrity: Arc::new(job),
            database,
            rate_limiter,
            meter,
        }));

        Ok(())
    }

    fn map_endpoints(&self, endpoints: &mut Endpoints) {
        if let Some(state) = &self.state {
            endpoints::map(endpoints, state);
        } else {
            tracing::error!("Reporting mapped its endpoints before registering its services");
        }
    }
}
