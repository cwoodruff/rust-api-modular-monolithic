//! Rate limiting, ported from `SharedKernel.TrafficControl` (ADR-0013).
//!
//! Each module has a fixed-window policy of its own, applied once on its route
//! group, so a burst against one module answers 429 from that module only.
//! `global:public-anon` covers the root endpoint and nothing else. Limits come
//! from `RateLimiting:Policies:<name>` — `PermitLimit`, `WindowSeconds`,
//! `QueueLimit` — defaulting to 60 requests per 60 seconds with no queue.
//!
//! Rejection is a bare 429: no `Retry-After`, as upstream writes none. The
//! host's status-code-pages layer gives it its problem body.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;

use crate::AppConfig;

/// Policy names. Port of `RateLimitPolicyRegistry`.
pub mod policies {
    /// The root endpoint, `GET /`, and nothing else.
    pub const GLOBAL_PUBLIC_ANON: &str = "global:public-anon";
    /// The Catalog route group.
    pub const CATALOG: &str = "catalog:api";
    /// The Orders route group.
    pub const ORDERS: &str = "orders:api";
    /// The Administration route group.
    pub const ADMINISTRATION: &str = "admin:api";
    /// The Identity route group.
    pub const IDENTITY: &str = "identity:api";
    /// The Reporting route group.
    pub const REPORTING: &str = "reporting:api";

    /// Every policy, in the order upstream declares them.
    pub const ALL: [&str; 6] = [
        GLOBAL_PUBLIC_ANON,
        CATALOG,
        ORDERS,
        ADMINISTRATION,
        IDENTITY,
        REPORTING,
    ];
}

/// Where a policy's limits are read from.
pub const CONFIGURATION_SECTION: &str = "RateLimiting:Policies";

/// Requests permitted per window when configuration says nothing.
pub const DEFAULT_PERMIT_LIMIT: u32 = 60;

/// Window length when configuration says nothing.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(60);

/// Queue depth when configuration says nothing. Zero: rejection is immediate.
pub const DEFAULT_QUEUE_LIMIT: u32 = 0;

/// The status returned when a request is rejected.
pub const REJECTION_STATUS: u16 = 429;

/// One policy's numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitOptions {
    /// Requests per window per partition.
    pub permit_limit: u32,
    /// The fixed window's length.
    pub window: Duration,
    /// Requests that may wait for the next window instead of being refused.
    pub queue_limit: u32,
}

impl Default for RateLimitOptions {
    fn default() -> Self {
        Self {
            permit_limit: DEFAULT_PERMIT_LIMIT,
            window: DEFAULT_WINDOW,
            queue_limit: DEFAULT_QUEUE_LIMIT,
        }
    }
}

impl RateLimitOptions {
    /// Reads `RateLimiting:Policies:<name>`, falling back member by member.
    ///
    /// The policy names contain a colon, so the configuration path is
    /// `RateLimiting:Policies:catalog:api:PermitLimit` — the same nesting
    /// upstream's `appsettings.json` produces.
    #[must_use]
    pub fn from_config(config: &AppConfig, name: &str) -> Self {
        let read =
            |member: &str| config.get_string(&format!("{CONFIGURATION_SECTION}:{name}:{member}"));
        let defaults = Self::default();

        Self {
            permit_limit: read("PermitLimit")
                .and_then(|value| value.parse().ok())
                .unwrap_or(defaults.permit_limit),
            window: read("WindowSeconds")
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
                .map_or(defaults.window, Duration::from_secs_f64),
            queue_limit: read("QueueLimit")
                .and_then(|value| value.parse().ok())
                .unwrap_or(defaults.queue_limit),
        }
    }
}

/// The claims a request carries, as far as partitioning is concerned.
///
/// Deliberately narrow: the shared kernel must not depend on the Identity
/// module, so it asks only for claim lookup by name.
pub trait RequestClaims {
    /// Returns the first value of `name`, if the request carries it.
    fn claim(&self, name: &str) -> Option<&str>;
}

