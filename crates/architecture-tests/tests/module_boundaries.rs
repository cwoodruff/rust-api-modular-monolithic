//! Port of the C# `ModuleBoundaryTests` and `SharedKernelDependencyTests`.
//!
//! These read the real dependency graph out of `cargo metadata` rather than
//! trusting a hand-maintained list, so a stray `shared-data-sqlite` line added
//! to a module's `Cargo.toml` fails the build the same way a stray assembly
//! reference failed the C# build.

#![allow(clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

/// The host crate — the composition root, allowed to reference everything.
const HOST: &str = "api";

/// The test crate itself, which sits outside the layering rules.
const ARCHITECTURE_TESTS: &str = "architecture-tests";

const SHARED_KERNEL: &str = "shared-kernel";
const SHARED_PERSISTENCE: &str = "shared-persistence";
const SHARED_DATA_SQLITE: &str = "shared-data-sqlite";

const SHARED_CRATES: [&str; 3] = [SHARED_KERNEL, SHARED_PERSISTENCE, SHARED_DATA_SQLITE];

const MODULES: [&str; 5] = [
    "module-admin",
    "module-identity",
    "module-music",
    "module-orders",
    "module-reporting",
];

/// The only workspace crates a module crate may depend on. `shared-data-sqlite`
/// is deliberately absent: modules see repository *traits*, never a driver.
const MODULE_ALLOWED_DEPENDENCIES: [&str; 2] = [SHARED_KERNEL, SHARED_PERSISTENCE];

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
    expected.extend(SHARED_CRATES);
    expected.insert(HOST);
    expected.insert(ARCHITECTURE_TESTS);

    let actual: BTreeSet<&str> = graph.keys().map(String::as_str).collect();

    assert_eq!(
        expected, actual,
        "the workspace membership drifted from the layout in docs/rust-translation-plan.md"
    );
}

/// Ports `ModuleBoundaryTests`: the C# theory asserted every ordered pair of
/// module assemblies, so this walks the same 20 pairs.
#[test]
fn modules_never_depend_on_each_other() {
    let graph = workspace_dependency_graph();

    for source in MODULES {
        let dependencies = dependencies_of(&graph, source);

        for target in MODULES {
            if source == target {
                continue;
            }

            assert!(
                !dependencies.contains(target),
                "`{source}` depends on `{target}`; modules must communicate through \
                 the shared kernel, never directly"
            );
        }
    }
}

#[test]
fn modules_depend_only_on_the_shared_contracts() {
    let graph = workspace_dependency_graph();
    let allowed: BTreeSet<&str> = MODULE_ALLOWED_DEPENDENCIES.into_iter().collect();

    for module in MODULES {
        for dependency in dependencies_of(&graph, module) {
            assert!(
                allowed.contains(dependency.as_str()),
                "`{module}` depends on `{dependency}`, which is outside the allowed set \
                 {MODULE_ALLOWED_DEPENDENCIES:?}"
            );
        }
    }
}

/// Ports `SharedKernelDependencyTests`: the shared crates are the leaves of the
/// graph and must never reach back into a module.
#[test]
fn shared_crates_never_depend_on_modules() {
    let graph = workspace_dependency_graph();

    for shared in SHARED_CRATES {
        let dependencies = dependencies_of(&graph, shared);

        for module in MODULES {
            assert!(
                !dependencies.contains(module),
                "`{shared}` depends on `{module}`; the shared crates must stay \
                 module-agnostic"
            );
        }
    }
}

#[test]
fn shared_kernel_is_the_root_of_the_graph() {
    let graph = workspace_dependency_graph();

    assert!(
        dependencies_of(&graph, SHARED_KERNEL).is_empty(),
        "`{SHARED_KERNEL}` must not depend on any other workspace crate"
    );
}

/// The database driver is an implementation detail of the host. Modules reach
/// persistence through the traits in `shared-persistence`.
#[test]
fn only_the_host_depends_on_the_sqlite_implementation() {
    let graph = workspace_dependency_graph();

    for (crate_name, dependencies) in &graph {
        if crate_name == HOST || crate_name == SHARED_DATA_SQLITE {
            continue;
        }

        assert!(
            !dependencies.contains(SHARED_DATA_SQLITE),
            "`{crate_name}` depends on `{SHARED_DATA_SQLITE}`; only `{HOST}` may bind \
             the concrete database driver"
        );
    }
}
