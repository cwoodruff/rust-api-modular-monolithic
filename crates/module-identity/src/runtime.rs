//! Assembling the module, ported from `AddIdentityAuth`.

use std::path::Path;
use std::sync::Arc;

use shared_kernel::{AppConfig, Environment};

use crate::keys::{DevKeyMaterial, KeyError, KeyMaterial};
use crate::options::{IDENTITY_SECTION, IdentityOptions, JWT_SECTION, JwtAuthOptions};
use crate::stores::{DisabledUserStore, InMemoryRefreshTokenStore, InMemoryUserStore, UserStore};
use crate::tokens::TokenService;

/// Everything the Identity module needs at runtime.
pub struct IdentityRuntime {
    tokens: TokenService,
}

impl std::fmt::Debug for IdentityRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityRuntime")
            .field("tokens", &self.tokens)
            .finish()
    }
}

impl IdentityRuntime {
    /// Builds the runtime from configuration.
    ///
    /// # Key provider
    ///
    /// `Dev` is refused outside Development and Demo, exactly as the original
    /// refuses it — a production host must name a Key Vault. `KeyVault` itself
    /// is not implemented here; the trait is in place for it.
    ///
    /// # User store
    ///
    /// The in-memory store is used only in Development and Demo. Anywhere else
    /// the disabled store takes over and refuses every login, so configured
    /// credentials cannot be used in production even if present.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError`] if the key provider is unusable or its material
    /// cannot be produced.
    pub fn from_config(config: &AppConfig, content_root: &Path) -> Result<Self, KeyError> {
        let options: JwtAuthOptions = config.section_or_default(JWT_SECTION);
        let environment = config.environment().clone();

        let keys: Arc<dyn KeyMaterial> = build_keys(&options, &environment, content_root)?;
        let users: Arc<dyn UserStore> = build_user_store(config, &environment);

        Ok(Self {
            tokens: TokenService::new(
                options,
                keys,
                Arc::new(InMemoryRefreshTokenStore::new()),
                users,
            ),
        })
    }

    /// Builds a runtime from parts, for tests.
    #[must_use]
    pub fn from_parts(tokens: TokenService) -> Self {
        Self { tokens }
    }

    /// Builds a runtime with key material supplied directly.
    ///
    /// The user store still comes from configuration, so the
    /// Development-versus-Production gating is exercised; only the key
    /// provider is bypassed. Tests use this to avoid writing a signing key
    /// into the working tree, and to build a Production host at all — the real
    /// provider refuses to run there, exactly as the original refuses.
    #[must_use]
    pub fn with_keys(config: &AppConfig, keys: Arc<dyn KeyMaterial>) -> Self {
        let options: JwtAuthOptions = config.section_or_default(JWT_SECTION);
        let users = build_user_store(config, config.environment());

        Self {
            tokens: TokenService::new(
                options,
                keys,
                Arc::new(InMemoryRefreshTokenStore::new()),
                users,
            ),
        }
    }

    /// The token service.
    #[must_use]
    pub fn tokens(&self) -> &TokenService {
        &self.tokens
    }
}

fn build_keys(
    options: &JwtAuthOptions,
    environment: &Environment,
    content_root: &Path,
) -> Result<Arc<dyn KeyMaterial>, KeyError> {
    match options.key_provider.trim() {
        "KeyVault" => Err(KeyError::Unavailable(
            "the KeyVault key provider is not implemented in this port yet; \
             the trait is in place for it"
                .to_owned(),
        )),

        provider if provider.is_empty() || provider.eq_ignore_ascii_case("Dev") => {
            if !environment.exposes_operational_metadata() {
                return Err(KeyError::Unavailable(format!(
                    "the development key provider cannot run in {}; \
                     set Jwt:KeyProvider=KeyVault with a vault URI and key name",
                    environment.name()
                )));
            }

            let path = content_root.join(&options.development_key_path);
            tracing::info!(path = %path.display(), "using the development signing key");

            Ok(Arc::new(DevKeyMaterial::load_or_create(&path)?))
        }

        other => Err(KeyError::Unavailable(format!(
            "unsupported Jwt:KeyProvider value '{other}'"
        ))),
    }
}

fn build_user_store(config: &AppConfig, environment: &Environment) -> Arc<dyn UserStore> {
    if !environment.exposes_operational_metadata() {
        tracing::warn!(
            environment = environment.name(),
            "in-memory logins are disabled outside Development and Demo"
        );
        return Arc::new(DisabledUserStore);
    }

    let identity: IdentityOptions = config.section_or_default(IDENTITY_SECTION);

    Arc::new(InMemoryUserStore::from_records(&identity.in_memory_users))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use figment::Figment;
    use figment::providers::Serialized;

    fn config_for(environment: Environment, values: serde_json::Value) -> AppConfig {
        AppConfig::from_figment(
            Figment::new().merge(Serialized::defaults(values)),
            environment,
        )
    }

    #[test]
    fn the_development_provider_is_refused_in_production() {
        // The original throws at startup rather than signing with a
        // development key in production.
        let config = config_for(Environment::Production, serde_json::json!({}));

        let error = IdentityRuntime::from_config(config_ref(&config), Path::new("."))
            .expect_err("a production host must not use the dev key provider");

        assert!(
            error.to_string().contains("KeyVault"),
            "the message should say what to do instead: {error}"
        );
    }

    #[test]
    fn an_unknown_provider_is_refused() {
        let config = config_for(
            Environment::Development,
            serde_json::json!({ "jwt": { "keyprovider": "Nonsense" } }),
        );

        let error = IdentityRuntime::from_config(config_ref(&config), Path::new("."))
            .expect_err("an unknown provider should fail");

        assert!(error.to_string().contains("Nonsense"), "{error}");
    }

    #[test]
    fn production_gets_the_disabled_user_store() {
        // Even with credentials configured, production must refuse them.
        let config = config_for(
            Environment::Production,
            serde_json::json!({
                "identity": { "inmemoryusers": [
                    { "username": "demo", "password": "secret", "userid": "user-1" }
                ]}
            }),
        );

        let store = build_user_store(config_ref(&config), &Environment::Production);

        assert!(store.validate_credentials("demo", "secret").is_none());
    }

    #[test]
    fn development_loads_the_configured_logins() {
        let config = config_for(
            Environment::Development,
            serde_json::json!({
                "identity": { "inmemoryusers": [
                    {
                        "username": "demo", "password": "secret", "userid": "user-1",
                        "roles": ["User"], "permissions": ["music.read"],
                        "tenant": "tenant-1"
                    }
                ]}
            }),
        );

        let store = build_user_store(config_ref(&config), &Environment::Development);
        let user = store
            .validate_credentials("demo", "secret")
            .expect("the configured login should authenticate");

        assert_eq!(user.user_id, "user-1");
        assert_eq!(user.permissions, vec!["music.read".to_owned()]);
        assert_eq!(user.tenant.as_deref(), Some("tenant-1"));
    }

    #[test]
    fn development_with_no_configured_logins_authenticates_nobody() {
        // The original ships an empty array and expects secrets to fill it, so
        // an unconfigured host must have no way in.
        let config = config_for(Environment::Development, serde_json::json!({}));

        let store = build_user_store(config_ref(&config), &Environment::Development);

        assert!(store.validate_credentials("demo", "secret").is_none());
    }

    fn config_ref(config: &AppConfig) -> &AppConfig {
        config
    }
}
