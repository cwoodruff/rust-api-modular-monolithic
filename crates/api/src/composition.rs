//! Building the application state.
//!
//! Port of everything `Program.cs` does before `builder.Build()`: resolving the
//! database, registering the ten repositories, and wiring central caching. The
//! C# container does this by type; here it is one explicit constructor, which
//! is also the only place a concrete database driver is named.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use shared_data_sqlite::{
    SqliteAlbumRepository, SqliteArtistRepository, SqliteCustomerRepository, SqliteDatabaseProbe,
    SqliteEmployeeRepository, SqliteGenreRepository, SqliteInvoiceLineRepository,
    SqliteInvoiceRepository, SqliteMediaTypeRepository, SqlitePlaylistRepository,
    SqliteTrackRepository, create_pool,
};
use shared_kernel::caching::{CacheFacade, CacheKeyComposer, CacheOptions};
use shared_kernel::{AppConfig, Environment};
use shared_persistence::state::{AppState, Repositories};
use shared_persistence::{CONNECTION_KEY, database};

/// Loads configuration from `content_root` for the process environment.
///
/// # Errors
///
/// Returns an error if a configuration file exists but cannot be read or parsed.
pub fn load_config(content_root: &Path) -> anyhow::Result<AppConfig> {
    AppConfig::load(content_root).context("failed to load configuration")
}

/// Builds the application state.
///
/// # Errors
///
/// Returns an error if the database cannot be opened.
pub async fn build_state(config: AppConfig, content_root: &Path) -> anyhow::Result<AppState> {
    let environment = config.environment().clone();

    let configured = config.get_string(CONNECTION_KEY);
    let database_path = database::resolve_database_path(configured.as_deref(), content_root);

    tracing::info!(path = %database_path.display(), "opening the Chinook database");

    let pool = create_pool(&database_path)
        .await
        .with_context(|| format!("failed to open {}", database_path.display()))?;

    // AddCentralCaching(configuration). The original ships no `Caching`
    // section, so this is the defaults in practice: enabled, L1 only.
    let cache_options: CacheOptions =
        config.section_or_default(shared_kernel::caching::CONFIGURATION_SECTION);

    Ok(AppState {
        cache: Arc::new(CacheFacade::new(cache_options)),
        cache_keys: Arc::new(CacheKeyComposer::new(&config)),
        database: Arc::new(SqliteDatabaseProbe::new(pool.clone())),
        repositories: Repositories {
            albums: Arc::new(SqliteAlbumRepository::new(pool.clone())),
            artists: Arc::new(SqliteArtistRepository::new(pool.clone())),
            tracks: Arc::new(SqliteTrackRepository::new(pool.clone())),
            playlists: Arc::new(SqlitePlaylistRepository::new(pool.clone())),
            genres: Arc::new(SqliteGenreRepository::new(pool.clone())),
            media_types: Arc::new(SqliteMediaTypeRepository::new(pool.clone())),
            customers: Arc::new(SqliteCustomerRepository::new(pool.clone())),
            employees: Arc::new(SqliteEmployeeRepository::new(pool.clone())),
            invoices: Arc::new(SqliteInvoiceRepository::new(pool.clone())),
            invoice_lines: Arc::new(SqliteInvoiceLineRepository::new(pool)),
        },
        config: Arc::new(config),
        environment,
    })
}

/// Builds state for an explicit environment, bypassing the process variable.
///
/// The C# tests reach for `WebApplicationFactory.UseEnvironment(...)` to do the
/// same thing.
///
/// # Errors
///
/// Returns an error if the database cannot be opened.
pub async fn build_state_for(
    content_root: &Path,
    environment: Environment,
) -> anyhow::Result<AppState> {
    let config =
        AppConfig::load_for(content_root, environment).context("failed to load configuration")?;

    build_state(config, content_root).await
}
