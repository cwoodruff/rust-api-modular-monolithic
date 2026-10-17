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
/// and `DefaultTTLSeconds`.
pub const SERVICE_TTL: Duration = Duration::from_secs(20 * 60);

/// Binding of the `Caching` configuration section.
///
/// Upstream slimmed this to four members in its bulkhead phase; the per-module
/// entry limits moved to `Caching:Modules:<Module>:SizeLimit`, read by each
/// module (see [`super::size_limit_key`]).
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
}

impl Default for CacheOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            tier: "L1".to_owned(),
            provider: "InMemory".to_owned(),
            default_ttl_seconds: DEFAULT_TTL_SECONDS,
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
        assert!(!options.uses_distributed_tier());
    }

    #[test]
    fn service_entries_use_a_twenty_minute_lifetime() {
        let options = CacheEntryOptions::for_service(["catalog:album", "catalog:album:by-id"]);

        assert_eq!(
            options.absolute_expiration_relative_to_now,
            Some(Duration::from_secs(1200))
        );
        assert_eq!(options.tags, vec!["catalog:album", "catalog:album:by-id"]);
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
            "modules": { "catalog": { "sizelimit": 1000 } }
        });

        let options: CacheOptions = serde_json::from_value(json).expect("section should bind");

        assert!(!options.enabled);
        assert!(options.uses_distributed_tier());
        assert_eq!(options.default_ttl_seconds, 60);
        // Untouched members keep their defaults, and the per-module limits
        // beside them do not disturb the binding.
        assert_eq!(options.provider, "InMemory");
    }
}
