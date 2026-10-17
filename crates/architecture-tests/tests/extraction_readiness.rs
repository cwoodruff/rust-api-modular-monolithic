//! Port of `ExtractionReadinessTests`: Catalog can leave the process.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;

use architecture_tests::workspace_dependency_graph;

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// Ports `CatalogModule_ReferencesOnlySharedKernelAndContracts`.
#[test]
fn catalog_references_only_the_kernel_and_contracts() {
    let graph = workspace_dependency_graph();
    let catalog = &graph["module-catalog"];
    let allowed = set(&["shared-kernel", "catalog-contracts", "orders-contracts", "identity-contracts"]);

    assert!(catalog.is_subset(&allowed), "module-catalog depends on {catalog:?}");
    assert!(catalog.contains("shared-kernel") && catalog.contains("orders-contracts"));
}

/// Ports `Contracts_ReferenceNothingButSharedKernel`.
#[test]
fn the_contracts_catalog_uses_reference_only_the_kernel() {
    let graph = workspace_dependency_graph();

    for contracts in ["catalog-contracts", "orders-contracts", "identity-contracts"] {
        assert!(graph[contracts].is_subset(&set(&["shared-kernel"])), "{contracts} depends on {:?}", graph[contracts]);
    }
}

/// Ports `CatalogHostSample_ReferencesOnlyCatalogAndSharedKernel`.
///
/// Upstream's csproj lists `Catalog.Module` and `SharedKernel` and reaches the
/// two contracts assemblies transitively. Cargo has no transitive naming, so
/// the sample lists the contracts it names as well; nothing else is allowed.
#[test]
fn the_catalog_host_references_only_catalog_the_kernel_and_their_contracts() {
    let graph = workspace_dependency_graph();
    let Some(sample) = graph.get("catalog-host") else {
        panic!("samples/catalog-host should be a workspace member");
    };

    assert!(
        sample.is_subset(&set(&["module-catalog", "shared-kernel", "identity-contracts", "orders-contracts"])),
        "catalog-host depends on {sample:?}"
    );
    assert!(sample.contains("module-catalog") && sample.contains("shared-kernel"));
}

/// ADR-0019: the HTTP client and the container runner stay out of the
/// application. Only the sample fetches JWKS; only tests start containers.
#[test]
fn the_host_and_modules_pull_in_no_http_client_or_container_runner() {
    for name in ["api", "shared-kernel", "module-admin", "module-catalog", "module-identity", "module-orders", "module-reporting"] {
        let dependencies = architecture_tests::direct_dependencies(name);
        for forbidden in ["reqwest", "testcontainers", "testcontainers-modules"] {
            assert!(!dependencies.contains(forbidden), "{name} depends on {forbidden}");
        }
    }
}
