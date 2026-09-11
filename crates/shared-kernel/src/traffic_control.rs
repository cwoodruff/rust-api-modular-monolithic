//! Rate limiting, ported from the C# `SharedKernel.TrafficControl` namespace.
//!
//! The original declares five policy names but registers exactly one, and
//! applies that one to every endpoint in the application — health checks, the
//! root endpoint, and every business route share a single budget per partition.
//! The four unused names are kept here because the documentation refers to them.
//!
//! The limiter itself is wired in the host (Phase 4); this module supplies the
//! partition key and the numbers.

use std::net::IpAddr;
use std::time::Duration;

/// Canonical policy names. Modules reference these rather than string literals.
pub mod policies {
    /// The only policy the original actually registers.
    pub const GLOBAL_PUBLIC_ANON: &str = "global:public-anon";

    /// Declared but never registered in the original.
    pub const GLOBAL_USER_STANDARD: &str = "global:user-standard";

    /// Declared but never registered in the original.
    pub const GLOBAL_TENANT_STANDARD: &str = "global:tenant-standard";

    /// Declared but never registered in the original.
    pub const GLOBAL_ADMIN_ELEVATED: &str = "global:admin-elevated";

    /// Declared but never registered in the original.
    pub const REPORTING_HEAVY: &str = "reporting:heavy";
}

/// The configuration section the original reserves but never binds.
pub const CONFIGURATION_SECTION: &str = "RateLimiting";

/// Requests permitted per window under `global:public-anon`.
pub const PUBLIC_ANON_PERMIT_LIMIT: u32 = 60;

/// Length of the fixed window under `global:public-anon`.
pub const PUBLIC_ANON_WINDOW: Duration = Duration::from_secs(60);

/// Queue depth under `global:public-anon`. Zero: rejection is immediate.
pub const PUBLIC_ANON_QUEUE_LIMIT: u32 = 0;

/// The status returned when a request is rejected.
///
/// The original writes no `Retry-After` header alongside it.
pub const REJECTION_STATUS: u16 = 429;

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
    fn the_registered_policy_keeps_its_original_name_and_limits() {
        assert_eq!(policies::GLOBAL_PUBLIC_ANON, "global:public-anon");
        assert_eq!(PUBLIC_ANON_PERMIT_LIMIT, 60);
        assert_eq!(PUBLIC_ANON_WINDOW, Duration::from_secs(60));
        assert_eq!(PUBLIC_ANON_QUEUE_LIMIT, 0);
        assert_eq!(REJECTION_STATUS, 429);
    }
}
