//! Header-based authentication for a module's own host. Port of upstream's
//! `TestAuth`.
//!
//! A request names its caller in headers instead of presenting a token:
//!
//! | Header | Claim |
//! |---|---|
//! | `X-Test-User` | `sub`, and the display name |
//! | `X-Test-Permissions` | `permissions`, comma-separated |
//! | `X-Test-Roles` | roles, comma-separated |
//! | `X-Test-Tenant` | `tenant` |
//!
//! The policies are the same rules Identity registers — one per permission,
//! `role.admin`, `tenant.scoped` — so a module tested on its own is
//! authorized exactly as it is in the monolith. This is the third copy of
//! those rules upstream's ADR-0020 counts.

use std::sync::Arc;

use axum::http::HeaderMap;
use identity_contracts::{ADMIN_ROLE, permissions, policies};
use shared_kernel::auth::{Authenticator, PolicyDecision, PolicyEvaluator, Principal};
use shared_kernel::hosting::BoxFuture;
use shared_kernel::{AuthenticatedUser, Services};

/// Reads the caller from `X-Test-*` headers.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeaderAuthenticator;

fn list(headers: &HeaderMap, name: &str) -> Vec<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(',').map(str::trim).filter(|item| !item.is_empty()).map(ToOwned::to_owned).collect())
        .unwrap_or_default()
}

impl Authenticator for HeaderAuthenticator {
    fn authenticate<'a>(&'a self, headers: &'a HeaderMap) -> BoxFuture<'a, Option<AuthenticatedUser>> {
        Box::pin(async move {
            let subject = headers.get("X-Test-User")?.to_str().ok()?.to_owned();
            Some(AuthenticatedUser {
                name: Some(subject.clone()),
                subject,
                email: None,
                tenant: headers.get("X-Test-Tenant").and_then(|value| value.to_str().ok()).map(ToOwned::to_owned),
                roles: list(headers, "X-Test-Roles"),
                permissions: list(headers, "X-Test-Permissions"),
            })
        })
    }
}

/// Identity's policies, applied to the header caller.
#[derive(Debug, Clone, Copy, Default)]
pub struct TestPolicies;

impl PolicyEvaluator for TestPolicies {
    fn is_registered(&self, policy: &str) -> bool {
        permissions::ALL.contains(&policy) || policies::ALL.contains(&policy)
    }

    fn evaluate(&self, policy: &str, principal: &Principal) -> PolicyDecision {
        let Some(user) = principal.user.as_ref() else {
            return PolicyDecision::Denied;
        };
        let allowed = if permissions::ALL.contains(&policy) {
            user.has_permission(policy)
        } else if policy == policies::ADMIN {
            user.has_role(ADMIN_ROLE)
        } else if policy == policies::TENANT_SCOPED {
            principal.tenant_scope_satisfied()
        } else {
            return PolicyDecision::Unknown;
        };
        if allowed { PolicyDecision::Allowed } else { PolicyDecision::Denied }
    }
}

/// Registers the header scheme and the policy copy. Port of `AddTestAuthentication`.
pub fn add_test_authentication(services: &mut Services) {
    services.add_identity(Arc::new(HeaderAuthenticator), Arc::new(TestPolicies));
}

/// A caller, as a test describes one.
#[derive(Debug, Clone, Default)]
pub struct TestUser {
    /// `X-Test-User`.
    pub id: String,
    /// `X-Test-Permissions`.
    pub permissions: Vec<&'static str>,
    /// `X-Test-Roles`.
    pub roles: Vec<&'static str>,
    /// `X-Test-Tenant`, and the `X-Tenant-Id` the request names.
    pub tenant: Option<String>,
    /// An `X-Tenant-Id` differing from the claim, for mismatch tests.
    pub request_tenant: Option<String>,
}

impl TestUser {
    /// A caller in `tenant-1` holding `permissions`.
    #[must_use]
    pub fn with(permissions: &[&'static str]) -> Self {
        Self {
            id: "user-1".to_owned(),
            permissions: permissions.to_vec(),
            roles: Vec::new(),
            tenant: Some("tenant-1".to_owned()),
            request_tenant: None,
        }
    }

    /// The same caller holding the `Admin` role as well.
    #[must_use]
    pub fn admin(mut self) -> Self {
        self.roles.push(ADMIN_ROLE);
        self
    }

    /// The same caller asking for another tenant.
    #[must_use]
    pub fn asking_for_tenant(mut self, tenant: &str) -> Self {
        self.request_tenant = Some(tenant.to_owned());
        self
    }

    /// The headers that make a request this caller's.
    #[must_use]
    pub fn headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![("X-Test-User".to_owned(), self.id.clone())];
        if !self.permissions.is_empty() {
            headers.push(("X-Test-Permissions".to_owned(), self.permissions.join(",")));
        }
        if !self.roles.is_empty() {
            headers.push(("X-Test-Roles".to_owned(), self.roles.join(",")));
        }
        if let Some(tenant) = &self.tenant {
            headers.push(("X-Test-Tenant".to_owned(), tenant.clone()));
        }
        if let Some(tenant) = self.request_tenant.as_ref().or(self.tenant.as_ref()) {
            headers.push(("X-Tenant-Id".to_owned(), tenant.clone()));
        }
        headers
    }
}
