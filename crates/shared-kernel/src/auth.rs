//! The authenticated principal and named-policy authorization, ported from
//! `ClaimsPrincipal` plus ASP.NET's `RequireAuthorization(policy)`.
//!
//! Upstream moved its policy names into `Identity.Contracts` as compiled
//! constants (ADR-0011), and every module now passes those constants to
//! `RequireAuthorization`. The framework evaluates each name against the
//! policies Identity registered, and a name nobody registered fails — at
//! startup, through `ModuleComposition.ValidateEndpoints`, and in the
//! guardrail tests.
//!
//! This is the same arrangement: an endpoint lists policy *names*
//! ([`EndpointConventions::require_authorization`](crate::endpoints::EndpointConventions::require_authorization)),
//! the [`PolicyEvaluator`] a host's identity provider registered decides each
//! one, and an unregistered name is something [`PolicyEvaluator::is_registered`]
//! can be asked about before the first request (ADR-0021).

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{FromRequestParts, Request};
use axum::http::HeaderMap;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::ProblemDetails;
use crate::errors::{ApiError, current_trace_id, status_code_page};
use crate::hosting::BoxFuture;

/// The header the tenant policy reads.
pub const TENANT_HEADER: &str = "X-Tenant-Id";

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

    /// Whether the caller holds `role`. Ordinal, as `RequireRole` compares.
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
    /// Authenticated, but a policy failed: answers 403.
    Forbidden,
}

/// A 401 an endpoint produced itself, with no `WWW-Authenticate` challenge.
///
/// Port of `Results.Unauthorized()` from a handler.
#[must_use]
pub fn unauthorized() -> Response {
    status_code_page(axum::http::StatusCode::UNAUTHORIZED, current_trace_id()).into_response()
}

impl From<AuthorizationFailure> for ApiError {
    fn from(failure: AuthorizationFailure) -> Self {
        match failure {
            AuthorizationFailure::Unauthenticated => Self::Unauthenticated,
            AuthorizationFailure::Forbidden => Self::Forbidden,
        }
    }
}

impl From<AuthorizationFailure> for ProblemDetails {
    fn from(failure: AuthorizationFailure) -> Self {
        ApiError::from(failure).into_problem(current_trace_id())
    }
}

impl IntoResponse for AuthorizationFailure {
    fn into_response(self) -> Response {
        ApiError::from(self).into_response()
    }
}

/// What an authorization decision reads: the caller and the tenant they named.
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
        Ok(Self::from_parts(&parts.extensions, &parts.headers))
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

    fn from_parts(extensions: &axum::http::Extensions, headers: &HeaderMap) -> Self {
        // Port of the tenant resolution in `TenantAuthorizationHandler`. It
        // checks the route values `tenant` and `tenantId` first, but no route
        // template in the application declares either, so only the header can
        // ever match.
        let request_tenant = headers
            .get(TENANT_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);

        Self {
            user: extensions.get::<AuthenticatedUser>().cloned(),
            request_tenant,
        }
    }

    /// The `tenant.scoped` requirement. Port of `TenantAuthorizationHandler`.
    ///
    /// Two halves, and the second surprises people:
    ///
    /// - A caller with **no tenant claim** always fails, whatever they asked for.
    /// - A caller who names **no tenant** succeeds, scoped implicitly to their
    ///   own. Isolation is therefore only enforced when the caller volunteers
    ///   the header (P6).
    ///
    /// It lives here, beside the principal, because three evaluators apply it —
    /// Identity's, `catalog-host`'s and the module test host's — and upstream
    /// keeps three copies of it (ADR-0020). One copy cannot drift.
    #[must_use]
    pub fn tenant_scope_satisfied(&self) -> bool {
        let Some(user) = self.user.as_ref() else {
            return false;
        };

        let Some(user_tenant) = user
            .tenant
            .as_deref()
            .map(str::trim)
            .filter(|tenant| !tenant.is_empty())
        else {
            tracing::warn!("Tenant-scoped access denied: user has no tenant claim");
            return false;
        };

        let Some(requested) = self.request_tenant.as_deref() else {
            tracing::debug!(
                "No X-Tenant-Id header; implicitly scoped to user tenant '{user_tenant}'"
            );
            return true;
        };

        if user_tenant == requested {
            return true;
        }

        tracing::warn!(
            "Tenant mismatch: user tenant '{user_tenant}' attempted to access tenant '{requested}'"
        );
        false
    }
}

/// How one policy came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    /// The caller satisfies it.
    Allowed,
    /// The caller does not.
    Denied,
    /// No policy by that name is registered.
    Unknown,
}

