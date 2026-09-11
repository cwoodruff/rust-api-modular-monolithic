//! Port of the C# `Identity.Module` project.
//!
//! Lands in Phase 5: RS256 token issuance, the dev and Key Vault key-material
//! providers, the in-memory user and refresh-token stores, the five auth
//! endpoints, and the policy guards every other module depends on.
//!
//! Two behaviors here are easy to "improve" by accident and must not be — both
//! are wire-visible, so the port keeps them exactly:
//!
//! - JWKS is served from `/api/identity/.well-known/jwks.json`, inside the
//!   module group rather than at the conventional root path.
//! - The tenant guard *succeeds* when a request carries no `X-Tenant-Id`
//!   header, scoping the caller to their own tenant implicitly. A missing
//!   tenant *claim*, by contrast, always fails.

/// Module name, mirroring `IModule.Name`.
pub const NAME: &str = "Identity";

/// Route prefix, mirroring `MapGroup("/api/identity")`.
pub const PREFIX: &str = "/api/identity";

/// Default access-token lifetime in minutes (`Jwt:AccessTokenMinutes`).
pub const DEFAULT_ACCESS_TOKEN_MINUTES: i64 = 15;

/// Default refresh-token lifetime in days (`Jwt:RefreshTokenDays`).
pub const DEFAULT_REFRESH_TOKEN_DAYS: i64 = 7;
