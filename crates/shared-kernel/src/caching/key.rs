//! Cache keys, ported from the `CacheKey` record and `CacheKeyComposer`.

use std::fmt;

use crate::AppConfig;
use crate::build_info::{DEFAULT_CACHE_APP_NAME, DEFAULT_CACHE_ENVIRONMENT};

/// A composed cache key.
///
/// Rendering is fixed and positional — nine colon-separated segments, with an
/// empty segment for each absent partition — so a key looks like:
///
/// ```text
/// development:modularmonolith.api:music:album:v1::::by-id:1
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    /// Deployment environment.
    pub environment: String,
    /// Application name.
    pub app: String,
    /// Owning module.
    pub module: String,
    /// Entity being cached.
    pub entity: String,
    /// Schema version of the cached shape.
    pub version: String,
    /// Tenant partition, if any.
    pub tenant: Option<String>,
    /// Locale partition, if any.
    pub locale: Option<String>,
    /// Feature partition, if any.
    pub feature: Option<String>,
    /// What distinguishes this entry within its entity, such as `by-id:1`.
    pub discriminator: String,
}

impl fmt::Display for CacheKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}:{}:{}:{}:{}:{}:{}",
            self.environment,
            self.app,
            self.module,
            self.entity,
            self.version,
            self.tenant.as_deref().unwrap_or_default(),
            self.locale.as_deref().unwrap_or_default(),
            self.feature.as_deref().unwrap_or_default(),
            self.discriminator,
        )
    }
}

/// Builds cache keys with the application and environment segments fixed.
///
/// Two details of the original are preserved deliberately:
///
/// - The discriminator is **not** lowercased, while every other segment is.
/// - The environment comes from the *configuration value*
///   `ASPNETCORE_ENVIRONMENT` (falling back to `DOTNET_ENVIRONMENT`, then to
///   `prod`), not from the host's resolved environment. A host running as
///   Development with neither variable set composes keys under `prod` while
///   reporting `Development` everywhere else.
#[derive(Debug, Clone)]
pub struct CacheKeyComposer {
    app: String,
    environment: String,
}

impl CacheKeyComposer {
    /// Reads the application and environment segments from configuration.
    #[must_use]
    pub fn new(config: &AppConfig) -> Self {
        let app = config
            .get_string("ServiceName")
            .unwrap_or_else(|| DEFAULT_CACHE_APP_NAME.to_owned())
            .to_lowercase();

        let environment = config
            .get_string(crate::environment::ENVIRONMENT_VARIABLE)
            .or_else(|| config.get_string(crate::environment::DOTNET_ENVIRONMENT_VARIABLE))
            .unwrap_or_else(|| DEFAULT_CACHE_ENVIRONMENT.to_owned())
            .to_lowercase();

        Self { app, environment }
    }

    /// Builds a composer from explicit segments, for tests.
    #[must_use]
    pub fn from_parts(app: impl AsRef<str>, environment: impl AsRef<str>) -> Self {
        Self {
            app: app.as_ref().to_lowercase(),
            environment: environment.as_ref().to_lowercase(),
        }
    }

    /// Composes an unpartitioned key — the only form any service uses.
    #[must_use]
    pub fn compose(
        &self,
        module: &str,
        entity: &str,
        version: &str,
        discriminator: &str,
    ) -> CacheKey {
        self.compose_partitioned(
            module,
            entity,
            version,
            discriminator,
            &CachePartitions::default(),
        )
    }

    /// Composes a key with tenant, locale, or feature partitions.
    ///
    /// The C# composer takes the three partitions as trailing optional
    /// arguments; grouping them keeps call sites from turning into a row of
    /// bare `None`s.
    #[must_use]
    pub fn compose_partitioned(
        &self,
        module: &str,
        entity: &str,
        version: &str,
        discriminator: &str,
        partitions: &CachePartitions<'_>,
    ) -> CacheKey {
        CacheKey {
            environment: self.environment.clone(),
            app: self.app.clone(),
            module: module.to_lowercase(),
            entity: entity.to_lowercase(),
            version: version.to_lowercase(),
            tenant: partitions.tenant.map(str::to_lowercase),
            locale: partitions.locale.map(str::to_lowercase),
            feature: partitions.feature.map(str::to_lowercase),
            // Left exactly as given, matching the original.
            discriminator: discriminator.to_owned(),
        }
    }
}

/// The optional partitions a key may carry.
///
/// Every one of these is unused by the original's services — they all call the
/// four-argument overload — which is why its caches are global rather than
/// per-tenant despite `Caching:Partitioning:TenantAware` existing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CachePartitions<'a> {
    /// Tenant this entry belongs to.
    pub tenant: Option<&'a str>,
    /// Locale this entry was produced for.
    pub locale: Option<&'a str>,
    /// Feature flag variant this entry was produced under.
    pub feature: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::Environment;
    use figment::Figment;
    use figment::providers::Serialized;

    fn composer() -> CacheKeyComposer {
        CacheKeyComposer::from_parts("ModularMonolith.Api", "Development")
    }

    #[test]
    fn renders_the_nine_segment_layout() {
        let key = composer().compose("Music", "Album", "v1", "by-id:1");

        assert_eq!(
            key.to_string(),
            "development:modularmonolith.api:music:album:v1::::by-id:1"
        );
    }

    #[test]
    fn every_segment_is_lowercased_except_the_discriminator() {
        let key = composer().compose("MUSIC", "Album", "V1", "By-Id:ABC");

        assert_eq!(key.module, "music");
        assert_eq!(key.entity, "album");
        assert_eq!(key.version, "v1");
        assert_eq!(
            key.discriminator, "By-Id:ABC",
            "the original leaves the discriminator untouched"
        );
    }

    #[test]
    fn partitions_occupy_their_own_segments() {
        let key = composer().compose_partitioned(
            "Music",
            "Album",
            "v1",
            "all",
            &CachePartitions {
                tenant: Some("Tenant-1"),
                locale: Some("en-US"),
                feature: None,
            },
        );

        assert_eq!(
            key.to_string(),
            "development:modularmonolith.api:music:album:v1:tenant-1:en-us::all"
        );
    }

    #[test]
    fn distinct_discriminators_produce_distinct_keys() {
        let composer = composer();

        assert_ne!(
            composer
                .compose("Music", "Track", "v1", "by-album:1")
                .to_string(),
            composer
                .compose("Music", "Track", "v1", "by-artist:1")
                .to_string()
        );
    }

    #[test]
    fn configuration_supplies_the_app_and_environment_segments() {
        let figment = Figment::new().merge(Serialized::defaults(serde_json::json!({
            "servicename": "ModularMonolith.Api",
            "aspnetcore_environment": "Development"
        })));
        let config = AppConfig::from_figment(figment, Environment::Development);

        let key = CacheKeyComposer::new(&config).compose("Music", "Album", "v1", "all");

        assert_eq!(
            key.to_string(),
            "development:modularmonolith.api:music:album:v1::::all"
        );
    }

    #[test]
    fn unconfigured_hosts_compose_under_the_original_fallbacks() {
        // The composer's fallbacks are `mmapi` and `prod`, independent of the
        // host's actual environment — a quirk the port keeps.
        let config = AppConfig::from_figment(Figment::new(), Environment::Development);

        let key = CacheKeyComposer::new(&config).compose("Music", "Album", "v1", "all");

        assert_eq!(key.to_string(), "prod:mmapi:music:album:v1::::all");
    }
}
