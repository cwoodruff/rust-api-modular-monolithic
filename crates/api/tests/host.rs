//! Host integration tests.
//!
//! These drive the same router the binary serves, which is the seam the C#
//! suite gets from `WebApplicationFactory<Program>`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use serde_json::Value;
use shared_kernel::Environment;
use tower::ServiceExt;

/// The repository root, which is the host's content root.
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

/// Builds the application for one environment.
async fn app_for(environment: Environment) -> Router {
    let root = content_root();
    let state = api::build_state_for(&root, environment)
        .await
        .expect("the application state should build");

    api::build(state)
}

async fn development_app() -> Router {
    app_for(Environment::Development).await
}

/// One response, already read.
struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl TestResponse {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|error| panic!("expected JSON, got {:?}: {error}", self.text()))
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
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

async fn get(app: &Router, uri: &str) -> TestResponse {
    let request = Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("the request should build");

    send(app, request).await
}

/// The five modules and the names their health endpoints report.
const MODULES: [(&str, &str); 5] = [
    ("/api/admin", "Administration"),
    ("/api/identity", "Identity"),
    ("/api/music", "Music"),
    ("/api/orders", "Orders"),
    ("/api/reporting", "Reporting"),
];

// ---------------------------------------------------------------------------
// Payload shapes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_root_endpoint_reports_the_service() {
    let app = development_app().await;

    let response = get(&app, "/").await;

    assert_eq!(response.status, StatusCode::OK);

    let document = response.json();
    assert_eq!(document["module"], "root");
    assert_eq!(document["status"], "Healthy");
    assert_eq!(document["environment"], "Development");
    assert_eq!(document["service"], "ModularMonolith.Api");
    assert!(document["version"].is_string());
    assert!(document["timestampUtc"].is_string());
}

#[tokio::test]
async fn every_module_answers_health_under_its_own_name() {
    let app = development_app().await;

    for (prefix, module) in MODULES {
        let response = get(&app, &format!("{prefix}/health")).await;

        assert_eq!(response.status, StatusCode::OK, "{prefix}/health");

        let document = response.json();
        assert_eq!(document["module"], module, "{prefix}/health");
        assert_eq!(document["status"], "Healthy", "{prefix}/health");
    }
}

#[tokio::test]
async fn every_module_answers_data_health_against_the_real_database() {
    let app = development_app().await;

    for (prefix, module) in MODULES {
        let response = get(&app, &format!("{prefix}/data-health")).await;

        assert_eq!(response.status, StatusCode::OK, "{prefix}/data-health");

        let document = response.json();
        assert_eq!(document["module"], module);
        assert_eq!(
            document["status"], "Data-Healthy",
            "the bundled database should be reachable"
        );
        assert_eq!(
            document["database"],
            serde_json::json!({ "connected": true })
        );
    }
}

#[tokio::test]
async fn the_health_timestamp_uses_the_round_trip_format() {
    let app = development_app().await;

    let stamp = get(&app, "/api/music/health").await.json()["timestampUtc"]
        .as_str()
        .expect("timestampUtc should be a string")
        .to_owned();

    // yyyy-MM-ddTHH:mm:ss.fffffffZ — seven fractional digits, as .NET's "O".
    assert!(stamp.ends_with('Z'), "{stamp}");
    assert_eq!(stamp.len(), 28, "{stamp}");
    let fraction = stamp.split('.').nth(1).expect("a fractional part");
    assert_eq!(
        fraction.len(),
        8,
        "{stamp} should carry seven digits and a Z"
    );
}

// ---------------------------------------------------------------------------
// Environment gating
// ---------------------------------------------------------------------------

#[tokio::test]
async fn production_omits_the_operational_metadata_entirely() {
    let app = app_for(Environment::Production).await;

    for uri in ["/", "/api/music/health", "/api/music/data-health"] {
        let document = get(&app, uri).await.json();

        assert!(document["module"].is_string(), "{uri}");
        assert!(document["status"].is_string(), "{uri}");
        assert!(document["timestampUtc"].is_string(), "{uri}");

        for absent in ["environment", "version", "service", "database"] {
            assert!(
                document.get(absent).is_none(),
                "{uri} should omit `{absent}` outside Development and Demo, \
                 not send it as null"
            );
        }
    }
}

