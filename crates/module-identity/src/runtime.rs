//! Assembling the module, ported from `AddIdentityAuth`.

use std::path::Path;
use std::sync::Arc;

use shared_kernel::{AppConfig, Environment};

use crate::keys::{DevKeyMaterial, KeyError, KeyMaterial, PemKeyMaterial};
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
    /// refuses it. `File` and `Environment` read an RSA private key supplied as
    /// a PEM and run anywhere, which is what lets a Production host start at
    /// all: `KeyVault`, the original's only other option, is not implemented
    /// here, so before those two a Production host had no usable provider and
    /// refused to boot.
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
    let key_id = options.key_id.clone();

    match options.key_provider.trim() {
        provider
            if provider.eq_ignore_ascii_case("File") || provider.eq_ignore_ascii_case("Pem") =>
        {
            let configured = options
                .pem_key_path
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .ok_or_else(|| {
                    KeyError::Misconfigured(
                        "Jwt:KeyProvider is File but Jwt:PemKeyPath is not set".to_owned(),
                    )
                })?;

            // An absolute path is honored as written; a relative one resolves
            // against the content root, as every other configured path does.
            let path = content_root.join(configured);

            Ok(Arc::new(PemKeyMaterial::from_file(&path, key_id)?))
        }

        provider
            if provider.eq_ignore_ascii_case("Environment")
                || provider.eq_ignore_ascii_case("Env") =>
        {
            Ok(Arc::new(PemKeyMaterial::from_environment(
                &options.pem_key_environment_variable,
                key_id,
            )?))
        }

        provider if provider.eq_ignore_ascii_case("KeyVault") => Err(KeyError::Unavailable(
            "the KeyVault key provider is not implemented in this port; \
             use Jwt:KeyProvider=File with Jwt:PemKeyPath, or Environment with \
             Jwt:PemKeyEnvironmentVariable, and have the platform deliver the key"
                .to_owned(),
        )),

        provider if provider.is_empty() || provider.eq_ignore_ascii_case("Dev") => {
            if !environment.exposes_operational_metadata() {
                return Err(KeyError::Unavailable(format!(
                    "the development key provider cannot run in {}; \
                     set Jwt:KeyProvider=File with Jwt:PemKeyPath, or Environment with \
                     Jwt:PemKeyEnvironmentVariable",
                    environment.name()
                )));
            }

            let path = content_root.join(&options.development_key_path);
            tracing::info!(path = %path.display(), "using the development signing key");

            Ok(Arc::new(DevKeyMaterial::load_or_create(&path)?))
        }

        other => Err(KeyError::Misconfigured(format!(
            "unsupported Jwt:KeyProvider value '{other}'; \
             expected Dev, File, Environment, or KeyVault"
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

    /// A PKCS#8 PEM written to a scratch file, plus its directory.
    fn scratch_key(label: &str) -> std::path::PathBuf {
        use rsa::pkcs8::{EncodePrivateKey, LineEnding};

        let private = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), crate::keys::KEY_SIZE_BITS)
            .expect("a key should generate");
        let pem = private
            .to_pkcs8_pem(LineEnding::LF)
            .expect("the key should encode");

        let path = std::env::temp_dir().join(format!(
            "runtime-signing-key-{}-{label}.pem",
            std::process::id()
        ));
        std::fs::write(&path, pem.as_bytes()).expect("the key should write");

        path
    }

    #[test]
    fn the_development_provider_is_refused_in_production() {
        // The original throws at startup rather than signing with a
        // development key in production.
        let config = config_for(Environment::Production, serde_json::json!({}));

        let error = IdentityRuntime::from_config(config_ref(&config), Path::new("."))
            .expect_err("a production host must not use the dev key provider");

        assert!(
            error.to_string().contains("Jwt:PemKeyPath"),
            "the message should name a provider that actually works: {error}"
        );
    }

    #[test]
    fn a_production_host_starts_on_a_pem_key_from_a_file() {
        // The point of the provider. Before it, Production had no usable key
        // provider at all: `Dev` is refused there and `KeyVault` is not built.
        let path = scratch_key("file");
        let config = config_for(
            Environment::Production,
            serde_json::json!({
                "jwt": { "keyprovider": "File", "pemkeypath": path.to_string_lossy() }
            }),
        );

        let runtime = IdentityRuntime::from_config(config_ref(&config), Path::new("."))
            .expect("a production host should start on a supplied key");

        assert!(
            !runtime.tokens().jwks()["keys"][0]["kid"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "the published JWKS should name the key"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_file_provider_says_what_is_missing_rather_than_failing_obscurely() {
        let config = config_for(
            Environment::Production,
            serde_json::json!({ "jwt": { "keyprovider": "File" } }),
        );

        let error = IdentityRuntime::from_config(config_ref(&config), Path::new("."))
            .expect_err("File with no path should fail");

        assert!(error.to_string().contains("Jwt:PemKeyPath"), "{error}");
    }

    #[test]
    fn the_environment_provider_says_which_variable_it_wanted() {
        let config = config_for(
            Environment::Production,
            serde_json::json!({
                "jwt": {
                    "keyprovider": "Environment",
                    "pemkeyenvironmentvariable": "A_VARIABLE_THAT_IS_NOT_SET"
                }
            }),
        );

        let error = IdentityRuntime::from_config(config_ref(&config), Path::new("."))
            .expect_err("an unset variable should fail");

        assert!(
            error.to_string().contains("A_VARIABLE_THAT_IS_NOT_SET"),
            "{error}"
        );
    }

    #[test]
    fn the_key_vault_provider_points_at_the_two_that_are_built() {
        let config = config_for(
            Environment::Production,
            serde_json::json!({ "jwt": { "keyprovider": "KeyVault" } }),
        );

        let error = IdentityRuntime::from_config(config_ref(&config), Path::new("."))
            .expect_err("KeyVault is still not implemented");

        assert!(error.to_string().contains("Jwt:PemKeyPath"), "{error}");
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
        assert!(
            error.to_string().contains("File"),
            "the message should list the providers that exist: {error}"
        );
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
