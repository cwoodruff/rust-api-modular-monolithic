//! The OpenAPI document, ported from the host's `AddSwaggerGen` call.
//!
//! The original describes one document, `v1`, titled "Modular Monolith API",
//! and declares a `Bearer` HTTP/JWT security scheme.
//!
//! It also adds a security *requirement* naming a second scheme, `X-API-Key`,
//! that is never defined anywhere in the repository — a latent bug that makes
//! the generated document reference a scheme that does not exist. This port
//! emits only the Bearer scheme (F3 in the plan).

use serde_json::{Value, json};
use shared_kernel::build_info;
use shared_persistence::AppState;

/// Builds the document.
///
/// Paths are assembled from what is actually mounted, so this grows as the
/// modules land rather than describing endpoints that do not exist yet.
#[must_use]
pub fn document(state: &AppState) -> Value {
    let mut paths = serde_json::Map::new();

    paths.insert("/".to_owned(), health_path("Root", "Root"));

    for (module, prefix) in crate::app::modules() {
        paths.insert(
            format!("{prefix}/health"),
            health_path(module, &format!("{module}Health")),
        );
        paths.insert(
            format!("{prefix}/data-health"),
            health_path(module, &format!("{module}DataHealth")),
        );
    }

    json!({
        "openapi": "3.0.1",
        "info": {
            "title": "Modular Monolith API",
            "version": "v1",
            "description": "ASP.NET Core Minimal API Modular Monolith with modules: \
                            Music, Orders, Administration, Reporting, Identity.",
            "contact": { "name": "API Team" }
        },
        "servers": [{ "url": "/" }],
        "paths": Value::Object(paths),
        "components": {
            "securitySchemes": {
                "Bearer": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                    "description": "Paste your JWT access token only (no 'Bearer ' prefix). \
                                    Swagger will add the prefix automatically."
                }
            }
        },
        "x-service": state.config.service_name(),
        "x-version": build_info::version()
    })
}

/// One health-shaped path entry.
///
/// Every endpoint in the application declares a 429, because every endpoint
/// carries the one rate-limit policy.
fn health_path(tag: &str, operation_id: &str) -> Value {
    json!({
        "get": {
            "tags": [tag],
            "operationId": operation_id,
            "responses": {
                "200": { "description": "Success" },
                "429": { "description": "Too Many Requests" }
            }
        }
    })
}
