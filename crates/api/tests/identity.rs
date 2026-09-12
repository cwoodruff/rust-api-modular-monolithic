//! Identity module integration tests.
//!
//! The expected shapes here were captured from the running C# service, not
//! inferred from its source — see the V-items in the translation plan.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use figment::Figment;
use figment::providers::Serialized;
use module_identity::{DevKeyMaterial, IdentityRuntime, KeyMaterial};
use serde_json::{Value, json};
use shared_kernel::{AppConfig, Environment};
use tower::ServiceExt;

fn content_root() -> PathBuf {
    let mut current = Path::new(env!("CARGO_MANIFEST_DIR"));

    loop {
        if current.join("appsettings.json").is_file() {
            return current.to_path_buf();
        }
        current = current
            .parent()
            .expect("appsettings.json should be at the root");
    }
}

/// The two logins these tests use, mirroring the C# suite's fixtures.
fn seeded_users() -> Value {
    json!([
        {
            "username": "admin",
            "password": "secret123",
            "userid": "admin-1",
            "displayname": "Admin User",
            "email": "admin@example.com",
            "tenant": "tenant-1",
            "roles": ["Admin"],
            "permissions": ["music.read", "administration.read", "administration.write"]
        },
        {
            "username": "demo",
            "password": "secret123",
            "userid": "user-1",
            "displayname": "Demo User",
            "tenant": "tenant-1",
            "roles": ["User"],
            "permissions": ["music.read"]
        }
    ])
}

fn config_for(environment: Environment, users: Value) -> AppConfig {
    let figment = Figment::new().merge(Serialized::defaults(json!({
        "servicename": "ModularMonolith.Api",
        "connectionstrings": { "appdatabase": "Data Source=./data/chinook.db" },
        "identity": { "inmemoryusers": users }
    })));

    AppConfig::from_figment(figment, environment)
}

async fn app_with(environment: Environment, users: Value) -> Router {
    let root = content_root();
    let config = config_for(environment, users);

    let keys: Arc<dyn KeyMaterial> =
        Arc::new(DevKeyMaterial::generate().expect("a key should generate"));
    let identity = Arc::new(IdentityRuntime::with_keys(&config, keys));

    let state = api::build_state(config, &root)
        .await
        .expect("the application state should build");

    api::build(state, identity)
}

async fn app() -> Router {
    app_with(Environment::Development, seeded_users()).await
}

struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl TestResponse {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected JSON, got {:?}: {error}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
    }
}

async fn send(app: &Router, request: Request<Body>) -> TestResponse {
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("the router should answer");
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body should be readable");

    TestResponse {
        status,
        headers,
        body,
    }
}

async fn post(app: &Router, uri: &str, body: Value) -> TestResponse {
    let request = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    send(app, request).await
}

async fn get_with_auth(app: &Router, uri: &str, authorization: Option<&str>) -> TestResponse {
    let mut builder = Request::builder().uri(uri);
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }

    send(app, builder.body(Body::empty()).unwrap()).await
}

async fn login(app: &Router, username: &str, password: &str) -> Value {
    let response = post(
        app,
        "/api/identity/login",
        json!({ "username": username, "password": password }),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK, "login should succeed");
    response.json()
}

/// Decodes a JWT payload without validating it.
fn payload_of(token: &str) -> Value {
    let payload = token.split('.').nth(1).expect("a JWT has three parts");
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .expect("the payload should decode");

    serde_json::from_slice(&bytes).expect("the payload should be JSON")
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_valid_login_returns_the_token_envelope() {
    let app = app().await;

    let body = login(&app, "admin", "secret123").await;

    assert_eq!(body["token_type"], "Bearer");
    assert!(
        body["access_token"]
            .as_str()
            .is_some_and(|t| t.split('.').count() == 3)
    );
    // 64 random bytes, base64 with padding.
    assert_eq!(body["refresh_token"].as_str().map(str::len), Some(88));

    let expires = body["expires_at_utc"].as_str().expect("an expiry");
    assert!(
        expires.ends_with("+00:00"),
        "a DateTimeOffset renders an offset, not a Z: {expires}"
    );
}

#[tokio::test]
async fn usernames_match_case_insensitively() {
    let app = app().await;

    for attempt in ["admin", "Admin", "ADMIN"] {
        let response = post(
            &app,
            "/api/identity/login",
            json!({ "username": attempt, "password": "secret123" }),
        )
        .await;

        assert_eq!(response.status, StatusCode::OK, "{attempt}");
    }
}

#[tokio::test]
async fn a_wrong_password_is_refused() {
    let app = app().await;

    let response = post(
        &app,
        "/api/identity/login",
        json!({ "username": "admin", "password": "wrong" }),
    )
    .await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(response.json()["title"], "Unauthorized");
}

#[tokio::test]
async fn an_unknown_user_is_refused_the_same_way_as_a_wrong_password() {
    // The two answers are indistinguishable, which is the point.
    let app = app().await;

    let unknown = post(
        &app,
        "/api/identity/login",
        json!({ "username": "nobody", "password": "secret123" }),
    )
    .await;
    let wrong = post(
        &app,
        "/api/identity/login",
        json!({ "username": "admin", "password": "wrong" }),
    )
    .await;

    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.json()["status"], wrong.json()["status"]);
    assert_eq!(unknown.json()["title"], wrong.json()["title"]);
}

