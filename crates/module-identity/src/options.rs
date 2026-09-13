//! Configuration, ported from `JwtAuthOptions` and `InMemoryUserStoreOptions`.

use serde::Deserialize;
use shared_kernel::REDACTED;

/// The configuration section the JWT options bind from.
pub const JWT_SECTION: &str = "Jwt";

/// The configuration section the in-memory users bind from.
pub const IDENTITY_SECTION: &str = "Identity";

/// Binding of the `Jwt` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct JwtAuthOptions {
    /// Token issuer.
    pub issuer: String,
    /// Token audience.
    pub audience: String,
    /// Access-token lifetime, in minutes.
    #[serde(rename = "accesstokenminutes", alias = "access_token_minutes")]
    pub access_token_minutes: i64,
    /// Refresh-token lifetime, in days.
    #[serde(rename = "refreshtokendays", alias = "refresh_token_days")]
    pub refresh_token_days: i64,
    /// `Dev` or `KeyVault`.
    #[serde(rename = "keyprovider", alias = "key_provider")]
    pub key_provider: String,
    /// Where the development provider persists its key.
    #[serde(rename = "developmentkeypath", alias = "development_key_path")]
    pub development_key_path: String,
    /// Key Vault URI, when the provider is `KeyVault`.
    #[serde(rename = "keyvaultvauturi", alias = "key_vault_vault_uri")]
    pub key_vault_vault_uri: Option<String>,
    /// Key Vault key name, when the provider is `KeyVault`.
    #[serde(rename = "keyvaultkeyname", alias = "key_vault_key_name")]
    pub key_vault_key_name: Option<String>,
}

impl Default for JwtAuthOptions {
    fn default() -> Self {
        Self {
            issuer: "https://auth.local".to_owned(),
            audience: "modular-api".to_owned(),
            access_token_minutes: 15,
            refresh_token_days: 7,
            key_provider: "Dev".to_owned(),
            development_key_path: "data/identity/dev-jwt-signing-key.json".to_owned(),
            key_vault_vault_uri: None,
            key_vault_key_name: None,
        }
    }
}

/// The clock skew the original allows when validating a token.
///
/// 30 seconds. Note the `authn-authz-plan.md` document says two minutes; the
/// code is what ships.
pub const CLOCK_SKEW_SECONDS: u64 = 30;

/// Binding of the `Identity` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct IdentityOptions {
    /// The development login users.
    #[serde(rename = "inmemoryusers", alias = "in_memory_users")]
    pub in_memory_users: Vec<InMemoryUserRecord>,
}

/// One configured login.
///
/// The original ships an **empty** array and documents `dotnet user-secrets`
/// for filling it, so there are no baked-in credentials to reproduce.
///
/// `Debug` is written out rather than derived so the password cannot reach a
/// log through `?record`.
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct InMemoryUserRecord {
    /// Login name. Matched case-insensitively.
    pub username: String,
    /// Password, compared in plain text exactly as the original does.
    pub password: String,
    /// The `sub` claim this login issues.
    #[serde(rename = "userid", alias = "user_id")]
    pub user_id: String,
    /// Display name. Falls back to the username.
    #[serde(rename = "displayname", alias = "display_name")]
    pub display_name: Option<String>,
    /// Role claims.
    pub roles: Vec<String>,
    /// Permission claims.
    pub permissions: Vec<String>,
    /// Email claim.
    pub email: Option<String>,
    /// Tenant claim.
    pub tenant: Option<String>,
}

impl std::fmt::Debug for InMemoryUserRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InMemoryUserRecord")
            .field("username", &self.username)
            .field("password", &REDACTED)
            .field("user_id", &self.user_id)
            .field("display_name", &self.display_name)
            .field("roles", &self.roles)
            .field("permissions", &self.permissions)
            .field("email", &self.email)
            .field("tenant", &self.tenant)
            .finish()
    }
}

impl InMemoryUserRecord {
    /// Whether this entry carries enough to be usable.
    ///
    /// The original drops entries missing any of username, password, or user
    /// id, logging a warning rather than failing startup.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.username.trim().is_empty()
            && !self.password.trim().is_empty()
            && !self.user_id.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn defaults_match_the_original() {
        let options = JwtAuthOptions::default();

        assert_eq!(options.issuer, "https://auth.local");
        assert_eq!(options.audience, "modular-api");
        assert_eq!(options.access_token_minutes, 15);
        assert_eq!(options.refresh_token_days, 7);
        assert_eq!(options.key_provider, "Dev");
        assert_eq!(CLOCK_SKEW_SECONDS, 30);
    }

    #[test]
    fn the_section_binds_from_lowercased_configuration_keys() {
        let bound: JwtAuthOptions = serde_json::from_value(serde_json::json!({
            "issuer": "https://example.test",
            "accesstokenminutes": 5,
            "keyprovider": "KeyVault",
            "keyvaultvauturi": "https://vault.example"
        }))
        .expect("the section should bind");

        assert_eq!(bound.issuer, "https://example.test");
        assert_eq!(bound.access_token_minutes, 5);
        assert_eq!(bound.key_provider, "KeyVault");
        assert_eq!(
            bound.key_vault_vault_uri.as_deref(),
            Some("https://vault.example")
        );
        // Untouched members keep their defaults.
        assert_eq!(bound.audience, "modular-api");
        assert_eq!(bound.refresh_token_days, 7);
    }

    #[test]
    fn incomplete_user_entries_are_rejected() {
        let complete = InMemoryUserRecord {
            username: "demo".to_owned(),
            password: "secret".to_owned(),
            user_id: "user-1".to_owned(),
            ..InMemoryUserRecord::default()
        };
        assert!(complete.is_usable());

        for missing in ["username", "password", "user_id"] {
            let mut record = complete.clone();
            match missing {
                "username" => record.username = "  ".to_owned(),
                "password" => record.password = String::new(),
                _ => record.user_id = String::new(),
            }
            assert!(!record.is_usable(), "missing {missing} should be rejected");
        }
    }

    #[test]
    fn a_configured_login_does_not_print_its_password() {
        let record = InMemoryUserRecord {
            username: "demo".to_owned(),
            password: "hunter2".to_owned(),
            user_id: "user-1".to_owned(),
            ..InMemoryUserRecord::default()
        };

        let rendered = format!("{record:?}");

        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(
            rendered.contains("demo"),
            "the useful half stays: {rendered}"
        );
        assert!(rendered.contains(REDACTED), "{rendered}");
    }

    #[test]
    fn the_identity_section_defaults_to_no_users() {
        // The original ships `"InMemoryUsers": []` and expects secrets to fill
        // it, so an absent section must not invent credentials.
        let options = IdentityOptions::default();

        assert!(options.in_memory_users.is_empty());
    }
}
