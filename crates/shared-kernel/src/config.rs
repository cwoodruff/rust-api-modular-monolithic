//! Configuration loading, ported from the C# host's `IConfiguration` stack.
//!
//! The layering matches `WebApplication.CreateBuilder`: `appsettings.json`,
//! then `appsettings.{Environment}.json`, then environment variables, each
//! overriding the last. Both JSON files are optional.
//!
//! ASP.NET Core configuration keys are **case-insensitive**, and environment
//! variables express nesting with `__`. Reproducing that matters for more than
//! tidiness: without it, `SERVICENAME` from the environment and `ServiceName`
//! from a JSON file would land under two different keys and the environment
//! would silently fail to override the file. This module therefore lowercases
//! every key as it loads, so a lookup resolves the same way whatever layer
//! supplied the value.

use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Env, Serialized};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::build_info::DEFAULT_SERVICE_NAME;
use crate::environment::Environment;

/// Something went wrong loading a configuration file.
///
/// A missing file is not an error — both JSON layers are optional, as they are
/// in the original — but a malformed one is, rather than silently starting with
/// half a configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file exists but could not be read.
    #[error("failed to read configuration file {path}")]
    Read {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The file was read but does not contain valid JSON.
    #[error("configuration file {path} is not valid JSON")]
    Parse {
        /// The file that failed to parse.
        path: PathBuf,
        /// The underlying deserialization error.
        #[source]
        source: serde_json::Error,
    },
}

/// The resolved configuration, plus the environment the host is running as.
#[derive(Debug, Clone)]
pub struct AppConfig {
    figment: Figment,
    environment: Environment,
}

impl AppConfig {
    /// Loads configuration from `content_root`, taking the environment from the
    /// process.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if a configuration file exists but cannot be
    /// read or parsed.
    pub fn load(content_root: &Path) -> Result<Self, ConfigError> {
        Self::load_for(content_root, Environment::from_process())
    }

    /// Loads configuration for an explicit environment.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if a configuration file exists but cannot be
    /// read or parsed.
    pub fn load_for(content_root: &Path, environment: Environment) -> Result<Self, ConfigError> {
        let layers = [
            content_root.join("appsettings.json"),
            content_root.join(format!("appsettings.{}.json", environment.name())),
        ];

        let mut figment = Figment::new();
        for layer in layers {
            if let Some(values) = read_json_layer(&layer)? {
                figment = figment.merge(Serialized::defaults(values));
            }
        }

        // `split("__")` turns CONNECTIONSTRINGS__APPDATABASE into the nested key
        // `connectionstrings.appdatabase`, matching the C# provider.
        figment = figment.merge(Env::raw().split("__"));

        Ok(Self {
            figment,
            environment,
        })
    }

    /// Builds a configuration directly from a figment, for tests and for hosts
    /// that assemble their own layers.
    #[must_use]
    pub fn from_figment(figment: Figment, environment: Environment) -> Self {
        Self {
            figment,
            environment,
        }
    }

    /// The environment the host is running as.
    #[must_use]
    pub fn environment(&self) -> &Environment {
        &self.environment
    }

    /// The underlying figment, for callers needing the full extraction API.
    #[must_use]
    pub fn figment(&self) -> &Figment {
        &self.figment
    }

    /// Reads a single scalar value.
    ///
    /// Keys may be written in the original's `Section:Key` form or in figment's
    /// `section.key` form; both resolve to the same value.
    #[must_use]
    pub fn get_string(&self, key: &str) -> Option<String> {
        self.figment
            .find_value(&normalize_key(key))
            .ok()
            .and_then(figment::value::Value::into_string)
    }

    /// Port of `BuildInfoProvider.GetServiceName`.
    #[must_use]
    pub fn service_name(&self) -> String {
        self.get_string("ServiceName")
            .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_owned())
    }

    /// Binds a configuration section, falling back to the type's defaults.
    ///
    /// This mirrors `Configure<TOptions>`: a section that is absent entirely
    /// leaves every default in place. A section that is present but malformed
    /// is reported through `tracing` and then also falls back, rather than
    /// taking the host down over, say, one mistyped TTL.
    pub fn section_or_default<T>(&self, key: &str) -> T
    where
        T: DeserializeOwned + Default,
    {
        let normalized = normalize_key(key);

        if self.figment.find_value(&normalized).is_err() {
            return T::default();
        }

        match self.figment.extract_inner(&normalized) {
            Ok(section) => section,
            Err(error) => {
                tracing::warn!(
                    section = key,
                    %error,
                    "configuration section could not be bound; using defaults"
                );
                T::default()
            }
        }
    }

    /// Binds a configuration section, reporting failures to the caller.
    ///
    /// # Errors
    ///
    /// Returns the figment error if the section is missing or malformed. It is
    /// boxed because `figment::Error` is large enough to bloat every `Result`
    /// that carries it.
    pub fn try_section<T>(&self, key: &str) -> Result<T, Box<figment::Error>>
    where
        T: DeserializeOwned,
    {
        self.figment
            .extract_inner(&normalize_key(key))
            .map_err(Box::new)
    }
}

