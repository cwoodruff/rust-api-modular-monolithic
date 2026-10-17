//! Identity's tokens, validated without Identity. Port of `RemoteIdentity`
//! (ADR-0020).
//!
//! The monolith signs access tokens with RS256 and publishes its public key at
//! `/api/identity/.well-known/jwks.json`. This host fetches that key set over
//! HTTP — a bare JWKS, not an OpenID discovery document, as upstream's
//! `JwksRetriever` reads it — caches it, and validates tokens against it with
//! Identity's own rules: issuer, audience, lifetime with 30 seconds of skew,
//! a `typ` of `at+jwt` or `JWT`, and a non-blank `sub`.
//!
//! The cache refreshes every 24 hours, and on a token naming a key it does not
//! hold — at most once every 5 minutes, the framework defaults upstream relies
//! on. A rotated key is therefore refused until the refresh, and no token can
//! be validated while the JWKS is unreachable: both are recorded in ADR-0020.
//!
//! The claim parsing and the policies below are copies of Identity's, which is
//! the cost ADR-0020 accepts: the rules live in three places.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, header};
use identity_contracts::{ADMIN_ROLE, permissions, policies};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use shared_kernel::auth::{Authenticator, PolicyDecision, PolicyEvaluator, Principal};
use shared_kernel::hosting::BoxFuture;
use shared_kernel::{AppConfig, AuthenticatedUser};
use tokio::sync::Mutex;

/// The issuer Identity signs with unless `Jwt:Issuer` says otherwise.
pub const DEFAULT_ISSUER: &str = "https://auth.local";

/// The audience Identity signs for unless `Jwt:Audience` says otherwise.
pub const DEFAULT_AUDIENCE: &str = "modular-api";

/// Clock skew allowed on `exp` and `nbf`.
pub const CLOCK_SKEW_SECONDS: u64 = 30;

/// How long a fetched key set is trusted before it is fetched again.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The least time between two refreshes caused by an unknown key.
pub const MISS_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// What a missing JWKS URL is reported as, verbatim from upstream.
pub const MISSING_JWKS_URL: &str = "CatalogHost:IdentityJwksUrl is not configured.";

const NAME_CLAIM: &str = "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name";
const EMAIL_CLAIM: &str = "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress";
const ROLE_CLAIM: &str = "http://schemas.microsoft.com/ws/2008/06/identity/claims/role";

/// One value or several: Identity collapses a repeated claim to a string when
/// it has one value.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ClaimValues {
    One(String),
    Many(Vec<String>),
}

fn values(claim: Option<ClaimValues>) -> Vec<String> {
    match claim {
        Some(ClaimValues::One(value)) => vec![value],
        Some(ClaimValues::Many(values)) => values,
        None => Vec::new(),
    }
}

/// The claims this host reads.
#[derive(Debug, Deserialize)]
struct Claims {
    #[serde(default)]
    sub: String,
    #[serde(
        rename = "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name",
        default
    )]
    name: Option<String>,
    #[serde(
        rename = "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress",
        default
    )]
    email: Option<String>,
    #[serde(default)]
    tenant: Option<String>,
    #[serde(
        rename = "http://schemas.microsoft.com/ws/2008/06/identity/claims/role",
        default
    )]
    roles: Option<ClaimValues>,
    #[serde(default)]
    permissions: Option<ClaimValues>,
}

// The claim-name constants are kept for the documentation they give the
// renames above; serde needs literals.
const _: [&str; 3] = [NAME_CLAIM, EMAIL_CLAIM, ROLE_CLAIM];

/// One key from the set.
#[derive(Debug, Deserialize)]
struct Jwk {
    kty: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

#[derive(Default)]
struct KeyCache {
    keys: Vec<(Option<String>, DecodingKey)>,
    fetched_at: Option<Instant>,
    last_miss_refresh: Option<Instant>,
}

/// Validates Identity's tokens against its published keys.
pub struct RemoteIdentity {
    jwks_url: String,
    issuer: String,
    audience: String,
    client: reqwest::Client,
    cache: Mutex<KeyCache>,
}

impl std::fmt::Debug for RemoteIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteIdentity")
            .field("jwks_url", &self.jwks_url)
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .finish_non_exhaustive()
    }
}

