//! The module contract, ported from `IModule`.
//!
//! ```csharp
//! string Name { get; }
//! void RegisterServices(IServiceCollection services, IConfiguration config);
//! void MapEndpoints(IEndpointRouteBuilder endpoints);
//! ```
//!
//! All three members survive. [`Services`] stands where `IServiceCollection`
//! stands, but it is a typed composition object rather than a container:
//! a module builds its own services from configuration, keeps them, and
//! registers with the host only what the host must run or report on — hosted
//! services, event subscriptions, its database, health checks, its cache, work
//! queue, gate and meter, its rate-limit policy. Nothing in [`Services`] lets
//! one module reach another module's services, which is the Rust form of
//! upstream's "no module service resolves to another module's type"
//! (ADR-0021).
//!
//! The host keeps an explicit, hand-written module list, as upstream's
//! `GetModules()` does. No discovery.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::AppConfig;
use crate::auth::{Authenticator, PolicyEvaluator};
use crate::caching::{self, CacheFacade, CacheKeyComposer, CacheOptions};
use crate::clock::{self, Clock};
use crate::concurrency::{self, ModuleGate, ModuleWorkQueue};
use crate::diagnostics::{DatabaseHealthCheck, HealthCheckRegistration, ModuleMeter};
use crate::endpoints::Endpoints;
use crate::environment::Environment;
use crate::events::dispatcher::DispatcherParts;
use crate::events::{
    EventDecoder, EventPublisher, EventSubscriptions, IntegrationEvent, IntegrationEventHandler,
    ModuleDatabases, OutboxDispatcher, OutboxOptions, Subscription,
};
use crate::hosting::HostedService;
use crate::persistence::ModuleDatabase;
use crate::traffic_control::RateLimiter;

/// What a module's registration can fail with.
pub type ModuleError = Box<dyn std::error::Error + Send + Sync>;

/// One module of the monolith. Port of `IModule`.
pub trait Module: Send + Sync {
    /// The module's name: `Catalog`, `Orders`, `Administration`, `Identity`, `Reporting`.
    fn name(&self) -> &'static str;

    /// Builds the module's services and registers what the host needs to know.
    ///
    /// # Errors
    ///
    /// Configuration the module cannot start with, such as a missing
    /// connection string.
    fn register_services(
        &mut self,
        services: &mut Services,
        config: &AppConfig,
    ) -> Result<(), ModuleError>;

    /// Maps the module's routes. Runs after every module has registered.
    fn map_endpoints(&self, endpoints: &mut Endpoints);
}

/// What modules register with the host. Port of `IServiceCollection`, typed.
pub struct Services {
    config: Arc<AppConfig>,
    clock: Arc<dyn Clock>,
    cache_keys: Arc<CacheKeyComposer>,
    hosted: Vec<Arc<dyn HostedService>>,
    subscriptions: EventSubscriptions,
    databases: ModuleDatabases,
    health_checks: Vec<HealthCheckRegistration>,
    meters: BTreeMap<String, Arc<ModuleMeter>>,
    caches: BTreeMap<String, Arc<CacheFacade>>,
    work_queues: BTreeMap<String, Arc<ModuleWorkQueue>>,
    gates: BTreeMap<String, ModuleGate>,
    rate_limiters: BTreeMap<&'static str, RateLimiter>,
    authenticator: Option<Arc<dyn Authenticator>>,
    policies: Option<Arc<dyn PolicyEvaluator>>,
}

