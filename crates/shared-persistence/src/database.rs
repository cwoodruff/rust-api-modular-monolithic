//! Locating the Chinook database, ported from
//! `PersistenceRegistration.ResolveConnectionString`.
//!
//! The original goes to some length to find the file, because the configured
//! value is the relative `Data Source=./data/chinook.db` and .NET resolves it
//! against the *build output* directory rather than the content root. The
//! search is:
//!
//! 1. Take the configured connection string's data source. Resolve it against
//!    the base directory if it is relative, and use it if the file exists and
//!    is non-empty.
//! 2. Otherwise walk up from the base directory looking for `data/chinook.db`.
//! 3. Otherwise fall back to `{base}/data/chinook.db`, creating the directory.
//!
//! "Exists" is not enough at any step: the file must also be non-empty, which
//! is what stops a zero-byte placeholder left by a previous run from being
//! mistaken for the real database.

use std::path::{Component, Path, PathBuf};

/// The connection string name the original binds.
pub const CONNECTION_NAME: &str = "AppDatabase";

/// The configuration key for that connection string.
///
/// The environment-variable form is `CONNECTIONSTRINGS__APPDATABASE`.
pub const CONNECTION_KEY: &str = "ConnectionStrings:AppDatabase";

/// Where the database sits relative to the repository root.
pub const RELATIVE_DATABASE_PATH: &str = "data/chinook.db";

/// Whether a path points at a database with content.
#[must_use]
pub fn has_usable_database(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|file| file.is_file() && file.len() > 0)
}

/// Extracts the data source from a connection string.
///
/// Accepts the key spellings `SqliteConnectionStringBuilder` accepts — `Data
/// Source`, `DataSource`, and `Filename` — case-insensitively, and tolerates
/// other parameters alongside. A bare path with no `=` is treated as the data
/// source, which is what the builder does.
#[must_use]
pub fn parse_data_source(connection_string: &str) -> Option<String> {
    let trimmed = connection_string.trim();
    if trimmed.is_empty() {
        return None;
    }

    if !trimmed.contains('=') {
        return Some(trimmed.to_owned());
    }

    for parameter in trimmed.split(';') {
        let Some((key, value)) = parameter.split_once('=') else {
            continue;
        };

        let key = key.trim().replace(' ', "").to_lowercase();
        if matches!(key.as_str(), "datasource" | "filename") {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }

    None
}

/// Walks up from `start` looking for `data/chinook.db`.
#[must_use]
pub fn find_database_upward(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);

    while let Some(directory) = current {
        let candidate = directory.join(RELATIVE_DATABASE_PATH);
        if has_usable_database(&candidate) {
            return Some(candidate);
        }
        current = directory.parent();
    }

    None
}

/// Resolves the database path the host should open.
///
/// `base_directory` stands in for .NET's `AppContext.BaseDirectory`.
#[must_use]
pub fn resolve_database_path(configured: Option<&str>, base_directory: &Path) -> PathBuf {
    if let Some(data_source) = configured
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(parse_data_source)
    {
        let candidate = if Path::new(&data_source).is_absolute() {
            PathBuf::from(&data_source)
        } else {
            normalize(&base_directory.join(&data_source))
        };

        if has_usable_database(&candidate) {
            return candidate;
        }
    }

    find_database_upward(base_directory)
        .unwrap_or_else(|| normalize(&base_directory.join(RELATIVE_DATABASE_PATH)))
}

/// Renders a path back into a connection string.
#[must_use]
pub fn connection_string(path: &Path) -> String {
    format!("Data Source={}", path.display())
}

