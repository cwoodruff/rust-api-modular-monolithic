//! The user and refresh-token stores, ported from `InMemoryStores.cs`.
//!
//! Both are process-local and lost on restart, exactly as in the original. The
//! refresh store never evicts expired entries either — it only checks expiry on
//! read.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{DateTime, Utc};
use dashmap::DashMap;

use shared_kernel::REDACTED;

use crate::options::InMemoryUserRecord;

/// A login the store can authenticate.
///
/// `Debug` is written out rather than derived: the password is held in plain
/// text, as the original holds it, and a derived `Debug` would print it on the
/// first `tracing` call that wrote `?user`.
#[derive(Clone, PartialEq, Eq)]
pub struct UserRecord {
    /// The login name.
    pub username: String,
    /// The password, stored and compared in plain text.
    pub password: String,
    /// The `sub` claim.
    pub user_id: String,
    /// The display name, defaulted to the username.
    pub display_name: String,
    /// Role claims.
    pub roles: Vec<String>,
    /// Permission claims.
    pub permissions: Vec<String>,
    /// Email claim.
    pub email: Option<String>,
    /// Tenant claim.
    pub tenant: Option<String>,
}

impl std::fmt::Debug for UserRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UserRecord")
            .field("username", &self.username)
            .field("password", &REDACTED)
            .field("user_id", &self.user_id)
            .field("display_name", &self.display_name)
            .field("roles", &self.roles)
            .field("permissions", &self.permissions)
            .field("email", &self.email)
            .field("tenant", &self.tenant)
            .finish()
    }
}

/// Where logins come from.
pub trait UserStore: Send + Sync {
    /// Checks a username and password.
    fn validate_credentials(&self, username: &str, password: &str) -> Option<UserRecord>;

    /// Looks a user up by their `sub`, used when refreshing.
    fn find_by_id(&self, user_id: &str) -> Option<UserRecord>;
}

/// The development store, ported from `InMemoryUserStore`.
///
/// `Debug` reports how many logins it holds, not which. Its values are
/// [`UserRecord`]s, and printing the map would defeat their own redaction by
/// also printing the usernames it is keyed by.
#[derive(Default)]
pub struct InMemoryUserStore {
    /// Keyed by lowercase username, since lookup is case-insensitive.
    users: HashMap<String, UserRecord>,
}

impl std::fmt::Debug for InMemoryUserStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InMemoryUserStore")
            .field("logins", &self.users.len())
            .finish()
    }
}

impl InMemoryUserStore {
    /// Builds the store from configuration, dropping unusable entries.
    #[must_use]
    pub fn from_records(records: &[InMemoryUserRecord]) -> Self {
        let mut users = HashMap::new();
        let mut ignored = 0_usize;

        for record in records {
            if !record.is_usable() {
                ignored += 1;
                continue;
            }

            let username = record.username.trim().to_owned();
            let display_name = record
                .display_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(&username)
                .to_owned();

            let user = UserRecord {
                user_id: record.user_id.trim().to_owned(),
                display_name,
                roles: non_blank(&record.roles),
                permissions: non_blank(&record.permissions),
                email: trimmed(record.email.as_deref()),
                tenant: trimmed(record.tenant.as_deref()),
                password: record.password.clone(),
                username: username.clone(),
            };

            // The original logs the effective entry at startup, never the
            // password.
            tracing::info!(
                username = %user.username,
                user_id = %user.user_id,
                roles = %user.roles.join(", "),
                permissions = %user.permissions.join(", "),
                tenant = user.tenant.as_deref().unwrap_or("-"),
                "effective in-memory login"
            );

            users.insert(username.to_lowercase(), user);
        }

        if ignored > 0 {
            tracing::warn!(ignored, "ignored incomplete in-memory login entries");
        }

        if users.is_empty() {
            tracing::warn!("no usable in-memory logins are configured");
        } else {
            tracing::info!(count = users.len(), "loaded in-memory logins");
        }

        Self { users }
    }
}

