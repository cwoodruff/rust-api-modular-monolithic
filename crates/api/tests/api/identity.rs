//! Identity through the whole host. Port of upstream's
//! `IdentityEndpointsTests`, carrying over what the earlier suite verified
//! against the running service — the claim names, the duplicated audience,
//! the `DateTimeOffset` expiry — and upstream's change since: body validation
//! answers the shared validation problem (18f7f86).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::common::http::{TestRequest, TestResponse};
use crate::common::{PASSWORD, TestHost};
use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use shared_kernel::Environment;

fn users() -> Value {
    json!([
        { "Username": "admin", "Password": PASSWORD, "UserId": "admin-1", "DisplayName": "Admin User",
          "Email": "admin@example.com", "Tenant": "tenant-1", "Roles": ["Admin"],
          "Permissions": ["catalog.read", "administration.read", "administration.write"] },
        { "Username": "demo", "Password": PASSWORD, "UserId": "user-1", "DisplayName": "Demo User",
          "Tenant": "tenant-1", "Roles": ["User"], "Permissions": ["catalog.read"] }
    ])
}

fn overrides() -> Value {
    json!({ "Identity": { "InMemoryUsers": users() } })
}

async fn host() -> TestHost {
    crate::common::host_with(Environment::Development, overrides()).await
}

async fn post(host: &TestHost, uri: &str, body: Value) -> TestResponse {
    host.send(TestRequest::post(uri).json(&body)).await
}

