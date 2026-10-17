//! The full host's suites, one module per upstream `Api.Tests` class.
//!
//! One test binary rather than one per file, so the suites run in parallel in
//! one process and share one template database — the shape of upstream's
//! single test assembly. Each test still gets a host and a database of its own.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

mod administration;
mod authorization_pipeline;
mod bulkheads;
mod configuration;
mod cross_module;
mod demo_environment;
mod endpoint_filters;
mod error_scenarios;
mod health;
mod host;
mod identity;
mod migration_order;
mod modules;
mod openapi;
mod rate_limiting;
mod security_headers;
mod seed_integrity;
mod user_store_diagnostics;