/// Derives the rate-limit partition for a request.
///
/// Port of `PartitionKeys.FromRequest`: the first of `client_id`, `tenant`, or
/// `sub` that is present and non-blank wins, otherwise the client's address.
///
/// Worth knowing when reading traffic: in the original the limiter runs *before*
/// authentication, so `claims` is empty for every request and partitioning
/// falls through to the address in practice. The port keeps that ordering, and
/// keeps this logic intact for when it changes.
#[must_use]
pub fn partition_key(claims: Option<&dyn RequestClaims>, remote_address: Option<IpAddr>) -> String {
    if let Some(claims) = claims {
        for (claim, prefix) in [
            ("client_id", "client"),
            ("tenant", "tenant"),
            ("sub", "sub"),
        ] {
            if let Some(value) = claims.claim(claim).map(str::trim).filter(|v| !v.is_empty()) {
                return format!("{prefix}:{}", value.to_lowercase());
            }
        }
    }

    match remote_address {
        Some(address) => format!("ip:{}", normalize_address(address)),
        None => "ip:unknown".to_owned(),
    }
}

/// Renders an address for use in a partition key.
///
/// The original calls `MapToIPv4()` on every IPv6 address, which produces a
/// meaningful result only for IPv4-mapped ones and a nonsense address for the
/// rest — `::1` becomes `0.0.0.1`. Here a genuine IPv6 address keeps its own
/// text. This changes only which bucket a client lands in, never a response.
fn normalize_address(address: IpAddr) -> String {
    match address {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map_or_else(|| v6.to_string(), |v4| v4.to_string()),
    }
}

/// One policy's fixed-window limiter, partitioned by [`partition_key`].
///
/// # Why partitions expire
///
/// Every distinct partition key gets a map entry, and the key is the caller's
/// address. An entry whose window closed carries no information — the next
/// request resets it anyway — so closed windows are swept, at most once per
/// window and only while a request is already being counted (F12).
#[derive(Debug, Clone)]
pub struct RateLimiter {
    name: &'static str,
    options: RateLimitOptions,
    windows: Arc<DashMap<String, Window>>,
    queued: Arc<AtomicU32>,
    last_sweep: Arc<std::sync::Mutex<Option<Instant>>>,
}

#[derive(Debug, Clone, Copy)]
struct Window {
    started: Instant,
    used: u32,
}

