//! Startup checks on the mapped endpoints, ported from `ModuleComposition`.
//!
//! Two mistakes compile cleanly and fail only when a request arrives: two
//! endpoints claiming one route and method, and an endpoint requiring a policy
//! nobody registered. The second is exactly what the Music-to-Catalog rename
//! produced upstream for one commit. In Development and Test the host refuses
//! to start, listing every problem; the guardrail tests run the same checks.

use std::collections::BTreeMap;

use shared_kernel::auth::PolicyEvaluator;
use shared_kernel::endpoints::EndpointDescriptor;

/// Every route and method mapped by more than one endpoint.
///
/// Routes compare as upstream compares them: `/` plus the route with its
/// slashes trimmed, case-insensitively — so `/api/catalog/albums` and
/// `/API/Catalog/Albums/` collide.
#[must_use]
pub fn find_duplicate_routes(endpoints: &[EndpointDescriptor]) -> Vec<String> {
    let mut groups: BTreeMap<(String, &str), Vec<String>> = BTreeMap::new();

    for endpoint in endpoints {
        let route = endpoint
            .normalized_route()
            .replace(":int", "")
            .replace(":guid", "");
        groups.entry((route, endpoint.method)).or_default().push(
            endpoint
                .name
                .clone()
                .unwrap_or_else(|| endpoint.route.clone()),
        );
    }

    groups
        .into_iter()
        .filter(|(_, names)| names.len() > 1)
        .map(|((route, method), names)| {
            format!(
                "{method} {route} is mapped by {} endpoints: {}",
                names.len(),
                names.join(", ")
            )
        })
        .collect()
}

/// Every policy an endpoint requires that `policies` does not know.
#[must_use]
pub fn find_unknown_policies(
    endpoints: &[EndpointDescriptor],
    policies: &dyn PolicyEvaluator,
) -> Vec<String> {
    let mut unknown: BTreeMap<&str, Vec<String>> = BTreeMap::new();

    for endpoint in endpoints {
        for policy in &endpoint.policies {
            if !policy.is_empty() && !policies.is_registered(policy) {
                let names = unknown.entry(policy).or_default();
                let name = endpoint
                    .name
                    .clone()
                    .unwrap_or_else(|| endpoint.route.clone());
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
    }

    unknown
        .into_iter()
        .map(|(policy, names)| {
            format!(
                "Policy '{policy}' is not registered but is required by: {}",
                names.join(", ")
            )
        })
        .collect()
}

/// Both checks.
///
/// # Errors
///
/// `Endpoint validation failed:` and one line per problem.
pub fn validate_endpoints(
    endpoints: &[EndpointDescriptor],
    policies: &dyn PolicyEvaluator,
) -> Result<(), String> {
    let problems: Vec<String> = find_duplicate_routes(endpoints)
        .into_iter()
        .chain(find_unknown_policies(endpoints, policies))
        .collect();

    if problems.is_empty() {
        return Ok(());
    }

    Err(format!(
        "Endpoint validation failed:\n{}",
        problems
            .iter()
            .map(|problem| format!("  - {problem}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}
