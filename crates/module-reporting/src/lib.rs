//! Port of the C# `Reporting.Module` project.
//!
//! Deliberately thin, and finished: the original registers no services and
//! exposes only `/health` and `/data-health`, so this module is complete as it
//! stands. The `report.view` permission and the `reporting:heavy` rate-limit
//! name exist in the original but reach no endpoint — this port keeps them
//! equally unused rather than inventing a surface the source does not have.

use axum::Router;
use shared_kernel::{Module, health};
use shared_persistence::AppState;

/// Module name, mirroring `IModule.Name`.
pub const NAME: &str = "Reporting";

/// Route prefix, mirroring `MapGroup("/api/reporting")`.
pub const PREFIX: &str = "/api/reporting";

/// The Reporting module.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReportingModule;

impl Module<AppState> for ReportingModule {
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
