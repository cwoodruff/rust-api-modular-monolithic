//! Port of the C# `ModularMonolith.Api` host.
//!
//! Phase 0 scaffold: the composition root exists and knows every module, but
//! the axum router, middleware stack, and database pool land in Phase 4. Run it
//! to confirm the workspace is wired and the bundled Chinook database is where
//! the host will look for it.

use std::path::{Path, PathBuf};

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

/// Locates the bundled database the way Phase 2's full probe will: start at the
/// working directory and walk upward looking for `data/chinook.db`, accepting
/// only a file with content.
fn find_bundled_database(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);

    while let Some(directory) = current {
        let candidate = directory.join(shared_persistence::BUNDLED_DATABASE_PATH);
        if candidate.metadata().is_ok_and(|file| file.len() > 0) {
            return Some(candidate);
        }
        current = directory.parent();
    }

    None
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    println!("Modular Monolith API — Rust port (Phase 0 scaffold)\n");

    println!("Modules registered:");
    for (name, prefix) in MODULES {
        println!("  {name:<16} {prefix}");
    }

    let working_directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match find_bundled_database(&working_directory) {
        Some(path) => println!("\nChinook database: {}", path.display()),
        None => println!(
            "\nChinook database: not found (expected {} at or above {})",
            shared_persistence::BUNDLED_DATABASE_PATH,
            working_directory.display()
        ),
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
        let crate_directory = Path::new(env!("CARGO_MANIFEST_DIR"));

        assert!(
            find_bundled_database(crate_directory).is_some(),
            "data/chinook.db should be bundled in the repository, as it is in the C# original"
        );
    }
}
