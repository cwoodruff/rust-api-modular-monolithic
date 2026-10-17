//! Port of upstream's `AuthorizationPipelineTests`: a real login's token
//! carries what each module's policies ask for, and nothing more.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::common::http::TestRequest;
use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;

fn claims(token: &str) -> Value {
    serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(token.split('.').nth(1).unwrap())
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn the_admin_token_carries_role_permissions_and_tenant() {
    let host = crate::common::host().await;
    let token = host.token("admin").await;
    let claims = claims(&token);

    assert_eq!(
        claims["http://schemas.microsoft.com/ws/2008/06/identity/claims/role"],
        "Admin"
    );
    let permissions: Vec<&str> = claims["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    for wanted in [
        "administration.read",
        "administration.write",
        "admin.users.manage",
    ] {
        assert!(permissions.contains(&wanted), "{wanted}");
    }
    assert_eq!(claims["tenant"], "tenant-1");
}

#[tokio::test]
async fn each_policy_admits_exactly_the_callers_it_names() {
    let host = crate::common::host().await;
    let admin = host.token("admin").await;
    let demo = host.token("demo").await;
    let usermo = host.token("usermo").await;
    let report = host.token("report").await;

    let cases: [(&str, &str, &str, StatusCode); 12] = [
        ("GET", "/api/admin/customers", &admin, StatusCode::OK),
        ("GET", "/api/admin/customers", &demo, StatusCode::FORBIDDEN),
        ("GET", "/api/catalog/albums/1", &demo, StatusCode::OK),
        (
            "GET",
            "/api/orders/invoices/1",
            &demo,
            StatusCode::FORBIDDEN,
        ),
        ("GET", "/api/orders/invoices/1", &usermo, StatusCode::OK),
        (
            "POST",
            "/api/orders/invoices/2/finalize",
            &usermo,
            StatusCode::FORBIDDEN,
        ),
        (
            "GET",
            "/api/reporting/sales-by-genre",
            &report,
            StatusCode::OK,
        ),
        (
            "GET",
            "/api/reporting/sales-by-genre",
            &demo,
            StatusCode::FORBIDDEN,
        ),
        (
            "GET",
            "/api/reporting/integrity/findings",
            &report,
            StatusCode::FORBIDDEN,
        ),
        (
            "GET",
            "/api/reporting/integrity/findings",
            &admin,
            StatusCode::OK,
        ),
        (
            "GET",
            "/api/orders/outbox/dead-letters",
            &usermo,
            StatusCode::FORBIDDEN,
        ),
        (
            "GET",
            "/api/orders/outbox/dead-letters",
            &admin,
            StatusCode::OK,
        ),
    ];

    for (method, uri, token, expected) in cases {
        let request = if method == "GET" {
            TestRequest::get(uri)
        } else {
            TestRequest::post(uri)
        };
        let response = host.send(request.bearer(token)).await;
        assert_eq!(response.status, expected, "{method} {uri}");
    }
}

#[tokio::test]
async fn the_dead_letter_list_is_empty_and_an_unknown_retry_is_a_404() {
    let host = crate::common::host().await;
    let admin = host.token("admin").await;

    assert_eq!(
        host.send(TestRequest::get("/api/orders/outbox/dead-letters").bearer(&admin))
            .await
            .json(),
        serde_json::json!([])
    );
    let retry = host
        .send(
            TestRequest::post(
                "/api/orders/outbox/dead-letters/00000000-0000-0000-0000-000000000001/retry",
            )
            .bearer(&admin),
        )
        .await;
    assert_eq!(retry.status, StatusCode::NOT_FOUND);
    let not_a_guid = host
        .send(TestRequest::post("/api/orders/outbox/dead-letters/abc/retry").bearer(&admin))
        .await;
    assert_eq!(
        not_a_guid.status,
        StatusCode::NOT_FOUND,
        "{{id:guid}} does not match"
    );
}
