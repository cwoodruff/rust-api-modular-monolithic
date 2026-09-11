//! Port of the C# `Admin.Module` project (namespace `Admin.Modules`).
//!
//! Lands in Phase 7: 12 read endpoints (customers, employees, genres, media
//! types), the Genre `POST` / `PUT` / `DELETE` trio — the only writes anywhere
//! in the application — plus `/health` and `/data-health`.
//!
//! Administration is the one module that stacks three policies: reads require
//! `role.admin` **and** `administration.read` **and** `tenant.scoped`; writes
//! swap in `administration.write`.

/// Module name, mirroring `IModule.Name`.
pub const NAME: &str = "Administration";

/// Route prefix, mirroring `MapGroup("/api/admin")`.
pub const PREFIX: &str = "/api/admin";
