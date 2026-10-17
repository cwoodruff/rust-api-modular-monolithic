//! Port of `GuardrailTests`, over the endpoints a real host maps.
//!
//! The host is built through `api::build` exactly as the binary builds it —
//! upstream's `HostWithoutDatabaseFactory` — with migration off and a
//! connection string that is never dialled: the pools are lazy.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::sync::Arc;

use api::module_composition::{find_duplicate_routes, find_unknown_policies, validate_endpoints};
use identity_contracts::{permissions, policies};
use shared_kernel::Environment;
use shared_kernel::auth::PolicyEvaluator;
use shared_kernel::endpoints::EndpointDescriptor;
use shared_kernel::traffic_control;

/// Each module's prefix and the one rate-limit policy its endpoints carry.
const MODULE_POLICIES: [(&str, &str); 5] = [
    ("/api/catalog/", traffic_control::policies::CATALOG),
    ("/api/orders/", traffic_control::policies::ORDERS),
    ("/api/admin/", traffic_control::policies::ADMINISTRATION),
    ("/api/identity/", traffic_control::policies::IDENTITY),
    ("/api/reporting/", traffic_control::policies::REPORTING),
];

/// Every endpoint that takes no caller, with the reason it may not.
///
/// Port-only: upstream relies on review here. Each entry has to be defensible
/// out loud, and a new unguarded endpoint fails until it is added with one.
const ANONYMOUS: [(&str, &str); 19] = [
    ("/", "the root reports liveness"),
    ("/healthz", "an orchestrator's health probe"),
    ("/api/catalog/health", "module health, anonymous upstream"),
    (
        "/api/catalog/data-health",
        "module health, anonymous upstream",
    ),
    ("/api/orders/health", "module health, anonymous upstream"),
    (
        "/api/orders/data-health",
        "module health, anonymous upstream",
    ),
    ("/api/admin/health", "module health, anonymous upstream"),
    (
        "/api/admin/data-health",
        "module health, anonymous upstream",
    ),
    ("/api/reporting/health", "module health, anonymous upstream"),
    (
        "/api/reporting/data-health",
        "module health, anonymous upstream",
    ),
    ("/api/identity/health", "module health, anonymous upstream"),
    (
        "/api/identity/data-health",
        "module health, anonymous upstream",
    ),
    (
        "/api/identity/login",
        "a caller cannot present a token before they have one",
    ),
    (
        "/api/identity/refresh",
        "the refresh token is the credential",
    ),
    (
        "/api/identity/.well-known/jwks.json",
        "public key material for anyone validating a token",
    ),
    // Swagger is mapped without descriptors, so these never appear; listed
    // to keep the reasoning in one place.
    ("/swagger", "Development and Demo only"),
    ("/swagger/index.html", "Development and Demo only"),
    (
        "/swagger/{document}/swagger.json",
        "Development and Demo only",
    ),
    ("/swagger/", "Development and Demo only"),
];

async fn host() -> api::Host {
    let config = test_support::config(
        Environment::Development,
        serde_json::json!({
            "ConnectionStrings": {
                "AppDatabase": "Host=localhost;Port=1;Database=none;Username=none;Password=none",
                "Reporting": "Host=localhost;Port=1;Database=none;Username=none;Password=none"
            },
            "Database": { "MigrateAndSeedOnStartup": false },
            "Outbox": { "Enabled": false }
        }),
    );
    let keys: Arc<dyn module_identity::KeyMaterial> =
        Arc::new(module_identity::DevKeyMaterial::generate().unwrap());
    let options = api::HostOptions {
        identity_keys: Some(keys),
        ..api::HostOptions::new(test_support::content_root())
    };

    api::build(config, options)
        .await
        .expect("the host should build without a database")
}

