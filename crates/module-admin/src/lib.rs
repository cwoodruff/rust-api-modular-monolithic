//! Port of the C# `Admin.Module` project (namespace `Admin.Modules`).
//!
//! Twelve read endpoints — customers, employees, genres, media types — the
//! Genre `POST` / `PUT` / `DELETE` trio, which is the only write surface
//! anywhere in the application, and health plus data-health.
//!
//! Administration is the one module that stacks three policies: reads require
//! `role.admin` **and** `administration.read` **and** `tenant.scoped`; writes
//! swap in `administration.write`.
//!
//! Note the name and the prefix disagree — the module reports `Administration`
//! while mounting at `/api/admin`. That is the original's arrangement.

mod endpoints;
mod services;

use axum::Router;
use shared_kernel::{Module, health};
use shared_persistence::AppState;

/// Module name, mirroring `IModule.Name`.
pub const NAME: &str = "Administration";

/// Route prefix, mirroring `MapGroup("/api/admin")`.
pub const PREFIX: &str = "/api/admin";

/// The Administration module.
#[derive(Debug, Clone, Copy, Default)]
pub struct AdministrationModule;

impl Module<AppState> for AdministrationModule {
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
