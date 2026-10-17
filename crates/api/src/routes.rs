//! The host's own endpoints: `GET /`, `GET /healthz`, and Swagger.

use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use serde_json::{Map, Value, json};
use shared_kernel::Services;
use shared_kernel::diagnostics::{HealthCheckRegistration, HealthStatus};
use shared_kernel::endpoints::Endpoints;
use shared_kernel::health::{HEALTHY, HealthResponse, ROOT_MODULE};
use shared_kernel::traffic_control;

use crate::openapi;

/// The tag the host's own endpoints carry, so they appear in `v1` only.
pub const ROOT_TAG: &str = "Root";

/// Maps `GET /` and `GET /healthz`.
///
/// `/` carries `global:public-anon` and nothing else does (ADR-0013).
/// `/healthz` carries no rate limit, as upstream's `MapHealthChecks` carries none.
pub fn map_root(endpoints: &mut Endpoints, services: &Services) {
    let limiter = services
        .rate_limiters()
        .get(traffic_control::policies::GLOBAL_PUBLIC_ANON)
        .cloned();

    {
        let host = endpoints.host().clone();
        let mut root = endpoints.map_group("", ROOT_TAG, ());
        if let Some(limiter) = limiter {
            root.require_rate_limiting(limiter);
        }
        root.map_get("/", move || async move {
            Json(HealthResponse::new(ROOT_MODULE, HEALTHY).gated(&host))
        })
        .with_name("Root")
        .produces(&[200, 429]);
    }

    let checks: Arc<[HealthCheckRegistration]> = services.health_checks().to_vec().into();
    let mut health = endpoints.map_group("", ROOT_TAG, checks);
    health
        .map_get("/healthz", healthz)
        .with_name("HealthZ")
        .produces(&[200, 503]);
}

/// Port of `MapHealthChecks("/healthz")` with upstream's `WriteHealthAsync`.
///
/// One entry per module database, tagged with the module's name. Healthy and
/// Degraded answer 200, Unhealthy 503, with the framework's no-cache headers.
async fn healthz(State(checks): State<Arc<[HealthCheckRegistration]>>) -> Response {
    let started = Instant::now();
    let mut worst = HealthStatus::Healthy;
    let mut modules = Map::new();

    // Concurrently, as ASP.NET's `HealthCheckService` runs them: one
    // unreachable database costs one timeout, not one per module.
    let running: Vec<_> = checks
        .iter()
        .map(|registration| {
            let check = Arc::clone(&registration.check);
            tokio::spawn(async move { check.check().await })
        })
        .collect();

    for (registration, task) in checks.iter().zip(running) {
        let result = task
            .await
            .unwrap_or_else(|_| shared_kernel::diagnostics::HealthCheckResult {
                status: HealthStatus::Unhealthy,
                description: format!("{}DbContext cannot connect", registration.name),
            });
        worst = match (worst, result.status) {
            (_, HealthStatus::Unhealthy) | (HealthStatus::Unhealthy, _) => HealthStatus::Unhealthy,
            (_, HealthStatus::Degraded) | (HealthStatus::Degraded, _) => HealthStatus::Degraded,
            _ => HealthStatus::Healthy,
        };
        modules.insert(
            registration.name.clone(),
            json!({
                "status": result.status.as_str(),
                "description": result.description,
                "tags": registration.tags,
            }),
        );
    }

    let body = json!({
        "status": worst.as_str(),
        "totalDurationMs": started.elapsed().as_secs_f64() * 1000.0,
        "modules": Value::Object(modules),
    });

    let status = if worst == HealthStatus::Unhealthy {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };

    let mut response = (status, Json(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache"),
    );
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(
        header::EXPIRES,
        HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT"),
    );
    response
}

/// Mounts Swagger UI and the OpenAPI documents in Development and Demo only.
///
/// Outside those environments the paths do not exist and answer 404, as
/// upstream's `UseSwagger` sits behind `ShouldExposeOperationalMetadata`. The
/// documents are generated from the endpoint descriptors after every module
/// has mapped, so they describe exactly what is served.
pub fn map_swagger(endpoints: &mut Endpoints) {
    if !endpoints.host().environment.exposes_operational_metadata() {
        return;
    }

    let documents = Arc::new(openapi::documents(
        endpoints.descriptors(),
        &endpoints.host().service_name,
    ));

    let router = axum::Router::new()
        .route("/swagger", get(swagger_ui))
        .route("/swagger/", get(swagger_ui))
        .route("/swagger/index.html", get(swagger_ui))
        .route(
            "/swagger/{document}/swagger.json",
            get(
                move |axum::extract::Path(name): axum::extract::Path<String>| {
                    let documents = Arc::clone(&documents);
                    async move {
                        documents
                            .get(&name)
                            .map(|document| Json(document.clone()).into_response())
                            .unwrap_or_else(|| shared_kernel::ApiError::NotFound.into_response())
                    }
                },
            ),
        );

    endpoints.merge_unlisted(router);
}

/// The Swagger UI page, listing `v1` then one document per module.
async fn swagger_ui() -> Html<String> {
    let urls: Vec<Value> = std::iter::once(
        json!({ "url": "/swagger/v1/swagger.json", "name": "All modules" }),
    )
    .chain(
        openapi::MODULE_DOCUMENTS.iter().map(
            |(name, tag)| json!({ "url": format!("/swagger/{name}/swagger.json"), "name": tag }),
        ),
    )
    .collect();

    Html(format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Modular Monolith API</title>
  <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css">
</head>
<body>
  <div id="swagger-ui"></div>
  <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
  <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-standalone-preset.js"></script>
  <script>
    window.ui = SwaggerUIBundle({{
      urls: {urls},
      dom_id: "#swagger-ui",
      presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset],
      layout: "StandaloneLayout"
    }});
  </script>
</body>
</html>"##,
        urls = Value::Array(urls)
    ))
}
