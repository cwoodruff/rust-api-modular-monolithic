//! Port of the C# `ModularMonolith.Api` host.
//!
//! The binary is a thin wrapper over this library so the integration tests can
//! build the very same router the server runs. The C# solution reaches for the
//! same seam from the other direction, exposing a dummy `public class Program`
//! purely so `WebApplicationFactory<Program>` can find an entry point.

pub mod app;
pub mod composition;
pub mod middleware;
pub mod openapi;
pub mod routes;

pub use app::build;
pub use composition::build_identity;
pub use composition::{build_state, build_state_for, load_config};

/// The port the original's `launchSettings.json` serves HTTP on.
pub const DEFAULT_HTTP_PORT: u16 = 5043;

/// The port the original's Dockerfile exposes.
pub const CONTAINER_HTTP_PORT: u16 = 8080;
