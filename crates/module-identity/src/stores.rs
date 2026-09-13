//! The user and refresh-token stores, ported from `InMemoryStores.cs`.
//!
//! Both are process-local and lost on restart, exactly as in the original. The
//! refresh store never evicts expired entries either — it only checks expiry on
//! read.

use std::collections::HashMap;

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

/// The refresh-token store, ported from `InMemoryRefreshTokenStore`.
///
/// Keyed by `"{userId}:{token}"`, exactly as the original keys it.
///
/// That key is why `Debug` is written out: it *contains* the refresh token, so
/// printing the map would print every live credential in the process.
#[derive(Default)]
pub struct InMemoryRefreshTokenStore {
    tokens: DashMap<String, DateTime<Utc>>,
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
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn key(user_id: &str, token: &str) -> String {
        format!("{user_id}:{token}")
    }

    /// Records a token against a user.
    pub fn store(&self, user_id: &str, token: &str, expires: DateTime<Utc>) {
        self.tokens.insert(Self::key(user_id, token), expires);
    }

    /// Whether the token is known and unexpired.
    #[must_use]
    pub fn validate(&self, user_id: &str, token: &str) -> bool {
        self.tokens
            .get(&Self::key(user_id, token))
            .is_some_and(|expires| *expires > Utc::now())
    }

    /// Forgets a token. Used both on logout and on rotation.
    pub fn revoke(&self, user_id: &str, token: &str) {
        self.tokens.remove(&Self::key(user_id, token));
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

        assert!(!store.validate("user-1", "stale"));
        assert_eq!(
            store.len(),
            1,
            "the original never evicts; expiry is only checked on read"
        );
    }
}
