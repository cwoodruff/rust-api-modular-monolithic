//! Cache configuration, ported from `CacheOptions` and `CacheEntryOptions`.
//!
//! The full `Caching` section shape is reproduced, including the members the
//! original declares but never reads, so an existing configuration file binds
//! unchanged. Each unread member is marked as such rather than quietly
//! implying behavior that does not exist.
//!
//! Note that the original ships *no* `Caching` section in either appsettings
//! file, so in practice every default here is what actually runs.

use std::time::Duration;

use serde::Deserialize;

/// Default entry lifetime when a caller supplies none, in seconds.
pub const DEFAULT_TTL_SECONDS: u64 = 300;

/// Default jitter applied to an entry's lifetime: ±10%.
pub const DEFAULT_JITTER_PERCENT: f64 = 0.1;

/// The lifetime every service in the original actually asks for.
///
/// Each service hard-codes 20 minutes, ignoring both [`DEFAULT_TTL_SECONDS`]
/// and the per-module values under `Caching:PerModule`.
pub const SERVICE_TTL: Duration = Duration::from_secs(20 * 60);

/// Binding of the `Caching` configuration section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CacheOptions {
    /// When false, every read goes straight to the factory.
    pub enabled: bool,

    /// `L1` for memory only, `L1L2` to add a distributed tier.
    pub tier: String,

    /// `InMemory` or `Redis`. Only `InMemory` is implemented, as in the original.
    pub provider: String,

    /// Fallback entry lifetime, in seconds.
    #[serde(rename = "defaultttlseconds", alias = "default_ttl_seconds")]
    pub default_ttl_seconds: u64,

    /// Stale-while-revalidate settings. Declared but unread, as in the original.
    pub swr: SwrOptions,

    /// Stampede protection. Declared but unread: single flight is unconditional.
    pub stampede: StampedeOptions,

    /// Cache partitioning. Declared but unread: no caller passes a tenant.
    pub partitioning: PartitioningOptions,

    /// Per-module lifetimes. Declared but unread: services hard-code 20 minutes.
    #[serde(rename = "permodule", alias = "per_module")]
    pub per_module: ModuleTtls,

    /// Redis connection settings, used only when the L2 tier lands.
    pub redis: RedisOptions,
}

impl Default for CacheOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            tier: "L1".to_owned(),
            provider: "InMemory".to_owned(),
            default_ttl_seconds: DEFAULT_TTL_SECONDS,
            swr: SwrOptions::default(),
            stampede: StampedeOptions::default(),
            partitioning: PartitioningOptions::default(),
            per_module: ModuleTtls::default(),
            redis: RedisOptions::default(),
        }
    }
}

impl CacheOptions {
    /// Whether a distributed second tier is configured.
    #[must_use]
    pub fn uses_distributed_tier(&self) -> bool {
        self.tier.eq_ignore_ascii_case("L1L2")
    }
}

/// Stale-while-revalidate settings. Not implemented in the original.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SwrOptions {
    /// Whether serving stale entries is allowed.
    pub enabled: bool,

    /// How long an entry may be served past expiry. Zero disables.
    #[serde(rename = "maxstaleseconds", alias = "max_stale_seconds")]
    pub max_stale_seconds: u64,
}

/// Stampede protection settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StampedeOptions {
    /// Whether concurrent misses for one key collapse into a single factory call.
    #[serde(rename = "singleflight", alias = "single_flight")]
    pub single_flight: bool,
}

impl Default for StampedeOptions {
    fn default() -> Self {
        Self {
            single_flight: true,
        }
    }
}

/// Cache partitioning settings. Not implemented in the original.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PartitioningOptions {
    /// Whether keys are partitioned by tenant.
    #[serde(rename = "tenantaware", alias = "tenant_aware")]
    pub tenant_aware: bool,

    /// Whether keys are partitioned by region.
    #[serde(rename = "regionaware", alias = "region_aware")]
    pub region_aware: bool,
}

/// Per-module lifetime overrides, in seconds. Not read by the original.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ModuleTtls {
    /// Lifetime for the Music module.
    pub music: Option<u64>,
    /// Lifetime for the Orders module.
    pub orders: Option<u64>,
    /// Lifetime for the Administration module.
    pub administration: Option<u64>,
    /// Lifetime for the Reporting module.
    pub reporting: Option<u64>,
    /// Lifetime for the Identity module.
    pub identity: Option<u64>,
}

