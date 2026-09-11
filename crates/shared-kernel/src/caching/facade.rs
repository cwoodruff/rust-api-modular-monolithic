//! The cache facade, ported from `CompositeCacheFacade`.
//!
//! # What changed, and why
//!
//! The C# facade is an interface (`ICacheFacade`) with generic methods. A Rust
//! trait with generic methods cannot be used as a trait object, so this is a
//! concrete type instead — which costs nothing here, because the facade was
//! never polymorphic in practice: one implementation, resolved through DI.
//!
//! Two behaviors differ from the original on purpose:
//!
//! 1. **`remove_by_tag` works.** The C# `RemoveByTagAsync` is a documented
//!    no-op, so in the original a write invalidates only the single `by-id`
//!    entry it touched — every collection entry (`all`, `by-artist:3`, …)
//!    stays stale for the rest of its 20-minute lifetime even though both the
//!    services and the docs are written as though tagging works. This port
//!    keeps a real tag index. The divergence only ever makes a read *fresher*.
//!
//! 2. **Single flight is exact.** The C# per-key semaphore is removed from its
//!    dictionary inside the same `finally` that releases it, so concurrent
//!    callers can end up waiting on different semaphore instances and more
//!    than one can run the factory. Here the coalescing is handled by the
//!    cache itself and is strict.
//!
//! The second tier is not built yet: the original defaults to `L1` and ships no
//! `Caching` section, so the L1 path is the one that actually runs. When L2
//! lands it slots in behind [`CacheOptions::uses_distributed_tier`].

use std::any::Any;
use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use moka::Expiry;
use moka::future::Cache;
use moka::notification::RemovalCause;

use super::key::CacheKey;
use super::options::{CacheEntryOptions, CacheOptions};

/// Maps a tag to the set of cache keys carrying it.
type TagIndex = Arc<DashMap<String, HashSet<String>>>;

/// A cached value plus the metadata needed to expire and invalidate it.
///
/// The value is type-erased so one cache can hold every module's shapes, the
/// way `IMemoryCache` stores boxed objects. Unlike the distributed tier, which
/// serializes, this keeps the value itself — no serialization round trip, and
/// so no chance of one changing a value on the way through.
#[derive(Clone)]
struct CachedEntry {
    value: Arc<dyn Any + Send + Sync>,
    absolute: Duration,
    sliding: Option<Duration>,
    tags: Arc<[String]>,
}

/// Applies each entry's own lifetime, since the original sets expiry per entry
/// rather than per cache.
struct EntryExpiry;

impl Expiry<String, CachedEntry> for EntryExpiry {
    fn expire_after_create(
        &self,
        _key: &String,
        value: &CachedEntry,
        _created_at: Instant,
    ) -> Option<Duration> {
        Some(value.absolute)
    }

    fn expire_after_update(
        &self,
        _key: &String,
        value: &CachedEntry,
        _updated_at: Instant,
        _duration_until_expiry: Option<Duration>,
    ) -> Option<Duration> {
        Some(value.absolute)
    }

    fn expire_after_read(
        &self,
        _key: &String,
        value: &CachedEntry,
        _read_at: Instant,
        duration_until_expiry: Option<Duration>,
        _last_modified_at: Instant,
    ) -> Option<Duration> {
        // Sliding expiration restarts the clock on every read; without it the
        // absolute deadline stands.
        value.sliding.or(duration_until_expiry)
    }
}

/// Cache-aside storage for the service layer.
pub struct CacheFacade {
    options: CacheOptions,
    entries: Cache<String, CachedEntry>,
    tags: TagIndex,
}

impl std::fmt::Debug for CacheFacade {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CacheFacade")
            .field("options", &self.options)
            .field("entries", &self.entries.entry_count())
            .field("tags", &self.tags.len())
            .finish()
    }
}

impl CacheFacade {
    /// Builds a facade from the `Caching` configuration section.
    #[must_use]
    pub fn new(options: CacheOptions) -> Self {
        let tags: TagIndex = Arc::new(DashMap::new());
        let eviction_tags = Arc::clone(&tags);

        let entries = Cache::builder()
            .expire_after(EntryExpiry)
            // Keeps the tag index from accumulating keys for entries that have
            // already expired or been evicted.
            .eviction_listener(
                move |key: Arc<String>, entry: CachedEntry, _cause: RemovalCause| {
                    forget_tags(&eviction_tags, key.as_str(), &entry.tags);
                },
            )
            .build();

        Self {
            options,
            entries,
            tags,
        }
    }

