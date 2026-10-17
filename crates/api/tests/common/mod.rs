//! The full host on a database of its own. Port of upstream's `ApiFactory` and
//! `PostgresFixture`.
//!
//! The first host in a test process builds a template — migrated through the
//! real `DbSeeder` and seeded with the real Chinook script, via the very
//! `api::build` the binary runs — and every host after that gets a clone.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use api::{Host, HostOptions};
use serde_json::{Value, json};
use shared_kernel::Environment;
use shared_kernel::hosting::RunningServices;
use test_support::http::{TestRequest, TestResponse};
use test_support::postgres;

#[allow(unused_imports)]
pub use test_support::http;

/// The password every test login uses.
pub const PASSWORD: &str = "T-pass-0!Aa1";

/// The logins upstream's `TestAuthHelpers` seeds, all in `tenant-1`.
#[must_use]
pub fn users() -> Value {
    json!([
        { "Username": "demo", "Password": PASSWORD, "UserId": "user-1", "DisplayName": "Demo User",
          "Email": "demo@example.com", "Tenant": "tenant-1", "Roles": ["User"], "Permissions": ["catalog.read"] },
        { "Username": "usermo", "Password": PASSWORD, "UserId": "user-2", "Tenant": "tenant-1",
          "Roles": ["User"], "Permissions": ["catalog.read", "orders.read"] },
        { "Username": "report", "Password": PASSWORD, "UserId": "user-3", "Tenant": "tenant-1",
          "Roles": ["User"], "Permissions": ["report.view"] },
        { "Username": "admin", "Password": PASSWORD, "UserId": "admin-1", "DisplayName": "Admin User",
          "Email": "admin@example.com", "Tenant": "tenant-1", "Roles": ["Admin"],
          "Permissions": ["catalog.read", "catalog.write", "orders.read", "orders.write", "admin.users.manage",
                          "administration.read", "administration.write", "report.view"] }
    ])
}

/// Signing keys generated once per process: no key file is written.
fn keys() -> Arc<dyn module_identity::KeyMaterial> {
    static KEYS: std::sync::OnceLock<Arc<dyn module_identity::KeyMaterial>> =
        std::sync::OnceLock::new();
    Arc::clone(KEYS.get_or_init(|| {
        Arc::new(module_identity::DevKeyMaterial::generate().expect("a key should generate"))
    }))
}

fn options() -> HostOptions {
    HostOptions {
        identity_keys: Some(keys()),
        ..HostOptions::new(test_support::content_root())
    }
}

/// The seeded template, built once per process.
async fn template() -> String {
    let server = postgres::server();
    server
        .template("api", |database| async move {
            let config = test_support::config(
                Environment::Development,
                json!({
                    "ConnectionStrings": {
                        "AppDatabase": server.connection_string(&database),
                        "Reporting": server.reporting_connection_string(&database)
                    },
                    "Database": { "MigrateAndSeedOnStartup": true },
                    "Outbox": { "Enabled": false }
                }),
            );
            api::build(config, options())
                .await
                .expect("the template host should migrate and seed");
        })
        .await
}

/// A host and the database it owns.
pub struct TestHost {
    /// The host.
    pub host: Host,
    /// Its database's name, for tests that look underneath.
    pub database: String,
    running: Option<RunningServices>,
}

impl TestHost {
    /// Sends a request.
    pub async fn send(&self, request: TestRequest) -> TestResponse {
        request.send(&self.host.router()).await
    }

    /// Logs in and returns the access token.
    pub async fn token(&self, username: &str) -> String {
        let response = self
            .send(
                TestRequest::post("/api/identity/login")
                    .json(&json!({ "username": username, "password": PASSWORD })),
            )
            .await;
        assert_eq!(
            response.status,
            200,
            "login for {username} failed: {}",
            response.text()
        );
        response.json()["access_token"]
            .as_str()
            .expect("a token")
            .to_owned()
    }

    /// Starts the dispatchers, queues and jobs, as the binary does.
    pub fn start_hosted_services(&mut self) {
        self.running = Some(self.host.start_hosted_services());
    }
}

/// The full host in Development, with the test logins, on a fresh clone.
pub async fn host() -> TestHost {
    host_with(Environment::Development, json!({})).await
}

/// The full host in `environment`, with `overrides` on top of the test defaults.
pub async fn host_with(environment: Environment, overrides: Value) -> TestHost {
    host_with_keys(environment, overrides, keys()).await
}

/// The same, signing with keys of its own: a token another host issued is
/// signed by someone else as far as this one is concerned.
pub async fn host_with_own_keys(environment: Environment, overrides: Value) -> TestHost {
    let keys: Arc<dyn module_identity::KeyMaterial> =
        Arc::new(module_identity::DevKeyMaterial::generate().expect("a key should generate"));
    host_with_keys(environment, overrides, keys).await
}

async fn host_with_keys(
    environment: Environment,
    overrides: Value,
    keys: Arc<dyn module_identity::KeyMaterial>,
) -> TestHost {
    let server = postgres::server();
    let database = server.clone_of(&template().await).await;

    let config = test_support::config(
        environment,
        json!({
            "ConnectionStrings": {
                "AppDatabase": server.connection_string(&database),
                "Reporting": server.reporting_connection_string(&database)
            },
            "Database": { "MigrateAndSeedOnStartup": false, "MaxPoolSize": 4 },
            "Outbox": { "Enabled": false },
            "Reporting": { "Integrity": { "Enabled": false } },
            "Identity": { "InMemoryUsers": users() }
        }),
    )
    .with_overrides(overrides);

    let options = HostOptions {
        identity_keys: Some(keys),
        ..HostOptions::new(test_support::content_root())
    };
    let host = api::build(config, options)
        .await
        .expect("the test host should build");

    TestHost {
        host,
        database,
        running: None,
    }
}
