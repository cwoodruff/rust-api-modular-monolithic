//! Port of `Orders.Module`: invoices, invoice lines, finalization, and the
//! outbox `InvoiceFinalized` leaves through.
//!
//! The module owns schema `orders` — its tables, its outbox, its migrations,
//! its history table — and reads nothing outside it (ADR-0003). A customer
//! and a track are ids here and nothing more (ADR-0004, ADR-0005). Everything
//! except [`OrdersModule`] and its constants is private to the crate, which is
//! upstream's `PublicSurfaceTests` rule held by the compiler.

mod endpoints;
mod handlers;
mod models;
mod services;

use std::sync::Arc;

use orders_contracts::events::InvoiceFinalized;
use shared_kernel::diagnostics::ModuleMeter;
use shared_kernel::endpoints::Endpoints;
use shared_kernel::events::OutboxDispatcher;
use shared_kernel::persistence::ModuleDatabase;
use shared_kernel::traffic_control::{self, RateLimiter};
use shared_kernel::{AppConfig, Module, ModuleError, Services};

use crate::services::{
    DeadLetterService, DeadLetters, InvoiceLineService, InvoiceLines, InvoiceService, Invoices,
    Store,
};

/// Module name. Port of `IModule.Name`, and every key the module registers under.
pub const NAME: &str = "Orders";

/// Route prefix. Port of `MapGroup("/api/orders")`.
pub const PREFIX: &str = "/api/orders";

/// The schema the module owns.
pub const SCHEMA: &str = "orders";

/// The OpenAPI tag, and the name of the module's own document.
pub const TAG: &str = "Orders";

/// Entries the module's cache holds when `Caching:Modules:Orders:SizeLimit` is unset.
pub const DEFAULT_CACHE_SIZE: u64 = 1000;

/// The dispatcher's name, as its log records report it.
pub const DISPATCHER_NAME: &str = "OrdersOutboxDispatcher";

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Everything an Orders handler reads.
pub(crate) struct OrdersState {
    invoices: Arc<dyn InvoiceService>,
    invoice_lines: Arc<dyn InvoiceLineService>,
    dead_letters: Arc<dyn DeadLetterService>,
    database: ModuleDatabase,
    rate_limiter: RateLimiter,
    meter: Arc<ModuleMeter>,
}

impl OrdersState {
    /// A state over fakes, for handler tests.
    #[cfg(test)]
    fn from_services(
        invoices: Arc<dyn InvoiceService>,
        invoice_lines: Arc<dyn InvoiceLineService>,
        dead_letters: Arc<dyn DeadLetterService>,
    ) -> Self {
        let config =
            AppConfig::from_figment(Default::default(), shared_kernel::Environment::Development);
        Self {
            invoices,
            invoice_lines,
            dead_letters,
            database: ModuleDatabase::unconnected(NAME, SCHEMA, &MIGRATOR),
            rate_limiter: RateLimiter::from_config(&config, traffic_control::policies::ORDERS),
            meter: Arc::new(ModuleMeter::new(NAME)),
        }
    }
}

/// The Orders module. Port of `OrdersModule.Modules`.
#[derive(Default)]
pub struct OrdersModule {
    state: Option<Arc<OrdersState>>,
    dispatcher: Option<OutboxDispatcher>,
}

impl std::fmt::Debug for OrdersModule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OrdersModule")
            .field("registered", &self.state.is_some())
            .finish()
    }
}

impl OrdersModule {
    /// The module's outbox dispatcher, once services are registered.
    ///
    /// The test seam upstream gets by resolving `OrdersOutboxDispatcher` from
    /// the container: a test with `Outbox:Enabled=false` calls
    /// [`OutboxDispatcher::process_batch`] itself rather than waiting on a poll.
    #[must_use]
    pub fn outbox_dispatcher(&self) -> Option<OutboxDispatcher> {
        self.dispatcher.clone()
    }
}

impl Module for OrdersModule {
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
        let rate_limiter = services.add_rate_limit_policy(traffic_control::policies::ORDERS);
        let meter = services.add_module_meter(NAME);
        let publisher = services.add_event_publisher(NAME, &database);
        let dispatcher = services.add_outbox_dispatcher(
            DISPATCHER_NAME,
            NAME,
            &database,
            &[OutboxDispatcher::publishes::<InvoiceFinalized>()],
        );

        let store = Store {
            pool: database.pool().clone(),
            cache,
            keys: services.cache_keys(),
        };

        self.state = Some(Arc::new(OrdersState {
            invoices: Arc::new(Invoices {
                store: store.clone(),
                publisher,
                clock: services.clock(),
            }),
            invoice_lines: Arc::new(InvoiceLines(store)),
            dead_letters: Arc::new(DeadLetters {
                pool: database.pool().clone(),
                clock: services.clock(),
            }),
            database,
            rate_limiter,
            meter,
        }));
        self.dispatcher = Some(dispatcher);

        Ok(())
    }

    fn map_endpoints(&self, endpoints: &mut Endpoints) {
        if let Some(state) = &self.state {
            endpoints::map(endpoints, state);
        } else {
            tracing::error!("Orders mapped its endpoints before registering its services");
        }
    }
}
