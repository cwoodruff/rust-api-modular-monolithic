//! Port of `samples/Catalog.Host`: the Catalog module in a process of its own.
//!
//! Never deployed. It exists so the build fails the day Catalog grows a hidden
//! dependency (ADR-0017, ADR-0020, and `docs/extraction-playbook.md`): it
//! registers Catalog and nothing else, validates Identity's tokens over HTTP
//! instead of hosting Identity, and reads `InvoiceFinalized` from Orders'
//! outbox by cursor instead of hosting Orders.
//!
//! # Dependencies
//!
//! Upstream's project references exactly `Catalog.Module` and `SharedKernel`;
//! the contracts it also uses arrive transitively, which C# allows. Cargo has
//! no transitive naming: to write `identity_contracts::permissions` or
//! `orders_contracts::events::InvoiceFinalized` this crate must name those
//! crates itself. They are the same contracts Catalog already depends on, so
//! the extraction-readiness test allows exactly these four.
//!
//! # What it serves
//!
//! Every `/api/catalog/*` route, under Catalog's own rate limit, and `/healthz`
//! as plain `Healthy` or `Unhealthy`. No root endpoint, no Swagger, no CORS,
//! no security headers — upstream's sample has none of them either.

pub mod feed;
pub mod remote_identity;

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use module_catalog::CatalogModule;
use shared_kernel::auth::{self, PolicyEvaluator};
use shared_kernel::clock;
use shared_kernel::diagnostics::{HealthCheckRegistration, HealthStatus};
use shared_kernel::endpoints::{Endpoints, HostInfo};
use shared_kernel::hosting::RunningServices;
use shared_kernel::{ApiError, AppConfig, Module, Services};

pub use feed::{FeedOptions, OrdersOutboxFeed};
pub use remote_identity::{RemoteIdentity, RemotePolicies};

/// The port `launchSettings.json` gives the sample.
pub const DEFAULT_HTTP_PORT: u16 = 5143;

/// The sample, composed.
pub struct CatalogHost {
    services: Services,
    module: CatalogModule,
    feed: OrdersOutboxFeed,
    router: Router,
}

impl std::fmt::Debug for CatalogHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CatalogHost")
            .field("module", &self.module)
            .finish_non_exhaustive()
    }
}

impl CatalogHost {
    /// The application, ready to serve.
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    /// The outbox feed, for tests that run a batch directly.
    #[must_use]
    pub fn feed(&self) -> &OrdersOutboxFeed {
        &self.feed
    }

    /// What Catalog registered.
    #[must_use]
    pub fn services(&self) -> &Services {
        &self.services
    }

    /// Starts the feed.
    #[must_use]
    pub fn start_hosted_services(&self) -> RunningServices {
        RunningServices::start(self.services.hosted_services())
    }
}

/// Builds the sample: remote identity, Catalog, the feed, the pipeline.
///
/// It does not migrate or seed: the `catalog` and `orders` schemas belong to
/// the monolith, which creates them.
///
/// # Errors
///
/// No `CatalogHost:IdentityJwksUrl`, or Catalog unable to register.
pub fn build(config: AppConfig, _content_root: &Path) -> anyhow::Result<CatalogHost> {
    let config = Arc::new(config);
    let mut services = Services::with_clock(Arc::clone(&config), clock::system());

    remote_identity::add_remote_identity(&mut services, &config)?;

    let mut module = CatalogModule::default();
    module
        .register_services(&mut services, &config)
        .map_err(|error| anyhow::anyhow!(error))
        .context("Catalog could not register its services")?;

    let database = services
        .databases()
        .get(module_catalog::NAME)
        .context("Catalog registered no database")?;
    let feed = OrdersOutboxFeed::new(
        database,
        services.subscriptions().clone(),
        services.clock(),
        FeedOptions::from_config(&config),
    );
    services.add_hosted_service(Arc::new(feed.clone()));

    let policies: Arc<dyn PolicyEvaluator> = services
        .policies()
        .context("remote identity registers the policies")?;
    let mut endpoints = Endpoints::new(
        policies,
        HostInfo {
            environment: config.environment().clone(),
            service_name: config.service_name(),
        },
    );
    module.map_endpoints(&mut endpoints);

    let checks: Arc<[HealthCheckRegistration]> = services.health_checks().to_vec().into();
    endpoints.merge_unlisted(Router::new().route(
        "/healthz",
        get(move || {
            let checks = Arc::clone(&checks);
            async move {
                for registration in checks.iter() {
                    if registration.check.check().await.status == HealthStatus::Unhealthy {
                        return (StatusCode::SERVICE_UNAVAILABLE, "Unhealthy");
                    }
                }
                (StatusCode::OK, "Healthy")
            }
        }),
    ));

    let (router, _) = endpoints.into_parts();
    let authenticator = services
        .authenticator()
        .context("remote identity registers the authenticator")?;
    let router = router
        .fallback(|| async { ApiError::NotFound.into_response() })
        .layer(axum::middleware::from_fn(move |request, next| {
            auth::authenticate(Arc::clone(&authenticator), request, next)
        }));

    Ok(CatalogHost {
        services,
        module,
        feed,
        router,
    })
}
