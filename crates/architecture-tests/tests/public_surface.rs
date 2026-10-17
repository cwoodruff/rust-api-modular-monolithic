//! Port of `PublicSurfaceTests`: a module exports its composition type and
//! its constants, and nothing else.
//!
//! The compiler already refuses a reach into a `pub(crate)` item; this pins
//! what each crate chose to make `pub`, so widening a module's surface is a
//! change someone sees.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;

use architecture_tests::workspace_root;

/// Top-level `pub` items in a crate's `lib.rs`.
fn exported(crate_dir: &str) -> BTreeSet<String> {
    let text = std::fs::read_to_string(workspace_root().join(crate_dir).join("src/lib.rs")).unwrap();
    let production = text.split("#[cfg(test)]").next().unwrap_or_default();

    production
        .lines()
        .filter(|line| line.starts_with("pub "))
        .map(|line| {
            line.trim_end_matches(['{', ';', ' '])
                .split(['=', ':', '('])
                .next()
                .unwrap_or(line)
                .trim()
                .to_owned()
        })
        .collect()
}

fn assert_exports(crate_dir: &str, allowed: &[&str]) {
    let allowed: BTreeSet<String> = allowed.iter().map(|item| (*item).to_owned()).collect();
    let exported = exported(crate_dir);
    let extra: Vec<&String> = exported.difference(&allowed).collect();

    assert!(extra.is_empty(), "{crate_dir} exports more than its composition surface: {extra:?}");
}

#[test]
fn catalog_exports_only_its_composition_surface() {
    assert_exports(
        "crates/module-catalog",
        &["pub struct CatalogModule", "pub const NAME", "pub const PREFIX", "pub const SCHEMA", "pub const TAG", "pub const DEFAULT_CACHE_SIZE"],
    );
}

#[test]
fn orders_exports_only_its_composition_surface() {
    assert_exports(
        "crates/module-orders",
        &[
            "pub struct OrdersModule",
            "pub const NAME",
            "pub const PREFIX",
            "pub const SCHEMA",
            "pub const TAG",
            "pub const DEFAULT_CACHE_SIZE",
            "pub const DISPATCHER_NAME",
        ],
    );
}

#[test]
fn administration_exports_only_its_composition_surface() {
    assert_exports(
        "crates/module-admin",
        &["pub struct AdministrationModule", "pub const NAME", "pub const PREFIX", "pub const SCHEMA", "pub const TAG", "pub const DEFAULT_CACHE_SIZE"],
    );
}

#[test]
fn reporting_exports_only_its_composition_surface() {
    assert_exports(
        "crates/module-reporting",
        &["pub struct ReportingModule", "pub const NAME", "pub const PREFIX", "pub const SCHEMA", "pub const TAG", "pub const CONNECTION_NAME"],
    );
}

/// Identity also exports its auth wiring, as upstream's `IdentityAuthExtensions`
/// is public: the key providers a host chooses between, the token service the
/// tests drive, and the policy registry `catalog-host`'s copy mirrors.
#[test]
fn identity_exports_its_composition_surface_and_auth_wiring() {
    assert_exports(
        "crates/module-identity",
        &[
            "pub struct IdentityModule",
            "pub const NAME",
            "pub const PREFIX",
            "pub const TAG",
            "pub const DEFAULT_ACCESS_TOKEN_MINUTES",
            "pub const DEFAULT_REFRESH_TOKEN_DAYS",
            "pub mod claims",
            "pub mod keys",
            "pub mod options",
            "pub mod runtime",
            "pub mod stores",
            "pub mod tokens",
            "pub use diagnostics",
            "pub use keys",
            "pub use options",
            "pub use policies",
            "pub use runtime",
            "pub use tokens",
        ],
    );
}