/// Removes `.` components and resolves `..` lexically.
///
/// Not [`std::fs::canonicalize`], which requires the path to exist — this has
/// to work for the fallback path, which by definition does not.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component);
                }
            }
            other => normalized.push(other),
        }
    }

    normalized
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// The repository root, found by walking up from this crate.
    fn repository_root() -> PathBuf {
        let mut current = Path::new(env!("CARGO_MANIFEST_DIR"));
        while !current.join(RELATIVE_DATABASE_PATH).exists() {
            current = current.parent().expect("the bundled database should exist");
        }
        current.to_path_buf()
    }

    struct TempFile(PathBuf);

    impl TempFile {
        fn with_size(label: &str, bytes: usize) -> Self {
            let path = std::env::temp_dir()
                .join(format!("chinook-probe-{}-{label}.db", std::process::id()));
            std::fs::write(&path, vec![0_u8; bytes]).expect("temp file should be writable");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn the_probe_finds_the_bundled_database() {
        // The acceptance check for this phase.
        let found = find_database_upward(Path::new(env!("CARGO_MANIFEST_DIR")))
            .expect("data/chinook.db should be found by walking up from the crate");

        assert!(found.ends_with("data/chinook.db"));
        assert!(has_usable_database(&found));
    }

    #[test]
    fn an_empty_file_does_not_count_as_a_database() {
        let empty = TempFile::with_size("empty", 0);
        let populated = TempFile::with_size("populated", 16);

        assert!(!has_usable_database(empty.path()));
        assert!(has_usable_database(populated.path()));
    }

    #[test]
    fn a_missing_file_does_not_count_either() {
        assert!(!has_usable_database(Path::new(
            "/nonexistent/data/chinook.db"
        )));
    }

    #[test]
    fn a_directory_does_not_count_as_a_database() {
        assert!(!has_usable_database(Path::new(env!("CARGO_MANIFEST_DIR"))));
    }

    #[test]
    fn data_sources_parse_from_every_spelling_the_builder_accepts() {
        for connection in [
            "Data Source=./data/chinook.db",
            "DataSource=./data/chinook.db",
            "data source=./data/chinook.db",
            "Filename=./data/chinook.db",
            "Data Source=./data/chinook.db;Cache=Shared",
            "Cache=Shared;Data Source=./data/chinook.db",
        ] {
            assert_eq!(
                parse_data_source(connection).as_deref(),
                Some("./data/chinook.db"),
                "failed on {connection:?}"
            );
        }
    }

    #[test]
    fn a_bare_path_is_taken_as_the_data_source() {
        assert_eq!(
            parse_data_source("/var/db/chinook.db").as_deref(),
            Some("/var/db/chinook.db")
        );
    }

    #[test]
    fn a_connection_string_without_a_data_source_yields_nothing() {
        assert_eq!(parse_data_source("Cache=Shared"), None);
        assert_eq!(parse_data_source(""), None);
        assert_eq!(parse_data_source("   "), None);
        assert_eq!(parse_data_source("Data Source="), None);
    }

    #[test]
    fn a_configured_relative_path_resolves_against_the_base_directory() {
        // The real configured value, resolved the way .NET resolves it.
        let root = repository_root();

        let resolved = resolve_database_path(Some("Data Source=./data/chinook.db"), &root);

        assert_eq!(resolved, root.join("data/chinook.db"));
        assert!(
            !resolved.to_string_lossy().contains("/./"),
            "the `.` component should be normalized away, got {}",
            resolved.display()
        );
    }

    #[test]
    fn a_configured_absolute_path_is_used_as_given() {
        let root = repository_root();
        let absolute = root.join("data/chinook.db");

        let resolved = resolve_database_path(
            Some(&format!("Data Source={}", absolute.display())),
            Path::new("/somewhere/else"),
        );

        assert_eq!(resolved, absolute);
    }

    #[test]
    fn an_unusable_configured_path_falls_back_to_the_upward_walk() {
        // This is the case that actually fires in the original: the configured
        // relative path misses because it resolves against the build output
        // directory, and the walk upward rescues it.
        let deep = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/validation");

        let resolved = resolve_database_path(Some("Data Source=./missing/nowhere.db"), &deep);

        assert_eq!(resolved, repository_root().join("data/chinook.db"));
    }

    #[test]
    fn with_nothing_configured_the_walk_still_finds_it() {
        let resolved = resolve_database_path(None, Path::new(env!("CARGO_MANIFEST_DIR")));

        assert_eq!(resolved, repository_root().join("data/chinook.db"));
    }

    #[test]
    fn a_hopeless_search_falls_back_to_the_conventional_location() {
        let nowhere = Path::new("/nonexistent/base");

        let resolved = resolve_database_path(None, nowhere);

        assert_eq!(resolved, nowhere.join("data/chinook.db"));
    }

    #[test]
    fn paths_render_back_into_a_connection_string() {
        assert_eq!(
            connection_string(Path::new("/srv/app/data/chinook.db")),
            "Data Source=/srv/app/data/chinook.db"
        );
    }

    #[test]
    fn normalizing_resolves_dot_segments_without_touching_the_disk() {
        assert_eq!(
            normalize(Path::new("/srv/app/./data/../data/chinook.db")),
            PathBuf::from("/srv/app/data/chinook.db")
        );
    }
}
