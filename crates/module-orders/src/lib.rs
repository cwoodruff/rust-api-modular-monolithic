//! Port of the C# `Orders.Module` project.
//!
//! Lands in Phase 6: 7 read endpoints (invoices, invoice lines) plus `/health`
//! and `/data-health`. Data endpoints stack `orders.read` and `tenant.scoped`.
//!
//! One asymmetry to carry over deliberately: `GET /invoice-lines/{id}` returns
//! the *entity* shape rather than an API model, because the C# repository's
//! `GetById` does.

/// Module name, mirroring `IModule.Name`.
pub const NAME: &str = "Orders";

/// Route prefix, mirroring `MapGroup("/api/orders")`.
pub const PREFIX: &str = "/api/orders";
