//! Port of `Catalog.Module`: artists, albums, tracks, playlists, and the track
//! sales `InvoiceFinalized` feeds.
//!
//! The module owns schema `catalog` — its tables, its migrations, its history
//! table — and reads nothing outside it (ADR-0003). Genre, media type and
//! invoice data belong to other modules, so a track carries their ids and
//! nothing more. Everything except [`CatalogModule`] and its constants is
//! private to the crate, which is upstream's `PublicSurfaceTests` rule held by
//! the compiler.

mod endpoints;
mod events;
mod handlers;
mod models;
mod services;

use std::sync::Arc;

use orders_contracts::events::InvoiceFinalized;
use shared_kernel::caching::CacheFacade;
use shared_kernel::diagnostics::ModuleMeter;
use shared_kernel::endpoints::Endpoints;
use shared_kernel::persistence::ModuleDatabase;
use shared_kernel::traffic_control::{self, RateLimiter};
use shared_kernel::{AppConfig, Module, ModuleError, Services};

use crate::services::{
    AlbumService, Albums, ArtistService, Artists, PlaylistService, Playlists, Store, TrackService,
    Tracks,
};

/// Module name. Port of `IModule.Name`, and every key the module registers under.
pub const NAME: &str = "Catalog";

/// Route prefix. Port of `MapGroup("/api/catalog")`.
pub const PREFIX: &str = "/api/catalog";

/// The schema the module owns.
pub const SCHEMA: &str = "catalog";

/// The OpenAPI tag, and the name of the module's own document.
pub const TAG: &str = "Catalog";

/// Entries the module's cache holds when `Caching:Modules:Catalog:SizeLimit` is unset.
pub const DEFAULT_CACHE_SIZE: u64 = 1000;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Everything a Catalog handler reads.
pub(crate) struct CatalogState {
    albums: Arc<dyn AlbumService>,
    artists: Arc<dyn ArtistService>,
    tracks: Arc<dyn TrackService>,
    playlists: Arc<dyn PlaylistService>,
    database: ModuleDatabase,
    rate_limiter: RateLimiter,
    meter: Arc<ModuleMeter>,
}

impl CatalogState {
    /// A state over fakes, for handler tests.
    #[cfg(test)]
    fn from_services(
        albums: Arc<dyn AlbumService>,
        artists: Arc<dyn ArtistService>,
        tracks: Arc<dyn TrackService>,
        playlists: Arc<dyn PlaylistService>,
    ) -> Self {
        let config =
            AppConfig::from_figment(Default::default(), shared_kernel::Environment::Development);
        Self {
            albums,
            artists,
            tracks,
            playlists,
            database: ModuleDatabase::unconnected(NAME, SCHEMA, &MIGRATOR),
            rate_limiter: RateLimiter::from_config(&config, traffic_control::policies::CATALOG),
            meter: Arc::new(ModuleMeter::new(NAME)),
        }
    }
}

/// The Catalog module. Port of `CatalogModule.Modules`.
#[derive(Default)]
pub struct CatalogModule {
    state: Option<Arc<CatalogState>>,
}

impl std::fmt::Debug for CatalogModule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CatalogModule")
            .field("registered", &self.state.is_some())
            .finish()
    }
}

impl Module for CatalogModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register_services(
        &mut self,
        services: &mut Services,
        config: &AppConfig,
    ) -> Result<(), ModuleError> {
        let database = ModuleDatabase::connect(config, NAME, SCHEMA, &MIGRATOR)?;
        services.add_module_database(&database);

        let cache: Arc<CacheFacade> = services.add_module_cache(NAME, DEFAULT_CACHE_SIZE);
        let rate_limiter = services.add_rate_limit_policy(traffic_control::policies::CATALOG);
        let meter = services.add_module_meter(NAME);
        services.add_integration_event_handler::<InvoiceFinalized, _>(
            NAME,
            events::InvoiceFinalizedHandler,
        );

        let store = Store {
            pool: database.pool().clone(),
            cache,
            keys: services.cache_keys(),
        };

        self.state = Some(Arc::new(CatalogState {
            albums: Arc::new(Albums(store.clone())),
            artists: Arc::new(Artists(store.clone())),
            tracks: Arc::new(Tracks(store.clone())),
            playlists: Arc::new(Playlists(store)),
            database,
            rate_limiter,
            meter,
        }));

        Ok(())
    }

    fn map_endpoints(&self, endpoints: &mut Endpoints) {
        if let Some(state) = &self.state {
            endpoints::map(endpoints, state);
        } else {
            tracing::error!("Catalog mapped its endpoints before registering its services");
        }
    }
}