/// Redis settings for the distributed tier.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RedisOptions {
    /// Connection string.
    #[serde(rename = "connectionstring", alias = "connection_string")]
    pub connection_string: Option<String>,

    /// Key prefix identifying this application.
    #[serde(rename = "instancename", alias = "instance_name")]
    pub instance_name: Option<String>,

    /// Whether to connect over TLS.
    pub ssl: bool,

    /// Connection pool size.
    #[serde(rename = "poolsize", alias = "pool_size")]
    pub pool_size: u32,
}

impl Default for RedisOptions {
    fn default() -> Self {
        Self {
            connection_string: None,
            instance_name: None,
            ssl: true,
            pool_size: 10,
        }
    }
}

/// Per-entry cache settings, ported from `CacheEntryOptions`.
#[derive(Debug, Clone)]
pub struct CacheEntryOptions {
    /// Lifetime from the moment the entry is written.
    pub absolute_expiration_relative_to_now: Option<Duration>,

    /// Lifetime from the entry's last read.
    pub sliding_expiration: Option<Duration>,

    /// Tags this entry belongs to, for bulk invalidation.
    pub tags: Vec<String>,

    /// Whether a stale entry may be served while it is refreshed. Unread.
    pub allow_stale_while_revalidate: bool,

    /// How long an entry may be served past expiry. Unread.
    pub max_stale: Option<Duration>,

    /// Random spread applied to the lifetime, as a fraction: `0.1` is ±10%.
    pub jitter_percent: f64,
}

impl Default for CacheEntryOptions {
    fn default() -> Self {
        Self {
            absolute_expiration_relative_to_now: None,
            sliding_expiration: None,
            tags: Vec::new(),
            allow_stale_while_revalidate: false,
            max_stale: None,
            jitter_percent: DEFAULT_JITTER_PERCENT,
        }
    }
}

impl CacheEntryOptions {
    /// The settings every service in the original uses: a 20-minute lifetime
    /// and the entity's tags.
    #[must_use]
    pub fn for_service(tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            absolute_expiration_relative_to_now: Some(SERVICE_TTL),
            tags: tags.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn defaults_match_the_original() {
        let options = CacheOptions::default();

        assert!(options.enabled);
        assert_eq!(options.tier, "L1");
        assert_eq!(options.provider, "InMemory");
        assert_eq!(options.default_ttl_seconds, 300);
        assert!(options.stampede.single_flight);
        assert!(options.redis.ssl);
        assert_eq!(options.redis.pool_size, 10);
        assert!(!options.uses_distributed_tier());
    }

    #[test]
    fn service_entries_use_a_twenty_minute_lifetime() {
        let options = CacheEntryOptions::for_service(["music:album", "music:album:by-id"]);

        assert_eq!(
            options.absolute_expiration_relative_to_now,
            Some(Duration::from_secs(1200))
        );
        assert_eq!(options.tags, vec!["music:album", "music:album:by-id"]);
        assert!((options.jitter_percent - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn section_binds_from_the_lowercased_configuration_keys() {
        // The config loader lowercases every key, so `DefaultTTLSeconds` from a
        // JSON file arrives here as `defaultttlseconds`.
        let json = serde_json::json!({
            "enabled": false,
            "tier": "L1L2",
            "defaultttlseconds": 60,
            "stampede": { "singleflight": false },
            "redis": { "connectionstring": "localhost:6379", "poolsize": 4 }
        });

        let options: CacheOptions = serde_json::from_value(json).expect("section should bind");

        assert!(!options.enabled);
        assert!(options.uses_distributed_tier());
        assert_eq!(options.default_ttl_seconds, 60);
        assert!(!options.stampede.single_flight);
        assert_eq!(
            options.redis.connection_string.as_deref(),
            Some("localhost:6379")
        );
        assert_eq!(options.redis.pool_size, 4);
        // Untouched members keep their defaults.
        assert_eq!(options.provider, "InMemory");
        assert!(options.redis.ssl);
    }
}
