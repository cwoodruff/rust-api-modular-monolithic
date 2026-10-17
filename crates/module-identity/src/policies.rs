//! The authorization policies Identity registers, ported from `PolicyRegistry`.
//!
//! One policy per permission, each requiring an authenticated caller with a
//! `permissions` claim of that value; `role.admin`, requiring the `Admin`
//! role; and `tenant.scoped`, requiring the tenant claim to match the request's
//! tenant (see [`Principal::tenant_scope_satisfied`]). Every name comes from
//! `identity-contracts`, so a module's endpoint and this registry cannot drift
//! apart without the startup check noticing.

use identity_contracts::{ADMIN_ROLE, permissions, policies};
use shared_kernel::auth::{PolicyDecision, PolicyEvaluator, Principal};

/// Port of `PolicyRegistry.RegisterPolicies`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PolicyRegistry;

impl PolicyEvaluator for PolicyRegistry {
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

        if allowed {
            PolicyDecision::Allowed
        } else {
            PolicyDecision::Denied
        }
    }
}

#[cfg(test)]
mod tests {
    use shared_kernel::AuthenticatedUser;

    use super::*;

    fn principal(
        roles: &[&str],
        permissions: &[&str],
        tenant: Option<&str>,
        header: Option<&str>,
    ) -> Principal {
        Principal::new(
            Some(AuthenticatedUser {
                subject: "user-1".to_owned(),
                name: None,
                email: None,
                tenant: tenant.map(ToOwned::to_owned),
                roles: roles.iter().map(|role| (*role).to_owned()).collect(),
                permissions: permissions
                    .iter()
                    .map(|permission| (*permission).to_owned())
                    .collect(),
            }),
            header.map(ToOwned::to_owned),
        )
    }

    #[test]
    fn every_contract_name_is_registered_and_nothing_else_is() {
        for name in permissions::ALL.iter().chain(policies::ALL.iter()) {
            assert!(PolicyRegistry.is_registered(name), "{name}");
        }
        assert!(
            !PolicyRegistry.is_registered("music.read"),
            "the pre-rename name is gone"
        );
    }

    #[test]
    fn a_permission_policy_needs_that_permission() {
        let reader = principal(&[], &["catalog.read"], Some("t"), None);

        assert_eq!(
            PolicyRegistry.evaluate("catalog.read", &reader),
            PolicyDecision::Allowed
        );
        assert_eq!(
            PolicyRegistry.evaluate("orders.read", &reader),
            PolicyDecision::Denied
        );
    }

    #[test]
    fn the_admin_policy_needs_the_admin_role() {
        assert_eq!(
            PolicyRegistry.evaluate("role.admin", &principal(&["Admin"], &[], None, None)),
            PolicyDecision::Allowed
        );
        assert_eq!(
            PolicyRegistry.evaluate("role.admin", &principal(&["User"], &[], None, None)),
            PolicyDecision::Denied
        );
    }

    #[test]
    fn the_tenant_policy_follows_the_header_rule() {
        assert_eq!(
            PolicyRegistry.evaluate(
                "tenant.scoped",
                &principal(&[], &[], Some("t-1"), Some("t-2"))
            ),
            PolicyDecision::Denied
        );
        assert_eq!(
            PolicyRegistry.evaluate("tenant.scoped", &principal(&[], &[], Some("t-1"), None)),
            PolicyDecision::Allowed
        );
    }

    #[test]
    fn an_unknown_name_is_unknown_not_denied() {
        assert_eq!(
            PolicyRegistry.evaluate("music.write", &principal(&[], &[], None, None)),
            PolicyDecision::Unknown
        );
    }
}
