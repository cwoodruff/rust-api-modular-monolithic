//! The authenticated principal and the policy guards, ported from
//! `ClaimsPrincipal` plus `PolicyRegistry`.
//!
//! # Why this is not in the Identity module
//!
//! In the original, `Permissions` and `PolicyRegistry` live inside
//! `Identity.Module` and every other module refers to the policies by raw
//! string, because a module may not reference another module. The C# source
//! even says so:
//!
//! > Note: In a later iteration, move these to SharedKernel to share constants
//! > across modules.
//!
//! That is exactly the move made here. `ClaimsPrincipal` is a framework type,
//! and this is the framework crate: the Identity module *produces* an
//! [`AuthenticatedUser`], and every other module *consumes* one, without either
//! side depending on the other.

use std::collections::HashSet;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};

use crate::ProblemDetails;
use crate::errors::{new_trace_id, status_code_page};

/// The header the tenant guard reads.
pub const TENANT_HEADER: &str = "X-Tenant-Id";

/// The role a caller needs for the Administration module.
pub const ADMIN_ROLE: &str = "Admin";

/// Policy names, which are the permission strings themselves.
///
/// Four of these reach no endpoint in the original — `music.write`,
/// `orders.write`, `admin.users.manage`, and `report.view` — and are kept
/// unused here for the same reason the plan keeps the dead rate-limit policies:
/// the documentation refers to them.
pub mod policies {
    /// Read the Music module.
    pub const MUSIC_READ: &str = "music.read";
    /// Write the Music module. Declared, never applied.
    pub const MUSIC_WRITE: &str = "music.write";
    /// Read the Orders module.
    pub const ORDERS_READ: &str = "orders.read";
    /// Write the Orders module. Declared, never applied.
    pub const ORDERS_WRITE: &str = "orders.write";
    /// Manage users. Declared, never applied.
    pub const ADMIN_USERS_MANAGE: &str = "admin.users.manage";
    /// Read the Administration module.
    pub const ADMINISTRATION_READ: &str = "administration.read";
    /// Write the Administration module.
    pub const ADMINISTRATION_WRITE: &str = "administration.write";
    /// View reports. Declared, never applied — Reporting has no endpoints.
    pub const REPORT_VIEW: &str = "report.view";

    /// The role-based convenience policy.
    pub const ROLE_ADMIN: &str = "role.admin";
    /// The tenant-scoping policy.
    pub const TENANT_SCOPED: &str = "tenant.scoped";
}

/// A validated caller, the port of `ClaimsPrincipal` for this application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedUser {
    /// The `sub` claim. A token without one is rejected before reaching here.
    pub subject: String,
    /// The display name, if the token carries one.
    pub name: Option<String>,
    /// The email address, if the token carries one.
    pub email: Option<String>,
    /// The `tenant` claim, if the token carries one.
    pub tenant: Option<String>,
    /// Every role claim.
    pub roles: Vec<String>,
    /// Every `permissions` claim.
    pub permissions: Vec<String>,
}

impl AuthenticatedUser {
    /// Whether the caller carries `permission`.
    #[must_use]
    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions.iter().any(|held| held == permission)
    }

    /// Whether the caller holds `role`.
    ///
    /// Ordinal comparison, matching `RequireRole`.
    #[must_use]
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|held| held == role)
    }
}

impl crate::traffic_control::RequestClaims for AuthenticatedUser {
    fn claim(&self, name: &str) -> Option<&str> {
        match name {
            "sub" => Some(self.subject.as_str()),
            "tenant" => self.tenant.as_deref(),
            _ => None,
        }
    }
}

/// One condition a request must satisfy.
///
/// The original stacks these by chaining `RequireAuthorization` calls, and
/// *every* one must pass — Administration's read endpoints carry three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// Any authenticated caller. Port of a bare `RequireAuthorization()`.
    Authenticated,
    /// A `permissions` claim with this value.
    Permission(&'static str),
    /// A role claim with this value.
    Role(&'static str),
    /// The `tenant.scoped` policy.
    TenantScope,
}

/// Why a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationFailure {
    /// No usable token: answers 401 with a `WWW-Authenticate` challenge.
    ///
    /// The header comes from the authentication middleware's challenge, so it
    /// appears only when a *protected endpoint* turns a caller away. A 401 an
    /// endpoint decides on itself — a rejected login, say — carries no
    /// challenge, and should use [`unauthorized`] instead.
    Unauthenticated,
    /// Authenticated, but a requirement failed: answers 403.
    Forbidden,
}

/// A 401 an endpoint produced itself, with no `WWW-Authenticate` challenge.
///
/// Port of `Results.Unauthorized()` from a handler. Verified against the
/// running service: a failed login answers 401 with no challenge header,
/// while a protected endpoint reached without a token answers 401 with one.
#[must_use]
pub fn unauthorized() -> Response {
    status_code_page(axum::http::StatusCode::UNAUTHORIZED, new_trace_id()).into_response()
}

