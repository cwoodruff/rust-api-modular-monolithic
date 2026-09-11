//! Port of the C# `Music.Module` project.
//!
//! Health endpoints are live. The 15 read endpoints — albums, artists,
//! playlists, tracks and their by-foreign-key lookups — land in Phase 6, where
//! every data endpoint stacks the `music.read` and `tenant.scoped` policies.
//! The two health endpoints stay anonymous, as they do in the original.
//!
//! Everything except this module's entry point stays `pub(crate)`, which is how
//! the port reproduces the C# `PublicSurfaceTests` rule that a module assembly
//! exports only its module type.

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
        health::routes(NAME)
    }
}
