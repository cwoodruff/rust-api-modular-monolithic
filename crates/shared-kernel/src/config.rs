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
///
/// `Debug` names the environment and nothing else. Every layer this holds is a
/// place secrets live — `ConnectionStrings:AppDatabase`, the passwords under
/// `Identity:InMemoryUsers`, whatever the process environment carries — and a
/// derived `Debug` would print the lot on the first `tracing` call that wrote
/// `?config`.
#[derive(Clone)]
pub struct AppConfig {
    figment: Figment,
    environment: Environment,
}

impl std::fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("environment", &self.environment)
            .field("values", &crate::redaction::REDACTED)
            .finish()
    }
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
        Self::load_layers(content_root, environment, true)
    }

    /// Loads configuration without user secrets: the JSON files and the
    /// environment only. Tests use it, so a developer's own logins cannot leak
    /// into what a test expects.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if a configuration file exists but cannot be
    /// read or parsed.
    pub fn load_without_user_secrets(
        content_root: &Path,
        environment: Environment,
    ) -> Result<Self, ConfigError> {
        Self::load_layers(content_root, environment, false)
    }

    fn load_layers(
        content_root: &Path,
        environment: Environment,
        user_secrets: bool,
    ) -> Result<Self, ConfigError> {
        let layers = [
            content_root.join("appsettings.json"),
            content_root.join(format!("appsettings.{}.json", environment.name())),
        ];

        // The layers are merged here, key by key, before figment sees them.
        // .NET flattens every layer to `Section:0:Key` paths, so a later
        // layer overrides one member of one array element and leaves the rest;
        // figment would replace the whole array instead, and an environment
        // variable setting one login's password would silently drop every
        // other login and every other field of that one.
        let mut merged = Value::Object(serde_json::Map::new());
        for layer in layers {
            if let Some(values) = read_json_layer(&layer)? {
                merge_layer(&mut merged, values);
            }
        }

        // Development and Demo read user secrets after the JSON files and
        // before the environment, as upstream's host does (4ad3c91). The id is
        // the original's, so `dotnet user-secrets --project
        // src/ModularMonolith.Api set …` seeds this host too.
        if user_secrets
            && environment.exposes_operational_metadata()
            && let Some(path) = user_secrets_path()
            && let Some(values) = read_json_layer(&path)?
        {
            merge_layer(&mut merged, values);
        }

        merge_layer(&mut merged, environment_layer(std::env::vars()));
        let figment = Figment::new().merge(Serialized::defaults(merged));

        Ok(Self {
            figment,
            environment,
        })
    }

    /// Lays `overrides` over everything loaded so far — what a test host's
    /// `ConfigureAppConfiguration` does. Keys may use either form,
    /// `"ConnectionStrings": { "AppDatabase": … }` or `"ConnectionStrings:AppDatabase"`.
    #[must_use]
    pub fn with_overrides(self, overrides: Value) -> Self {
        Self {
            figment: self
                .figment
                .merge(Serialized::defaults(lowercase_keys(overrides))),
            environment: self.environment,
        }
    }

    /// A configuration made only of `values`, for tests.
    #[must_use]
    pub fn from_values(values: Value, environment: Environment) -> Self {
        Self::from_figment(
            Figment::new().merge(Serialized::defaults(lowercase_keys(values))),
            environment,
        )
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
        // Numbers and booleans read as text too, as `IConfiguration` stores
        // every value as a string: `"PermitLimit": 60` reads as `"60"`.
        match self.value_at(key)? {
            Value::String(text) => Some(text),
            Value::Number(number) => Some(number.to_string()),
            Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        }
    }

    /// The value at `key`, walking array indices as .NET's `Section:0:Key`
    /// paths do — figment's own lookup indexes maps only.
    fn value_at(&self, key: &str) -> Option<Value> {
        let mut current = self.figment.extract::<Value>().ok()?;
        for segment in normalize_key(key).split('.') {
            current = match current {
                Value::Object(mut map) => map.remove(segment)?,
                Value::Array(mut items) => {
                    let index: usize = segment.parse().ok()?;
                    (index < items.len()).then(|| items.swap_remove(index))?
                }
                _ => return None,
            };
        }
        Some(current)
    }

    /// Reads a value as `T`, falling back to `default` when it is absent or
    /// does not parse. Port of `IConfiguration.GetValue<T>(key, default)`.
    pub fn get_or<T: std::str::FromStr>(&self, key: &str, default: T) -> T {
        self.get_string(key)
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(default)
    }

    /// Whether `key` is present at all, as a value or a section.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.figment.find_value(&normalize_key(key)).is_ok()
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

/// The original host's `UserSecretsId`.
pub const USER_SECRETS_ID: &str = "e5c98386-8439-49a8-a3d8-cf17fcb7b6dc";