#[tokio::test]
async fn blank_credentials_are_a_bad_request_not_an_unauthorized() {
    let app = app().await;

    for body in [
        json!({ "username": "", "password": "secret123" }),
        json!({ "username": "admin", "password": "" }),
        json!({ "username": "   ", "password": "   " }),
    ] {
        let response = post(&app, "/api/identity/login", body.clone()).await;

        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{body}");

        let document = response.json();
        assert_eq!(document["title"], "Invalid request");
        assert_eq!(document["detail"], "Username and password are required.");
        assert_eq!(
            document["type"], "https://tools.ietf.org/html/rfc9110#section-15.5.1",
            "Results.Problem without an explicit type takes the framework's \
             default, which is the tools.ietf.org vocabulary rather than the \
             www.rfc-editor.org one the custom exception handler passes"
        );
    }
}

#[tokio::test]
async fn a_rejected_login_carries_no_challenge_header() {
    // The challenge comes from the authentication middleware, so a 401 the
    // endpoint decided on itself does not carry one. Verified against the
    // running service.
    let app = app().await;

    let response = post(
        &app,
        "/api/identity/login",
        json!({ "username": "admin", "password": "wrong" }),
    )
    .await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert!(
        response.header("www-authenticate").is_none(),
        "only a protected endpoint's challenge sends this"
    );
}

// ---------------------------------------------------------------------------
// Token contents
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_token_carries_the_claim_names_the_original_emits() {
    // Captured from the running service. Reading the C# source suggests the
    // short names; the outbound claim-type map is not cleared, so they are the
    // full XML Schema URIs instead.
    let app = app().await;
    let body = login(&app, "admin", "secret123").await;
    let claims = payload_of(body["access_token"].as_str().unwrap());

    assert_eq!(claims["sub"], "admin-1");
    assert_eq!(claims["iss"], "https://auth.local");
    assert_eq!(
        claims["http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name"],
        "Admin User"
    );
    assert_eq!(
        claims["http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress"],
        "admin@example.com"
    );
    assert_eq!(
        claims["http://schemas.microsoft.com/ws/2008/06/identity/claims/role"], "Admin",
        "a single role collapses to a string"
    );
    assert_eq!(claims["tenant"], "tenant-1");
    assert_eq!(
        claims["permissions"],
        json!(["music.read", "administration.read", "administration.write"])
    );
    assert!(claims["jti"].as_str().is_some_and(|jti| jti.len() == 32));
}

#[tokio::test]
async fn the_audience_is_emitted_twice_as_an_array() {
    // `JwtSecurityToken`'s constructor adds the audience and the claim list
    // already carries one, so the value is duplicated.
    let app = app().await;
    let body = login(&app, "admin", "secret123").await;

    assert_eq!(
        payload_of(body["access_token"].as_str().unwrap())["aud"],
        json!(["modular-api", "modular-api"])
    );
}

#[tokio::test]
async fn the_token_header_names_the_algorithm_and_key() {
    let app = app().await;
    let body = login(&app, "admin", "secret123").await;
    let token = body["access_token"].as_str().unwrap();

    let header = token.split('.').next().unwrap();
    let decoded: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).unwrap()).unwrap();

    assert_eq!(decoded["alg"], "RS256");
    assert_eq!(decoded["typ"], "JWT");
    assert!(decoded["kid"].as_str().is_some());
}

// ---------------------------------------------------------------------------
// userinfo
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userinfo_reports_the_callers_claims() {
    let app = app().await;
    let body = login(&app, "admin", "secret123").await;
    let bearer = format!("Bearer {}", body["access_token"].as_str().unwrap());

    let response = get_with_auth(&app, "/api/identity/userinfo", Some(&bearer)).await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.json(),
        json!({
            "sub": "admin-1",
            "name": "Admin User",
            "email": "admin@example.com",
            "roles": ["Admin"],
            "permissions": ["music.read", "administration.read", "administration.write"]
        })
    );
}