impl RateLimiter {
    /// A limiter for the policy `name`.
    #[must_use]
    pub fn new(name: &'static str, options: RateLimitOptions) -> Self {
        Self {
            name,
            options,
            windows: Arc::new(DashMap::new()),
            queued: Arc::new(AtomicU32::new(0)),
            last_sweep: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// The limiter for `name`, with its limits read from configuration.
    #[must_use]
    pub fn from_config(config: &AppConfig, name: &'static str) -> Self {
        Self::new(name, RateLimitOptions::from_config(config, name))
    }

    /// The policy's name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The policy's numbers.
    #[must_use]
    pub fn options(&self) -> RateLimitOptions {
        self.options
    }

    /// Records a request against `partition`, reporting whether it is allowed now.
    fn try_acquire(&self, partition: &str) -> bool {
        self.try_acquire_at(partition, Instant::now()).is_ok()
    }

    /// `Ok` when allowed; otherwise when the partition's window ends.
    fn try_acquire_at(&self, partition: &str, now: Instant) -> Result<(), Instant> {
        self.sweep_if_due(now);

        let mut entry = self.windows.entry(partition.to_owned()).or_insert(Window {
            started: now,
            used: 0,
        });

        if now.duration_since(entry.started) >= self.options.window {
            entry.started = now;
            entry.used = 0;
        }

        if entry.used >= self.options.permit_limit {
            return Err(entry.started + self.options.window);
        }

        entry.used += 1;
        Ok(())
    }

    /// Admits a request now, after queueing for the next window, or not at all.
    ///
    /// With `QueueLimit` zero — every policy upstream configures — a request
    /// over the limit is refused at once.
    pub async fn acquire(&self, partition: &str) -> bool {
        let reopens = match self.try_acquire_at(partition, Instant::now()) {
            Ok(()) => return true,
            Err(reopens) => reopens,
        };

        if self.queued.fetch_add(1, Ordering::SeqCst) >= self.options.queue_limit {
            self.queued.fetch_sub(1, Ordering::SeqCst);
            return false;
        }

        tokio::time::sleep_until(reopens.into()).await;
        let admitted = self.try_acquire(partition);
        self.queued.fetch_sub(1, Ordering::SeqCst);
        admitted
    }

    /// Drops every partition whose window has already closed, returning how many.
    pub fn sweep_closed_windows(&self) -> usize {
        self.sweep_closed_windows_as_of(Instant::now())
    }

    fn sweep_closed_windows_as_of(&self, now: Instant) -> usize {
        let before = self.windows.len();
        self.windows
            .retain(|_, window| now.duration_since(window.started) < self.options.window);
        before.saturating_sub(self.windows.len())
    }

    /// Sweeps, but at most once per window. A poisoned lock means "not due".
    fn sweep_if_due(&self, now: Instant) {
        {
            let Ok(mut last) = self.last_sweep.lock() else {
                return;
            };
            if last.is_some_and(|last| now.duration_since(last) < self.options.window) {
                return;
            }
            *last = Some(now);
        }

        self.sweep_closed_windows_as_of(now);
    }

    /// How many partitions are being tracked, for tests.
    #[must_use]
    pub fn tracked_partitions(&self) -> usize {
        self.windows.len()
    }
}

/// Applies a policy to a route group, rejecting with a bare 429.
///
/// Port of `RequireRateLimiting(name)`. The partition never sees claims:
/// upstream's limiter runs before authentication, so every caller is
/// partitioned by address, and that ordering is kept (P7, P24).
pub async fn rate_limit(limiter: RateLimiter, request: Request, next: Next) -> Response {
    let address = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(socket)| socket.ip());

    let partition = partition_key(None, address);

    if limiter.acquire(&partition).await {
        next.run(request).await
    } else {
        tracing::debug!(policy = limiter.name, partition, "rate limit exceeded");
        StatusCode::TOO_MANY_REQUESTS.into_response()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::collections::HashMap;

    use super::*;

    struct Claims(HashMap<&'static str, &'static str>);

    impl Claims {
        fn new(pairs: impl IntoIterator<Item = (&'static str, &'static str)>) -> Self {
            Self(pairs.into_iter().collect())
        }
    }

    impl RequestClaims for Claims {
        fn claim(&self, name: &str) -> Option<&str> {
            self.0.get(name).copied()
        }
    }

    fn loopback() -> Option<IpAddr> {
        Some("203.0.113.7".parse().expect("valid address"))
    }

    #[test]
    fn client_id_outranks_every_other_claim() {
        let claims = Claims::new([
            ("client_id", "Portal"),
            ("tenant", "tenant-1"),
            ("sub", "user-1"),
        ]);

        assert_eq!(partition_key(Some(&claims), loopback()), "client:portal");
    }

    #[test]
    fn tenant_outranks_subject() {
        let claims = Claims::new([("tenant", "Tenant-1"), ("sub", "user-1")]);

        assert_eq!(partition_key(Some(&claims), loopback()), "tenant:tenant-1");
    }

    #[test]
    fn subject_is_used_when_it_is_the_only_claim() {
        let claims = Claims::new([("sub", "User-1")]);

        assert_eq!(partition_key(Some(&claims), loopback()), "sub:user-1");
    }

    #[test]
    fn blank_claims_are_skipped_rather_than_used() {
        // The original tests each candidate with IsNullOrWhiteSpace, so a claim
        // present but empty must not win the partition.
        let claims = Claims::new([("client_id", "   "), ("tenant", "tenant-1")]);

        assert_eq!(partition_key(Some(&claims), loopback()), "tenant:tenant-1");
    }

    #[test]
    fn values_are_trimmed_and_lowercased() {
        let claims = Claims::new([("sub", "  USER-1  ")]);

        assert_eq!(partition_key(Some(&claims), loopback()), "sub:user-1");
    }

    #[test]
    fn requests_without_claims_partition_by_address() {
        assert_eq!(partition_key(None, loopback()), "ip:203.0.113.7");
    }

    #[test]
    fn an_unknown_address_still_yields_a_stable_key() {
        assert_eq!(partition_key(None, None), "ip:unknown");
    }

    #[test]
    fn ipv4_mapped_addresses_collapse_to_their_ipv4_form() {
        let mapped: IpAddr = "::ffff:203.0.113.7".parse().expect("valid address");

        assert_eq!(partition_key(None, Some(mapped)), "ip:203.0.113.7");
    }

    #[test]
    fn genuine_ipv6_addresses_keep_their_own_text() {
        let address: IpAddr = "2001:db8::1".parse().expect("valid address");

        assert_eq!(partition_key(None, Some(address)), "ip:2001:db8::1");
    }

    #[test]
    fn the_policies_keep_upstreams_names_and_defaults() {
        assert_eq!(
            policies::ALL,
            [
                "global:public-anon",
                "catalog:api",
                "orders:api",
                "admin:api",
                "identity:api",
                "reporting:api"
            ]
        );
        let defaults = RateLimitOptions::default();
        assert_eq!(defaults.permit_limit, 60);
        assert_eq!(defaults.window, Duration::from_secs(60));
        assert_eq!(defaults.queue_limit, 0);
        assert_eq!(REJECTION_STATUS, 429);
    }

    #[test]
    fn the_limiter_allows_exactly_the_permitted_number() {
        let limiter = RateLimiter::new(
            "test",
            RateLimitOptions {
                permit_limit: 3,
                ..RateLimitOptions::default()
            },
        );

        assert!(limiter.try_acquire("ip:198.51.100.1"));
        assert!(limiter.try_acquire("ip:198.51.100.1"));
        assert!(limiter.try_acquire("ip:198.51.100.1"));
        assert!(
            !limiter.try_acquire("ip:198.51.100.1"),
            "the fourth request should be rejected"
        );
    }

    #[test]
    fn partitions_have_independent_allowances() {
        let limiter = RateLimiter::new(
            "test",
            RateLimitOptions {
                permit_limit: 1,
                ..RateLimitOptions::default()
            },
        );

        assert!(limiter.try_acquire("ip:198.51.100.1"));
        assert!(!limiter.try_acquire("ip:198.51.100.1"));
        assert!(
            limiter.try_acquire("ip:198.51.100.2"),
            "a different partition has its own budget"
        );
    }

    #[test]
    fn two_policies_never_share_a_budget() {
        let catalog = RateLimiter::new(
            policies::CATALOG,
            RateLimitOptions {
                permit_limit: 1,
                ..RateLimitOptions::default()
            },
        );
        let orders = RateLimiter::new(
            policies::ORDERS,
            RateLimitOptions {
                permit_limit: 1,
                ..RateLimitOptions::default()
            },
        );

        assert!(catalog.try_acquire("ip:198.51.100.1"));
        assert!(!catalog.try_acquire("ip:198.51.100.1"));
        assert!(
            orders.try_acquire("ip:198.51.100.1"),
            "Orders has its own window"
        );
    }

    #[test]
    fn the_window_resets_once_it_has_elapsed() {
        let limiter = RateLimiter::new(
            "test",
            RateLimitOptions {
                permit_limit: 1,
                window: Duration::from_millis(1),
                queue_limit: 0,
            },
        );

        assert!(limiter.try_acquire("ip:198.51.100.1"));
        assert!(!limiter.try_acquire("ip:198.51.100.1"));
        std::thread::sleep(Duration::from_millis(5));
        assert!(
            limiter.try_acquire("ip:198.51.100.1"),
            "a fixed window starts over rather than sliding"
        );
    }

    #[test]
    fn partitions_whose_window_has_closed_are_forgotten() {
        let limiter = RateLimiter::new(
            "test",
            RateLimitOptions {
                permit_limit: 1,
                window: Duration::from_millis(5),
                queue_limit: 0,
            },
        );

        for octet in 1..=20 {
            assert!(limiter.try_acquire(&format!("ip:198.51.100.{octet}")));
        }
        assert_eq!(limiter.tracked_partitions(), 20);
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(limiter.sweep_closed_windows(), 20);
        assert_eq!(limiter.tracked_partitions(), 0);
    }

    #[test]
    fn a_sweep_leaves_an_open_window_alone() {
        let limiter = RateLimiter::new(
            "test",
            RateLimitOptions {
                permit_limit: 1,
                ..RateLimitOptions::default()
            },
        );

        assert!(limiter.try_acquire("ip:198.51.100.1"));
        assert_eq!(limiter.sweep_closed_windows(), 0);
        assert!(
            !limiter.try_acquire("ip:198.51.100.1"),
            "the allowance must have survived the sweep"
        );
    }

    #[tokio::test]
    async fn a_queued_request_waits_for_the_next_window() {
        let limiter = RateLimiter::new(
            "test",
            RateLimitOptions {
                permit_limit: 1,
                window: Duration::from_millis(20),
                queue_limit: 1,
            },
        );

        assert!(limiter.acquire("ip:198.51.100.1").await);
        assert!(
            limiter.acquire("ip:198.51.100.1").await,
            "the second waits rather than failing"
        );
    }
}
