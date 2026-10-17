//! Port of `Admin.Module`: customers, employees, genres, media types, and the
//! purchase summaries `InvoiceFinalized` feeds.
//!
//! The module owns schema `administration` and reads nothing outside it
//! (ADR-0003). It is the only module with a write surface: genre create,
//! update and delete. Everything except [`AdministrationModule`] and its
//! constants is private to the crate.

mod endpoints;
mod events;
mod handlers;
mod models;
mod services;
mod validation;

use std::sync::Arc;

use orders_contracts::events::InvoiceFinalized;
use shared_kernel::diagnostics::ModuleMeter;
use shared_kernel::endpoints::Endpoints;
use shared_kernel::persistence::ModuleDatabase;
use shared_kernel::traffic_control::{self, RateLimiter};
use shared_kernel::{AppConfig, Module, ModuleError, Services};

use crate::services::{
    CustomerService, Customers, EmployeeService, Employees, GenreService, Genres, MediaTypeService,
    MediaTypes, Store,
};

/// Module name. Port of `IModule.Name`.
pub const NAME: &str = "Administration";

/// Route prefix. Port of `MapGroup("/api/admin")` — note it is not the name.
pub const PREFIX: &str = "/api/admin";

/// The schema the module owns.
pub const SCHEMA: &str = "administration";

/// The OpenAPI tag, and the name of the module's own document.
pub const TAG: &str = "Administration";

/// Entries the module's cache holds when `Caching:Modules:Administration:SizeLimit` is unset.
pub const DEFAULT_CACHE_SIZE: u64 = 500;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Everything an Administration handler reads.
pub(crate) struct AdministrationState {
    customers: Arc<dyn CustomerService>,
    employees: Arc<dyn EmployeeService>,
    genres: Arc<dyn GenreService>,
    media_types: Arc<dyn MediaTypeService>,
    database: ModuleDatabase,
    rate_limiter: RateLimiter,
    meter: Arc<ModuleMeter>,
}

impl AdministrationState {
    /// A state over fakes, for handler tests.
    #[cfg(test)]
    fn from_services(
        customers: Arc<dyn CustomerService>,
        employees: Arc<dyn EmployeeService>,
        genres: Arc<dyn GenreService>,
        media_types: Arc<dyn MediaTypeService>,
    ) -> Self {
        let config =
            AppConfig::from_figment(Default::default(), shared_kernel::Environment::Development);
        Self {
            customers,
            employees,
            genres,
            media_types,
            database: ModuleDatabase::unconnected(NAME, SCHEMA, &MIGRATOR),
            rate_limiter: RateLimiter::from_config(
                &config,
                traffic_control::policies::ADMINISTRATION,
            ),
            meter: Arc::new(ModuleMeter::new(NAME)),
        }
    }
}

/// The Administration module. Port of `AdministrationModule.Modules`.
#[derive(Default)]
pub struct AdministrationModule {
    state: Option<Arc<AdministrationState>>,
}

impl std::fmt::Debug for AdministrationModule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdministrationModule")
            .field("registered", &self.state.is_some())
            .finish()
    }
}

impl Module for AdministrationModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register_services(
        &mut self,
        services: &mut Services,
        config: &AppConfig,
    ) -> Result<(), ModuleError> {
        let database = ModuleDatabase::connect(config, NAME, SCHEMA, &MIGRATOR)?;
        services.add_module_database(&database);

        let cache = services.add_module_cache(NAME, DEFAULT_CACHE_SIZE);
        let rate_limiter =
            services.add_rate_limit_policy(traffic_control::policies::ADMINISTRATION);
        let meter = services.add_module_meter(NAME);
        services.add_integration_event_handler::<InvoiceFinalized, _>(
            NAME,
            events::InvoiceFinalizedHandler,
        );

        let store = Store {
            pool: database.pool().clone(),
            cache,
            keys: services.cache_keys(),
        };

        self.state = Some(Arc::new(AdministrationState {
            customers: Arc::new(Customers(store.clone())),
            employees: Arc::new(Employees(store.clone())),
            genres: Arc::new(Genres(store.clone())),
            media_types: Arc::new(MediaTypes(store)),
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
            tracing::error!("Administration mapped its endpoints before registering its services");
        }
    }
}