/// Decides named policies. Port of the policies `AddAuthorization` registers.
///
/// Identity registers the application's; `catalog-host` registers a copy that
/// trusts Identity's tokens over HTTP; the module test host registers one that
/// trusts request headers.
pub trait PolicyEvaluator: Send + Sync {
    /// Whether a policy by this name exists.
    fn is_registered(&self, policy: &str) -> bool;

    /// Decides one policy for an authenticated caller.
    fn evaluate(&self, policy: &str, principal: &Principal) -> PolicyDecision;
}

/// Turns a request's credentials into a caller. Port of an authentication scheme.
///
/// `None` means "no usable credentials" — absent, malformed, expired, or
/// signed by someone else. It is not a refusal: an anonymous endpoint still
/// runs, and a protected one answers 401.
pub trait Authenticator: Send + Sync {
    /// Reads the caller from the request's headers.
    fn authenticate<'a>(
        &'a self,
        headers: &'a HeaderMap,
    ) -> BoxFuture<'a, Option<AuthenticatedUser>>;
}

/// Populates the caller for every request. Port of `UseAuthentication()`.
pub async fn authenticate(
    authenticator: Arc<dyn Authenticator>,
    mut request: Request,
    next: Next,
) -> Response {
    if let Some(user) = authenticator.authenticate(request.headers()).await {
        request.extensions_mut().insert(user);
    }

    next.run(request).await
}

/// Applies an endpoint's policies. Port of `UseAuthorization()` meeting
/// `RequireAuthorization(...)` metadata.
///
/// Every policy must pass. A caller with no credentials is challenged (401)
/// before any policy is asked; an authenticated caller who fails one is
/// forbidden (403). An unregistered name is the server's fault and answers 500
/// — the startup check exists so that never reaches a client.
pub async fn authorize(
    evaluator: Arc<dyn PolicyEvaluator>,
    policies: Arc<[&'static str]>,
    request: Request,
    next: Next,
) -> Response {
    let principal = Principal::from_parts(request.extensions(), request.headers());

    if principal.user.is_none() {
        return ApiError::Unauthenticated.into_response();
    }

    for policy in policies.iter().copied() {
        match evaluator.evaluate(policy, &principal) {
            PolicyDecision::Allowed => {}
            PolicyDecision::Denied => {
                tracing::debug!(policy, "authorization refused");
                return ApiError::Forbidden.into_response();
            }
            PolicyDecision::Unknown => {
                tracing::error!(policy, "Policy '{policy}' is not registered");
                return ApiError::Internal.into_response();
            }
        }
    }

    next.run(request).await
}

/// The caller of an endpoint that required authorization.
///
/// The authorization layer has already run by the time a handler extracts
/// this, so its absence means the endpoint was mapped without
/// `require_authorization` — and that answers 401 rather than serving data to
/// nobody in particular.
#[derive(Debug, Clone)]
pub struct Caller(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for Caller
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthenticatedUser>()
            .cloned()
            .map(Self)
            .ok_or(ApiError::Unauthenticated)
    }
}

impl std::ops::Deref for Caller {
    type Target = AuthenticatedUser;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
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

    fn user(tenant: Option<&str>) -> AuthenticatedUser {
        AuthenticatedUser {
            subject: "user-1".to_owned(),
            name: Some("Demo".to_owned()),
            email: None,
            tenant: tenant.map(ToOwned::to_owned),
            roles: vec!["User".to_owned()],
            permissions: vec!["catalog.read".to_owned()],
        }
    }

    #[test]
    fn a_caller_without_a_tenant_claim_always_fails_tenant_scope() {
        let principal = Principal::new(Some(user(None)), None);

        assert!(!principal.tenant_scope_satisfied());
    }

    #[test]
    fn naming_no_tenant_succeeds_implicitly() {
        let principal = Principal::new(Some(user(Some("tenant-1"))), None);

        assert!(principal.tenant_scope_satisfied());
    }

    #[test]
    fn naming_the_matching_tenant_succeeds() {
        let principal = Principal::new(Some(user(Some("tenant-1"))), Some("tenant-1".to_owned()));

        assert!(principal.tenant_scope_satisfied());
    }

    #[test]
    fn naming_another_tenant_is_refused() {
        let principal = Principal::new(Some(user(Some("tenant-1"))), Some("tenant-2".to_owned()));

        assert!(!principal.tenant_scope_satisfied());
    }

    #[test]
    fn tenant_comparison_is_ordinal() {
        let principal = Principal::new(Some(user(Some("tenant-1"))), Some("TENANT-1".to_owned()));

        assert!(!principal.tenant_scope_satisfied());
    }

    #[test]
    fn duplicates_collapse_while_keeping_order() {
        let values = vec!["b".to_owned(), "a".to_owned(), "b".to_owned()];

        assert_eq!(deduplicate(values), ["b", "a"]);
    }
}
