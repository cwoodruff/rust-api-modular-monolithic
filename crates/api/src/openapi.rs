//! The OpenAPI documents, ported from the host's `AddSwaggerGen` call.
//!
//! Upstream serves `v1`, holding every endpoint, plus one document per module
//! (`catalog`, `orders`, `admin`, `identity`, `reporting`), each including an
//! endpoint only when it carries that module's tag (U20). These are generated
//! from the endpoint descriptors, so a document cannot describe a route that
//! is not served, nor miss one that is.
//!
//! Upstream's global security requirement also names an `X-API-Key` scheme
//! that is never defined. Only `Bearer` is emitted here (F3).

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use shared_kernel::build_info;
use shared_kernel::endpoints::EndpointDescriptor;

/// Each module's document name and the tag it selects. Port of `ModuleOpenApiDocuments`.
pub const MODULE_DOCUMENTS: [(&str, &str); 5] = [
    ("catalog", "Catalog"),
    ("orders", "Orders"),
    ("admin", "Administration"),
    ("identity", "Identity"),
    ("reporting", "Reporting"),
];

/// Every document, by name: `v1` and one per module.
#[must_use]
pub fn documents(endpoints: &[EndpointDescriptor], service_name: &str) -> BTreeMap<String, Value> {
    let mut documents = BTreeMap::new();

    documents.insert(
        "v1".to_owned(),
        document(
            "Modular Monolith API",
            "ASP.NET Core Minimal API Modular Monolith with modules: Catalog, Orders, Administration, Reporting, Identity.",
            endpoints.iter(),
            service_name,
        ),
    );

    for (name, tag) in MODULE_DOCUMENTS {
        documents.insert(
            name.to_owned(),
            document(
                &format!("Modular Monolith API: {tag}"),
                &format!("The {tag} module's endpoints only."),
                endpoints.iter().filter(|endpoint| endpoint.tag == tag),
                service_name,
            ),
        );
    }

    documents
}

fn document<'a>(
    title: &str,
    description: &str,
    endpoints: impl Iterator<Item = &'a EndpointDescriptor>,
    service_name: &str,
) -> Value {
    let mut paths: BTreeMap<String, Map<String, Value>> = BTreeMap::new();

    for endpoint in endpoints {
        let (path, parameters) = openapi_path(&endpoint.route);
        paths.entry(path).or_default().insert(
            endpoint.method.to_lowercase(),
            operation(endpoint, parameters),
        );
    }

    json!({
        "openapi": "3.0.1",
        "info": {
            "title": title,
            "version": "v1",
            "description": description,
            "contact": { "name": "API Team" }
        },
        "paths": paths,
        "components": {
            "securitySchemes": {
                "Bearer": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                    "name": "Authorization",
                    "in": "header",
                    "description": "Paste your JWT access token only (no 'Bearer ' prefix). Swagger will add the prefix automatically."
                }
            }
        },
        "security": [{ "Bearer": [] }],
        "x-service": service_name,
        "x-version": build_info::version()
    })
}

/// `/api/catalog/albums/{id:int}` becomes `/api/catalog/albums/{id}` plus a
/// path parameter typed as the constraint says.
fn openapi_path(route: &str) -> (String, Vec<Value>) {
    let mut parameters = Vec::new();
    let segments: Vec<String> = route
        .trim_end_matches('/')
        .split('/')
        .map(|segment| {
            match segment
                .strip_prefix('{')
                .and_then(|rest| rest.strip_suffix('}'))
            {
                Some(inner) => {
                    let (name, schema) = match inner.split_once(':') {
                        Some((name, "int")) => {
                            (name, json!({ "type": "integer", "format": "int32" }))
                        }
                        Some((name, "guid")) => {
                            (name, json!({ "type": "string", "format": "uuid" }))
                        }
                        _ => (inner, json!({ "type": "string" })),
                    };
                    parameters.push(
                        json!({ "name": name, "in": "path", "required": true, "schema": schema }),
                    );
                    format!("{{{name}}}")
                }
                None => segment.to_owned(),
            }
        })
        .collect();

    let path = segments.join("/");
    (
        if path.is_empty() {
            "/".to_owned()
        } else {
            path
        },
        parameters,
    )
}

fn operation(endpoint: &EndpointDescriptor, parameters: Vec<Value>) -> Value {
    let mut responses = Map::new();
    for status in &endpoint.responses {
        responses.insert(
            status.to_string(),
            json!({ "description": reason(*status) }),
        );
    }
    if responses.is_empty() {
        responses.insert("200".to_owned(), json!({ "description": "OK" }));
    }

    let mut operation = Map::new();
    operation.insert("tags".to_owned(), json!([endpoint.tag]));
    if let Some(name) = &endpoint.name {
        operation.insert("operationId".to_owned(), json!(name));
    }
    if let Some(description) = &endpoint.description {
        operation.insert("description".to_owned(), json!(description));
    }
    if !parameters.is_empty() {
        operation.insert("parameters".to_owned(), Value::Array(parameters));
    }
    if let Some(body) = endpoint.validates {
        operation.insert(
            "requestBody".to_owned(),
            json!({ "required": true, "content": { "application/json": { "schema": { "title": body } } } }),
        );
    }
    if !endpoint.requires_authorization {
        operation.insert("security".to_owned(), json!([]));
    }
    if !endpoint.policies.is_empty() {
        operation.insert("x-policies".to_owned(), json!(endpoint.policies));
    }
    operation.insert("responses".to_owned(), Value::Object(responses));

    Value::Object(operation)
}

fn reason(status: u16) -> &'static str {
    axum::http::StatusCode::from_u16(status)
        .ok()
        .and_then(|status| status.canonical_reason())
        .unwrap_or("Response")
}
