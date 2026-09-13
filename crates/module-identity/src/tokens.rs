//! Token issuance and validation, ported from `TokenService` and the JWT
//! bearer configuration in `IdentityAuthExtensions`.

use std::collections::HashSet;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, Header, Validation};
use rand::RngCore;
use shared_kernel::{AuthenticatedUser, REDACTED};

use crate::claims::{AccessTokenClaims, ClaimValues};
use crate::keys::KeyMaterial;
use crate::options::{CLOCK_SKEW_SECONDS, JwtAuthOptions};
use crate::stores::{InMemoryRefreshTokenStore, UserRecord, UserStore};

/// How many random bytes back a refresh token.
pub const REFRESH_TOKEN_BYTES: usize = 64;

/// An issued access and refresh token pair.
///
/// Both members are bearer credentials, so `Debug` reports only when the access
/// token expires.
#[derive(Clone)]
pub struct TokenPair {
    /// The signed JWT.
    pub access_token: String,
    /// The opaque refresh token.
    pub refresh_token: String,
    /// When the access token expires.
    pub expires_at_utc: DateTime<Utc>,
}

impl std::fmt::Debug for TokenPair {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TokenPair")
            .field("access_token", &REDACTED)
            .field("refresh_token", &REDACTED)
            .field("expires_at_utc", &self.expires_at_utc)
            .finish()
    }
}

/// Issues and refreshes tokens.
pub struct TokenService {
    options: JwtAuthOptions,
    keys: Arc<dyn KeyMaterial>,
    refresh_tokens: Arc<InMemoryRefreshTokenStore>,
    users: Arc<dyn UserStore>,
}

impl std::fmt::Debug for TokenService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TokenService")
            .field("issuer", &self.options.issuer)
            .field("audience", &self.options.audience)
            .finish_non_exhaustive()
    }
}

impl TokenService {
    /// Assembles the service.
    #[must_use]
    pub fn new(
        options: JwtAuthOptions,
        keys: Arc<dyn KeyMaterial>,
        refresh_tokens: Arc<InMemoryRefreshTokenStore>,
        users: Arc<dyn UserStore>,
    ) -> Self {
        Self {
            options,
            keys,
            refresh_tokens,
            users,
        }
    }

    /// The configured options.
    #[must_use]
    pub fn options(&self) -> &JwtAuthOptions {
        &self.options
    }

    /// The JWKS document.
    #[must_use]
    pub fn jwks(&self) -> serde_json::Value {
        self.keys.jwks()
    }

    /// Issues a pair for a user.
    ///
    /// # Errors
    ///
    /// Returns the signing error if the token cannot be produced.
    pub fn issue(&self, user: &UserRecord) -> Result<TokenPair, jsonwebtoken::errors::Error> {
        self.issue_at(user, Utc::now())
    }

    /// Issues a pair at an explicit instant, for tests.
    ///
    /// # Errors
    ///
    /// Returns the signing error if the token cannot be produced.
    pub fn issue_at(
        &self,
        user: &UserRecord,
        now: DateTime<Utc>,
    ) -> Result<TokenPair, jsonwebtoken::errors::Error> {
        let expires = now + Duration::minutes(self.options.access_token_minutes);

        let claims = AccessTokenClaims {
            sub: user.user_id.clone(),
            // A GUID with no dashes, as `Guid.NewGuid().ToString("N")` writes it.
            jti: uuid::Uuid::new_v4().simple().to_string(),
            iat: now.timestamp(),
            nbf: now.timestamp(),
            exp: expires.timestamp(),
            iss: self.options.issuer.clone(),
            // Twice, because the original writes it twice. See `claims`.
            aud: vec![self.options.audience.clone(), self.options.audience.clone()],
            name: Some(user.display_name.clone()).filter(|name| !name.trim().is_empty()),
            email: user.email.clone(),
            tenant: user.tenant.clone(),
            roles: ClaimValues::from_values(user.roles.clone()),
            permissions: ClaimValues::from_values(user.permissions.clone()),
        };

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.keys.current_key_id().to_owned());

        let access_token = jsonwebtoken::encode(&header, &claims, self.keys.signing_key())?;

        let refresh_token = new_refresh_token();
        self.refresh_tokens.store(
            &user.user_id,
            &refresh_token,
            now + Duration::days(self.options.refresh_token_days),
        );

