//! Port of the C# `SharedKernel` project.
//!
//! Cross-cutting primitives only, nothing domain-shaped: the module contract,
//! caching, traffic control, events, concurrency, diagnostics, validation,
//! persistence and the authorization framework. Upstream's ADR-0012 sets a
//! budget on this crate's public surface; `architecture-tests` enforces it.
//!
//! This crate never depends on a module or a contracts crate, and only
//! [`persistence`] names the database driver's own types.

pub mod auth;
pub mod build_info;
pub mod caching;
pub mod clock;
pub mod concurrency;
pub mod config;
pub mod data;
pub mod diagnostics;
pub mod endpoints;
pub mod environment;
pub mod errors;
pub mod events;
pub mod health;
pub mod hosting;
pub mod json;
pub mod module;
pub mod persistence;
pub mod redaction;
pub mod traffic_control;
pub mod validation;

pub use auth::{
    AuthenticatedUser, AuthorizationFailure, Caller, PolicyDecision, PolicyEvaluator, Principal,
};
pub use config::AppConfig;
pub use data::{DataError, DataResult};
pub use environment::Environment;
pub use errors::{ApiError, ProblemDetails};
pub use health::{DatabaseProbe, HealthResponse};
pub use json::JsonBody;
pub use module::{Module, ModuleError, Services};
pub use redaction::REDACTED;
