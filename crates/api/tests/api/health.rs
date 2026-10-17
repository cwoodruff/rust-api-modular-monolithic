//! Port of upstream's `HealthEndpointsTests`: what `/` and each module's
//! health endpoints expose, by environment.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::common::http::TestRequest;
use axum::http::StatusCode;
use serde_json::{Value, json};
use shared_kernel::Environment;

const HEALTH: [(&str, &str); 6] = [
    ("/", "root"),
    ("/api/catalog/health", "Catalog"),
    ("/api/orders/health", "Orders"),
    ("/api/admin/health", "Administration"),
    ("/api/reporting/health", "Reporting"),
    ("/api/identity/health", "Identity"),
];

fn keys(document: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = document
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[tokio::test]
async fn production_exposes_only_module_status_and_time() {
    let host = crate::common::host_with(Environment::Production, json!({})).await;

    for (uri, module) in HEALTH {
        let document = host.send(TestRequest::get(uri)).await.json();
        assert_eq!(
            keys(&document),
            ["module", "status", "timestampUtc"],
            "{uri}"
        );
        assert_eq!(document["module"], module);
        assert_eq!(document["status"], "Healthy");
    }

    for (uri, _) in &HEALTH[1..] {
        let data = host
            .send(TestRequest::get(&uri.replace("/health", "/data-health")))
            .await
            .json();
        assert_eq!(keys(&data), ["module", "status", "timestampUtc"], "{uri}");
        assert!(matches!(
            data["status"].as_str(),
            Some("Data-Healthy" | "Degraded")
        ));
    }

    assert_eq!(
        host.send(TestRequest::get("/swagger/v1/swagger.json"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        host.send(TestRequest::get("/swagger/index.html"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn demo_adds_the_operational_metadata_and_the_database_member() {
    let host = crate::common::host_with(Environment::Demo, json!({})).await;

    for (uri, _) in HEALTH {
        let document = host.send(TestRequest::get(uri)).await.json();
        assert_eq!(
            keys(&document),
            [
                "environment",
                "module",
                "service",
                "status",
                "timestampUtc",
                "version"
            ],
            "{uri}"
        );
        assert_eq!(document["environment"], "Demo");
    }

    for (uri, _) in &HEALTH[1..] {
        let data = host
            .send(TestRequest::get(&uri.replace("/health", "/data-health")))
            .await
            .json();
        assert_eq!(data["status"], "Data-Healthy", "{uri}");
        assert!(data["database"]["connected"].is_boolean(), "{uri}");
    }

    assert!(
        host.send(TestRequest::get("/swagger/v1/swagger.json"))
            .await
            .status
            .is_success()
    );
    assert!(
        host.send(TestRequest::get("/swagger/index.html"))
            .await
            .status
            .is_success()
    );
}
