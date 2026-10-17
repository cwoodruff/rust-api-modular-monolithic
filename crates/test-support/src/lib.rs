//! Test fixtures shared by every database-backed suite.
//!
//! - [`postgres`]: one PostgreSQL server per test process, template databases
//!   built once and cloned per host — upstream's `PostgresFixture` and
//!   `PostgresServer`.
//! - [`auth`]: header-based authentication and a copy of Identity's policies —
//!   upstream's `TestAuth`, which stands in for Identity on a module's own host.
//! - [`http`]: sending a request to a router and reading the answer.
//! - [`content_root`] and [`config`]: the repository's real configuration,
//!   with a test's overrides on top.

#![allow(clippy::expect_used, clippy::unwrap_used)]

pub mod auth;
pub mod http;
pub mod postgres;

use std::path::{Path, PathBuf};

use shared_kernel::{AppConfig, Environment};

/// The repository root: the directory holding `appsettings.json`.
#[must_use]
pub fn content_root() -> PathBuf {
    let mut current = Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        if current.join("appsettings.json").is_file() {
            return current.to_path_buf();
        }
        current = current.parent().expect("appsettings.json should be at the repository root");
    }
}

/// The repository's configuration for `environment`, with `overrides` on top.
///
/// User secrets are never read: a developer's own logins must not leak into a
/// test's expectations.
#[must_use]
pub fn config(environment: Environment, overrides: serde_json::Value) -> AppConfig {
    AppConfig::load_without_user_secrets(&content_root(), environment)
        .expect("the repository's configuration should load")
        .with_overrides(overrides)
}
