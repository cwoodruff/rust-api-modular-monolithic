//! Port of upstream's `RateLimitingTests`. Requests here carry no connection
//! info, so every caller shares the `ip:unknown` partition — the limiter runs
//! before authentication, upstream and here (P24).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::common::http::TestRequest;
use axum::http::StatusCode;

#[tokio::test]
async fn the_root_allows_sixty_a_window_and_refuses_the_rest() {
    let host = crate::common::host().await;

    let mut ok = 0;
    let mut rejected = None;
    for _ in 0..65 {
        let response = host.send(TestRequest::get("/")).await;
        match response.status {
            StatusCode::OK => ok += 1,
            StatusCode::TOO_MANY_REQUESTS => rejected = Some(response),
            other => panic!("unexpected {other}"),
        }
    }

    assert_eq!(ok, 60);
    let rejected = rejected.expect("at least one 429");
    assert!(
        rejected.header("retry-after").is_none(),
        "upstream writes no Retry-After"
    );
    assert_eq!(
        rejected.header("x-content-type-options").as_deref(),
        Some("nosniff")
    );
    let document = rejected.json();
    assert!(
        document.get("type").is_none(),
        "429 has no entry in ASP.NET's defaults table"
    );
    assert_eq!(document["title"], "Too Many Requests");
    assert_eq!(document["status"], 429);
}

#[tokio::test]
async fn ten_requests_are_all_allowed() {
    let host = crate::common::host().await;

    for _ in 0..10 {
        assert_eq!(
            host.send(TestRequest::get("/")).await.status,
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn an_authenticated_caller_is_limited_by_the_modules_policy() {
    let host = crate::common::host().await;
    let token = host.token("demo").await;

    let mut statuses = Vec::new();
    for _ in 0..70 {
        statuses.push(
            host.send(TestRequest::get("/api/catalog/albums/1").bearer(&token))
                .await
                .status,
        );
    }

    assert!(statuses.contains(&StatusCode::TOO_MANY_REQUESTS));
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        60
    );
}

#[tokio::test]
async fn logins_are_limited_by_identitys_policy() {
    let host = crate::common::host().await;

    let mut limited = false;
    for _ in 0..65 {
        let response = host
            .send(
                TestRequest::post("/api/identity/login")
                    .json(&serde_json::json!({ "username": "demo", "password": "wrong" })),
            )
            .await;
        limited |= response.status == StatusCode::TOO_MANY_REQUESTS;
    }
    assert!(limited);
}
