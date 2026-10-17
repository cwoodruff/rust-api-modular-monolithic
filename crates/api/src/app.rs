//! The middleware pipeline, ported from `Program.cs`.

use axum::http::{HeaderValue, Method, header};
use axum::{Router, middleware};
use shared_kernel::Services;
use shared_kernel::auth;
use shared_kernel::endpoints::EndpointDescriptor;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::middleware::{self as own, ALLOWED_ORIGINS};

/// Wraps the mapped endpoints in the host's middleware.
///
/// Layers are applied bottom-up — the last added runs first — so this reads in
/// reverse of `Program.cs`:
///
/// 1. HSTS — outside Development only
/// 2. panic → RFC 7807, where `UseExceptionHandler` sits
/// 3. `UseStatusCodePages()`
/// 4. `charset=utf-8` on JSON responses
/// 5. the six security headers, on every response including error responses
///    (upstream's fix in 09a4c25, which this port always had)
/// 6. CORS
/// 7. authentication — the rate limits and authorization live on each
///    module's group and endpoints, inside routing
pub fn pipeline(router: Router, services: &Services, _endpoints: &[EndpointDescriptor]) -> Router {
    let environment = services.environment().clone();

    let mut app = router;

    if let Some(authenticator) = services.authenticator() {
        app = app.layer(middleware::from_fn(move |request, next| {
            auth::authenticate(std::sync::Arc::clone(&authenticator), request, next)
        }));
    }

    app = app
        .layer(cors())
        .layer(middleware::from_fn(own::security_headers))
        .layer(middleware::from_fn(own::json_charset))
        .layer(middleware::from_fn(own::status_code_pages))
        .layer(CatchPanicLayer::custom(own::panic_to_problem))
        .layer(TraceLayer::new_for_http())
        // Outermost, so the identifier it establishes is in scope for every
        // layer below — the panic handler and the status-code pages included.
        .layer(middleware::from_fn(own::request_id));

    if !environment.is_development() {
        app = app.layer(middleware::from_fn(own::hsts));
    }

    app
}

/// The `Default` CORS policy.
fn cors() -> CorsLayer {
    let origins: Vec<HeaderValue> = ALLOWED_ORIGINS
        .iter()
        .filter_map(|origin| HeaderValue::from_str(origin).ok())
        .collect();

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_headers(tower_http::cors::Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
            Method::HEAD,
            Method::OPTIONS,
        ])
        .expose_headers([header::CONTENT_TYPE, own::TRACE_ID_HEADER])
}