impl RemoteIdentity {
    /// Reads `CatalogHost:IdentityJwksUrl`, `Jwt:Issuer` and `Jwt:Audience`.
    ///
    /// # Errors
    ///
    /// [`MISSING_JWKS_URL`] when the URL is absent or blank.
    pub fn from_config(config: &AppConfig) -> anyhow::Result<Self> {
        let jwks_url = config
            .get_string("CatalogHost:IdentityJwksUrl")
            .filter(|url| !url.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!(MISSING_JWKS_URL))?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            // Upstream's `RequireHttps` is "the URL is https": an http URL is
            // fetched as written, an https one must stay https.
            .https_only(jwks_url.starts_with("https"))
            .build()?;

        Ok(Self {
            jwks_url,
            issuer: config
                .get_string("Jwt:Issuer")
                .unwrap_or_else(|| DEFAULT_ISSUER.to_owned()),
            audience: config
                .get_string("Jwt:Audience")
                .unwrap_or_else(|| DEFAULT_AUDIENCE.to_owned()),
            client,
            cache: Mutex::new(KeyCache::default()),
        })
    }

    async fn fetch(&self) -> Option<Vec<(Option<String>, DecodingKey)>> {
        let response = match self.client.get(&self.jwks_url).send().await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(%error, url = %self.jwks_url, "could not fetch Identity's JWKS");
                return None;
            }
        };

        let set: JwkSet = match response.error_for_status() {
            Ok(response) => match response.json().await {
                Ok(set) => set,
                Err(error) => {
                    tracing::warn!(%error, "Identity's JWKS is not a key set");
                    return None;
                }
            },
            Err(error) => {
                tracing::warn!(%error, "Identity's JWKS answered with an error");
                return None;
            }
        };

        Some(
            set.keys
                .into_iter()
                .filter(|key| key.kty == "RSA")
                .filter_map(|key| {
                    let decoding =
                        DecodingKey::from_rsa_components(key.n.as_deref()?, key.e.as_deref()?)
                            .ok()?;
                    Some((key.kid, decoding))
                })
                .collect(),
        )
    }

    /// The keys a token naming `kid` may be signed by, refreshing when due.
    async fn keys_for(&self, kid: Option<&str>) -> Vec<DecodingKey> {
        let mut cache = self.cache.lock().await;
        let now = Instant::now();

        let stale = cache
            .fetched_at
            .is_none_or(|fetched| now.duration_since(fetched) >= REFRESH_INTERVAL);
        let missing = kid.is_some_and(|kid| {
            !cache
                .keys
                .iter()
                .any(|(held, _)| held.as_deref() == Some(kid))
        });
        let miss_refresh_allowed = cache
            .last_miss_refresh
            .is_none_or(|last| now.duration_since(last) >= MISS_REFRESH_INTERVAL);

        if stale || (missing && miss_refresh_allowed) {
            if !stale {
                cache.last_miss_refresh = Some(now);
            }
            if let Some(keys) = self.fetch().await {
                cache.keys = keys;
                cache.fetched_at = Some(now);
            }
        }

        cache
            .keys
            .iter()
            .filter(|(held, _)| kid.is_none() || held.is_none() || held.as_deref() == kid)
            .map(|(_, key)| key.clone())
            .collect()
    }

    /// Validates `token`, yielding the caller it names.
    pub async fn validate(&self, token: &str) -> Option<AuthenticatedUser> {
        let header = jsonwebtoken::decode_header(token).ok()?;

        if header.alg != Algorithm::RS256 {
            return None;
        }
        if let Some(typ) = header.typ.as_deref()
            && !typ.eq_ignore_ascii_case("at+jwt")
            && !typ.eq_ignore_ascii_case("JWT")
        {
            return None;
        }

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);
        validation.leeway = CLOCK_SKEW_SECONDS;
        validation.validate_nbf = true;
        validation.required_spec_claims = HashSet::from([
            "exp".to_owned(),
            "iss".to_owned(),
            "aud".to_owned(),
            "sub".to_owned(),
        ]);

        for key in self.keys_for(header.kid.as_deref()).await {
            match jsonwebtoken::decode::<Claims>(token, &key, &validation) {
                Ok(data) if !data.claims.sub.trim().is_empty() => {
                    let claims = data.claims;
                    return Some(AuthenticatedUser {
                        subject: claims.sub,
                        name: claims.name,
                        email: claims.email,
                        tenant: claims.tenant,
                        roles: values(claims.roles),
                        permissions: values(claims.permissions),
                    });
                }
                Ok(_) => return None,
                Err(error) => tracing::debug!(%error, "token validation failed"),
            }
        }

        None
    }
}

