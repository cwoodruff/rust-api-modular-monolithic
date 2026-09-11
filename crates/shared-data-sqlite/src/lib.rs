//! Port of the C# `SharedKernel.DataSQLite` project.
//!
//! Only the host (`api`) may depend on this crate — module crates see the
//! repository *traits* from `shared-persistence` and nothing more. The
//! architecture tests enforce that edge.
//!
//! # Why the SQL is written by hand
//!
//! The C# repositories contain no SQL at all: despite the project name, they
//! are EF Core LINQ over `AppDbContext`, with `Microsoft.Data.Sqlite` appearing
//! only in the connection-string builder. There was nothing to lift, so every
//! statement here is re-derived from the LINQ it replaces.
//!
//! Where the original made a deliberate query-shape choice, this follows it:
//!
//! - **Album and artist by id** run several statements rather than one wide
//!   join, which is what `AsSplitQuery()` does. The original's comment calls it
//!   "important on SQLite to avoid cartesian explosion".
//! - **Playlist by id** runs two lean statements — a header, then the tracks —
//!   rather than materializing an entity graph.
//! - **Tracks by invoice** uses an `EXISTS` subquery, matching
//!   `Where(t => t.InvoiceLines.Any(l => l.InvoiceId == id))` and avoiding the
//!   duplicates a join would produce.
//!
//! Statements also carry an explicit `ORDER BY "Id"`. EF Core emits none, and
//! SQLite then returns rows in rowid order, which for these tables is key
//! order — so this pins the order the original gets by accident.
//!
//! Queries are prepared at runtime rather than through sqlx's compile-time
//! macros, which would require a live database or a checked-in metadata cache
//! at build time. Nothing else in the workspace needs a database to compile,
//! and that is worth keeping.

pub mod pool;
pub mod repositories;
mod rows;

pub use pool::{MAX_POOL_CONNECTIONS, create_pool};
pub use repositories::{
    SqliteAlbumRepository, SqliteArtistRepository, SqliteCustomerRepository,
    SqliteEmployeeRepository, SqliteGenreRepository, SqliteInvoiceLineRepository,
    SqliteInvoiceRepository, SqliteMediaTypeRepository, SqlitePlaylistRepository,
    SqliteTrackRepository,
};

#[cfg(test)]
mod tests {
    /// Shared fixtures for the tests in this crate.
    pub(crate) mod support {
        use std::path::{Path, PathBuf};

        /// The bundled database, found by walking up from this crate.
        pub(crate) fn bundled_database() -> PathBuf {
            let mut current = Path::new(env!("CARGO_MANIFEST_DIR"));

            loop {
                let candidate = current.join("data/chinook.db");
                if candidate.is_file() {
                    return candidate;
                }

                current = current.parent().unwrap_or_else(|| {
                    panic!("data/chinook.db should be bundled in the repository")
                });
            }
        }
    }
}
