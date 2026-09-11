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
use figment::providers::Serialized;
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

        figment = figment.merge(Serialized::defaults(environment_layer(std::env::vars())));

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

/// Builds the environment-variable layer.
///
/// The C# provider splits a name on `__` to express nesting, and expresses
/// *array elements* with numeric segments — which is how the original's own
/// documentation tells you to seed logins:
///
/// ```text
/// Identity__InMemoryUsers__0__Username=demo
/// Identity__InMemoryUsers__0__Permissions__0=music.read
/// ```
///
/// Splitting alone is not enough: that produces a map keyed `"0"`, which will
/// not deserialize into a `Vec`. Any map whose keys are exactly the indices
/// `0..n` is therefore rewritten as an array, which is the rule ASP.NET's
/// binder applies. Without this the variables bind silently to nothing and the
/// host starts with no logins at all — found by running it.
fn environment_layer(variables: impl Iterator<Item = (String, String)>) -> Value {
    let mut root = serde_json::Map::new();

    for (name, value) in variables {
        let path: Vec<String> = name.split("__").map(str::to_lowercase).collect();
        insert_at(&mut root, &path, Value::String(value));
    }

    numeric_maps_to_arrays(Value::Object(root))
}

/// Writes `value` at `path`, creating intermediate maps.
fn insert_at(root: &mut serde_json::Map<String, Value>, path: &[String], value: Value) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };

    if rest.is_empty() {
        root.insert(head.clone(), value);
        return;
    }

    let child = root
        .entry(head.clone())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));

    // A scalar already sitting here is replaced: a deeper variable wins over a
    // shallower one, and neither is useful as a prefix of the other.
    if !child.is_object() {
        *child = Value::Object(serde_json::Map::new());
    }

    if let Value::Object(map) = child {
        insert_at(map, rest, value);
    }
}

/// Rewrites index-keyed maps as arrays, depth first.
fn numeric_maps_to_arrays(value: Value) -> Value {
    match value {
        Value::Object(entries) => {
            let converted: serde_json::Map<String, Value> = entries
                .into_iter()
                .map(|(key, nested)| (key, numeric_maps_to_arrays(nested)))
                .collect();

            if let Some(items) = as_index_sequence(&converted) {
                Value::Array(items)
            } else {
                Value::Object(converted)
            }
        }
        Value::Array(items) => {
            Value::Array(items.into_iter().map(numeric_maps_to_arrays).collect())
        }
        scalar => scalar,
    }
}

/// Reads a map as `0..n` if its keys are exactly those indices.
fn as_index_sequence(entries: &serde_json::Map<String, Value>) -> Option<Vec<Value>> {
    if entries.is_empty() {
        return None;
    }

    let mut indexed: Vec<(usize, &Value)> = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        indexed.push((key.parse::<usize>().ok()?, value));
    }

    indexed.sort_by_key(|(index, _)| *index);

    // Only a dense run starting at zero is an array; anything else is a map
    // that happens to use numeric keys.
    if indexed
        .iter()
        .enumerate()
        .any(|(position, (index, _))| position != *index)
    {
        return None;
    }

    Some(
        indexed
            .into_iter()
            .map(|(_, value)| value.clone())
            .collect(),
    )
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
    fn environment_variables_nest_on_the_double_underscore() {
        let layer = environment_layer(
            [(
                "CONNECTIONSTRINGS__APPDATABASE".to_owned(),
                "Data Source=/tmp/x.db".to_owned(),
            )]
            .into_iter(),
        );

        assert_eq!(
            layer["connectionstrings"]["appdatabase"],
            json!("Data Source=/tmp/x.db")
        );
    }

    #[test]
    fn index_segments_become_arrays_so_lists_actually_bind() {
        // The form the original documents for seeding logins. Splitting alone
        // yields a map keyed "0", which will not deserialize into a Vec — the
        // host then starts with no logins and every password is rejected.
        let layer = environment_layer(
            [
                (
                    "Identity__InMemoryUsers__0__Username".to_owned(),
                    "demo".to_owned(),
                ),
                (
                    "Identity__InMemoryUsers__0__Password".to_owned(),
                    "secret".to_owned(),
                ),
                (
                    "Identity__InMemoryUsers__0__Permissions__0".to_owned(),
                    "music.read".to_owned(),
                ),
                (
                    "Identity__InMemoryUsers__0__Permissions__1".to_owned(),
                    "orders.read".to_owned(),
                ),
                (
                    "Identity__InMemoryUsers__1__Username".to_owned(),
                    "admin".to_owned(),
                ),
            ]
            .into_iter(),
        );

        let users = &layer["identity"]["inmemoryusers"];

        assert!(users.is_array(), "expected an array, got {users}");
        assert_eq!(users[0]["username"], json!("demo"));
        assert_eq!(
            users[0]["permissions"],
            json!(["music.read", "orders.read"])
        );
        assert_eq!(users[1]["username"], json!("admin"));
    }

    #[test]
    fn a_map_with_gappy_numeric_keys_stays_a_map() {
        // Only a dense run from zero is an array; anything else is a map that
        // happens to use numeric keys.
        let layer = environment_layer(
            [
                ("Thing__0__Name".to_owned(), "first".to_owned()),
                ("Thing__2__Name".to_owned(), "third".to_owned()),
            ]
            .into_iter(),
        );

        assert!(layer["thing"].is_object(), "got {}", layer["thing"]);
    }

    #[test]
    fn a_map_with_ordinary_keys_stays_a_map() {
        let layer = environment_layer(
            [("Logging__LogLevel__Default".to_owned(), "Debug".to_owned())].into_iter(),
        );

        assert_eq!(layer["logging"]["loglevel"]["default"], json!("Debug"));
    }

    #[test]
    fn a_deeper_variable_wins_over_a_shallower_one() {
        let layer = environment_layer(
            [
                ("Jwt".to_owned(), "ignored".to_owned()),
                ("Jwt__Issuer".to_owned(), "https://auth.local".to_owned()),
            ]
            .into_iter(),
        );

        assert_eq!(layer["jwt"]["issuer"], json!("https://auth.local"));
    }

    #[test]
    fn missing_files_are_not_an_error() {
        let empty_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

        let config = AppConfig::load_for(&empty_directory, Environment::Production)
            .expect("absent appsettings files should load as an empty configuration");

        assert_eq!(config.service_name(), "ModularMonolith.Api");
    }
}