#[tokio::test]
async fn demo_exposes_the_same_metadata_as_development() {
    // Demo is first-class in the original, not a synonym for Production.
    let app = app_for(Environment::Demo).await;

    let document = get(&app, "/api/music/health").await.json();

    assert_eq!(document["environment"], "Demo");
    assert!(document["version"].is_string());
    assert!(document["service"].is_string());
}

#[tokio::test]
async fn an_unrecognized_environment_is_treated_as_production() {
    let app = app_for(Environment::Other("Staging".to_owned())).await;

    let document = get(&app, "/api/music/health").await.json();

    assert!(document.get("environment").is_none());
}

// ---------------------------------------------------------------------------
// Security headers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_response_carries_the_security_headers() {
    let app = development_app().await;

    for uri in ["/", "/api/orders/health", "/does-not-exist"] {
        let response = get(&app, uri).await;

        assert_eq!(
            response.header("x-content-type-options").as_deref(),
            Some("nosniff"),
            "{uri}"
        );
        assert_eq!(
            response.header("x-frame-options").as_deref(),
            Some("DENY"),
            "{uri}"
        );
        assert_eq!(
            response.header("x-xss-protection").as_deref(),
            Some("0"),
            "{uri}"
        );
        assert_eq!(
            response.header("referrer-policy").as_deref(),
            Some("strict-origin-when-cross-origin"),
            "{uri}"
        );
        assert_eq!(
            response.header("content-security-policy").as_deref(),
            Some("default-src 'self'; frame-ancestors 'none'"),
            "{uri}"
        );
        assert_eq!(
            response.header("permissions-policy").as_deref(),
            Some("camera=(), microphone=(), geolocation=()"),
            "{uri}"
        );
    }
}

#[tokio::test]
async fn development_does_not_send_hsts_but_production_does() {
    // UseHsts() sits behind the same non-Development check in the original.
    let development = development_app().await;
    assert!(
        get(&development, "/")
            .await
            .header("strict-transport-security")
            .is_none()
    );

    let production = app_for(Environment::Production).await;
    assert!(
        get(&production, "/")
            .await
            .header("strict-transport-security")
            .is_some()
    );
}

// ---------------------------------------------------------------------------
// CORS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_preflight_from_an_allowed_origin_succeeds() {
    let app = development_app().await;

    let request = Request::builder()
        .method(Method::OPTIONS)
        .uri("/api/music/health")
        .header(header::ORIGIN, "http://localhost:3000")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
        .body(Body::empty())
        .unwrap();

    let response = send(&app, request).await;

    assert!(
        response.status.is_success(),
        "preflight answered {}",
        response.status
    );
    assert_eq!(
        response.header("access-control-allow-origin").as_deref(),
        Some("http://localhost:3000")
    );
}

#[tokio::test]
async fn a_preflight_from_an_unlisted_origin_is_not_granted() {
    let app = development_app().await;

    let request = Request::builder()
        .method(Method::OPTIONS)
        .uri("/api/music/health")
        .header(header::ORIGIN, "https://example.com")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
        .body(Body::empty())
        .unwrap();

    let response = send(&app, request).await;

    assert!(
        response.header("access-control-allow-origin").is_none(),
        "only the six hard-coded localhost origins are allowed"
    );
}

#[tokio::test]
async fn every_configured_origin_is_accepted() {
    let app = development_app().await;

    for origin in api::middleware::ALLOWED_ORIGINS {
        let request = Request::builder()
            .uri("/")
            .header(header::ORIGIN, origin)
            .body(Body::empty())
            .unwrap();

        let response = send(&app, request).await;

        assert_eq!(
            response.header("access-control-allow-origin").as_deref(),
            Some(origin),
            "{origin} should be allowed"
        );
    }
}

// ---------------------------------------------------------------------------
// Status code pages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unknown_route_gets_a_problem_document() {
    // Verified against the running C# service: AddProblemDetails() replaces
    // the status-code-pages middleware's plain-text default, so a bodiless 404
    // answers with application/problem+json.
    let app = development_app().await;

    let response = get(&app, "/does-not-exist").await;

    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert!(
        response
            .header("content-type")
            .is_some_and(|value| value.starts_with("application/problem+json")),
        "got {:?}",
        response.header("content-type")
    );

    let document = response.json();
    assert_eq!(
        document["type"],
        "https://tools.ietf.org/html/rfc9110#section-15.5.5"
    );
    assert_eq!(document["title"], "Not Found");
    assert_eq!(document["status"], 404);
    assert!(document["traceId"].is_string());
}

