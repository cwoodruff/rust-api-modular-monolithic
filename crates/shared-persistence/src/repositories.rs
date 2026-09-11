//! Repository contracts, ported from `SharedKernel.Persistence.Repositories`.
//!
//! These are traits only — the sqlx implementations live in
//! `shared-data-sqlite`, so a module crate can depend on the contract without
//! seeing a database driver. The architecture tests enforce that split.
//!
//! # Shape of the original
//!
//! The C# `IRepository<T>` is worth reading closely, because two of its quirks
//! are carried over deliberately:
//!
//! ```csharp
//! public interface IRepository<T> {
//!     Task<bool> EntityExists(int id);
//!     Task<List<T>> GetAll();
//!     //Task<T?> GetById(int id);     // commented out
//!     Task<T> Add(T entity);
//!     Task<bool> Update(T entity);
//!     Task<bool> Delete(int id);
//! }
//! ```
//!
//! 1. **`GetById` is commented out of the base contract** and redeclared on
//!    each entity's interface with a *different return type*. Most return an
//!    API model; `Genre`, `MediaType`, and `InvoiceLine` return the entity.
//!    That inconsistency reaches the wire — it is why
//!    `GET /api/orders/invoice-lines/{id}` returns an entity shape — so it is
//!    reproduced rather than tidied.
//! 2. **`GetAll` is unbounded.** No paging, no limit; the track repository
//!    returns all 3,503 rows. Matching the original means matching that.
//!
//! Two things are dropped. The C# interfaces extend `IDisposable`, and the base
//! implementation's `Dispose` disposes a *pooled* `DbContext` — a bug that
//! never fires only because nothing calls it. Ownership handles lifetimes here,
//! so there is nothing to reproduce. The methods also take no
//! `CancellationToken`; that omission is kept, since adding one would change
//! every call site for no behavior the original has.

use async_trait::async_trait;

use crate::api_models::{
    AlbumApiModel, ArtistApiModel, CustomerApiModel, EmployeeApiModel, InvoiceApiModel,
    PlaylistApiModel, TrackApiModel,
};
use crate::entities::{
    Album, Artist, Customer, Employee, Genre, Invoice, InvoiceLine, MediaType, Playlist, Track,
};

