//! Port of the C# `Orders.Module` project.
//!
//! Seven read endpoints — invoices and invoice lines — plus health and
//! data-health. Data endpoints stack the `orders.read` and `tenant.scoped`
//! policies.
//!
//! One asymmetry carried over deliberately: `GET /invoice-lines/{id}` returns
//! the *entity* shape rather than an API model, because the C# repository's
//! `GetById` does and the service passes it straight through. It is the only
//! endpoint in the application that does this.

mod endpoints;
mod services;

use axum::Router;
use shared_kernel::{Module, health};
use shared_persistence::AppState;

/// Module name, mirroring `IModule.Name`.
pub const NAME: &str = "Orders";

/// Route prefix, mirroring `MapGroup("/api/orders")`.
pub const PREFIX: &str = "/api/orders";

/// The Orders module.
#[derive(Debug, Clone, Copy, Default)]
pub struct OrdersModule;

impl Module<AppState> for OrdersModule {
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
