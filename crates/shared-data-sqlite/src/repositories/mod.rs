//! sqlx implementations of the repository traits.
//!
//! Grouped by the module that consumes them, not one file per type, because
//! the graph-building queries share projections within a group.

mod administration;
mod common;
mod music;
mod orders;

pub use administration::{
    SqliteCustomerRepository, SqliteEmployeeRepository, SqliteGenreRepository,
    SqliteMediaTypeRepository,
};
pub use music::{
    SqliteAlbumRepository, SqliteArtistRepository, SqlitePlaylistRepository, SqliteTrackRepository,
};
pub use orders::{SqliteInvoiceLineRepository, SqliteInvoiceRepository};