/// Something went wrong talking to the database.
///
/// The driver's own error type is boxed because this crate must not depend on
/// a driver. The C# equivalent is an unhandled exception, which the host turns
/// into a 500 — [`Self::into_problem`] keeps that mapping explicit.
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    /// The underlying database reported a failure.
    #[error("database operation failed")]
    Database(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl RepositoryError {
    /// Wraps a driver error.
    pub fn database(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self::Database(source.into())
    }

    /// The 500 the host returns for a repository failure, leaking no detail.
    #[must_use]
    pub fn into_problem(self, trace_id: impl Into<String>) -> shared_kernel::ProblemDetails {
        shared_kernel::ProblemDetails::internal_server_error(trace_id)
    }
}

/// Result of a repository call.
pub type RepositoryResult<T> = Result<T, RepositoryError>;

/// Port of `IRepository<T>`.
#[async_trait]
pub trait Repository<T>: Send + Sync {
    /// Whether a row with this key exists.
    async fn entity_exists(&self, id: i32) -> RepositoryResult<bool>;

    /// Every row. Unbounded, as in the original.
    async fn get_all(&self) -> RepositoryResult<Vec<T>>;

    /// Inserts a row, returning it with its generated key.
    async fn add(&self, entity: T) -> RepositoryResult<T>;

    /// Updates a row, reporting `false` if no such row exists.
    async fn update(&self, entity: T) -> RepositoryResult<bool>;

    /// Deletes a row, reporting `false` if no such row exists.
    async fn delete(&self, id: i32) -> RepositoryResult<bool>;
}

/// Port of `IAlbumRepository`.
#[async_trait]
pub trait AlbumRepository: Repository<Album> {
    /// Albums recorded by one artist.
    async fn get_by_artist_id(&self, id: i32) -> RepositoryResult<Vec<Album>>;

    /// One album with its artist and tracks.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<AlbumApiModel>>;
}

/// Port of `IArtistRepository`.
#[async_trait]
pub trait ArtistRepository: Repository<Artist> {
    /// One artist with their albums, each carrying its tracks.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<ArtistApiModel>>;
}

/// Port of `ITrackRepository`.
#[async_trait]
pub trait TrackRepository: Repository<Track> {
    /// Tracks on one album.
    async fn get_by_album_id(&self, id: i32) -> RepositoryResult<Vec<Track>>;

    /// Tracks in one genre.
    async fn get_by_genre_id(&self, id: i32) -> RepositoryResult<Vec<Track>>;

    /// Tracks in one media format.
    async fn get_by_media_type_id(&self, id: i32) -> RepositoryResult<Vec<Track>>;

    /// Tracks sold on one invoice.
    async fn get_by_invoice_id(&self, id: i32) -> RepositoryResult<Vec<Track>>;

    /// Tracks on one playlist.
    async fn get_by_playlist_id(&self, id: i32) -> RepositoryResult<Vec<Track>>;

    /// Tracks across every album by one artist.
    async fn get_by_artist_id(&self, id: i32) -> RepositoryResult<Vec<Track>>;

    /// One track with its denormalized album, genre, and media type names.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<TrackApiModel>>;
}

/// Port of `IPlaylistRepository`.
#[async_trait]
pub trait PlaylistRepository: Repository<Playlist> {
    /// Playlists containing one track.
    async fn get_by_track_id(&self, id: i32) -> RepositoryResult<Vec<Playlist>>;

    /// One playlist with its tracks, ordered by track key.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<PlaylistApiModel>>;
}

/// Port of `IGenreRepository`.
///
/// Returns the **entity** from `get_by_id`, unlike most of its siblings.
#[async_trait]
pub trait GenreRepository: Repository<Genre> {
    /// One genre.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<Genre>>;
}

/// Port of `IMediaTypeRepository`.
///
/// Returns the **entity** from `get_by_id`, unlike most of its siblings.
#[async_trait]
pub trait MediaTypeRepository: Repository<MediaType> {
    /// One media format.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<MediaType>>;
}

/// Port of `ICustomerRepository`.
#[async_trait]
pub trait CustomerRepository: Repository<Customer> {
    /// Customers supported by one employee.
    async fn get_by_support_rep_id(&self, id: i32) -> RepositoryResult<Vec<Customer>>;

    /// One customer with their support representative and invoices.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<CustomerApiModel>>;
}

/// Port of `IEmployeeRepository`.
#[async_trait]
pub trait EmployeeRepository: Repository<Employee> {
    /// The employee with this key.
    ///
    /// Named for the manager relationship, but the C# implementation looks the
    /// row up by `id` directly — the "manager" reading comes entirely from what
    /// the caller passes in. The name is kept so the two code bases line up.
    async fn get_reports_to(&self, id: i32) -> RepositoryResult<Option<Employee>>;

    /// Employees reporting to this one.
    async fn get_direct_reports(&self, id: i32) -> RepositoryResult<Vec<Employee>>;

    /// One employee, with their manager flattened to a `"First Last"` string.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<EmployeeApiModel>>;
}

/// Port of `IInvoiceRepository`.
#[async_trait]
pub trait InvoiceRepository: Repository<Invoice> {
    /// Invoices raised for one customer.
    async fn get_by_customer_id(&self, id: i32) -> RepositoryResult<Vec<Invoice>>;

    /// One invoice with its customer and lines.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<InvoiceApiModel>>;
}

/// Port of `IInvoiceLineRepository`.
///
/// Returns the **entity** from `get_by_id`, which is what makes
/// `GET /api/orders/invoice-lines/{id}` the one endpoint serving an entity
/// shape rather than an API model.
#[async_trait]
pub trait InvoiceLineRepository: Repository<InvoiceLine> {
    /// Lines belonging to one invoice.
    async fn get_by_invoice_id(&self, id: i32) -> RepositoryResult<Vec<InvoiceLine>>;

    /// Lines that sold one track.
    async fn get_by_track_id(&self, id: i32) -> RepositoryResult<Vec<InvoiceLine>>;

    /// One line.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<InvoiceLine>>;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn a_repository_error_reports_a_500_without_detail() {
        let error = RepositoryError::database("no such table: Album");

        let problem = error.into_problem("trace-1");

        assert_eq!(problem.status, 500);
        assert_eq!(
            problem.detail, None,
            "the driver's message must not reach the client"
        );
    }

    #[test]
    fn the_driver_error_is_still_available_for_logging() {
        let error = RepositoryError::database("no such table: Album");

        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some("no such table: Album".to_owned())
        );
    }

    /// Compile-time proof that every contract stays usable as a trait object,
    /// so the host can hold them behind `Arc<dyn …>` the way the DI container
    /// held their C# counterparts. Naming one of them in a field is enough —
    /// an object-unsafe trait would fail to compile here.
    ///
    /// This also previews the shape of Phase 4's application state.
    #[allow(dead_code)]
    struct ObjectSafetyProof {
        albums: std::sync::Arc<dyn AlbumRepository>,
        artists: std::sync::Arc<dyn ArtistRepository>,
        tracks: std::sync::Arc<dyn TrackRepository>,
        playlists: std::sync::Arc<dyn PlaylistRepository>,
        genres: std::sync::Arc<dyn GenreRepository>,
        media_types: std::sync::Arc<dyn MediaTypeRepository>,
        customers: std::sync::Arc<dyn CustomerRepository>,
        employees: std::sync::Arc<dyn EmployeeRepository>,
        invoices: std::sync::Arc<dyn InvoiceRepository>,
        invoice_lines: std::sync::Arc<dyn InvoiceLineRepository>,
    }
}
