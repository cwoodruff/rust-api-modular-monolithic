//! Port of the C# `ModuleBoundaryTests` and `SharedKernelDependencyTests`.
//!
//! These read the real dependency graph out of `cargo metadata` rather than
//! trusting a hand-maintained list, so a stray `module-orders` line added
//! to a module's `Cargo.toml` fails the build the same way a stray assembly
//! reference failed the C# build.

#![allow(clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

/// The host crate — the composition root, allowed to reference everything.
const HOST: &str = "api";

/// The test crates, which sit outside the layering rules.
const TEST_CRATES: [&str; 3] = ["architecture-tests", "module-tests", "test-support"];

/// Port-only: the sample that runs Catalog on its own.
const SAMPLES: [&str; 1] = ["catalog-host"];

const SHARED_KERNEL: &str = "shared-kernel";

/// FluentValidation's counterpart: a library modules use, never the kernel.
const VALIDATION_RULES: &str = "validation-rules";

const MODULES: [&str; 5] = [
    "module-admin",
    "module-catalog",
    "module-identity",
    "module-orders",
    "module-reporting",
];

const CONTRACTS: [&str; 4] = [
    "administration-contracts",
    "catalog-contracts",
    "identity-contracts",
    "orders-contracts",
];

/// The only workspace crates a module crate may depend on: the kernel, any
/// contracts crate, and the rules library.
fn module_may_depend_on(name: &str) -> bool {
    name == SHARED_KERNEL || name == VALIDATION_RULES || CONTRACTS.contains(&name)
}

/// Maps every workspace crate to the set of workspace crates it depends on.
///
/// Only normal dependencies are considered — dev-dependencies exist to support
/// tests and do not express production layering.
fn workspace_dependency_graph() -> BTreeMap<String, BTreeSet<String>> {
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"),
        ])
        .output()
        .expect("`cargo metadata` should be runnable from the test harness");

    assert!(
        output.status.success(),
        "`cargo metadata` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("`cargo metadata` should emit valid JSON");

    let packages = metadata["packages"]
        .as_array()
        .expect("metadata should contain a packages array");

    let mut graph = BTreeMap::new();

    for package in packages {
        let name = package["name"]
            .as_str()
            .expect("every package should have a name")
            .to_owned();

        let dependencies = package["dependencies"]
            .as_array()
            .expect("every package should have a dependencies array")
            .iter()
            // `kind` is null for normal dependencies, "dev" or "build" otherwise.
            .filter(|dependency| dependency["kind"].is_null())
            // A path dependency is by definition inside this workspace.
            .filter(|dependency| dependency["path"].is_string())
            .filter_map(|dependency| dependency["name"].as_str())
            .map(str::to_owned)
            .collect();

        graph.insert(name, dependencies);
    }

    graph
}

fn dependencies_of<'a>(
    graph: &'a BTreeMap<String, BTreeSet<String>>,
    crate_name: &str,
) -> &'a BTreeSet<String> {
    graph
        .get(crate_name)
        .unwrap_or_else(|| panic!("`{crate_name}` should be a workspace member"))
}

#[test]
fn every_expected_crate_is_a_workspace_member() {
    let graph = workspace_dependency_graph();

    let mut expected: BTreeSet<&str> = MODULES.into_iter().collect();
    expected.extend(CONTRACTS);
    expected.extend(TEST_CRATES);
    expected.extend(SAMPLES);
    expected.extend([SHARED_KERNEL, VALIDATION_RULES, HOST]);

    let actual: BTreeSet<&str> = graph.keys().map(String::as_str).collect();

    // Crates still to land in later steps are allowed to be absent; nothing
    // unexpected is allowed to be present.
    for name in &actual {
        assert!(
            expected.contains(name),
            "`{name}` is not in the layout docs/upgrade-plan.md describes"
        );
    }
    for name in MODULES
        .iter()
        .chain(CONTRACTS.iter())
        .chain([&SHARED_KERNEL, &HOST])
    {
        assert!(
            actual.contains(name),
            "`{name}` should be a workspace member"
        );
    }
}

/// Ports `Module_Should_Not_Depend_On_Other_Module`: all 20 ordered pairs.
#[test]
fn modules_never_depend_on_each_other() {
    let graph = workspace_dependency_graph();

    for source in MODULES {
        let dependencies = dependencies_of(&graph, source);

        for target in MODULES {
            if source != target {
                assert!(
                    !dependencies.contains(target),
                    "`{source}` depends on `{target}`; modules talk through contracts and events, never directly"
                );
            }
        }
    }
}

/// Ports `Module_Should_Only_Depend_On_SharedKernel_And_Framework`.
#[test]
fn modules_depend_only_on_the_kernel_and_contracts() {
    let graph = workspace_dependency_graph();

    for module in MODULES {
        for dependency in dependencies_of(&graph, module) {
            assert!(
                module_may_depend_on(dependency),
                "`{module}` depends on `{dependency}`; a module may reference only shared-kernel, \
                 validation-rules and *-contracts crates"
            );
        }
    }
}

/// Ports `Contracts_Should_Not_Depend_On_Any_Module`: 4 × 5 cases.
#[test]
fn contracts_never_depend_on_modules() {
    let graph = workspace_dependency_graph();

    for contract in CONTRACTS {
        for dependency in dependencies_of(&graph, contract) {
            assert_eq!(
                dependency, SHARED_KERNEL,
                "`{contract}` depends on `{dependency}`; contracts reference nothing but the kernel"
            );
        }
    }
}

/// Ports `SharedKernel_Should_Not_Depend_On_Any_Module_Or_Contracts`.
#[test]
fn shared_kernel_is_the_root_of_the_graph() {
    let graph = workspace_dependency_graph();

    assert!(
        dependencies_of(&graph, SHARED_KERNEL).is_empty(),
        "`{SHARED_KERNEL}` must not depend on any other workspace crate"
    );
}

/// Ports the fence that SharedKernel references no FluentValidation: the
/// rules library is the modules' to use.
#[test]
fn the_validation_library_depends_on_nothing_in_the_workspace() {
    let graph = workspace_dependency_graph();

    assert!(dependencies_of(&graph, VALIDATION_RULES).is_empty());
}
