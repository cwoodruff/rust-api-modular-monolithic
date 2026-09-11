//! Port of the C# `Reporting.Module` project.
//!
//! Lands in Phase 7, and stays deliberately thin: the original registers no
//! services and exposes only `/health` and `/data-health`. The `report.view`
//! permission and the `reporting:heavy` rate-limit name exist in the original
//! but reach no endpoint — this port keeps them equally unused rather than
//! inventing a surface the source does not have.

/// Module name, mirroring `IModule.Name`.
pub const NAME: &str = "Reporting";

/// Route prefix, mirroring `MapGroup("/api/reporting")`.
pub const PREFIX: &str = "/api/reporting";
