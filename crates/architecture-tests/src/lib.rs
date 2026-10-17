//! Port of the C# `ModularMonolith.Architecture.Tests` project.
//!
//! Upstream reads assemblies with ArchUnitNET and reflection; this reads the
//! crate graph from `cargo metadata`, the crates' own sources, and the
//! endpoint descriptors of a host built exactly as the binary builds it.
//!
//! - `module_boundaries.rs` — `ModuleBoundaryTests`, `SharedKernelDependencyTests`.
//! - `guardrails.rs` — `GuardrailTests`: policies registered and compiled, one
//!   rate limit per module group, unique routes, handlers keyed by module, and
//!   (port-only) every unguarded endpoint named and justified.
//! - `fences.rs` — `FenceTests`: contracts hold data and constants only; the
//!   kernel's public surface stays within its budget and names the database
//!   driver in one file.
//! - `extraction_readiness.rs` — `ExtractionReadinessTests`.
//! - `public_surface.rs` — `PublicSurfaceTests`: each module exports its
//!   composition type and constants, nothing else.
//!
//! This library holds the helpers they share.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The workspace root.
///
/// # Panics
///
/// Outside a workspace, which cannot happen for this crate.
#[must_use]
pub fn workspace_root() -> PathBuf {
    let mut current = Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        let manifest = current.join("Cargo.toml");
        if manifest.is_file()
            && std::fs::read_to_string(&manifest).is_ok_and(|text| text.contains("[workspace]"))
        {
            return current.to_path_buf();
        }
        current = current.parent().unwrap_or_else(|| panic!("no workspace above {}", current.display()));
    }
}

/// Every workspace crate and the workspace crates it depends on (normal
/// dependencies only — tests do not express layering).
///
/// # Panics
///
/// When `cargo metadata` cannot run.
#[must_use]
pub fn workspace_dependency_graph() -> BTreeMap<String, BTreeSet<String>> {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1", "--manifest-path"])
        .arg(workspace_root().join("Cargo.toml"))
        .output()
        .unwrap_or_else(|error| panic!("`cargo metadata` should run: {error}"));
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("metadata should be JSON: {error}"));

    metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|package| {
            let name = package["name"].as_str().unwrap_or_default().to_owned();
            let dependencies = package["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|dependency| dependency["kind"].is_null() && dependency["path"].is_string())
                .filter_map(|dependency| dependency["name"].as_str().map(ToOwned::to_owned))
                .collect();
            (name, dependencies)
        })
        .collect()
}

/// Every normal dependency a crate names, workspace or not.
///
/// # Panics
///
/// When `cargo metadata` cannot run or the crate is unknown.
#[must_use]
pub fn direct_dependencies(crate_name: &str) -> BTreeSet<String> {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1", "--manifest-path"])
        .arg(workspace_root().join("Cargo.toml"))
        .output()
        .unwrap_or_else(|error| panic!("`cargo metadata` should run: {error}"));
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("metadata should be JSON: {error}"));

    metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|package| package["name"] == crate_name)
        .unwrap_or_else(|| panic!("`{crate_name}` should be a workspace member"))["dependencies"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|dependency| dependency["kind"].is_null())
        .filter_map(|dependency| dependency["name"].as_str().map(ToOwned::to_owned))
        .collect()
}

/// Every `.rs` file under a crate's `src/`, with its text, test modules removed.
///
/// A `#[cfg(test)]` module is cut from its attribute to the end of the file,
/// which is where every crate here keeps them.
///
/// # Panics
///
/// When the crate's sources cannot be read.
#[must_use]
pub fn sources(crate_dir: &str) -> Vec<(PathBuf, String)> {
    let root = workspace_root().join(crate_dir).join("src");
    let mut files = Vec::new();
    let mut pending = vec![root];

    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory).unwrap_or_else(|error| panic!("{}: {error}", directory.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                let production = text.split("#[cfg(test)]").next().unwrap_or_default().to_owned();
                files.push((path, production));
            }
        }
    }

    files.sort();
    files
}
