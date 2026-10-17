//! Port of `FenceTests` (ADR-0012): what the kernel and the contracts may hold.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use architecture_tests::sources;

const CONTRACTS: [&str; 4] = [
    "crates/administration-contracts",
    "crates/catalog-contracts",
    "crates/identity-contracts",
    "crates/orders-contracts",
];

/// Public types the kernel may export: `pub struct`, `pub enum`, `pub trait`
/// and `pub type` items at any module level. Raising it is a decision the
/// reviewer sees in this line (ADR-0012).
///
/// Rust counts differently from .NET — a module path is not a type, and a
/// trait object alias is — so this is the port's own number, set when the
/// upgrade landed — the count then, plus two, as upstream set its own — not
/// upstream's 30.
const KERNEL_PUBLIC_TYPE_BUDGET: usize = 76;

/// Ports `Contracts_Contain_Only_Interfaces_Enums_Records_And_Constant_Holders`.
///
/// A contracts crate may declare constants, data types and the
/// `IntegrationEvent` accessors its events need. A function with a body
/// anywhere else is behavior, and behavior belongs to the module that owns it.
#[test]
fn contracts_hold_only_data_and_constants() {
    for crate_dir in CONTRACTS {
        for (path, text) in sources(crate_dir) {
            let mut in_event_impl = false;
            let mut depth = 0_i32;

            for line in text.lines() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("impl IntegrationEvent for") {
                    in_event_impl = true;
                    depth = 0;
                }
                if in_event_impl {
                    depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
                    if depth <= 0 && line.contains('}') {
                        in_event_impl = false;
                    }
                    continue;
                }

                assert!(
                    !(trimmed.starts_with("fn ")
                        || trimmed.starts_with("pub fn ")
                        || trimmed.starts_with("pub(crate) fn ")),
                    "{} has a function outside an IntegrationEvent impl: {trimmed}",
                    path.display()
                );
                assert!(
                    !trimmed.starts_with("static ") && !trimmed.starts_with("pub static "),
                    "{} holds mutable or computed state: {trimmed}",
                    path.display()
                );
            }
        }
    }
}

fn public_types(crate_dir: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (path, text) in sources(crate_dir) {
        for line in text.lines() {
            let trimmed = line.trim_start();
            for keyword in ["pub struct ", "pub enum ", "pub trait ", "pub type "] {
                if let Some(rest) = trimmed.strip_prefix(keyword) {
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    found.push(format!(
                        "{}::{name}",
                        path.file_stem().unwrap().to_string_lossy()
                    ));
                }
            }
        }
    }
    found
}

/// Ports `SharedKernel_Public_Surface_Stays_Within_Budget`.
#[test]
fn the_kernels_public_surface_stays_within_budget() {
    let types = public_types("crates/shared-kernel");

    assert!(
        types.len() > 30,
        "the scan found only {} types; is it reading the kernel?",
        types.len()
    );
    assert!(
        types.len() <= KERNEL_PUBLIC_TYPE_BUDGET,
        "shared-kernel exports {} public types, over its budget of {KERNEL_PUBLIC_TYPE_BUDGET}: {types:#?}",
        types.len()
    );
}

/// Ports `SharedKernel_Uses_Npgsql_Only_From_Its_Persistence_Namespace`.
///
/// The PostgreSQL driver's own types — `PgPool`, `PgConnection`, `Postgres`,
/// anything under `sqlx::postgres` — appear in `persistence.rs` and nowhere
/// else; every other file reaches them through its aliases.
#[test]
fn the_kernel_names_the_database_driver_in_one_file() {
    let forbidden = [
        "sqlx::postgres",
        "sqlx::Postgres",
        "PgPool",
        "PgConnection",
        "PgRow",
        "PgConnectOptions",
    ];

    for (path, text) in sources("crates/shared-kernel") {
        if path
            .file_name()
            .is_some_and(|name| name == "persistence.rs")
        {
            continue;
        }
        for name in forbidden {
            assert!(
                !text.contains(name),
                "{} names `{name}`; use the aliases in persistence.rs",
                path.display()
            );
        }
    }
}

/// Ports `Contracts_Grant_No_InternalsVisibleTo`: Rust has no such grant, so
/// the counterpart is that a contracts crate keeps nothing `pub(crate)` for a
/// friend to reach — everything in it is either public or private.
#[test]
fn contracts_expose_everything_they_declare() {
    for crate_dir in CONTRACTS {
        for (path, text) in sources(crate_dir) {
            assert!(
                !text.contains("pub(crate)"),
                "{} hides part of a contract",
                path.display()
            );
        }
    }
}