    /// The configuration this facade was built with.
    #[must_use]
    pub fn options(&self) -> &CacheOptions {
        &self.options
    }

    /// Reads through the cache, running `factory` only on a miss.
    ///
    /// A factory returning `None` is not cached, matching the original: a
    /// lookup that found nothing must not pin a negative result for the next
    /// twenty minutes.
    pub async fn get_or_add<T, F, Fut>(
        &self,
        key: &CacheKey,
        factory: F,
        options: Option<CacheEntryOptions>,
    ) -> Option<T>
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Option<T>> + Send,
    {
        if !self.options.enabled {
            return factory().await;
        }

        let effective = self.effective(options);
        let cache_key = key.to_string();
        let index_key = cache_key.clone();
        let index = Arc::clone(&self.tags);

        let entry = self
            .entries
            .optionally_get_with(cache_key, async move {
                let value = factory().await?;

                register_tags(&index, &index_key, &effective.tags);

                Some(CachedEntry {
                    value: Arc::new(value),
                    absolute: effective.absolute,
                    sliding: effective.sliding,
                    tags: effective.tags,
                })
            })
            .await;

        entry.and_then(|cached| cached.value.downcast_ref::<T>().cloned())
    }

    /// Writes a value directly.
    pub async fn set<T>(&self, key: &CacheKey, value: T, options: Option<CacheEntryOptions>)
    where
        T: Send + Sync + 'static,
    {
        if !self.options.enabled {
            return;
        }

        let effective = self.effective(options);
        let cache_key = key.to_string();

        register_tags(&self.tags, &cache_key, &effective.tags);

        self.entries
            .insert(
                cache_key,
                CachedEntry {
                    value: Arc::new(value),
                    absolute: effective.absolute,
                    sliding: effective.sliding,
                    tags: effective.tags,
                },
            )
            .await;
    }

    /// Drops one entry.
    pub async fn remove(&self, key: &CacheKey) {
        self.entries.invalidate(&key.to_string()).await;
    }

    /// Drops every entry carrying `tag`.
    ///
    /// This is the method the C# original stubs out. Services call it after
    /// every write, so without it the collection entries a write should
    /// invalidate simply survive.
    pub async fn remove_by_tag(&self, tag: &str) {
        let Some((_, keys)) = self.tags.remove(tag) else {
            return;
        };

        for key in keys {
            self.entries.invalidate(&key).await;
        }
    }

    /// Resolves an entry's settings: the caller's lifetime or the configured
    /// default, then jitter.
    fn effective(&self, options: Option<CacheEntryOptions>) -> EffectiveEntryOptions {
        let options = options.unwrap_or_default();

        let base = options
            .absolute_expiration_relative_to_now
            .unwrap_or_else(|| Duration::from_secs(self.options.default_ttl_seconds));

        EffectiveEntryOptions {
            absolute: apply_jitter(base, options.jitter_percent),
            sliding: options.sliding_expiration,
            tags: Arc::from(options.tags),
        }
    }
}

/// An entry's settings after defaults and jitter have been applied.
struct EffectiveEntryOptions {
    absolute: Duration,
    sliding: Option<Duration>,
    tags: Arc<[String]>,
}

/// Spreads a lifetime by ±`jitter_percent` so entries written together do not
/// expire together.
///
/// The factor is clamped at zero: in C# an absurd `JitterPercent` yields a
/// negative `TimeSpan` and a harmless fallback, whereas here it would panic.
fn apply_jitter(ttl: Duration, jitter_percent: f64) -> Duration {
    if ttl.is_zero() || jitter_percent <= 0.0 {
        return ttl;
    }

    let roll: f64 = rand::random();
    let jitter = (roll - 0.5) * 2.0 * jitter_percent;
    let adjusted = ttl.mul_f64((1.0 + jitter).max(0.0));

    if adjusted.is_zero() { ttl } else { adjusted }
}

