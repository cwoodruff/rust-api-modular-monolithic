//! Port of upstream's `SecurityHeadersTests`. Upstream fixed its handler to
//! keep these on exception-handled responses (09a4c25); this port always did,
//! so the parity harness's known divergence is gone.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::common::http::{TestRequest, TestResponse};
use axum::http::StatusCode;
use serde_json::json;

fn assert_headers(response: &TestResponse, what: &str) {
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("x-xss-protection", "0"),
        ("referrer-policy", "strict-origin-when-cross-origin"),
        (
            "content-security-policy",
            "default-src 'self'; frame-ancestors 'none'",
        ),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        ),
    ] {
        assert_eq!(
            response.header(name).as_deref(),
            Some(value),
            "{what}: {name}"
        );
    }
}

#[tokio::test]
async fn every_response_carries_the_six_headers_errors_included() {
    let host = crate::common::host().await;
    let token = host.token("admin").await;

    let root = host.send(TestRequest::get("/")).await;
    assert_eq!(root.status, StatusCode::OK);
    assert_headers(&root, "GET /");

    let invalid = host
        .send(
            TestRequest::post("/api/admin/genres")
                .bearer(&token)
                .json(&json!({ "Name": null })),
        )
        .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        invalid.header("content-type").as_deref(),
        Some("application/problem+json")
    );
    assert_headers(&invalid, "a validation 400");

    assert_headers(
        &host.send(TestRequest::get("/does-not-exist")).await,
        "a 404",
    );
    assert_headers(
        &host.send(TestRequest::get("/api/catalog/albums/1")).await,
        "a 401",
    );
}