        Ok(TokenPair {
            access_token,
            refresh_token,
            expires_at_utc: expires,
        })
    }

    /// Exchanges a refresh token for a new pair.
    ///
    /// Two safeguards from the original are kept:
    ///
    /// - Permissions and tenant are **re-read from the user store**, so a
    ///   revoked permission cannot ride along into the new token.
    /// - The old refresh token is revoked, so it is single-use. A user who has
    ///   since been removed has their token revoked and gets nothing back.
    #[must_use]
    pub fn refresh(&self, user_id: &str, refresh_token: &str) -> Option<TokenPair> {
        if !self.refresh_tokens.validate(user_id, refresh_token) {
            return None;
        }

        let Some(user) = self.users.find_by_id(user_id) else {
            self.refresh_tokens.revoke(user_id, refresh_token);
            return None;
        };

        let pair = self.issue(&user).ok()?;
        self.refresh_tokens.revoke(user_id, refresh_token);

        Some(pair)
    }

    /// Validates a bearer token, yielding the principal it names.
    ///
    /// Mirrors the original's `TokenValidationParameters`: issuer, audience,
    /// lifetime and signature are all checked, with 30 seconds of clock skew,
    /// and a token without a `sub` is rejected outright by `OnTokenValidated`.
    #[must_use]
    pub fn validate(&self, token: &str) -> Option<AuthenticatedUser> {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[self.options.issuer.as_str()]);
        validation.set_audience(&[self.options.audience.as_str()]);
        validation.leeway = CLOCK_SKEW_SECONDS;
        validation.validate_exp = true;
        validation.validate_nbf = true;

        // `sub` is required here rather than checked afterwards, which is what
        // the original's OnTokenValidated hook does.
        validation.required_spec_claims = HashSet::from([
            "exp".to_owned(),
            "iss".to_owned(),
            "aud".to_owned(),
            "sub".to_owned(),
        ]);

        let decoded = jsonwebtoken::decode::<AccessTokenClaims>(
            token,
            self.keys.validation_key(),
            &validation,
        );

        match decoded {
            Ok(data) if !data.claims.sub.trim().is_empty() => Some(data.claims.into_user()),
            Ok(_) => {
                tracing::warn!("rejected a token with no subject");
                None
            }
            Err(error) => {
                tracing::debug!(%error, "token validation failed");
                None
            }
        }
    }

    /// Revokes one refresh token.
    pub fn revoke(&self, user_id: &str, refresh_token: &str) {
        self.refresh_tokens.revoke(user_id, refresh_token);
    }

    /// Authenticates a login.
    #[must_use]
    pub fn validate_credentials(&self, username: &str, password: &str) -> Option<UserRecord> {
        self.users.validate_credentials(username, password)
    }
}

/// Builds an opaque refresh token: 64 random bytes, base64 with padding.
#[must_use]
pub fn new_refresh_token() -> String {
    let mut bytes = [0_u8; REFRESH_TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);

    STANDARD.encode(bytes)
}

