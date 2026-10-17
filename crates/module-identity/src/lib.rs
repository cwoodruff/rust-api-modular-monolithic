//! Port of the C# `Identity.Module` project.
//!
//! RS256 token issuance, the key providers, the in-memory user and
//! refresh-token stores, the five auth endpoints, and the two things every
//! other module relies on without depending on this crate: the
//! [`Authenticator`](shared_kernel::auth::Authenticator) that turns a bearer
//! token into a caller, and the [`PolicyRegistry`] that decides the policy
//! names `identity-contracts` declares. Identity registers both with the host
//! in [`Module::register_services`] — upstream's `AddIdentityAuth`.
//!
//! Two behaviors here are easy to "improve" by accident, and both are
//! wire-visible, so the port keeps them exactly:
//!
//! - JWKS is served from `/api/identity/.well-known/jwks.json`, inside the
//!   module group rather than at the conventional root path.
//! - The tenant policy *succeeds* when a request carries no `X-Tenant-Id`
//!   header. A missing tenant *claim*, by contrast, always fails.

pub mod claims;
mod diagnostics;
mod endpoints;
pub mod keys;
pub mod options;
mod policies;
pub mod runtime;
pub mod stores;
pub mod tokens;

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::{HeaderMap, header};
use shared_kernel::auth::Authenticator;
use shared_kernel::endpoints::Endpoints;
use shared_kernel::hosting::BoxFuture;
use shared_kernel::traffic_control;
use shared_kernel::{AppConfig, AuthenticatedUser, Module, ModuleError, Services};

pub use diagnostics::suspicious_password_reason;
pub use keys::{DevKeyMaterial, KeyError, KeyMaterial};
pub use options::{IdentityOptions, InMemoryUserRecord, JwtAuthOptions};
pub use policies::PolicyRegistry;
pub use runtime::IdentityRuntime;
pub use tokens::{TokenPair, TokenService};

/// Module name, mirroring `IModule.Name`.
pub const NAME: &str = "Identity";

/// Route prefix, mirroring `MapGroup("/api/identity")`.
pub const PREFIX: &str = "/api/identity";

/// The OpenAPI tag, and the name of the module's own document.
pub const TAG: &str = "Identity";

/// Default access-token lifetime in minutes (`Jwt:AccessTokenMinutes`).
pub const DEFAULT_ACCESS_TOKEN_MINUTES: i64 = 15;

/// Default refresh-token lifetime in days (`Jwt:RefreshTokenDays`).
pub const DEFAULT_REFRESH_TOKEN_DAYS: i64 = 7;

/// How the module gets its signing key.
enum KeySource {
    /// From configuration, resolving relative paths against the content root.
    Configured(PathBuf),
    /// Supplied directly — tests, which must not write a key into the tree.
    Supplied(Arc<dyn KeyMaterial>),
}

/// The Identity module. Port of `IdentityModule.Modules`.
pub struct IdentityModule {
    keys: KeySource,
    state: Option<Arc<endpoints::IdentityState>>,
}

impl IdentityModule {
    /// The module, reading its key from configuration under `content_root`.
    #[must_use]
    pub fn new(content_root: impl Into<PathBuf>) -> Self {
        Self {
            keys: KeySource::Configured(content_root.into()),
            state: None,
        }
    }

    /// The module with key material supplied directly.
    #[must_use]
    pub fn with_keys(keys: Arc<dyn KeyMaterial>) -> Self {
        Self {
            keys: KeySource::Supplied(keys),
            state: None,
        }
    }

    /// The runtime, once registered. Tests reach the token service through it.
    #[must_use]
    pub fn runtime(&self) -> Option<Arc<IdentityRuntime>> {
        self.state.as_ref().map(|state| Arc::clone(&state.runtime))
    }
}

impl std::fmt::Debug for IdentityModule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityModule")
            .field("registered", &self.state.is_some())
            .finish()
    }
}

impl Module for IdentityModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register_services(
        &mut self,
        services: &mut Services,
        config: &AppConfig,
    ) -> Result<(), ModuleError> {
        let runtime = Arc::new(match &self.keys {
            KeySource::Configured(content_root) => {
                IdentityRuntime::from_config(config, content_root)?
            }
            KeySource::Supplied(keys) => IdentityRuntime::with_keys(config, Arc::clone(keys)),
        });

        services.add_identity(
            Arc::new(BearerAuthenticator(Arc::clone(&runtime))),
            Arc::new(PolicyRegistry),
        );

        self.state = Some(Arc::new(endpoints::IdentityState {
            runtime,
            rate_limiter: services.add_rate_limit_policy(traffic_control::policies::IDENTITY),
            meter: services.add_module_meter(NAME),
            config: Arc::clone(services.config()),
        }));

        Ok(())
    }

    fn map_endpoints(&self, endpoints: &mut Endpoints) {
        if let Some(state) = &self.state {
            endpoints::map(endpoints, state);
        } else {
            tracing::error!("Identity mapped its endpoints before registering its services");
        }
    }
}

/// Port of the JwtBearer scheme `AddIdentityAuth` registers.
///
/// A missing or bad token is *not* refused here: endpoints are anonymous
/// unless they opt in, so refusal is the authorization layer's decision.
struct BearerAuthenticator(Arc<IdentityRuntime>);

impl Authenticator for BearerAuthenticator {
    fn authenticate<'a>(
        &'a self,
        headers: &'a HeaderMap,
    ) -> BoxFuture<'a, Option<AuthenticatedUser>> {
        Box::pin(async move {
            let token = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(tokens::strip_bearer)?;

            self.0.tokens().validate(token)
        })
    }
}