#[tokio::test]
async fn userinfo_without_a_token_challenges() {
    let app = app().await;

    let response = get_with_auth(&app, "/api/identity/userinfo", None).await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.header("www-authenticate").as_deref(),
        Some("Bearer"),
        "the JWT challenge writes this header"
    );
    assert_eq!(
        response.json()["type"],
        "https://tools.ietf.org/html/rfc9110#section-15.5.2"
    );
}

#[tokio::test]
async fn an_unusable_token_is_treated_as_no_token() {
    let app = app().await;

    for value in [
        "Bearer not-a-token",
        "Bearer a.b.c",
        "Basic dXNlcjpwYXNz",
        "Bearer ",
    ] {
        let response = get_with_auth(&app, "/api/identity/userinfo", Some(value)).await;

        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{value}");
    }
}

#[tokio::test]
async fn a_doubled_bearer_prefix_is_tolerated() {
    // The original strips it explicitly in OnMessageReceived.
    let app = app().await;
    let body = login(&app, "admin", "secret123").await;
    let doubled = format!("Bearer Bearer {}", body["access_token"].as_str().unwrap());

    let response = get_with_auth(&app, "/api/identity/userinfo", Some(&doubled)).await;

    assert_eq!(response.status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// JWKS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn jwks_is_served_from_inside_the_module_group() {
    let app = app().await;

    let response = get_with_auth(&app, "/api/identity/.well-known/jwks.json", None).await;

    assert_eq!(
        response.status,
        StatusCode::OK,
        "anonymous, as in the original"
    );

    let entry = &response.json()["keys"][0];
    assert_eq!(entry["kty"], "RSA");
    assert_eq!(entry["use"], "sig");
    assert_eq!(entry["alg"], "RS256");
    assert_eq!(entry["e"], "AQAB");
    assert!(entry["kid"].as_str().is_some());
    assert!(entry["n"].as_str().is_some_and(|n| n.len() > 300));
}

#[tokio::test]
async fn jwks_is_not_at_the_conventional_root_path() {
    // A quirk worth pinning: the original mounts this inside the group, so the
    // well-known path everyone expects is not there.
    let app = app().await;

    let response = get_with_auth(&app, "/.well-known/jwks.json", None).await;

    assert_eq!(response.status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Refresh
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refresh_token_exchanges_for_a_new_pair() {
    let app = app().await;
    let first = login(&app, "admin", "secret123").await;

    let response = post(
        &app,
        "/api/identity/refresh",
        json!({
            "userId": "admin-1",
            "refreshToken": first["refresh_token"].as_str().unwrap()
        }),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);

    let second = response.json();
    assert_eq!(second["token_type"], "Bearer");
    assert_ne!(second["refresh_token"], first["refresh_token"], "rotation");
}

#[tokio::test]
async fn a_spent_refresh_token_cannot_be_used_again() {
    // Rotation is the whole point: the old token is revoked on use.
    let app = app().await;
    let first = login(&app, "admin", "secret123").await;
    let spent = first["refresh_token"].as_str().unwrap().to_owned();

    let body = json!({ "userId": "admin-1", "refreshToken": spent });

    assert_eq!(
        post(&app, "/api/identity/refresh", body.clone())
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        post(&app, "/api/identity/refresh", body).await.status,
        StatusCode::UNAUTHORIZED,
        "the spent token should be revoked"
    );
}

#[tokio::test]
async fn an_unknown_refresh_token_is_refused() {
    let app = app().await;

    let response = post(
        &app,
        "/api/identity/refresh",
        json!({ "userId": "admin-1", "refreshToken": "made-up" }),
    )
    .await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_refresh_token_belonging_to_another_user_is_refused() {
    let app = app().await;
    let admin = login(&app, "admin", "secret123").await;

    let response = post(
        &app,
        "/api/identity/refresh",
        json!({
            "userId": "user-1",
            "refreshToken": admin["refresh_token"].as_str().unwrap()
        }),
    )
    .await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn blank_refresh_fields_are_a_bad_request() {
    let app = app().await;

    for body in [
        json!({ "userId": "", "refreshToken": "x" }),
        json!({ "userId": "admin-1", "refreshToken": "" }),
    ] {
        let response = post(&app, "/api/identity/refresh", body).await;

        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            response.json()["detail"],
            "UserId and refreshToken are required."
        );
    }
}

#[tokio::test]
async fn a_refreshed_token_carries_the_users_current_permissions() {
    // The original re-reads the user on refresh so a revoked permission cannot
    // ride along into the new token.
    let app = app().await;
    let first = login(&app, "demo", "secret123").await;

    let refreshed = post(
        &app,
        "/api/identity/refresh",
        json!({
            "userId": "user-1",
            "refreshToken": first["refresh_token"].as_str().unwrap()
        }),
    )
    .await
    .json();

    let claims = payload_of(refreshed["access_token"].as_str().unwrap());

    assert_eq!(claims["sub"], "user-1");
    assert_eq!(claims["permissions"], "music.read");
    assert_eq!(claims["tenant"], "tenant-1");
}

// ---------------------------------------------------------------------------
// Logout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_caller_can_revoke_their_own_session() {
    let app = app().await;
    let body = login(&app, "admin", "secret123").await;
    let bearer = format!("Bearer {}", body["access_token"].as_str().unwrap());
    let refresh = body["refresh_token"].as_str().unwrap().to_owned();

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/identity/logout")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "userId": "admin-1", "refreshToken": refresh }).to_string(),
        ))
        .unwrap();

    assert_eq!(send(&app, request).await.status, StatusCode::NO_CONTENT);

    // The refresh token is gone.
    let after = post(
        &app,
        "/api/identity/refresh",
        json!({ "userId": "admin-1", "refreshToken": refresh }),
    )
    .await;
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_caller_cannot_revoke_someone_elses_session() {
    let app = app().await;
    let body = login(&app, "demo", "secret123").await;
    let bearer = format!("Bearer {}", body["access_token"].as_str().unwrap());

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/identity/logout")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "userId": "admin-1", "refreshToken": "whatever" }).to_string(),
        ))
        .unwrap();

    assert_eq!(
        send(&app, request).await.status,
        StatusCode::FORBIDDEN,
        "ownership is checked against the token's subject"
    );
}

