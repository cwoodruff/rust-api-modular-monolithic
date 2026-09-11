//! The application state every module handler is given.
//!
//! This is what the C# DI container held: the repositories, the cache, the key
//! composer, and configuration. There is no container here, so the host builds
//! this once and axum hands out cheap clones — every field is behind an `Arc`.
//!
//! It lives in this crate rather than in `shared-kernel` because it names the
//! repository traits, and `shared-kernel` is the root of the dependency graph.

use std::sync::Arc;

use shared_kernel::caching::{CacheFacade, CacheKeyComposer};
use shared_kernel::health::{DatabaseProbe, HealthContext};
use shared_kernel::{AppConfig, Environment};

use crate::repositories::{
    AlbumRepository, ArtistRepository, CustomerRepository, EmployeeRepository, GenreRepository,
    InvoiceLineRepository, InvoiceRepository, MediaTypeRepository, PlaylistRepository,
    TrackRepository,
};

/// Every repository, as the host resolves them.
#[derive(Clone)]
pub struct Repositories {
    /// Albums.
    pub albums: Arc<dyn AlbumRepository>,
    /// Artists.
    pub artists: Arc<dyn ArtistRepository>,
    /// Tracks.
    pub tracks: Arc<dyn TrackRepository>,
    /// Playlists.
    pub playlists: Arc<dyn PlaylistRepository>,
    /// Genres.
    pub genres: Arc<dyn GenreRepository>,
    /// Media types.
    pub media_types: Arc<dyn MediaTypeRepository>,
    /// Customers.
    pub customers: Arc<dyn CustomerRepository>,
    /// Employees.
    pub employees: Arc<dyn EmployeeRepository>,
    /// Invoices.
    pub invoices: Arc<dyn InvoiceRepository>,
    /// Invoice lines.
    pub invoice_lines: Arc<dyn InvoiceLineRepository>,
}

/// State shared by every request.
#[derive(Clone)]
pub struct AppState {
    /// The resolved configuration.
    pub config: Arc<AppConfig>,
    /// The environment the host is running as.
    pub environment: Environment,
    /// The service-layer cache.
    pub cache: Arc<CacheFacade>,
    /// Builds the cache's keys.
    pub cache_keys: Arc<CacheKeyComposer>,
    /// Answers the data-health question.
    pub database: Arc<dyn DatabaseProbe>,
    /// Every repository.
    pub repositories: Repositories,
}

impl AppState {
    /// Whether this host exposes version, environment, and service name.
    ///
    /// Port of `BuildInfoProvider.ShouldExposeOperationalMetadata`.
    #[must_use]
    pub fn exposes_operational_metadata(&self) -> bool {
        self.environment.exposes_operational_metadata()
    }
}

impl HealthContext for AppState {
    fn environment(&self) -> &Environment {
        &self.environment
    }

    fn service_name(&self) -> String {
        self.config.service_name()
    }

    fn database_probe(&self) -> Arc<dyn DatabaseProbe> {
        Arc::clone(&self.database)
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppState")
            .field("environment", &self.environment)
            .finish_non_exhaustive()
    }
}
