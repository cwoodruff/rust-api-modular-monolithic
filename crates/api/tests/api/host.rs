//! The host's own behavior: root, health, security headers, CORS, status code
//! pages, Swagger gating, trace identifiers. Ported from the earlier suite onto
//! the PostgreSQL host.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::common::http::TestRequest;
use axum::http::{Method, StatusCode};
use serde_json::json;
use shared_kernel::Environment;

const MODULES: [(&str, &str); 5] = [
    ("Administration", "/api/admin"),
    ("Identity", "/api/identity"),
    ("Catalog", "/api/catalog"),
    ("Orders", "/api/orders"),
    ("Reporting", "/api/reporting"),
];

#[tokio::test]
async fn the_root_endpoint_reports_the_service() {
    let host = crate::common::host().await;
    let response = host.send(TestRequest::get("/")).await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.header("content-type").as_deref(),
        Some("application/json; charset=utf-8")
    );
    let document = response.json();
    assert_eq!(document["module"], "root");
    assert_eq!(document["status"], "Healthy");
    assert_eq!(document["environment"], "Development");
    assert_eq!(document["service"], "ModularMonolith.Api");
    assert!(document["version"].is_string());
    assert!(document.get("database").is_none());
}

#[tokio::test]
async fn every_module_answers_health_and_data_health_under_its_own_name() {
    let host = crate::common::host().await;

    for (module, prefix) in MODULES {
        let health = host
            .send(TestRequest::get(&format!("{prefix}/health")))
            .await;
        assert_eq!(health.status, StatusCode::OK, "{prefix}/health");
        assert_eq!(health.json()["module"], module);
        assert_eq!(health.json()["status"], "Healthy");

        let data = host
            .send(TestRequest::get(&format!("{prefix}/data-health")))
            .await;
        assert_eq!(data.status, StatusCode::OK, "{prefix}/data-health");
        let document = data.json();
        assert_eq!(document["module"], module);
        assert_eq!(document["status"], "Data-Healthy", "{prefix}: {document}");
        assert_eq!(document["database"], json!({ "connected": true }));
    }
}

#[tokio::test]
async fn the_health_timestamp_uses_the_round_trip_format() {
    let host = crate::common::host().await;
    let document = host
        .send(TestRequest::get("/api/catalog/health"))
        .await
        .json();
    let stamp = document["timestampUtc"].as_str().unwrap();

    assert!(stamp.ends_with('Z'), "{stamp}");
    assert_eq!(stamp.len(), 28, "seven fractional digits: {stamp}");
}

#[tokio::test]
async fn an_unrecognized_environment_is_treated_as_production() {
    let host = crate::common::host_with(Environment::Other("Staging".to_owned()), json!({})).await;
    let document = host
        .send(TestRequest::get("/api/catalog/health"))
        .await
        .json();

    assert!(document.get("environment").is_none());
}

#[tokio::test]
async fn development_does_not_send_hsts_but_production_does() {
    let development = crate::common::host().await;
    assert!(
        development
            .send(TestRequest::get("/"))
            .await
            .header("strict-transport-security")
            .is_none()
    );

    let production = crate::common::host_with(Environment::Production, json!({})).await;
    assert_eq!(
        production
            .send(TestRequest::get("/"))
            .await
            .header("strict-transport-security")
            .as_deref(),
        Some("max-age=2592000")
    );
}

#[tokio::test]
async fn cors_grants_exactly_the_configured_origins() {
    let host = crate::common::host().await;

    let preflight = host
        .send(
            TestRequest::new(Method::OPTIONS, "/api/catalog/health")
                .header("Origin", "http://localhost:3000")
                .header("Access-Control-Request-Method", "GET"),
        )
        .await;
    assert!(
        preflight.status.is_success(),
        "preflight answered {}",
        preflight.status
    );
    assert_eq!(
        preflight.header("access-control-allow-origin").as_deref(),
        Some("http://localhost:3000")
    );

    let foreign = host
        .send(
            TestRequest::new(Method::OPTIONS, "/api/catalog/health")
                .header("Origin", "https://example.com")
                .header("Access-Control-Request-Method", "GET"),
        )
        .await;
    assert!(foreign.header("access-control-allow-origin").is_none());

    for origin in api::middleware::ALLOWED_ORIGINS {
        let response = host
            .send(TestRequest::get("/").header("Origin", origin))
            .await;
        assert_eq!(
            response.header("access-control-allow-origin").as_deref(),
            Some(origin)
        );
    }
}