fn non_blank(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

impl UserStore for InMemoryUserStore {
    fn validate_credentials(&self, username: &str, password: &str) -> Option<UserRecord> {
        let user = self.users.get(&username.to_lowercase())?;

        // Plain-text comparison, as in the original. Nothing here hashes.
        (user.password == password).then(|| user.clone())
    }

    fn find_by_id(&self, user_id: &str) -> Option<UserRecord> {
        self.users
            .values()
            .find(|user| user.user_id == user_id)
            .cloned()
    }
}

/// The store used outside Development and Demo, ported from
/// `DisabledUserStore`.
///
/// Refuses everything, so a production host cannot be logged into with
/// configured credentials even if some are present.
#[derive(Debug, Default, Clone, Copy)]
pub struct DisabledUserStore;

impl UserStore for DisabledUserStore {
    fn validate_credentials(&self, _username: &str, _password: &str) -> Option<UserRecord> {
        None
    }

    fn find_by_id(&self, _user_id: &str) -> Option<UserRecord> {
        None
    }
}

/// How often the refresh store sweeps entries that have already expired.
///
/// A sweep runs at most this often, and only when something is being written,
/// so an idle process does no work.
pub const PURGE_INTERVAL_SECONDS: i64 = 60;

/// The refresh-token store, ported from `InMemoryRefreshTokenStore`.
///
/// Keyed by `"{userId}:{token}"`, exactly as the original keys it.
///
/// That key is why `Debug` is written out: it *contains* the refresh token, so
/// printing the map would print every live credential in the process.
///
/// # Expiry
///
/// The original never evicts: it checks expiry on read and leaves the entry in
/// the map forever. Every login writes one, nothing ever removes it, and the
/// map grows for the life of the process — a slow leak whose contents are
/// credentials. Here each entry has a real lifetime: expired ones are swept, at
/// most once per [`PURGE_INTERVAL_SECONDS`] and only while something is being
/// written. Expiry is still checked on read, so a token that outlives its
/// deadline between sweeps is refused regardless.
pub struct InMemoryRefreshTokenStore {
    tokens: DashMap<String, DateTime<Utc>>,
    /// When the last sweep ran, as a Unix timestamp. Zero means "never".
    last_purge: AtomicI64,
    purge_interval_seconds: i64,
}

impl Default for InMemoryRefreshTokenStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for InMemoryRefreshTokenStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InMemoryRefreshTokenStore")
            .field("tokens", &self.tokens.len())
            .finish()
    }
}

impl InMemoryRefreshTokenStore {
    /// An empty store, sweeping at the default interval.
    #[must_use]
    pub fn new() -> Self {
        Self::with_purge_interval(PURGE_INTERVAL_SECONDS)
    }

    /// An empty store with an explicit sweep interval, for tests.
    #[must_use]
    pub fn with_purge_interval(purge_interval_seconds: i64) -> Self {
        Self {
            tokens: DashMap::new(),
            last_purge: AtomicI64::new(0),
            purge_interval_seconds,
        }
    }

    fn key(user_id: &str, token: &str) -> String {
        format!("{user_id}:{token}")
    }

    /// Records a token against a user.
    pub fn store(&self, user_id: &str, token: &str, expires: DateTime<Utc>) {
        // Before the insert, so the entry just written is never swept by the
        // same call that wrote it.
        self.purge_if_due(Utc::now());

        self.tokens.insert(Self::key(user_id, token), expires);
    }

    /// Whether the token is known and unexpired.
    #[must_use]
    pub fn validate(&self, user_id: &str, token: &str) -> bool {
        self.tokens
            .get(&Self::key(user_id, token))
            .is_some_and(|expires| *expires > Utc::now())
    }

    /// Consumes a token, reporting whether it was valid.
    ///
    /// This is one operation, not a check followed by a removal, and that is
    /// the whole point. `DashMap::remove` returns the entry to exactly one
    /// caller, so of any number of concurrent attempts to spend the same token
    /// only one is told it was valid. A `validate` then `revoke` pair has a
    /// window between the two calls in which every concurrent caller passes the
    /// check, and each is then issued a fresh pair off a token that should have
    /// been spendable once — a captured refresh token could be replayed as
    /// often as the race could be won.
    #[must_use]
    pub fn take(&self, user_id: &str, token: &str) -> bool {
        self.tokens
            .remove(&Self::key(user_id, token))
            .is_some_and(|(_, expires)| expires > Utc::now())
    }

    /// Forgets a token. Used on logout.
    pub fn revoke(&self, user_id: &str, token: &str) {
        self.tokens.remove(&Self::key(user_id, token));
    }

