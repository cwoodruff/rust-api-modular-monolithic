//! Composition: the module registry, the router, and the middleware stack.

use axum::http::{HeaderValue, Method, header};
use axum::{Router, middleware};
use shared_kernel::module::{Module, mount_all};
use shared_persistence::AppState;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::middleware::{ALLOWED_ORIGINS, RateLimiter};
use crate::{middleware as own, routes};

/// The module registry, mirroring the hard-coded `GetModules()` list.
///
/// There is no reflection here and none there: the original returns a literal
/// array, so adding a module means editing this function. Order is the
/// original's — Administration, Identity, Music, Orders, Reporting.
#[must_use]
pub fn registry() -> Vec<Box<dyn Module<AppState>>> {
    vec![
        Box::new(module_admin::AdministrationModule),
        Box::new(module_identity::IdentityModule),
        Box::new(module_music::MusicModule),
        Box::new(module_orders::OrdersModule),
        Box::new(module_reporting::ReportingModule),
    ]
}

/// The registered modules as `(name, prefix)` pairs.
#[must_use]
pub fn modules() -> Vec<(&'static str, &'static str)> {
    registry()
        .iter()
        .map(|module| (module.name(), module.prefix()))
        .collect()
}

/// Builds the application.
///
/// Layers are applied bottom-up: the last one added runs first, so this reads
/// in reverse of the original's `Program.cs`. The comments name each one's
/// counterpart.
pub fn build(state: AppState) -> Router {
    let secure_transport = !state.environment.is_development();

    let router = mount_all(routes::root().merge(routes::swagger(&state)), &registry());

    let mut app = router
        // UseIdentityAuth() sits here. Phase 5 fills it in; until then every
        // endpoint is anonymous, which for health endpoints is also the final
        // answer.
        //
        // UseRateLimiter()
        .layer(middleware::from_fn_with_state(
            RateLimiter::default(),
            own::rate_limit,
        ))
        // UseCors("Default")
        .layer(cors())
        // The OWASP A05 security headers
        .layer(middleware::from_fn(own::security_headers))
        // UseStatusCodePages()
        .layer(middleware::from_fn(own::status_code_pages))
        // UseExceptionHandler(...) — a panic is the closest thing Rust has to
        // the unhandled exception that handler exists for.
        .layer(CatchPanicLayer::custom(own::panic_to_problem))
        .layer(TraceLayer::new_for_http());

    // UseHsts() — non-Development only, as in the original.
    if secure_transport {
        app = app.layer(middleware::from_fn(own::hsts));
    }

    app.with_state(state)
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
        .expose_headers([header::CONTENT_TYPE])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn the_registry_matches_the_original_list_and_order() {
        assert_eq!(
            modules(),
            vec![
                ("Administration", "/api/admin"),
                ("Identity", "/api/identity"),
                ("Music", "/api/music"),
                ("Orders", "/api/orders"),
                ("Reporting", "/api/reporting"),
            ]
        );
    }

    #[test]
    fn the_administration_module_reports_a_name_its_prefix_does_not_match() {
        // Worth pinning: the module is "Administration" but mounts at
        // "/api/admin", and both halves are wire-visible.
        let (name, prefix) = modules()[0];

        assert_eq!(name, "Administration");
        assert_eq!(prefix, "/api/admin");
    }
}
