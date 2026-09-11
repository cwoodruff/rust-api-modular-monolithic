//! Port of the C# `Music.Module` project.
//!
//! Fifteen read endpoints — albums, artists, playlists, tracks and their
//! by-foreign-key lookups — plus health and data-health. Every data endpoint
//! stacks the `music.read` and `tenant.scoped` policies; the two health
//! endpoints stay anonymous, as they do in the original.
//!
//! Everything except this module's entry point stays `pub(crate)`, which is how
//! the port reproduces the C# `PublicSurfaceTests` rule that a module assembly
//! exports only its module type.

mod endpoints;
mod services;

use axum::Router;
use shared_kernel::{Module, health};
use shared_persistence::AppState;

/// Module name, mirroring `IModule.Name`.
pub const NAME: &str = "Music";

/// Route prefix, mirroring `MapGroup("/api/music")`.
pub const PREFIX: &str = "/api/music";

/// The Music module.
#[derive(Debug, Clone, Copy, Default)]
pub struct MusicModule;

impl Module<AppState> for MusicModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn prefix(&self) -> &'static str {
        PREFIX
    }

    fn router(&self) -> Router<AppState> {
        health::routes(NAME).merge(endpoints::routes())
    }
}