    /// Drops every entry whose deadline has passed.
    ///
    /// Returns how many were removed.
    pub fn purge_expired(&self) -> usize {
        self.purge_expired_as_of(Utc::now())
    }

    fn purge_expired_as_of(&self, now: DateTime<Utc>) -> usize {
        let before = self.tokens.len();
        self.tokens.retain(|_, expires| *expires > now);

        let removed = before.saturating_sub(self.tokens.len());
        if removed > 0 {
            tracing::debug!(removed, "swept expired refresh tokens");
        }

        removed
    }

    /// Sweeps, but only if the interval has elapsed since the last sweep.
    ///
    /// The compare-and-exchange is what keeps concurrent writers from all
    /// sweeping at once: whichever one claims the new timestamp does the work
    /// and the rest carry on.
    fn purge_if_due(&self, now: DateTime<Utc>) {
        let last = self.last_purge.load(Ordering::Relaxed);
        let now_seconds = now.timestamp();

        if now_seconds.saturating_sub(last) < self.purge_interval_seconds {
            return;
        }

        if self
            .last_purge
            .compare_exchange(last, now_seconds, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        self.purge_expired_as_of(now);
    }

    /// How many tokens are held, for tests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use chrono::Duration;

    fn records() -> Vec<InMemoryUserRecord> {
        vec![
            InMemoryUserRecord {
                username: "  Demo  ".to_owned(),
                password: "secret".to_owned(),
                user_id: "user-1".to_owned(),
                roles: vec!["User".to_owned(), "  ".to_owned()],
                permissions: vec!["music.read".to_owned()],
                tenant: Some("tenant-1".to_owned()),
                ..InMemoryUserRecord::default()
            },
            // Unusable: no password.
            InMemoryUserRecord {
                username: "broken".to_owned(),
                user_id: "user-2".to_owned(),
                ..InMemoryUserRecord::default()
            },
        ]
    }

    #[test]
    fn usernames_match_case_insensitively_and_are_trimmed() {
        let store = InMemoryUserStore::from_records(&records());

        for attempt in ["Demo", "demo", "DEMO"] {
            assert!(
                store.validate_credentials(attempt, "secret").is_some(),
                "{attempt} should authenticate"
            );
        }
    }

    #[test]
    fn passwords_match_exactly() {
        let store = InMemoryUserStore::from_records(&records());

        assert!(store.validate_credentials("demo", "secret").is_some());
        assert!(store.validate_credentials("demo", "Secret").is_none());
        assert!(store.validate_credentials("demo", "wrong").is_none());
    }

    #[test]
    fn incomplete_entries_are_dropped_rather_than_failing_startup() {
        let store = InMemoryUserStore::from_records(&records());

        assert!(store.find_by_id("user-1").is_some());
        assert!(store.find_by_id("user-2").is_none());
    }

    #[test]
    fn blank_roles_are_discarded_and_the_display_name_defaults() {
        let store = InMemoryUserStore::from_records(&records());
        let user = store.validate_credentials("demo", "secret").unwrap();

        assert_eq!(user.roles, vec!["User".to_owned()]);
        assert_eq!(
            user.display_name, "Demo",
            "defaults to the trimmed username"
        );
        assert_eq!(user.user_id, "user-1");
    }

    #[test]
    fn no_debug_rendering_here_can_disclose_a_credential() {
        // `?user` and `?store` both reach these, and a derived Debug would put
        // the password and every live refresh token in the log.
        let store = InMemoryUserStore::from_records(&records());
        let user = store.validate_credentials("demo", "secret").unwrap();

        let rendered = format!("{user:?}");
        assert!(rendered.contains("username: \"Demo\""), "{rendered}");
        assert!(!rendered.contains("secret"), "{rendered}");
        assert!(rendered.contains(REDACTED), "{rendered}");

        assert!(!format!("{store:?}").contains("secret"));

        let tokens = InMemoryRefreshTokenStore::new();
        tokens.store(
            "user-1",
            "a-real-refresh-token",
            Utc::now() + Duration::days(7),
        );

        let rendered = format!("{tokens:?}");
        assert!(
            !rendered.contains("a-real-refresh-token"),
            "the store is keyed by the token itself: {rendered}"
        );
        assert!(
            rendered.contains('1'),
            "the count is still useful: {rendered}"
        );
    }

    #[test]
    fn the_disabled_store_refuses_everything() {
        let store = DisabledUserStore;

        assert!(store.validate_credentials("demo", "secret").is_none());
        assert!(store.find_by_id("user-1").is_none());
    }

    #[test]
    fn a_refresh_token_validates_until_it_is_revoked() {
        let store = InMemoryRefreshTokenStore::new();
        let expires = Utc::now() + Duration::days(7);

        store.store("user-1", "token-a", expires);

        assert!(store.validate("user-1", "token-a"));
        assert!(!store.validate("user-1", "token-b"), "a different token");
        assert!(!store.validate("user-2", "token-a"), "a different user");

        store.revoke("user-1", "token-a");
        assert!(!store.validate("user-1", "token-a"));
    }

    #[test]
    fn an_expired_token_does_not_validate() {
        let store = InMemoryRefreshTokenStore::new();

        store.store("user-1", "stale", Utc::now() - Duration::seconds(1));

        assert!(
            !store.validate("user-1", "stale"),
            "expiry is checked on read, whether or not a sweep has run"
        );
        assert!(!store.take("user-1", "stale"));
    }

    #[test]
    fn a_token_can_be_spent_exactly_once() {
        let store = InMemoryRefreshTokenStore::new();
        store.store("user-1", "token-a", Utc::now() + Duration::days(7));

        assert!(store.take("user-1", "token-a"));
        assert!(
            !store.take("user-1", "token-a"),
            "the second attempt has nothing left to spend"
        );
        assert!(store.is_empty());
    }

    #[test]
    fn only_one_of_many_racing_callers_can_spend_a_token() {
        // The reason `take` is one operation. With a `validate` then `revoke`
        // pair every one of these threads would pass the check, and a captured
        // refresh token could be replayed as often as the race could be won.
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;

        let store = Arc::new(InMemoryRefreshTokenStore::new());
        store.store("user-1", "token-a", Utc::now() + Duration::days(7));

        let winners = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(16));

        let threads: Vec<_> = (0..16)
            .map(|_| {
                let store = Arc::clone(&store);
                let winners = Arc::clone(&winners);
                let barrier = Arc::clone(&barrier);

                std::thread::spawn(move || {
                    barrier.wait();
                    if store.take("user-1", "token-a") {
                        winners.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();

        for thread in threads {
            thread.join().expect("the thread should finish");
        }

        assert_eq!(
            winners.load(Ordering::SeqCst),
            1,
            "exactly one caller may spend a single-use token"
        );
    }

    #[test]
    fn expired_entries_are_swept_rather_than_accumulating_forever() {
        // The original checks expiry on read and never removes anything, so the
        // map grows for the life of the process — and its contents are
        // credentials.
        let store = InMemoryRefreshTokenStore::new();

        for index in 0..10 {
            store.store(
                "user-1",
                &format!("stale-{index}"),
                Utc::now() - Duration::seconds(1),
            );
        }
        store.store("user-1", "live", Utc::now() + Duration::days(7));
        assert_eq!(store.len(), 11);

        assert_eq!(store.purge_expired(), 10);
        assert_eq!(store.len(), 1);
        assert!(
            store.validate("user-1", "live"),
            "a sweep must not touch a token that is still good"
        );
    }

    #[test]
    fn a_write_sweeps_once_the_interval_has_elapsed() {
        // Zero interval, so every write sweeps; the real store waits a minute.
        let store = InMemoryRefreshTokenStore::with_purge_interval(0);

        store.store("user-1", "stale", Utc::now() - Duration::seconds(1));
        assert_eq!(store.len(), 1, "the entry just written is never swept");

        store.store("user-1", "live", Utc::now() + Duration::days(7));

        assert_eq!(store.len(), 1, "the stale entry went with the next write");
        assert!(store.validate("user-1", "live"));
    }

    #[test]
    fn a_long_lived_token_survives_every_sweep() {
        let store = InMemoryRefreshTokenStore::with_purge_interval(0);
        store.store("user-1", "live", Utc::now() + Duration::days(7));

        for index in 0..5 {
            store.store(
                "user-2",
                &format!("other-{index}"),
                Utc::now() + Duration::days(7),
            );
        }

        assert!(store.validate("user-1", "live"));
        assert_eq!(store.len(), 6);
    }
}