impl IntoResponse for AuthorizationFailure {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Unauthenticated => axum::http::StatusCode::UNAUTHORIZED,
            Self::Forbidden => axum::http::StatusCode::FORBIDDEN,
        };

        let mut response = status_code_page(status, new_trace_id()).into_response();

        if self == Self::Unauthenticated {
            // The JWT challenge writes this; verified on the running service.
            response.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                axum::http::HeaderValue::from_static("Bearer"),
            );
        }

        response
    }
}

impl From<AuthorizationFailure> for ProblemDetails {
    fn from(failure: AuthorizationFailure) -> Self {
        let status = match failure {
            AuthorizationFailure::Unauthenticated => axum::http::StatusCode::UNAUTHORIZED,
            AuthorizationFailure::Forbidden => axum::http::StatusCode::FORBIDDEN,
        };

        status_code_page(status, new_trace_id())
    }
}

/// What a handler needs to make an authorization decision.
///
/// Extracted per request: the principal the auth layer validated, and the
/// tenant the caller asked for.
#[derive(Debug, Clone, Default)]
pub struct Principal {
    /// The validated caller, absent when the request carried no usable token.
    pub user: Option<AuthenticatedUser>,
    /// The tenant named by the request, from the `X-Tenant-Id` header.
    pub request_tenant: Option<String>,
}

impl<S> FromRequestParts<S> for Principal
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let user = parts.extensions.get::<AuthenticatedUser>().cloned();

        // Port of `HttpContextTenantResolutionService`. It checks the route
        // values `tenant` and `tenantId` first, but no route template in the
        // application declares either, so only the header can ever match.
        let request_tenant = parts
            .headers
            .get(TENANT_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);

        Ok(Self {
            user,
            request_tenant,
        })
    }
}

impl Principal {
    /// Builds a principal directly, for tests.
    #[must_use]
    pub fn new(user: Option<AuthenticatedUser>, request_tenant: Option<String>) -> Self {
        Self {
            user,
            request_tenant,
        }
    }

    /// Checks every requirement, as the original checks every stacked policy.
    ///
    /// # Errors
    ///
    /// [`AuthorizationFailure::Unauthenticated`] when no principal is present,
    /// [`AuthorizationFailure::Forbidden`] when one is but a requirement fails.
    pub fn authorize(
        &self,
        requirements: &[Requirement],
    ) -> Result<&AuthenticatedUser, AuthorizationFailure> {
        let Some(user) = self.user.as_ref() else {
            return Err(AuthorizationFailure::Unauthenticated);
        };

        for requirement in requirements {
            let satisfied = match requirement {
                Requirement::Authenticated => true,
                Requirement::Permission(permission) => user.has_permission(permission),
                Requirement::Role(role) => user.has_role(role),
                Requirement::TenantScope => self.tenant_scope_satisfied(user),
            };

            if !satisfied {
                return Err(AuthorizationFailure::Forbidden);
            }
        }

        Ok(user)
    }

    /// Port of `TenantAuthorizationHandler`.
    ///
    /// Two halves, and the second surprises people:
    ///
    /// - A caller with **no tenant claim** always fails, whatever they asked for.
    /// - A caller who names **no tenant** succeeds, scoped implicitly to their
    ///   own. The original's comment explains it: this "allows Swagger/curl
    ///   usage without the header while still enforcing tenant isolation when
    ///   the header IS provided". Isolation is therefore only enforced when the
    ///   caller volunteers the header — which is worth knowing before relying
    ///   on it, and is why the port keeps it rather than tightening it.
    fn tenant_scope_satisfied(&self, user: &AuthenticatedUser) -> bool {
        let Some(user_tenant) = user
            .tenant
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        else {
            tracing::warn!("tenant-scoped access denied: user has no tenant claim");
            return false;
        };

        let Some(requested) = self.request_tenant.as_deref() else {
            tracing::debug!(
                user_tenant,
                "no X-Tenant-Id header; implicitly scoped to the user's tenant"
            );
            return true;
        };

        if user_tenant == requested {
            return true;
        }

        tracing::warn!(
            user_tenant,
            requested,
            "tenant mismatch: cross-tenant access attempt"
        );
        false
    }
}

/// The requirements the Administration module's read endpoints stack.
///
/// Three policies, all of which must pass — the only module that does this.
#[must_use]
pub fn administration_read() -> Vec<Requirement> {
    vec![
        Requirement::Role(ADMIN_ROLE),
        Requirement::Permission(policies::ADMINISTRATION_READ),
        Requirement::TenantScope,
    ]
}

/// The requirements the Administration module's write endpoints stack.
#[must_use]
pub fn administration_write() -> Vec<Requirement> {
    vec![
        Requirement::Role(ADMIN_ROLE),
        Requirement::Permission(policies::ADMINISTRATION_WRITE),
        Requirement::TenantScope,
    ]
}

/// The requirements a Music or Orders data endpoint stacks.
#[must_use]
pub fn read_scoped(permission: &'static str) -> Vec<Requirement> {
    vec![
        Requirement::Permission(permission),
        Requirement::TenantScope,
    ]
}

