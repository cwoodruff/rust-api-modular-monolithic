//! Port of the C# `Identity.Module` project.
//!
//! Health endpoints are live. Phase 5 adds RS256 token issuance, the dev and
//! Key Vault key-material providers, the in-memory user and refresh-token
//! stores, the five auth endpoints, and the policy guards every other module
//! depends on.
//!
//! Two behaviors there are easy to "improve" by accident and must not be — both
//! are wire-visible, so the port will keep them exactly:
//!
//! - JWKS is served from `/api/identity/.well-known/jwks.json`, inside the
//!   module group rather than at the conventional root path.
//! - The tenant guard *succeeds* when a request carries no `X-Tenant-Id`
//!   header, scoping the caller to their own tenant implicitly. A missing
//!   tenant *claim*, by contrast, always fails.

use axum::Router;
use shared_kernel::{Module, health};
use shared_persistence::AppState;

/// Module name, mirroring `IModule.Name`.
pub const NAME: &str = "Identity";

/// Route prefix, mirroring `MapGroup("/api/identity")`.
pub const PREFIX: &str = "/api/identity";

/// Default access-token lifetime in minutes (`Jwt:AccessTokenMinutes`).
pub const DEFAULT_ACCESS_TOKEN_MINUTES: i64 = 15;

/// Default refresh-token lifetime in days (`Jwt:RefreshTokenDays`).
pub const DEFAULT_REFRESH_TOKEN_DAYS: i64 = 7;

/// The Identity module.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityModule;

impl Module<AppState> for IdentityModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn prefix(&self) -> &'static str {
        PREFIX
    }

    fn router(&self) -> Router<AppState> {
        health::routes(NAME)
    }
}
