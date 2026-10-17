//! Port of the C# `ModularMonolith.Api` host.
//!
//! - [`host_composition`]: the module list and service registration
//!   (`HostComposition`).
//! - [`module_composition`]: the startup checks on mapped endpoints
//!   (`ModuleComposition`).
//! - [`db_seeder`]: migrations in order and the Chinook seed (`DbSeeder`).
//! - [`app`] and [`middleware`]: the pipeline (`Program.cs`).
//! - [`routes`] and [`openapi`]: `/`, `/healthz`, Swagger.
//!
//! The binary is a thin wrapper over this library so the integration tests
//! build the very same host the server runs.

pub mod app;
pub mod db_seeder;
pub mod host_composition;
pub mod middleware;
pub mod module_composition;
pub mod openapi;
pub mod routes;

pub use host_composition::{Host, HostOptions, build, configure_services, get_modules};

/// The port the original's `launchSettings.json` serves HTTP on.
pub const DEFAULT_HTTP_PORT: u16 = 5043;

/// The port the original's Dockerfile exposes.
pub const CONTAINER_HTTP_PORT: u16 = 8080;