/// Records that `cache_key` belongs to each of `tags`.
fn register_tags(index: &TagIndex, cache_key: &str, tags: &[String]) {
    for tag in tags {
        index
            .entry(tag.clone())
            .or_default()
            .insert(cache_key.to_owned());
    }
}

/// Removes `cache_key` from each of `tags`, dropping tags left empty.
fn forget_tags(index: &TagIndex, cache_key: &str, tags: &[String]) {
    for tag in tags {
        let emptied = {
            let Some(mut keys) = index.get_mut(tag) else {
                continue;
            };
            keys.remove(cache_key);
            keys.is_empty()
        };

        if emptied {
            index.remove_if(tag, |_, keys| keys.is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::caching::key::CacheKeyComposer;

    fn facade() -> CacheFacade {
        CacheFacade::new(CacheOptions::default())
    }

    fn key(discriminator: &str) -> CacheKey {
        CacheKeyComposer::from_parts("mmapi", "test").compose("Music", "Album", "v1", discriminator)
    }

    fn album_options() -> CacheEntryOptions {
        CacheEntryOptions::for_service(["music:album", "music:album:by-id"])
    }

    /// A factory that counts how many times it actually ran.
    struct CountingFactory {
        calls: AtomicUsize,
    }

    impl CountingFactory {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }

        async fn produce(&self, value: &str) -> Option<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Some(value.to_owned())
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn a_second_read_is_served_from_the_cache() {
        let cache = facade();
        let factory = CountingFactory::new();
        let key = key("by-id:1");

        let first: Option<String> = cache
            .get_or_add(&key, || factory.produce("Let It Be"), Some(album_options()))
            .await;
        let second: Option<String> = cache
            .get_or_add(&key, || factory.produce("Let It Be"), Some(album_options()))
            .await;

        assert_eq!(first.as_deref(), Some("Let It Be"));
        assert_eq!(second.as_deref(), Some("Let It Be"));
        assert_eq!(
            factory.calls(),
            1,
            "the second read should not hit the factory"
        );
    }

    #[tokio::test]
    async fn missing_values_are_not_cached() {
        // A 404 must not pin a negative result for the entry's whole lifetime.
        let cache = facade();
        let calls = AtomicUsize::new(0);
        let key = key("by-id:999");

        for _ in 0..2 {
            let found: Option<String> = cache
                .get_or_add(
                    &key,
                    || async {
                        calls.fetch_add(1, Ordering::SeqCst);
                        None
                    },
                    Some(album_options()),
                )
                .await;

            assert!(found.is_none());
        }

        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_disabled_cache_always_calls_the_factory() {
        let cache = CacheFacade::new(CacheOptions {
            enabled: false,
            ..CacheOptions::default()
        });
        let factory = CountingFactory::new();
        let key = key("by-id:1");

        for _ in 0..3 {
            let _: Option<String> = cache
                .get_or_add(&key, || factory.produce("Let It Be"), Some(album_options()))
                .await;
        }

        assert_eq!(factory.calls(), 3);
    }

    #[tokio::test]
    async fn removing_a_key_forces_the_next_read_to_recompute() {
        let cache = facade();
        let factory = CountingFactory::new();
        let key = key("by-id:1");

        let _: Option<String> = cache
            .get_or_add(&key, || factory.produce("first"), Some(album_options()))
            .await;
        cache.remove(&key).await;
        let refreshed: Option<String> = cache
            .get_or_add(&key, || factory.produce("second"), Some(album_options()))
            .await;

        assert_eq!(refreshed.as_deref(), Some("second"));
        assert_eq!(factory.calls(), 2);
    }

    /// The headline fix: in the C# original this method does nothing, so the
    /// collection entries below would still be served stale after a write.
    #[tokio::test]
    async fn removing_a_tag_invalidates_every_entry_carrying_it() {
        let cache = facade();
        let factory = CountingFactory::new();
        let collection = key("all");
        let by_artist = key("by-artist:3");
        let single = key("by-id:1");

        for entry in [&collection, &by_artist, &single] {
            let _: Option<String> = cache
                .get_or_add(entry, || factory.produce("cached"), Some(album_options()))
                .await;
        }
        assert_eq!(factory.calls(), 3);

        // What a service does after a write.
        cache.remove_by_tag("music:album").await;

        for entry in [&collection, &by_artist, &single] {
            let _: Option<String> = cache
                .get_or_add(entry, || factory.produce("fresh"), Some(album_options()))
                .await;
        }

        assert_eq!(
            factory.calls(),
            6,
            "every entry carrying the tag should have been invalidated"
        );
    }

    #[tokio::test]
    async fn removing_a_tag_leaves_other_entities_alone() {
        let cache = facade();
        let factory = CountingFactory::new();

        let album = key("by-id:1");
        let artist = CacheKeyComposer::from_parts("mmapi", "test")
            .compose("Music", "Artist", "v1", "by-id:1");
        let artist_options = CacheEntryOptions::for_service(["music:artist"]);

        let _: Option<String> = cache
            .get_or_add(&album, || factory.produce("album"), Some(album_options()))
            .await;
        let _: Option<String> = cache
            .get_or_add(
                &artist,
                || factory.produce("artist"),
                Some(artist_options.clone()),
            )
            .await;

        cache.remove_by_tag("music:album").await;

        let _: Option<String> = cache
            .get_or_add(&artist, || factory.produce("artist"), Some(artist_options))
            .await;

        assert_eq!(factory.calls(), 2, "the artist entry should have survived");
    }

    #[tokio::test]
    async fn removing_an_unknown_tag_is_harmless() {
        let cache = facade();

        cache.remove_by_tag("music:nonexistent").await;
    }

    #[tokio::test]
    async fn concurrent_misses_collapse_into_one_factory_call() {
        let cache = facade();
        let calls = AtomicUsize::new(0);
        let key = key("by-id:1");

        let read = || async {
            cache
                .get_or_add::<String, _, _>(
                    &key,
                    || async {
                        calls.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        Some("Let It Be".to_owned())
                    },
                    Some(album_options()),
                )
                .await
        };

        let results = tokio::join!(read(), read(), read(), read(), read(), read());

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        for result in [
            results.0, results.1, results.2, results.3, results.4, results.5,
        ] {
            assert_eq!(result.as_deref(), Some("Let It Be"));
        }
    }

    #[tokio::test]
    async fn values_round_trip_without_serialization() {
        // The L1 tier stores the value itself, so shapes that would not survive
        // a JSON round trip unchanged come back exactly as written.
        let cache = facade();
        let key = key("all");

        let stored = vec![
            ("Let It Be".to_owned(), 1_i64),
            ("Abbey Road".to_owned(), 2),
        ];
        cache.set(&key, stored.clone(), Some(album_options())).await;

        let loaded: Option<Vec<(String, i64)>> = cache
            .get_or_add(&key, || async { None }, Some(album_options()))
            .await;

        assert_eq!(loaded, Some(stored));
    }

    #[test]
    fn jitter_stays_within_the_configured_band() {
        let base = Duration::from_secs(1200);
        let percent = 0.1;

        let mut saw_below = false;
        let mut saw_above = false;

        for _ in 0..1_000 {
            let adjusted = apply_jitter(base, percent);

            assert!(
                adjusted >= base.mul_f64(1.0 - percent) && adjusted <= base.mul_f64(1.0 + percent),
                "{adjusted:?} fell outside ±{percent} of {base:?}"
            );

            if adjusted < base {
                saw_below = true;
            }
            if adjusted > base {
                saw_above = true;
            }
        }

        assert!(
            saw_below && saw_above,
            "jitter should spread in both directions"
        );
    }

    #[test]
    fn jitter_is_skipped_when_disabled() {
        let base = Duration::from_secs(1200);

        assert_eq!(apply_jitter(base, 0.0), base);
        assert_eq!(apply_jitter(Duration::ZERO, 0.1), Duration::ZERO);
    }

    #[test]
    fn an_absurd_jitter_percent_cannot_panic() {
        // C# would produce a negative TimeSpan here and fall back; a negative
        // Duration would panic, so the factor is clamped instead.
        let base = Duration::from_secs(60);

        for _ in 0..100 {
            let _ = apply_jitter(base, 50.0);
        }
    }
}
