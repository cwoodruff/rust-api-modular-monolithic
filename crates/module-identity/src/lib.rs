//! Port of the C# `Identity.Module` project.
//!
//! RS256 token issuance, the development key provider, the in-memory user and
//! refresh-token stores, the five auth endpoints, and the authentication layer
//! that populates the principal every other module's guards read.
//!
//! Two behaviors here are easy to "improve" by accident, and both are
//! wire-visible, so the port keeps them exactly:
//!
//! - JWKS is served from `/api/identity/.well-known/jwks.json`, inside the
//!   module group rather than at the conventional root path.
//! - The tenant guard *succeeds* when a request carries no `X-Tenant-Id`
//!   header, scoping the caller to their own tenant implicitly. A missing
//!   tenant *claim*, by contrast, always fails. See
//!   [`shared_kernel::auth::Principal`].
//!
//! The claim names in [`claims`] were captured from a token the original
//! issued rather than read off its source, which is the only reason they are
//! right — see that module's documentation.

pub mod claims;
pub mod endpoints;
pub mod keys;
pub mod options;
pub mod runtime;
pub mod stores;
pub mod tokens;

use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;
use shared_kernel::{Module, health};
use shared_persistence::AppState;

pub use keys::{DevKeyMaterial, KeyError, KeyMaterial};
pub use options::{IdentityOptions, InMemoryUserRecord, JwtAuthOptions};
pub use runtime::IdentityRuntime;
pub use tokens::{TokenPair, TokenService};

/// Module name, mirroring `IModule.Name`.
pub const NAME: &str = "Identity";

/// Route prefix, mirroring `MapGroup("/api/identity")`.
pub const PREFIX: &str = "/api/identity";

/// Default access-token lifetime in minutes (`Jwt:AccessTokenMinutes`).
pub const DEFAULT_ACCESS_TOKEN_MINUTES: i64 = 15;

/// Default refresh-token lifetime in days (`Jwt:RefreshTokenDays`).
pub const DEFAULT_REFRESH_TOKEN_DAYS: i64 = 7;

/// The Identity module.
pub struct IdentityModule {
    runtime: Arc<IdentityRuntime>,
}

impl IdentityModule {
    /// Builds the module around an assembled runtime.
    #[must_use]
    pub fn new(runtime: Arc<IdentityRuntime>) -> Self {
        Self { runtime }
    }
}

impl std::fmt::Debug for IdentityModule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("IdentityModule").finish()
    }
}

impl Module<AppState> for IdentityModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn prefix(&self) -> &'static str {
        PREFIX
    }

    fn router(&self) -> Router<AppState> {
        health::routes(NAME).merge(endpoints::routes(Arc::clone(&self.runtime)))
    }
}

/// Port of `UseAuthentication()`.
///
/// Validates a bearer token and, when it checks out, puts the principal in the
/// request's extensions for the guards to read. A missing or bad token is *not*
/// rejected here: the original leaves endpoints anonymous unless they opt in,
/// so refusal is each endpoint's decision.
pub async fn authenticate(
    axum::extract::State(runtime): axum::extract::State<Arc<IdentityRuntime>>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(tokens::strip_bearer)
        .map(ToOwned::to_owned);

    if let Some(token) = token
        && let Some(user) = runtime.tokens().validate(&token)
    {
        request.extensions_mut().insert(user);
    }

    next.run(request).await
}
