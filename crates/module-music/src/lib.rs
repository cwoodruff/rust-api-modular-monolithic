//! Port of the C# `Music.Module` project.
//!
//! Lands in Phase 6: 15 read endpoints (albums, artists, playlists, tracks and
//! their by-foreign-key lookups) plus `/health` and `/data-health`. Every data
//! endpoint stacks the `music.read` and `tenant.scoped` policies; the two
//! health endpoints stay anonymous.
//!
//! Everything except the module's own entry point stays `pub(crate)`, which is
//! how this port reproduces the C# `PublicSurfaceTests` rule that a module
//! assembly exports only its module type.

/// Module name, mirroring `IModule.Name`.
pub const NAME: &str = "Music";

/// Route prefix, mirroring `MapGroup("/api/music")`.
pub const PREFIX: &str = "/api/music";
