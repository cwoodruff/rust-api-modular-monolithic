//! Port of the C# `ModularMonolith.Api` host.
//!
//! The composition root exists and knows every module, and it resolves the
//! database the way the real host will. The axum router, middleware stack, and
//! connection pool land in Phase 4.

use std::path::PathBuf;

use shared_persistence::database;

/// The module registry, mirroring the hard-coded `GetModules()` list in
/// `Program.cs`. Order is the original's: Administration, Identity, Music,
/// Orders, Reporting.
const MODULES: [(&str, &str); 5] = [
    (module_admin::NAME, module_admin::PREFIX),
    (module_identity::NAME, module_identity::PREFIX),
    (module_music::NAME, module_music::PREFIX),
    (module_orders::NAME, module_orders::PREFIX),
    (module_reporting::NAME, module_reporting::PREFIX),
];

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    println!("Modular Monolith API — Rust port (Phase 2)\n");

    println!("Modules registered:");
    for (name, prefix) in MODULES {
        println!("  {name:<16} {prefix}");
    }

    // Phase 4 will read the configured connection string here; until then the
    // probe runs with nothing configured, which is the path the original falls
    // back to anyway.
    let working_directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let resolved = database::resolve_database_path(None, &working_directory);

    println!("\nChinook database: {}", resolved.display());
    if !database::has_usable_database(&resolved) {
        println!("  (not found — the host would create the directory and open an empty file)");
    }

    println!("\nThe HTTP host lands in Phase 4; see docs/rust-translation-plan.md.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_registry_matches_the_original_prefixes() {
        let prefixes: Vec<&str> = MODULES.iter().map(|(_, prefix)| *prefix).collect();

        assert_eq!(
            prefixes,
            vec![
                "/api/admin",
                "/api/identity",
                "/api/music",
                "/api/orders",
                "/api/reporting"
            ]
        );
    }

    #[test]
    fn bundled_database_ships_with_the_repository() {
        let crate_directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        let resolved = database::resolve_database_path(None, crate_directory);

        assert!(
            database::has_usable_database(&resolved),
            "data/chinook.db should be bundled in the repository, as it is in the C# original; \
             resolved to {}",
            resolved.display()
        );
    }
}
