//! The access token's claim set.
//!
//! # These names were verified, not inferred
//!
//! The host clears `JwtSecurityTokenHandler.DefaultInboundClaimTypeMap` but
//! leaves the **outbound** map alone, so the name, email and role claims are
//! written as the full XML Schema URIs rather than the short `name`/`email`/
//! `role` the code's `ClaimTypes.Name` spelling suggests. Reading the source
//! would lead you to the short names; a captured token shows otherwise.
//!
//! Two more quirks came from the same capture:
//!
//! - **`aud` appears twice.** `JwtSecurityToken`'s constructor takes an
//!   audience *and* the claim list already carries one, so the value is
//!   duplicated and the member is an array.
//! - **Repeated claims collapse.** One role serializes as a string, two or more
//!   as an array. Any reader has to accept both.

use serde::{Deserialize, Serialize};
use shared_kernel::AuthenticatedUser;

/// The `name` claim, as actually emitted.
pub const NAME_CLAIM: &str = "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name";

/// The `email` claim, as actually emitted.
pub const EMAIL_CLAIM: &str = "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress";

/// The `role` claim, as actually emitted.
pub const ROLE_CLAIM: &str = "http://schemas.microsoft.com/ws/2008/06/identity/claims/role";

/// The tenant claim, which is plain.
pub const TENANT_CLAIM: &str = "tenant";

/// The permissions claim, which is plain.
pub const PERMISSIONS_CLAIM: &str = "permissions";

/// A claim that is a single string when there is one value and an array when
/// there are several.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ClaimValues {
    /// Exactly one value.
    One(String),
    /// Several values.
    Many(Vec<String>),
}

impl ClaimValues {
    /// Builds the collapsed form, or `None` when there is nothing to emit.
    #[must_use]
    pub fn from_values(mut values: Vec<String>) -> Option<Self> {
        match values.len() {
            0 => None,
            1 => Some(Self::One(values.remove(0))),
            _ => Some(Self::Many(values)),
        }
    }

    /// Expands back into a list.
    #[must_use]
    pub fn into_values(self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

/// Expands an optional collapsed claim.
#[must_use]
pub fn values_of(claim: Option<ClaimValues>) -> Vec<String> {
    claim.map(ClaimValues::into_values).unwrap_or_default()
}

/// The access token payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessTokenClaims {
    /// The subject. A token without one is rejected.
    pub sub: String,
    /// Token identifier, a GUID with no dashes.
    pub jti: String,
    /// Issued-at, in seconds.
    pub iat: i64,
    /// Not-before, in seconds.
    pub nbf: i64,
    /// Expiry, in seconds.
    pub exp: i64,
    /// The issuer.
    pub iss: String,
    /// The audience — emitted **twice**, as an array. See the module docs.
    pub aud: Vec<String>,

    /// The display name.
    #[serde(rename = "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,

    /// The email address.
    #[serde(rename = "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub email: Option<String>,

    /// The tenant.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tenant: Option<String>,

    /// The roles.
    #[serde(rename = "http://schemas.microsoft.com/ws/2008/06/identity/claims/role")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub roles: Option<ClaimValues>,

    /// The permissions.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permissions: Option<ClaimValues>,
}

impl AccessTokenClaims {
    /// Turns validated claims into the principal the guards read.
    #[must_use]
    pub fn into_user(self) -> AuthenticatedUser {
        AuthenticatedUser {
            subject: self.sub,
            name: self.name,
            email: self.email,
            tenant: self.tenant,
            roles: values_of(self.roles),
            permissions: values_of(self.permissions),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use serde_json::json;

    fn claims() -> AccessTokenClaims {
        AccessTokenClaims {
            sub: "admin-1".to_owned(),
            jti: "fe007a21e998409c8102c019e4166292".to_owned(),
            iat: 1_789_167_203,
            nbf: 1_789_167_203,
            exp: 1_789_168_103,
            iss: "https://auth.local".to_owned(),
            aud: vec!["modular-api".to_owned(), "modular-api".to_owned()],
            name: Some("Admin User".to_owned()),
            email: Some("admin@example.com".to_owned()),
            tenant: Some("tenant-1".to_owned()),
            roles: ClaimValues::from_values(vec!["Admin".to_owned()]),
            permissions: ClaimValues::from_values(vec![
                "music.read".to_owned(),
                "administration.read".to_owned(),
            ]),
        }
    }

    /// The payload captured from the running C# service, byte for byte.
    #[test]
    fn the_payload_matches_a_token_the_original_issued() {
        assert_eq!(
            serde_json::to_value(claims()).unwrap(),
            json!({
                "sub": "admin-1",
                "jti": "fe007a21e998409c8102c019e4166292",
                "iat": 1_789_167_203,
                "nbf": 1_789_167_203,
                "exp": 1_789_168_103,
                "iss": "https://auth.local",
                "aud": ["modular-api", "modular-api"],
                "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name": "Admin User",
                "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress": "admin@example.com",
                "tenant": "tenant-1",
                "http://schemas.microsoft.com/ws/2008/06/identity/claims/role": "Admin",
                "permissions": ["music.read", "administration.read"]
            })
        );
    }

    #[test]
    fn the_audience_is_duplicated_as_the_original_duplicates_it() {
        let document = serde_json::to_value(claims()).unwrap();

        assert_eq!(document["aud"], json!(["modular-api", "modular-api"]));
    }

    #[test]
    fn one_value_collapses_to_a_string_and_several_stay_an_array() {
        let single =
            serde_json::to_value(ClaimValues::from_values(vec!["Admin".to_owned()])).unwrap();
        let several = serde_json::to_value(ClaimValues::from_values(vec![
            "Admin".to_owned(),
            "Manager".to_owned(),
        ]))
        .unwrap();

        assert_eq!(single, json!("Admin"));
        assert_eq!(several, json!(["Admin", "Manager"]));
        assert_eq!(ClaimValues::from_values(Vec::new()), None);
    }

    #[test]
    fn both_collapsed_forms_read_back() {
        for (raw, expected) in [
            (json!("Admin"), vec!["Admin".to_owned()]),
            (
                json!(["Admin", "Manager"]),
                vec!["Admin".to_owned(), "Manager".to_owned()],
            ),
        ] {
            let parsed: ClaimValues = serde_json::from_value(raw).unwrap();
            assert_eq!(parsed.into_values(), expected);
        }
    }

    #[test]
    fn absent_optional_claims_are_omitted() {
        let minimal = AccessTokenClaims {
            name: None,
            email: None,
            tenant: None,
            roles: None,
            permissions: None,
            ..claims()
        };

        let document = serde_json::to_value(&minimal).unwrap();

        for absent in [
            NAME_CLAIM,
            EMAIL_CLAIM,
            ROLE_CLAIM,
            TENANT_CLAIM,
            PERMISSIONS_CLAIM,
        ] {
            assert!(document.get(absent).is_none(), "{absent} should be omitted");
        }
    }

    #[test]
    fn claims_become_the_principal_the_guards_read() {
        let user = claims().into_user();

        assert_eq!(user.subject, "admin-1");
        assert_eq!(user.name.as_deref(), Some("Admin User"));
        assert_eq!(user.tenant.as_deref(), Some("tenant-1"));
        assert_eq!(user.roles, vec!["Admin".to_owned()]);
        assert!(user.has_permission("music.read"));
        assert!(user.has_permission("administration.read"));
        assert!(!user.has_permission("administration.write"));
    }
}