async fn login(host: &TestHost, username: &str) -> Value {
    let response = post(
        host,
        "/api/identity/login",
        json!({ "username": username, "password": PASSWORD }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    response.json()
}

fn payload_of(token: &str) -> Value {
    let payload = token.split('.').nth(1).unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap()
}

fn validation_problem(response: &TestResponse, field: &str) {
    assert_eq!(
        response.status,
        StatusCode::BAD_REQUEST,
        "{}",
        response.text()
    );
    let document = response.json();
    assert_eq!(
        document["type"],
        "https://www.rfc-editor.org/rfc/rfc9110#section-15.5.1"
    );
    assert_eq!(document["title"], "Request validation failed.");
    assert_eq!(
        document["detail"],
        "One or more validation errors occurred."
    );
    assert_eq!(
        document["errors"][field],
        json!([format!("'{field}' is required.")])
    );
    assert!(
        document["traceId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
}

#[tokio::test]
async fn login_issues_the_token_envelope_and_refuses_bad_credentials() {
    let host = host().await;

    let body = login(&host, "admin").await;
    assert_eq!(body["token_type"], "Bearer");
    assert_eq!(body["access_token"].as_str().unwrap().split('.').count(), 3);
    assert_eq!(body["refresh_token"].as_str().map(str::len), Some(88));
    assert!(body["expires_at_utc"].as_str().unwrap().ends_with("+00:00"));

    for attempt in ["Admin", "ADMIN"] {
        let response = post(
            &host,
            "/api/identity/login",
            json!({ "username": attempt, "password": PASSWORD }),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK, "{attempt}");
    }

    let wrong = post(
        &host,
        "/api/identity/login",
        json!({ "username": "admin", "password": "wrong" }),
    )
    .await;
    let unknown = post(
        &host,
        "/api/identity/login",
        json!({ "username": "nobody", "password": PASSWORD }),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.json()["title"], unknown.json()["title"]);
    assert!(
        wrong.header("www-authenticate").is_none(),
        "a 401 the endpoint decides carries no challenge"
    );
}

#[tokio::test]
async fn blank_bodies_answer_the_shared_validation_problem() {
    let host = host().await;

    validation_problem(
        &post(
            &host,
            "/api/identity/login",
            json!({ "username": "admin", "password": "" }),
        )
        .await,
        "password",
    );
    validation_problem(
        &post(
            &host,
            "/api/identity/login",
            json!({ "username": "   ", "password": "x" }),
        )
        .await,
        "username",
    );
    validation_problem(
        &post(&host, "/api/identity/login", json!({})).await,
        "username",
    );
    validation_problem(
        &post(
            &host,
            "/api/identity/refresh",
            json!({ "userId": "", "refreshToken": "x" }),
        )
        .await,
        "userId",
    );
    validation_problem(
        &post(
            &host,
            "/api/identity/refresh",
            json!({ "userId": "admin-1" }),
        )
        .await,
        "refreshToken",
    );

    let token = login(&host, "admin").await["access_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let logout = host
        .send(
            TestRequest::post("/api/identity/logout")
                .bearer(&token)
                .json(&json!({ "userId": "admin-1", "refreshToken": "" })),
        )
        .await;
    validation_problem(&logout, "refreshToken");

    let anonymous = post(
        &host,
        "/api/identity/logout",
        json!({ "userId": "", "refreshToken": "" }),
    )
    .await;
    assert_eq!(
        anonymous.status,
        StatusCode::UNAUTHORIZED,
        "authorization runs before the body is validated"
    );
}

#[tokio::test]
async fn the_token_carries_the_claims_the_original_emits() {
    let host = host().await;
    let body = login(&host, "admin").await;
    let token = body["access_token"].as_str().unwrap();
    let claims = payload_of(token);

    assert_eq!(claims["sub"], "admin-1");
    assert_eq!(claims["iss"], "https://auth.local");
    assert_eq!(claims["aud"], json!(["modular-api", "modular-api"]));
    assert_eq!(
        claims["http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name"],
        "Admin User"
    );
    assert_eq!(
        claims["http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress"],
        "admin@example.com"
    );
    assert_eq!(
        claims["http://schemas.microsoft.com/ws/2008/06/identity/claims/role"],
        "Admin"
    );
    assert_eq!(claims["tenant"], "tenant-1");
    assert_eq!(
        claims["permissions"],
        json!([
            "catalog.read",
            "administration.read",
            "administration.write"
        ])
    );
    assert_eq!(claims["jti"].as_str().map(str::len), Some(32));

    let header: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(token.split('.').next().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(header["alg"], "RS256");
    assert!(header["kid"].is_string());
}

#[tokio::test]
async fn userinfo_reports_the_callers_claims_and_challenges_without_a_token() {
    let host = host().await;
    let token = login(&host, "admin").await["access_token"]
        .as_str()
        .unwrap()
        .to_owned();

    let info = host
        .send(TestRequest::get("/api/identity/userinfo").bearer(&token))
        .await;
    assert_eq!(info.status, StatusCode::OK);
    assert_eq!(
        info.json(),
        json!({ "sub": "admin-1", "name": "Admin User", "email": "admin@example.com", "roles": ["Admin"],
                "permissions": ["catalog.read", "administration.read", "administration.write"] })
    );

    let mismatched_tenant = host
        .send(
            TestRequest::get("/api/identity/userinfo")
                .bearer(&token)
                .header("X-Tenant-Id", "tenant-999"),
        )
        .await;
    assert_eq!(
        mismatched_tenant.status,
        StatusCode::OK,
        "userinfo is not tenant scoped"
    );

    let doubled = host
        .send(
            TestRequest::get("/api/identity/userinfo")
                .header("Authorization", &format!("Bearer Bearer {token}")),
        )
        .await;
    assert_eq!(
        doubled.status,
        StatusCode::OK,
        "a doubled Bearer prefix is tolerated"
    );

    let anonymous = host.send(TestRequest::get("/api/identity/userinfo")).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        anonymous.header("www-authenticate").as_deref(),
        Some("Bearer")
    );
    assert_eq!(
        anonymous.json()["type"],
        "https://tools.ietf.org/html/rfc9110#section-15.5.2"
    );

    for value in [
        "Bearer not-a-token",
        "Bearer a.b.c",
        "Basic dXNlcjpwYXNz",
        "Bearer expired.token.here",
    ] {
        let response = host
            .send(TestRequest::get("/api/identity/userinfo").header("Authorization", value))
            .await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{value}");
    }
}

#[tokio::test]
async fn jwks_is_served_anonymously_from_inside_the_group() {
    let host = host().await;

    let first = host
        .send(TestRequest::get("/api/identity/.well-known/jwks.json"))
        .await;
    assert_eq!(first.status, StatusCode::OK);
    let entry = &first.json()["keys"][0];
    assert_eq!(entry["kty"], "RSA");
    assert_eq!(entry["use"], "sig");
    assert_eq!(entry["alg"], "RS256");
    assert_eq!(entry["e"], "AQAB");
    assert!(entry["n"].as_str().is_some_and(|n| n.len() > 300));

    let second = host
        .send(TestRequest::get("/api/identity/.well-known/jwks.json"))
        .await;
    assert_eq!(first.json(), second.json(), "consistent between calls");

    assert_eq!(
        host.send(TestRequest::get("/.well-known/jwks.json"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn refresh_rotates_and_refuses_everything_else() {
    let host = host().await;
    let first = login(&host, "demo").await;
    let spent = first["refresh_token"].as_str().unwrap().to_owned();
    let body = json!({ "userId": "user-1", "refreshToken": spent });

    let refreshed = post(&host, "/api/identity/refresh", body.clone()).await;
    assert_eq!(refreshed.status, StatusCode::OK);
    assert_ne!(refreshed.json()["refresh_token"], first["refresh_token"]);
    let claims = payload_of(refreshed.json()["access_token"].as_str().unwrap());
    assert_eq!(claims["sub"], "user-1");
    assert_eq!(claims["permissions"], "catalog.read");

    assert_eq!(
        post(&host, "/api/identity/refresh", body).await.status,
        StatusCode::UNAUTHORIZED,
        "spent"
    );
    assert_eq!(
        post(
            &host,
            "/api/identity/refresh",
            json!({ "userId": "user-1", "refreshToken": "made-up" })
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );

    let admin = login(&host, "admin").await;
    assert_eq!(
        post(
            &host,
            "/api/identity/refresh",
            json!({ "userId": "user-1", "refreshToken": admin["refresh_token"] })
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED,
        "another user's token"
    );
}

#[tokio::test]
async fn logout_revokes_only_the_callers_own_session() {
    let host = host().await;
    let admin = login(&host, "admin").await;
    let token = admin["access_token"].as_str().unwrap().to_owned();
    let refresh = admin["refresh_token"].as_str().unwrap().to_owned();

    let demo = login(&host, "demo").await["access_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let foreign = host
        .send(
            TestRequest::post("/api/identity/logout")
                .bearer(&demo)
                .json(&json!({ "userId": "admin-1", "refreshToken": "x" })),
        )
        .await;
    assert_eq!(foreign.status, StatusCode::FORBIDDEN);

    let own = host
        .send(
            TestRequest::post("/api/identity/logout")
                .bearer(&token)
                .json(&json!({ "userId": "admin-1", "refreshToken": refresh })),
        )
        .await;
    assert_eq!(own.status, StatusCode::NO_CONTENT);

    let after = post(
        &host,
        "/api/identity/refresh",
        json!({ "userId": "admin-1", "refreshToken": refresh }),
    )
    .await;
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);

    let anonymous = post(
        &host,
        "/api/identity/logout",
        json!({ "userId": "admin-1", "refreshToken": "x" }),
    )
    .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn in_memory_logins_work_only_in_development_and_demo() {
    let production = crate::common::host_with(Environment::Production, overrides()).await;
    let refused = post(
        &production,
        "/api/identity/login",
        json!({ "username": "admin", "password": PASSWORD }),
    )
    .await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);

    let demo = crate::common::host_with(Environment::Demo, overrides()).await;
    assert_eq!(
        post(
            &demo,
            "/api/identity/login",
            json!({ "username": "admin", "password": PASSWORD })
        )
        .await
        .status,
        StatusCode::OK
    );

    let empty = crate::common::host_with(
        Environment::Development,
        json!({ "Identity": { "InMemoryUsers": [] } }),
    )
    .await;
    assert_eq!(
        post(
            &empty,
            "/api/identity/login",
            json!({ "username": "admin", "password": PASSWORD })
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_token_from_one_host_is_not_accepted_by_another() {
    let issuer = host().await;
    let other = crate::common::host_with_own_keys(Environment::Development, overrides()).await;
    let token = login(&issuer, "admin").await["access_token"]
        .as_str()
        .unwrap()
        .to_owned();

    assert_eq!(
        issuer
            .send(TestRequest::get("/api/identity/userinfo").bearer(&token))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        other
            .send(TestRequest::get("/api/identity/userinfo").bearer(&token))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_access_token_opens_protected_endpoints_of_other_modules() {
    let host = host().await;
    let token = login(&host, "demo").await["access_token"]
        .as_str()
        .unwrap()
        .to_owned();

    assert_eq!(
        host.send(TestRequest::get("/api/catalog/albums/1").bearer(&token))
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_identity_health_endpoints_stay_anonymous() {
    let host = host().await;

    for uri in ["/api/identity/health", "/api/identity/data-health"] {
        let response = host.send(TestRequest::get(uri)).await;
        assert_eq!(response.status, StatusCode::OK, "{uri}");
        assert_eq!(response.json()["module"], "Identity");
    }
    assert_eq!(
        host.send(TestRequest::get("/api/identity/data-health"))
            .await
            .json()["status"],
        "Data-Healthy",
        "Identity has no tables and asks whether AppDatabase answers"
    );
}