/// Rewrites a configuration key into the canonical lowercase, dot-separated
/// form every layer is stored under.
fn normalize_key(key: &str) -> String {
    key.replace(':', ".").to_lowercase()
}

/// Reads one optional JSON layer, lowercasing every key on the way in.
fn read_json_layer(path: &Path) -> Result<Option<Value>, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let value = serde_json::from_str(&text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    Ok(Some(lowercase_keys(value)))
}

/// Recursively lowercases object keys, leaving values untouched.
fn lowercase_keys(value: Value) -> Value {
    match value {
        Value::Object(entries) => Value::Object(
            entries
                .into_iter()
                .map(|(key, nested)| (key.to_lowercase(), lowercase_keys(nested)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(lowercase_keys).collect()),
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    fn config_from(values: Value, environment: Environment) -> AppConfig {
        let figment = Figment::new().merge(Serialized::defaults(lowercase_keys(values)));
        AppConfig::from_figment(figment, environment)
    }

    #[test]
    fn keys_resolve_regardless_of_case_or_separator() {
        let config = config_from(
            json!({ "ConnectionStrings": { "AppDatabase": "Data Source=./data/chinook.db" } }),
            Environment::Development,
        );

        for key in [
            "ConnectionStrings:AppDatabase",
            "connectionstrings:appdatabase",
            "ConnectionStrings.AppDatabase",
            "CONNECTIONSTRINGS:APPDATABASE",
        ] {
            assert_eq!(
                config.get_string(key).as_deref(),
                Some("Data Source=./data/chinook.db"),
                "key `{key}` should resolve"
            );
        }
    }

    #[test]
    fn service_name_falls_back_to_the_original_default() {
        let empty = config_from(json!({}), Environment::Production);
        assert_eq!(empty.service_name(), "ModularMonolith.Api");

        let configured = config_from(json!({ "ServiceName": "custom" }), Environment::Production);
        assert_eq!(configured.service_name(), "custom");
    }

    #[test]
    fn later_layers_override_earlier_ones() {
        // Mirrors appsettings.json then appsettings.Development.json: the
        // Development layer wins, and keys it does not mention survive.
        let figment = Figment::new()
            .merge(Serialized::defaults(lowercase_keys(json!({
                "ServiceName": "base",
                "Logging": { "LogLevel": { "Default": "Information" } }
            }))))
            .merge(Serialized::defaults(lowercase_keys(json!({
                "Logging": { "LogLevel": { "Default": "Debug" } }
            }))));

        let config = AppConfig::from_figment(figment, Environment::Development);

        assert_eq!(
            config.get_string("Logging:LogLevel:Default").as_deref(),
            Some("Debug")
        );
        assert_eq!(config.service_name(), "base");
    }

    #[derive(Debug, Default, PartialEq, Eq, Deserialize)]
    struct Jwt {
        issuer: String,
        audience: String,
    }

    #[test]
    fn sections_bind_into_typed_structs() {
        let config = config_from(
            json!({ "Jwt": { "Issuer": "https://auth.local", "Audience": "modular-api" } }),
            Environment::Development,
        );

        let jwt: Jwt = config.section_or_default("Jwt");

        assert_eq!(
            jwt,
            Jwt {
                issuer: "https://auth.local".to_owned(),
                audience: "modular-api".to_owned(),
            }
        );
    }

    #[test]
    fn absent_sections_fall_back_to_defaults() {
        // The original ships no `Caching` section at all and relies entirely on
        // the options defaults; binding must not fail in that case.
        let config = config_from(json!({}), Environment::Production);

        assert_eq!(config.section_or_default::<Jwt>("Jwt"), Jwt::default());
    }

    #[test]
    fn missing_files_are_not_an_error() {
        let empty_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

        let config = AppConfig::load_for(&empty_directory, Environment::Production)
            .expect("absent appsettings files should load as an empty configuration");

        assert_eq!(config.service_name(), "ModularMonolith.Api");
    }
}
