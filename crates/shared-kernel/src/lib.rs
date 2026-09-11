//! Port of the C# `SharedKernel` project.
//!
//! This crate is the root of the dependency graph: it never depends on a module
//! crate or on a persistence crate, and the architecture tests enforce that.
//!
//! Where the C# original used an interface purely so the DI container could
//! hand out an implementation, this port often uses a concrete type instead —
//! Rust does not need the indirection, and generic methods (`GetOrAddAsync<T>`)
//! cannot live on a trait object at all. Where the interface carried real
//! polymorphism, such as [`Module`](module::Module), it stays a trait.

pub mod build_info;
pub mod caching;
pub mod config;
pub mod environment;
pub mod errors;
pub mod health;
pub mod module;
pub mod traffic_control;

pub use config::AppConfig;
pub use environment::Environment;
pub use errors::ProblemDetails;
pub use health::{DatabaseProbe, HealthContext, HealthResponse};
pub use module::Module;