/// Strips `Bearer `, tolerating the doubled prefix Identity's handler accepts.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let mut token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?
        .trim();
    if let Some(rest) = token.strip_prefix("Bearer ") {
        token = rest.trim();
    }
    (!token.is_empty()).then_some(token)
}

impl Authenticator for RemoteIdentity {
    fn authenticate<'a>(
        &'a self,
        headers: &'a HeaderMap,
    ) -> BoxFuture<'a, Option<AuthenticatedUser>> {
        Box::pin(async move { self.validate(bearer(headers)?).await })
    }
}

/// Identity's policies, applied to tokens this host validated: one per
/// permission, `role.admin`, `tenant.scoped`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RemotePolicies;

impl PolicyEvaluator for RemotePolicies {
    fn is_registered(&self, policy: &str) -> bool {
        permissions::ALL.contains(&policy) || policies::ALL.contains(&policy)
    }

    fn evaluate(&self, policy: &str, principal: &Principal) -> PolicyDecision {
        let Some(user) = principal.user.as_ref() else {
            return PolicyDecision::Denied;
        };

        let allowed = if permissions::ALL.contains(&policy) {
            user.has_permission(policy)
        } else if policy == policies::ADMIN {
            user.has_role(ADMIN_ROLE)
        } else if policy == policies::TENANT_SCOPED {
            principal.tenant_scope_satisfied()
        } else {
            return PolicyDecision::Unknown;
        };

        if allowed {
            PolicyDecision::Allowed
        } else {
            PolicyDecision::Denied
        }
    }
}

/// The pair `Services::add_identity` takes.
///
/// # Errors
///
/// As [`RemoteIdentity::from_config`].
pub fn add_remote_identity(
    services: &mut shared_kernel::Services,
    config: &AppConfig,
) -> anyhow::Result<()> {
    let identity: Arc<dyn Authenticator> = Arc::new(RemoteIdentity::from_config(config)?);
    services.add_identity(identity, Arc::new(RemotePolicies));
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_missing_url_is_upstreams_error() {
        let config = AppConfig::from_values(
            serde_json::json!({}),
            shared_kernel::Environment::Development,
        );

        assert_eq!(
            RemoteIdentity::from_config(&config)
                .unwrap_err()
                .to_string(),
            MISSING_JWKS_URL
        );
    }

    #[test]
    fn a_doubled_bearer_prefix_is_tolerated() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer Bearer abc".parse().unwrap());

        assert_eq!(bearer(&headers), Some("abc"));
    }

    #[test]
    fn one_role_or_several_both_read() {
        let one: Claims =
            serde_json::from_value(serde_json::json!({ "sub": "u", ROLE_CLAIM: "Admin" })).unwrap();
        let many: Claims =
            serde_json::from_value(serde_json::json!({ "sub": "u", "permissions": ["a", "b"] }))
                .unwrap();

        assert_eq!(values(one.roles), ["Admin"]);
        assert_eq!(values(many.permissions), ["a", "b"]);
    }
}