#[tokio::test]
async fn every_policy_an_endpoint_references_is_registered() {
    let host = host().await;
    let policies = host
        .services()
        .policies()
        .expect("Identity registers the policies");

    assert!(
        host.endpoints()
            .iter()
            .any(|endpoint| !endpoint.policies.is_empty())
    );
    assert_eq!(
        find_unknown_policies(host.endpoints(), policies.as_ref()),
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn every_policy_name_is_an_identity_contracts_constant() {
    let host = host().await;
    let constants: BTreeSet<&str> = permissions::ALL
        .iter()
        .chain(policies::ALL.iter())
        .copied()
        .collect();

    for endpoint in host.endpoints() {
        for policy in &endpoint.policies {
            assert!(
                constants.contains(policy),
                "{} requires '{policy}', which identity-contracts does not declare",
                endpoint.route
            );
        }
    }
}

#[tokio::test]
async fn every_module_endpoint_carries_exactly_its_modules_rate_limit() {
    let host = host().await;

    for endpoint in host.endpoints() {
        let Some((_, expected)) = MODULE_POLICIES
            .iter()
            .find(|(prefix, _)| endpoint.route.starts_with(prefix))
        else {
            continue;
        };
        assert_eq!(
            endpoint.rate_limit_policy,
            Some(*expected),
            "{} {}",
            endpoint.method,
            endpoint.route
        );
    }

    let root = host
        .endpoints()
        .iter()
        .find(|endpoint| endpoint.route == "/")
        .expect("the root is mapped");
    assert_eq!(
        root.rate_limit_policy,
        Some(traffic_control::policies::GLOBAL_PUBLIC_ANON)
    );
}

#[tokio::test]
async fn no_two_endpoints_share_a_route_and_method() {
    let host = host().await;

    assert_eq!(
        find_duplicate_routes(host.endpoints()),
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn every_unguarded_endpoint_is_named_with_its_reason() {
    let host = host().await;
    let allowed: BTreeSet<&str> = ANONYMOUS.iter().map(|(route, _)| *route).collect();

    for endpoint in host
        .endpoints()
        .iter()
        .filter(|endpoint| !endpoint.requires_authorization)
    {
        assert!(
            allowed.contains(endpoint.route.as_str()),
            "{} {} takes no caller; guard it or name it in ANONYMOUS with the reason",
            endpoint.method,
            endpoint.route
        );
    }
    assert!(
        host.endpoints().len() >= 50,
        "the descriptors should cover every route, found {}",
        host.endpoints().len()
    );
}

#[tokio::test]
async fn integration_event_handlers_resolve_only_within_their_own_module_key() {
    let host = host().await;
    let subscriptions = host.services().subscriptions();

    let expected = [
        ("Catalog", "Catalog.Modules.Events."),
        ("Administration", "Admin.Modules.Events."),
    ];
    for (module, namespace) in expected {
        let handlers = subscriptions.for_module(module);
        assert!(
            !handlers.is_empty(),
            "{module} should handle InvoiceFinalized"
        );
        for handler in handlers {
            assert!(
                handler.handler_name.starts_with(namespace),
                "{module}'s key yields {}",
                handler.handler_name
            );
            assert_eq!(handler.module_key, module);
        }
    }

    let keyed: usize = [
        "Catalog",
        "Administration",
        "Orders",
        "Reporting",
        "Identity",
    ]
    .iter()
    .map(|module| subscriptions.for_module(module).len())
    .sum();
    assert_eq!(
        keyed,
        subscriptions.all().len(),
        "every handler is keyed by a module; none is unkeyed"
    );
}

fn descriptor(
    method: &'static str,
    route: &str,
    policies: Vec<&'static str>,
) -> EndpointDescriptor {
    EndpointDescriptor {
        method,
        route: route.to_owned(),
        name: Some(format!("{method} {route}")),
        tag: "Test",
        requires_authorization: !policies.is_empty(),
        policies,
        rate_limit_policy: None,
        description: None,
        responses: Vec::new(),
        validates: None,
    }
}

#[test]
fn endpoint_validation_reports_duplicate_routes_and_unknown_policies() {
    let endpoints = vec![
        descriptor("GET", "/api/catalog/albums", vec![]),
        descriptor("GET", "/API/Catalog/Albums/", vec![]),
        descriptor("POST", "/api/catalog/albums", vec!["music.write"]),
    ];
    let evaluator = module_identity::PolicyRegistry;

    assert_eq!(find_duplicate_routes(&endpoints).len(), 1);
    let unknown = find_unknown_policies(&endpoints, &evaluator);
    assert_eq!(unknown.len(), 1);
    assert!(unknown[0].contains("'music.write'"), "{}", unknown[0]);

    let error = validate_endpoints(&endpoints, &evaluator).unwrap_err();
    assert!(
        error.starts_with("Endpoint validation failed:\n  - "),
        "{error}"
    );
    assert!(!evaluator.is_registered("music.write"));
}
