//! Port of the C# `SharedKernel.Persistence` project.
//!
//! Holds the shapes and contracts, never a concrete database driver — the sqlx
//! implementations live in `shared-data-sqlite` so that module crates can
//! depend on the contracts without seeing SQLite at all.
//!
//! Lands in Phase 2 (see `docs/rust-translation-plan.md`):
//!
//! - `entities` — the 12 Chinook tables.
//! - `api_models` — the wire shapes, including the denormalized name fields and
//!   the by-id-returns-a-graph / collections-return-flat asymmetry.
//! - `validation` — the FluentValidation rules, preserving their subtle
//!   semantics (length and regex rules pass on null unless NotNull is chained).
//! - `repositories` — one trait per entity, mirroring the C# interfaces.

/// The connection-string key the original binds, kept so an existing
/// `ConnectionStrings__AppDatabase` environment variable works unchanged.
pub const CONNECTION_STRING_NAME: &str = "AppDatabase";

/// Path of the bundled Chinook database, relative to the repository root.
///
/// Phase 2 adds the parent-directory walk the C# host performs when the
/// configured connection string is absent or points at a missing file.
pub const BUNDLED_DATABASE_PATH: &str = "data/chinook.db";