/// Strips the `Bearer` prefix from an `Authorization` header.
///
/// The original tolerates a doubled `Bearer Bearer ` prefix, so this does too.
#[must_use]
pub fn strip_bearer(header: &str) -> Option<&str> {
    let mut rest = header.trim();

    for _ in 0..2 {
        if let Some(stripped) = rest
            .strip_prefix("Bearer ")
            .or_else(|| rest.strip_prefix("bearer "))
        {
            rest = stripped.trim_start();
        }
    }

    (!rest.is_empty() && rest != header.trim()).then_some(rest)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::keys::DevKeyMaterial;

    fn service() -> TokenService {
        let keys: Arc<dyn KeyMaterial> =
            Arc::new(DevKeyMaterial::generate().expect("a key should generate"));

        TokenService::new(
            JwtAuthOptions::default(),
            keys,
            Arc::new(InMemoryRefreshTokenStore::new()),
            Arc::new(crate::stores::DisabledUserStore),
        )
    }

    fn user() -> UserRecord {
        UserRecord {
            username: "admin".to_owned(),
            password: "secret".to_owned(),
            user_id: "admin-1".to_owned(),
            display_name: "Admin User".to_owned(),
            roles: vec!["Admin".to_owned()],
            permissions: vec!["music.read".to_owned(), "administration.read".to_owned()],
            email: Some("admin@example.com".to_owned()),
            tenant: Some("tenant-1".to_owned()),
        }
    }

    #[test]
    fn an_issued_token_validates_and_carries_every_claim() {
        let service = service();
        let pair = service.issue(&user()).expect("issuing should succeed");

        let principal = service
            .validate(&pair.access_token)
            .expect("the token it just issued should validate");

        assert_eq!(principal.subject, "admin-1");
        assert_eq!(principal.name.as_deref(), Some("Admin User"));
        assert_eq!(principal.email.as_deref(), Some("admin@example.com"));
        assert_eq!(principal.tenant.as_deref(), Some("tenant-1"));
        assert_eq!(principal.roles, vec!["Admin".to_owned()]);
        assert!(principal.has_permission("administration.read"));
    }

    #[test]
    fn the_token_header_names_the_signing_key() {
        let service = service();
        let pair = service.issue(&user()).unwrap();

        let header = jsonwebtoken::decode_header(&pair.access_token).unwrap();

        assert_eq!(header.alg, Algorithm::RS256);
        assert!(header.kid.is_some());
        // The original emits "JWT", though it accepts "at+jwt" too.
        assert_eq!(header.typ.as_deref(), Some("JWT"));
    }

    #[test]
    fn the_access_token_expires_in_fifteen_minutes() {
        let service = service();
        let now = Utc::now();

        let pair = service.issue_at(&user(), now).unwrap();

        assert_eq!((pair.expires_at_utc - now).num_minutes(), 15);
    }

    #[test]
    fn a_token_from_another_key_is_refused() {
        let issuer = service();
        let other = service();

        let pair = issuer.issue(&user()).unwrap();

        assert!(
            other.validate(&pair.access_token).is_none(),
            "a token signed by a different key must not validate"
        );
    }

    #[test]
    fn an_expired_token_is_refused_once_past_the_skew() {
        let service = service();
        // Far enough back that the 30 second leeway cannot save it.
        let long_ago = Utc::now() - Duration::minutes(60);

        let pair = service.issue_at(&user(), long_ago).unwrap();

        assert!(service.validate(&pair.access_token).is_none());
    }

    #[test]
    fn a_token_inside_the_clock_skew_is_still_accepted() {
        let service = service();
        // Expired 10 seconds ago; the allowance is 30.
        let issued = Utc::now() - Duration::minutes(15) - Duration::seconds(10);

        let pair = service.issue_at(&user(), issued).unwrap();

        assert!(
            service.validate(&pair.access_token).is_some(),
            "30 seconds of clock skew should cover this"
        );
    }

    #[test]
    fn garbage_is_refused_without_panicking() {
        let service = service();

        for candidate in ["", "not-a-token", "a.b.c", "Bearer x"] {
            assert!(service.validate(candidate).is_none(), "{candidate:?}");
        }
    }

    #[test]
    fn refresh_tokens_are_opaque_and_distinct() {
        let first = new_refresh_token();
        let second = new_refresh_token();

        assert_ne!(first, second);
        // 64 bytes, base64 with padding.
        assert_eq!(first.len(), 88);
        assert_eq!(STANDARD.decode(&first).unwrap().len(), 64);
    }

    #[test]
    fn an_issued_pair_does_not_print_either_of_its_credentials() {
        let service = service();
        let pair = service.issue(&user()).unwrap();

        let rendered = format!("{pair:?}");

        assert!(!rendered.contains(&pair.access_token), "{rendered}");
        assert!(!rendered.contains(&pair.refresh_token), "{rendered}");
        assert!(rendered.contains(REDACTED), "{rendered}");
    }

    #[test]
    fn the_bearer_prefix_is_stripped_including_the_doubled_form() {
        assert_eq!(strip_bearer("Bearer abc"), Some("abc"));
        assert_eq!(strip_bearer("bearer abc"), Some("abc"));
        // The original tolerates this, so it stays tolerated.
        assert_eq!(strip_bearer("Bearer Bearer abc"), Some("abc"));
        assert_eq!(
            strip_bearer("abc"),
            None,
            "a bare token is not a bearer header"
        );
        assert_eq!(strip_bearer("Bearer "), None);
    }
}