#[tokio::test]
async fn logout_requires_authentication() {
    let app = app().await;

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/identity/logout")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "userId": "admin-1", "refreshToken": "x" }).to_string(),
        ))
        .unwrap();

    assert_eq!(send(&app, request).await.status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Environment gating
// ---------------------------------------------------------------------------

#[tokio::test]
async fn production_refuses_every_login_even_with_credentials_configured() {
    // The disabled store takes over outside Development and Demo, so a
    // production host cannot be logged into with configured credentials.
    let app = app_with(Environment::Production, seeded_users()).await;

    let response = post(
        &app,
        "/api/identity/login",
        json!({ "username": "admin", "password": "secret123" }),
    )
    .await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn demo_allows_logins_just_as_development_does() {
    let app = app_with(Environment::Demo, seeded_users()).await;

    let response = post(
        &app,
        "/api/identity/login",
        json!({ "username": "admin", "password": "secret123" }),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
}

#[tokio::test]
async fn a_host_with_no_configured_logins_authenticates_nobody() {
    let app = app_with(Environment::Development, json!([])).await;

    let response = post(
        &app,
        "/api/identity/login",
        json!({ "username": "admin", "password": "secret123" }),
    )
    .await;

    assert_eq!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "the original ships an empty user list and expects secrets to fill it"
    );
}

#[tokio::test]
async fn the_identity_health_endpoints_stay_anonymous() {
    let app = app().await;

    for uri in ["/api/identity/health", "/api/identity/data-health"] {
        assert_eq!(
            get_with_auth(&app, uri, None).await.status,
            StatusCode::OK,
            "{uri}"
        );
    }
}

// ---------------------------------------------------------------------------
// The authorization pipeline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_token_from_one_host_is_not_accepted_by_another() {
    // Each host generates its own key, so this also proves the signature is
    // actually checked rather than the payload merely decoded.
    let issuer = app().await;
    let other = app().await;

    let body = login(&issuer, "admin", "secret123").await;
    let bearer = format!("Bearer {}", body["access_token"].as_str().unwrap());

    assert_eq!(
        get_with_auth(&issuer, "/api/identity/userinfo", Some(&bearer))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        get_with_auth(&other, "/api/identity/userinfo", Some(&bearer))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_tenant_header_does_not_affect_an_endpoint_that_is_not_tenant_scoped() {
    // userinfo requires only authentication, so a mismatched tenant header is
    // irrelevant there. The tenant guard is exercised by the module endpoints
    // in Phases 6 and 7.
    let app = app().await;
    let body = login(&app, "admin", "secret123").await;
    let bearer = format!("Bearer {}", body["access_token"].as_str().unwrap());

    let request = Request::builder()
        .uri("/api/identity/userinfo")
        .header(header::AUTHORIZATION, &bearer)
        .header("X-Tenant-Id", "tenant-999")
        .body(Body::empty())
        .unwrap();

    assert_eq!(send(&app, request).await.status, StatusCode::OK);
}
