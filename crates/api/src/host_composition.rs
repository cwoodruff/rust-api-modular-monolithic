//! Service registration and the module list, ported from `HostComposition`.
//!
//! Upstream pulled this out of `Program.cs` so its architecture tests could
//! build the very container the application builds. The same reason holds:
//! [`build`] is what the binary runs and what every full-host test runs.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use shared_kernel::auth::{PolicyDecision, PolicyEvaluator, Principal};
use shared_kernel::clock::{self, Clock};
use shared_kernel::endpoints::{EndpointDescriptor, Endpoints, HostInfo};
use shared_kernel::hosting::RunningServices;
use shared_kernel::traffic_control;
use shared_kernel::{AppConfig, Module, Services};

use crate::{app, db_seeder, module_composition, routes};

/// The environment that loads user secrets in a place of its own.
pub const DEMO_ENVIRONMENT: &str = "Demo";

/// How a host is put together.
#[derive(Clone)]
pub struct HostOptions {
    /// Where `appsettings.json`, `data/` and the signing key live.
    pub content_root: PathBuf,
    /// The clock every module tells time with.
    pub clock: Arc<dyn Clock>,
    /// Signing keys to use instead of the configured provider — tests.
    pub identity_keys: Option<Arc<dyn module_identity::KeyMaterial>>,
}

impl std::fmt::Debug for HostOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostOptions")
            .field("content_root", &self.content_root)
            .finish_non_exhaustive()
    }
}

impl HostOptions {
    /// Options for a host rooted at `content_root`, on the system clock.
    #[must_use]
    pub fn new(content_root: impl Into<PathBuf>) -> Self {
        Self {
            content_root: content_root.into(),
            clock: clock::system(),
            identity_keys: None,
        }
    }
}

/// The module list. Port of `GetModules()`: explicit, in upstream's order —
/// Administration, Identity, Catalog, Orders, Reporting. No discovery.
#[must_use]
pub fn get_modules(options: &HostOptions) -> Vec<Box<dyn Module>> {
    let identity = match &options.identity_keys {
        Some(keys) => module_identity::IdentityModule::with_keys(Arc::clone(keys)),
        None => module_identity::IdentityModule::new(options.content_root.clone()),
    };

    vec![
        Box::new(module_admin::AdministrationModule::default()),
        Box::new(identity),
        Box::new(module_catalog::CatalogModule::default()),
        Box::new(module_orders::OrdersModule::default()),
        Box::new(module_reporting::ReportingModule::default()),
    ]
}

/// Registers the host's own services, then every module's. Port of
/// `HostComposition.ConfigureServices`.
///
/// # Errors
///
/// The first module that cannot register — a missing connection string, an
/// unusable key provider.
pub fn configure_services(
    config: Arc<AppConfig>,
    modules: &mut [Box<dyn Module>],
    clock: Arc<dyn Clock>,
) -> anyhow::Result<Services> {
    let mut services = Services::with_clock(Arc::clone(&config), clock);

    // The root endpoint's policy; each module registers its own.
    services.add_rate_limit_policy(traffic_control::policies::GLOBAL_PUBLIC_ANON);

    for module in modules.iter_mut() {
        let name = module.name();
        module
            .register_services(&mut services, &config)
            .map_err(|error| anyhow::anyhow!(error))
            .with_context(|| format!("the {name} module could not register its services"))?;
    }

    Ok(services)
}

/// A composed host: its services, its modules, its endpoints, its router.
pub struct Host {
    config: Arc<AppConfig>,
    services: Services,
    modules: Vec<Box<dyn Module>>,
    endpoints: Vec<EndpointDescriptor>,
    router: Router,
}

impl std::fmt::Debug for Host {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Host")
            .field("environment", self.config.environment())
            .field("endpoints", &self.endpoints.len())
            .finish_non_exhaustive()
    }
}

impl Host {
    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &Arc<AppConfig> {
        &self.config
    }

    /// What the modules registered.
    #[must_use]
    pub fn services(&self) -> &Services {
        &self.services
    }

    /// The modules, in order, for tests that need one by type.
    #[must_use]
    pub fn modules(&self) -> &[Box<dyn Module>] {
        &self.modules
    }

    /// Every mapped endpoint.
    #[must_use]
    pub fn endpoints(&self) -> &[EndpointDescriptor] {
        &self.endpoints
    }

    /// The application, ready to serve.
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    /// Starts the dispatchers, work queues and jobs the modules registered.
    #[must_use]
    pub fn start_hosted_services(&self) -> RunningServices {
        RunningServices::start(self.services.hosted_services())
    }
}

/// Decides nothing: what a host without Identity falls back to, so every
/// protected endpoint fails closed with 500 and the startup check names it.
struct NoPolicies;

impl PolicyEvaluator for NoPolicies {
    fn is_registered(&self, _policy: &str) -> bool {
        false
    }

    fn evaluate(&self, _policy: &str, _principal: &Principal) -> PolicyDecision {
        PolicyDecision::Unknown
    }
}

/// Builds the whole host: registration, migration and seed where the
/// environment calls for it, endpoint mapping, the startup check, the pipeline.
///
/// # Errors
///
/// Registration, migration, seeding, or endpoint validation failing.
pub async fn build(config: AppConfig, options: HostOptions) -> anyhow::Result<Host> {
    let config = Arc::new(config);
    let environment = config.environment().clone();
    let mut modules = get_modules(&options);
    let services = configure_services(
        Arc::clone(&config),
        &mut modules,
        Arc::clone(&options.clock),
    )?;

    if environment.migrates_on_startup() && config.get_or("Database:MigrateAndSeedOnStartup", false)
    {
        db_seeder::migrate_and_seed(services.databases().all(), &options.content_root)
            .await
            .context("migrating and seeding the database failed")?;
    }

    let policies = services.policies().unwrap_or_else(|| Arc::new(NoPolicies));
    let mut endpoints = Endpoints::new(
        Arc::clone(&policies),
        HostInfo {
            environment: environment.clone(),
            service_name: config.service_name(),
        },
    );

    routes::map_root(&mut endpoints, &services);
    for module in &modules {
        module.map_endpoints(&mut endpoints);
    }
    routes::map_swagger(&mut endpoints);

    let (router, descriptors) = endpoints.into_parts();

    if environment.validates_endpoints() {
        module_composition::validate_endpoints(&descriptors, policies.as_ref())
            .map_err(anyhow::Error::msg)?;
    }

    let router = app::pipeline(router, &services, &descriptors);

    Ok(Host {
        config,
        services,
        modules,
        endpoints: descriptors,
        router,
    })
}