#[tokio::test]
async fn bodiless_errors_get_problem_documents() {
    let host = crate::common::host().await;

    let missing = host.send(TestRequest::get("/does-not-exist")).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert!(
        missing
            .header("content-type")
            .unwrap()
            .starts_with("application/problem+json")
    );
    let document = missing.json();
    assert_eq!(
        document["type"],
        "https://tools.ietf.org/html/rfc9110#section-15.5.5"
    );
    assert_eq!(document["title"], "Not Found");
    assert_eq!(document["status"], 404);
    assert!(document["traceId"].is_string());

    let wrong_method = host.send(TestRequest::post("/api/catalog/health")).await;
    assert_eq!(wrong_method.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(wrong_method.json()["title"], "Method Not Allowed");
}

#[tokio::test]
async fn a_route_constraint_that_fails_is_a_404_before_authentication() {
    // `{id:int}` upstream: `/albums/abc` matches no route, so no token is asked for.
    let host = crate::common::host().await;

    assert_eq!(
        host.send(TestRequest::get("/api/catalog/albums/abc"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        host.send(TestRequest::get("/api/catalog/albums/1"))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn swagger_is_mounted_in_development_and_absent_in_production() {
    let development = crate::common::host().await;
    let document = development
        .send(TestRequest::get("/swagger/v1/swagger.json"))
        .await;
    assert_eq!(document.status, StatusCode::OK);
    let spec = document.json();
    assert_eq!(spec["info"]["title"], "Modular Monolith API");
    assert_eq!(spec["info"]["version"], "v1");
    assert!(spec["paths"]["/api/catalog/health"].is_object());
    assert!(spec["components"]["securitySchemes"]["Bearer"].is_object());
    assert!(
        spec["components"]["securitySchemes"]["X-API-Key"].is_null(),
        "F3: the undefined scheme is omitted"
    );
    assert_eq!(
        development.send(TestRequest::get("/swagger")).await.status,
        StatusCode::OK
    );

    let production = crate::common::host_with(Environment::Production, json!({})).await;
    assert_eq!(
        production
            .send(TestRequest::get("/swagger/v1/swagger.json"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        production.send(TestRequest::get("/swagger")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn trace_identifiers_tie_the_header_to_the_body() {
    let host = crate::common::host().await;

    for uri in ["/", "/api/catalog/health", "/no-such-route"] {
        let header = host
            .send(TestRequest::get(uri))
            .await
            .header("x-trace-id")
            .expect("a trace id");
        let segments: Vec<&str> = header.split('-').collect();
        assert_eq!(segments.len(), 4, "{uri}: {header}");
        assert_eq!(segments[1].len(), 32, "{uri}: {header}");
    }

    let missing = host.send(TestRequest::get("/no-such-route")).await;
    assert_eq!(
        missing.json()["traceId"].as_str().map(ToOwned::to_owned),
        missing.header("x-trace-id")
    );

    let refused = host.send(TestRequest::get("/api/catalog/albums/1")).await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        refused.json()["traceId"].as_str().map(ToOwned::to_owned),
        refused.header("x-trace-id")
    );

    let first = host.send(TestRequest::get("/")).await.header("x-trace-id");
    let second = host.send(TestRequest::get("/")).await.header("x-trace-id");
    assert_ne!(first, second);

    let upstream = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let continued = host
        .send(TestRequest::get("/no-such-route").header("traceparent", upstream))
        .await;
    assert_eq!(continued.header("x-trace-id").as_deref(), Some(upstream));
    assert_eq!(continued.json()["traceId"], upstream);

    for nonsense in [
        "not-a-traceparent",
        "00-0000000000000000000000000000000-x-00",
    ] {
        let issued = host
            .send(TestRequest::get("/no-such-route").header("traceparent", nonsense))
            .await
            .header("x-trace-id")
            .unwrap();
        assert_ne!(issued, nonsense);
    }
}

#[tokio::test]
async fn healthz_reports_every_module_database() {
    let host = crate::common::host().await;
    let response = host.send(TestRequest::get("/healthz")).await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.header("cache-control").as_deref(),
        Some("no-store, no-cache")
    );
    let document = response.json();
    assert_eq!(document["status"], "Healthy");
    assert!(document["totalDurationMs"].is_number());
    let modules = document["modules"].as_object().unwrap();
    assert_eq!(
        modules.keys().map(String::as_str).collect::<Vec<_>>(),
        ["Administration", "Catalog", "Orders", "Reporting"],
        "Identity has no database and no entry"
    );
    assert_eq!(
        modules["Catalog"]["description"],
        "CatalogDbContext reachable"
    );
    assert_eq!(modules["Catalog"]["tags"], json!(["Catalog", "database"]));
}

#[tokio::test]
async fn an_unreachable_database_is_unhealthy_on_healthz_and_degraded_on_data_health() {
    let host = crate::common::host_with(
        Environment::Development,
        json!({ "ConnectionStrings": {
            "AppDatabase": "Host=127.0.0.1;Port=1;Database=nowhere;Username=x;Password=x",
            "Reporting": "Host=127.0.0.1;Port=1;Database=nowhere;Username=x;Password=x"
        } }),
    )
    .await;

    let healthz = host.send(TestRequest::get("/healthz")).await;
    assert_eq!(healthz.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        healthz.json()["modules"]["Orders"]["description"],
        "OrdersDbContext cannot connect"
    );

    let data = host.send(TestRequest::get("/api/orders/data-health")).await;
    assert_eq!(
        data.status,
        StatusCode::OK,
        "data-health always answers 200"
    );
    assert_eq!(data.json()["status"], "Degraded");
    assert_eq!(data.json()["database"]["connected"], false);
}
