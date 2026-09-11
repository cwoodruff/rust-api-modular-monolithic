//! Port of the C# `SharedKernel.Persistence` project.
//!
//! Holds the shapes and the contracts, never a concrete database driver: the
//! sqlx implementations live in `shared-data-sqlite` so module crates can
//! depend on repository traits without seeing SQLite at all.
//!
//! - [`entities`] — the twelve Chinook tables, as row shapes.
//! - [`api_models`] — the wire shapes, including the denormalized name fields.
//! - [`convert`] — conversions between the two, in both directions.
//! - [`validation`] — the ten validators, with FluentValidation's semantics.
//! - [`repositories`] — one trait per entity.
//! - [`database`] — locating the bundled Chinook file.
//! - [`state`] — the application state the host builds and every handler reads.

pub mod api_models;
pub mod convert;
pub mod database;
pub mod entities;
pub mod repositories;
pub mod state;
pub mod validation;

pub use convert::Convert;
pub use database::{CONNECTION_KEY, CONNECTION_NAME, resolve_database_path};
pub use repositories::{RepositoryError, RepositoryResult};
pub use state::{AppState, Repositories};
pub use validation::{Validate, ValidationFailure};

/// The connection-string key the original binds, kept so an existing
/// `ConnectionStrings__AppDatabase` environment variable works unchanged.
pub const CONNECTION_STRING_NAME: &str = CONNECTION_NAME;

/// Path of the bundled Chinook database, relative to the repository root.
pub const BUNDLED_DATABASE_PATH: &str = database::RELATIVE_DATABASE_PATH;
