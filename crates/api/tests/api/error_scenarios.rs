//! Port of upstream's `ErrorScenarioTests`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::common::http::TestRequest;
use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn malformed_bodies_are_malformed_requests() {
    let host = crate::common::host().await;
    let token = host.token("admin").await;

    for body in [r#"{"Name": "#, r#"{"Name": "Rock""#, "", "not json"] {
        let response = host
            .send(
                TestRequest::post("/api/admin/genres")
                    .bearer(&token)
                    .raw("application/json", body),
            )
            .await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{body:?}");
        assert_eq!(
            response.header("content-type").as_deref(),
            Some("application/problem+json")
        );
        let document = response.json();
        assert_eq!(document["title"], "Malformed request.", "{body:?}");
        assert_eq!(document["status"], 400);
        assert!(document["traceId"].is_string());
    }

    let login = host
        .send(TestRequest::post("/api/identity/login").raw("application/json", "username=admin"))
        .await;
    assert_eq!(login.status, StatusCode::BAD_REQUEST);
    assert_eq!(login.json()["title"], "Malformed request.");
}

#[tokio::test]
async fn the_wrong_content_type_and_the_wrong_method_are_refused() {
    let host = crate::common::host().await;
    let token = host.token("admin").await;

    let text = host
        .send(
            TestRequest::post("/api/admin/genres")
                .bearer(&token)
                .raw("text/plain", r#"{"Name":"x"}"#),
        )
        .await;
    assert!(matches!(text.status.as_u16(), 400 | 415), "{}", text.status);

    let patch = host
        .send(TestRequest::new(Method::PATCH, "/api/admin/genres/1").bearer(&token))
        .await;
    assert!(
        matches!(patch.status.as_u16(), 404 | 405),
        "{}",
        patch.status
    );
}

#[tokio::test]
async fn bad_credentials_and_bad_tokens_are_401() {
    let host = crate::common::host().await;

    for (username, password) in [("nobody", crate::common::PASSWORD), ("admin", "wrong")] {
        let response = host
            .send(
                TestRequest::post("/api/identity/login")
                    .json(&json!({ "username": username, "password": password })),
            )
            .await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    }

    for token in ["expired.token.here", "not-a-valid-jwt"] {
        let response = host
            .send(TestRequest::get("/api/catalog/albums/1").bearer(token))
            .await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{token}");
    }
}