impl std::fmt::Debug for Services {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Services")
            .field("hosted", &self.hosted.len())
            .field("databases", &self.databases)
            .field("caches", &self.caches.keys().collect::<Vec<_>>())
            .field("work_queues", &self.work_queues.keys().collect::<Vec<_>>())
            .field("gates", &self.gates.keys().collect::<Vec<_>>())
            .field(
                "rate_limiters",
                &self.rate_limiters.keys().collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl Services {
    /// An empty collection over `config`, telling time with the system clock.
    #[must_use]
    pub fn new(config: Arc<AppConfig>) -> Self {
        Self::with_clock(config, clock::system())
    }

    /// An empty collection with an explicit clock, for tests.
    #[must_use]
    pub fn with_clock(config: Arc<AppConfig>, clock: Arc<dyn Clock>) -> Self {
        let cache_keys = Arc::new(CacheKeyComposer::new(&config));

        Self {
            config,
            clock,
            cache_keys,
            hosted: Vec::new(),
            subscriptions: EventSubscriptions::default(),
            databases: ModuleDatabases::default(),
            health_checks: Vec::new(),
            meters: BTreeMap::new(),
            caches: BTreeMap::new(),
            work_queues: BTreeMap::new(),
            gates: BTreeMap::new(),
            rate_limiters: BTreeMap::new(),
            authenticator: None,
            policies: None,
        }
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &Arc<AppConfig> {
        &self.config
    }

    /// The environment the host runs as.
    #[must_use]
    pub fn environment(&self) -> &Environment {
        self.config.environment()
    }

    /// The clock. Port of `TimeProvider`.
    #[must_use]
    pub fn clock(&self) -> Arc<dyn Clock> {
        Arc::clone(&self.clock)
    }

    /// The cache-key composer every module shares. Port of `ICacheKeyComposer`.
    #[must_use]
    pub fn cache_keys(&self) -> Arc<CacheKeyComposer> {
        Arc::clone(&self.cache_keys)
    }

    /// Port of `AddHostedService`.
    pub fn add_hosted_service(&mut self, service: Arc<dyn HostedService>) {
        self.hosted.push(service);
    }

    /// Every hosted service, in registration order.
    #[must_use]
    pub fn hosted_services(&self) -> &[Arc<dyn HostedService>] {
        &self.hosted
    }

    /// Port of `AddModuleDbContext`: registers the module's database for
    /// migration, for the dispatcher's inbox lookups, and as a `/healthz` check.
    pub fn add_module_database(&mut self, database: &ModuleDatabase) {
        self.databases.add(database.clone());
        self.health_checks
            .push(DatabaseHealthCheck::registration(database));
    }

    /// Every module database, by module name.
    #[must_use]
    pub fn databases(&self) -> &ModuleDatabases {
        &self.databases
    }

    /// Adds a `/healthz` check.
    pub fn add_health_check(&mut self, registration: HealthCheckRegistration) {
        self.health_checks.push(registration);
    }

    /// Every `/healthz` check, in registration order.
    #[must_use]
    pub fn health_checks(&self) -> &[HealthCheckRegistration] {
        &self.health_checks
    }

    /// Port of `AddModuleMeter`: the module's meter, created once.
    pub fn add_module_meter(&mut self, module: &str) -> Arc<ModuleMeter> {
        Arc::clone(
            self.meters
                .entry(module.to_owned())
                .or_insert_with(|| Arc::new(ModuleMeter::new(module))),
        )
    }

    /// Every module meter, by module name.
    #[must_use]
    pub fn meters(&self) -> &BTreeMap<String, Arc<ModuleMeter>> {
        &self.meters
    }

    /// Port of `AddModuleCache(module, sizeLimit)`: a cache of the module's own.
    ///
    /// The limit is read from `Caching:Modules:<module>:SizeLimit`, falling
    /// back to `default_size_limit`.
    pub fn add_module_cache(&mut self, module: &str, default_size_limit: u64) -> Arc<CacheFacade> {
        let options: CacheOptions = self
            .config
            .section_or_default(caching::CONFIGURATION_SECTION);
        let size_limit = self
            .config
            .get_or(&caching::size_limit_key(module), default_size_limit)
            .max(1);
        let meter = self.add_module_meter(module);

        let cache = Arc::new(CacheFacade::for_module(options, module, size_limit, meter));
        self.caches.insert(module.to_owned(), Arc::clone(&cache));
        cache
    }

    /// Every module cache, by module name.
    #[must_use]
    pub fn caches(&self) -> &BTreeMap<String, Arc<CacheFacade>> {
        &self.caches
    }

    /// Port of `AddModuleWorkQueue`: a bounded queue of
    /// `Concurrency:<module>:QueueCapacity` (default 64), started as a hosted
    /// service, its depth reported on the module's meter.
    pub fn add_module_work_queue(&mut self, module: &'static str) -> Arc<ModuleWorkQueue> {
        if let Some(queue) = self.work_queues.get(module) {
            return Arc::clone(queue);
        }

        let capacity = self
            .config
            .get_or(
                &format!("Concurrency:{module}:QueueCapacity"),
                concurrency::DEFAULT_QUEUE_CAPACITY,
            )
            .max(1);
        let queue = Arc::new(ModuleWorkQueue::new(module, capacity));

        self.add_module_meter(module)
            .observe_queue_depth(queue.depth_probe());
        self.hosted.push(Arc::new(Arc::clone(&queue)));
        self.work_queues
            .insert(module.to_owned(), Arc::clone(&queue));
        queue
    }

    /// Every module work queue, by module name.
    #[must_use]
    pub fn work_queues(&self) -> &BTreeMap<String, Arc<ModuleWorkQueue>> {
        &self.work_queues
    }

    /// Port of `AddModuleGate`: `Concurrency:<module>:MaxConcurrentExpensive`
    /// (default 4) at once.
    pub fn add_module_gate(&mut self, module: &'static str) -> ModuleGate {
        let limit = self
            .config
            .get_or(
                &format!("Concurrency:{module}:MaxConcurrentExpensive"),
                concurrency::DEFAULT_MAX_CONCURRENT_EXPENSIVE,
            )
            .max(1);

        self.gates
            .entry(module.to_owned())
            .or_insert_with(|| ModuleGate::new(module, limit))
            .clone()
    }

    /// Every module gate, by module name.
    #[must_use]
    pub fn gates(&self) -> &BTreeMap<String, ModuleGate> {
        &self.gates
    }

    /// Port of `AddModuleRateLimitPolicy(name)`: the policy's limiter, created
    /// once, with its limits from `RateLimiting:Policies:<name>`.
    pub fn add_rate_limit_policy(&mut self, name: &'static str) -> RateLimiter {
        self.rate_limiters
            .entry(name)
            .or_insert_with(|| RateLimiter::from_config(&self.config, name))
            .clone()
    }

    /// Every rate-limit policy, by name.
    #[must_use]
    pub fn rate_limiters(&self) -> &BTreeMap<&'static str, RateLimiter> {
        &self.rate_limiters
    }

    /// Port of `AddIntegrationEventHandler<TEvent, THandler>(moduleKey)`.
    pub fn add_integration_event_handler<E, H>(&mut self, module_key: &'static str, handler: H)
    where
        E: IntegrationEvent,
        H: IntegrationEventHandler<E>,
    {
        self.subscriptions
            .add(Subscription::new::<E, H>(module_key, handler));
    }

    /// Every module's subscriptions.
    #[must_use]
    pub fn subscriptions(&self) -> &EventSubscriptions {
        &self.subscriptions
    }

    /// Port of `AddEventPublisher(module)`: writes into `database`'s outbox.
    pub fn add_event_publisher(
        &mut self,
        module: &str,
        database: &ModuleDatabase,
    ) -> EventPublisher {
        let meter = self.add_module_meter(module);
        EventPublisher::new(database.schema(), self.clock(), meter)
    }

    /// Port of `AddOutboxDispatcher<T>(module)`: polls `database`'s outbox for
    /// `publishes`, through the module's work queue, as a hosted service.
    pub fn add_outbox_dispatcher(
        &mut self,
        name: &str,
        module: &'static str,
        database: &ModuleDatabase,
        publishes: &[EventDecoder],
    ) -> OutboxDispatcher {
        let queue = self.add_module_work_queue(module);
        let meter = self.add_module_meter(module);

        let dispatcher = OutboxDispatcher::new(
            name,
            DispatcherParts {
                database: database.clone(),
                subscriptions: self.subscriptions.clone(),
                databases: self.databases.clone(),
                queue,
                meter,
                clock: self.clock(),
                options: OutboxOptions::from_config(&self.config),
            },
            publishes,
        );

        self.hosted.push(Arc::new(dispatcher.clone()));
        dispatcher
    }

    /// Port of `AddAuthentication` + `AddAuthorization`: how callers are
    /// identified and how named policies are decided.
    pub fn add_identity(
        &mut self,
        authenticator: Arc<dyn Authenticator>,
        policies: Arc<dyn PolicyEvaluator>,
    ) {
        self.authenticator = Some(authenticator);
        self.policies = Some(policies);
    }

    /// The registered authenticator, if a module registered one.
    #[must_use]
    pub fn authenticator(&self) -> Option<Arc<dyn Authenticator>> {
        self.authenticator.clone()
    }

    /// The registered policy evaluator, if a module registered one.
    #[must_use]
    pub fn policies(&self) -> Option<Arc<dyn PolicyEvaluator>> {
        self.policies.clone()
    }
}