/// Collapses duplicate values while preserving order.
#[must_use]
pub fn deduplicate(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn user() -> AuthenticatedUser {
        AuthenticatedUser {
            subject: "user-1".to_owned(),
            name: Some("Demo".to_owned()),
            email: None,
            tenant: Some("tenant-1".to_owned()),
            roles: vec!["User".to_owned()],
            permissions: vec![policies::MUSIC_READ.to_owned()],
        }
    }

    fn admin() -> AuthenticatedUser {
        AuthenticatedUser {
            subject: "admin-1".to_owned(),
            name: Some("Admin".to_owned()),
            email: None,
            tenant: Some("tenant-1".to_owned()),
            roles: vec![ADMIN_ROLE.to_owned()],
            permissions: vec![
                policies::ADMINISTRATION_READ.to_owned(),
                policies::ADMINISTRATION_WRITE.to_owned(),
            ],
        }
    }

    #[test]
    fn a_request_without_a_principal_is_unauthenticated_not_forbidden() {
        // The distinction is wire-visible: 401 with a challenge, versus 403.
        let principal = Principal::default();

        assert_eq!(
            principal.authorize(&[Requirement::Authenticated]),
            Err(AuthorizationFailure::Unauthenticated)
        );
    }

    #[test]
    fn a_principal_without_the_permission_is_forbidden() {
        let principal = Principal::new(Some(user()), None);

        assert_eq!(
            principal.authorize(&read_scoped(policies::ORDERS_READ)),
            Err(AuthorizationFailure::Forbidden)
        );
    }

    #[test]
    fn a_principal_with_the_permission_passes() {
        let principal = Principal::new(Some(user()), None);

        assert!(
            principal
                .authorize(&read_scoped(policies::MUSIC_READ))
                .is_ok()
        );
    }

    #[test]
    fn every_stacked_requirement_must_pass() {
        // Administration stacks role, permission and tenant. Holding the
        // permission but not the role is not enough.
        let almost = AuthenticatedUser {
            roles: vec!["User".to_owned()],
            ..admin()
        };

        assert_eq!(
            Principal::new(Some(almost), None).authorize(&administration_read()),
            Err(AuthorizationFailure::Forbidden)
        );
        assert!(
            Principal::new(Some(admin()), None)
                .authorize(&administration_read())
                .is_ok()
        );
    }

    #[test]
    fn the_write_policy_is_not_satisfied_by_the_read_permission() {
        let reader = AuthenticatedUser {
            permissions: vec![policies::ADMINISTRATION_READ.to_owned()],
            ..admin()
        };

        assert!(
            Principal::new(Some(reader), None)
                .authorize(&administration_read())
                .is_ok()
        );
        assert_eq!(
            Principal::new(
                Some(AuthenticatedUser {
                    permissions: vec![policies::ADMINISTRATION_READ.to_owned()],
                    ..admin()
                }),
                None
            )
            .authorize(&administration_write()),
            Err(AuthorizationFailure::Forbidden)
        );
    }

    #[test]
    fn a_caller_without_a_tenant_claim_always_fails_tenant_scope() {
        let tenantless = AuthenticatedUser {
            tenant: None,
            ..user()
        };

        assert_eq!(
            Principal::new(Some(tenantless), None).authorize(&[Requirement::TenantScope]),
            Err(AuthorizationFailure::Forbidden),
            "a missing tenant claim fails even with no tenant requested"
        );
    }

    #[test]
    fn naming_no_tenant_succeeds_implicitly() {
        // The surprising half: isolation is enforced only when the caller
        // volunteers the header.
        let principal = Principal::new(Some(user()), None);

        assert!(principal.authorize(&[Requirement::TenantScope]).is_ok());
    }

    #[test]
    fn naming_the_matching_tenant_succeeds() {
        let principal = Principal::new(Some(user()), Some("tenant-1".to_owned()));

        assert!(principal.authorize(&[Requirement::TenantScope]).is_ok());
    }

    #[test]
    fn naming_another_tenant_is_refused() {
        let principal = Principal::new(Some(user()), Some("tenant-2".to_owned()));

        assert_eq!(
            principal.authorize(&[Requirement::TenantScope]),
            Err(AuthorizationFailure::Forbidden)
        );
    }

    #[test]
    fn tenant_comparison_is_ordinal() {
        // `string.Equals(..., StringComparison.Ordinal)` — case matters.
        let principal = Principal::new(Some(user()), Some("TENANT-1".to_owned()));

        assert_eq!(
            principal.authorize(&[Requirement::TenantScope]),
            Err(AuthorizationFailure::Forbidden)
        );
    }

    #[test]
    fn duplicates_collapse_while_keeping_order() {
        assert_eq!(
            deduplicate(vec![
                "a".to_owned(),
                "b".to_owned(),
                "a".to_owned(),
                "c".to_owned()
            ]),
            vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]
        );
    }
}