#[tokio::test]
async fn a_wrong_method_also_gets_a_status_code_page() {
    let app = development_app().await;

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/music/health")
        .body(Body::empty())
        .unwrap();

    let response = send(&app, request).await;

    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);

    let document = response.json();
    assert_eq!(
        document["type"],
        "https://tools.ietf.org/html/rfc9110#section-15.5.6"
    );
    assert_eq!(document["title"], "Method Not Allowed");
}

#[tokio::test]
async fn a_successful_response_keeps_its_own_body() {
    let app = development_app().await;

    let response = get(&app, "/api/music/health").await;

    assert!(response.text().contains("\"module\":\"Music\""));
}

// ---------------------------------------------------------------------------
// Swagger gating
// ---------------------------------------------------------------------------

#[tokio::test]
async fn swagger_is_mounted_in_development() {
    let app = development_app().await;

    let document = get(&app, "/swagger/v1/swagger.json").await;
    assert_eq!(document.status, StatusCode::OK);

    let spec = document.json();
    assert_eq!(spec["info"]["title"], "Modular Monolith API");
    assert_eq!(spec["info"]["version"], "v1");
    assert!(spec["paths"]["/api/music/health"].is_object());
    assert!(
        spec["components"]["securitySchemes"]["Bearer"].is_object(),
        "the Bearer scheme should be declared"
    );
    assert!(
        spec["components"]["securitySchemes"]["X-API-Key"].is_null(),
        "the original references this scheme without defining it; we omit it"
    );

    assert_eq!(get(&app, "/swagger").await.status, StatusCode::OK);
}

#[tokio::test]
async fn swagger_is_absent_in_production() {
    let app = app_for(Environment::Production).await;

    assert_eq!(
        get(&app, "/swagger/v1/swagger.json").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(get(&app, "/swagger").await.status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Rate limiting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_sixty_first_request_in_a_window_is_rejected() {
    // 60 permits per 60 seconds, queue depth zero. Requests here carry no
    // connection info, so they all share the `ip:unknown` partition.
    let app = development_app().await;

    for index in 0..60 {
        let response = get(&app, "/api/music/health").await;
        assert_eq!(
            response.status,
            StatusCode::OK,
            "request {index} should be within the allowance"
        );
    }

    let rejected = get(&app, "/api/music/health").await;

    assert_eq!(rejected.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        rejected.header("retry-after").is_none(),
        "the original writes no Retry-After"
    );

    // 429 is absent from ASP.NET's defaults table, so the document carries no
    // `type` at all and falls back to the reason phrase for its title.
    let document = rejected.json();
    assert!(
        document.get("type").is_none(),
        "429 should carry no type, got {:?}",
        document.get("type")
    );
    assert_eq!(document["title"], "Too Many Requests");
    assert_eq!(document["status"], 429);
}

#[tokio::test]
async fn the_allowance_is_shared_across_every_endpoint() {
    // One policy covers the whole application — health, root and business
    // routes all draw on the same budget.
    let app = development_app().await;

    for _ in 0..30 {
        assert_eq!(get(&app, "/").await.status, StatusCode::OK);
    }
    for _ in 0..30 {
        assert_eq!(get(&app, "/api/orders/health").await.status, StatusCode::OK);
    }

    assert_eq!(
        get(&app, "/api/reporting/health").await.status,
        StatusCode::TOO_MANY_REQUESTS,
        "the root and health endpoints share one allowance"
    );
}

#[tokio::test]
async fn a_rejected_request_still_carries_the_security_headers() {
    let app = development_app().await;

    for _ in 0..60 {
        let _ = get(&app, "/").await;
    }

    let rejected = get(&app, "/").await;

    assert_eq!(rejected.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        rejected.header("x-content-type-options").as_deref(),
        Some("nosniff"),
        "the headers layer sits outside the limiter"
    );
}