/// Where `dotnet user-secrets` keeps this application's secrets.
///
/// `~/.microsoft/usersecrets/<id>/secrets.json` on macOS and Linux,
/// `%APPDATA%\Microsoft\UserSecrets\<id>\secrets.json` on Windows.
/// `USER_SECRETS_PATH` overrides it, which is how the tests point at a file of
/// their own.
#[must_use]
pub fn user_secrets_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("USER_SECRETS_PATH") {
        return Some(PathBuf::from(path));
    }

    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|root| {
            PathBuf::from(root)
                .join("Microsoft")
                .join("UserSecrets")
                .join(USER_SECRETS_ID)
                .join("secrets.json")
        })
    } else {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join(".microsoft")
                .join("usersecrets")
                .join(USER_SECRETS_ID)
                .join("secrets.json")
        })
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

    // `dotnet user-secrets` writes secrets.json with a UTF-8 byte-order mark,
    // and .NET's JSON provider skips it. serde_json does not.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

    let value = serde_json::from_str(text).map_err(|source| ConfigError::Parse {
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
///
/// A key containing `:` is split into nested levels, as .NET's JSON provider
/// flattens every path on `:`. Two things depend on it: the rate-limit policy
/// names (`"catalog:api": { … }` is `RateLimiting:Policies:catalog:api:*`), and
/// `secrets.json`, which stores every value under a flat colon-joined key.
pub(crate) fn lowercase_keys(value: Value) -> Value {
    match value {
        Value::Object(entries) => {
            let mut root = serde_json::Map::new();
            for (key, nested) in entries {
                let path: Vec<String> = key.split(':').map(str::to_lowercase).collect();
                merge_at(&mut root, &path, lowercase_keys(nested));
            }
            numeric_maps_to_arrays(Value::Object(root))
        }
        Value::Array(items) => Value::Array(items.into_iter().map(lowercase_keys).collect()),
        scalar => scalar,
    }
}

/// Lays `layer` over `base` the way .NET's configuration stacks providers:
/// objects merge member by member, arrays merge element by element, and a
/// scalar in the later layer wins.
fn merge_layer(base: &mut Value, layer: Value) {
    match (base, layer) {
        (Value::Object(base), Value::Object(layer)) => {
            for (key, value) in layer {
                match base.get_mut(&key) {
                    Some(existing) => merge_layer(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (Value::Array(base), Value::Array(layer)) => {
            for (index, value) in layer.into_iter().enumerate() {
                match base.get_mut(index) {
                    Some(existing) => merge_layer(existing, value),
                    None => base.push(value),
                }
            }
        }
        (base, layer) => *base = layer,
    }
}

/// Writes `value` at `path`, merging objects that meet there.
fn merge_at(root: &mut serde_json::Map<String, Value>, path: &[String], value: Value) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };

    if rest.is_empty() {
        match (root.get_mut(head), value) {
            (Some(Value::Object(existing)), Value::Object(incoming)) => {
                for (key, nested) in incoming {
                    merge_at(existing, std::slice::from_ref(&key), nested);
                }
            }
            (_, value) => {
                root.insert(head.clone(), value);
            }
        }
        return;
    }

    let child = root
        .entry(head.clone())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !child.is_object() {
        *child = Value::Object(serde_json::Map::new());
    }
    if let Value::Object(map) = child {
        merge_at(map, rest, value);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn a_byte_order_mark_is_skipped_as_dotnet_skips_it() {
        let path = std::env::temp_dir().join(format!("bom-{}.json", std::process::id()));
        std::fs::write(&path, "\u{feff}{\"ServiceName\": \"bom\"}").unwrap();

        let value = read_json_layer(&path).unwrap().unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(value["servicename"], "bom");
    }

    #[test]
    fn a_later_layer_overrides_one_member_of_one_array_element() {
        let mut base = json!({ "identity": { "inmemoryusers": [
            { "username": "demo", "password": "file" },
            { "username": "other", "password": "file" }
        ] } });
        merge_layer(
            &mut base,
            json!({ "identity": { "inmemoryusers": [{ "password": "env" }] } }),
        );

        assert_eq!(
            base,
            json!({ "identity": { "inmemoryusers": [
                { "username": "demo", "password": "env" },
                { "username": "other", "password": "file" }
            ] } })
        );
    }
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
    fn the_configuration_does_not_print_the_values_it_holds() {
        // A connection string and a password both live in here.
        let config = config_from(
            json!({
                "ConnectionStrings": { "AppDatabase": "Data Source=/secret/path.db" },
                "Identity": { "InMemoryUsers": [{ "Username": "demo", "Password": "hunter2" }] }
            }),
            Environment::Production,
        );

        let rendered = format!("{config:?}");

        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(!rendered.contains("/secret/path.db"), "{rendered}");
        assert!(rendered.contains("Production"), "{rendered}");
    }

    #[test]
    fn missing_files_are_not_an_error() {
        let empty_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

        let config = AppConfig::load_for(&empty_directory, Environment::Production)
            .expect("absent appsettings files should load as an empty configuration");

        assert_eq!(config.service_name(), "ModularMonolith.Api");
    }
}
