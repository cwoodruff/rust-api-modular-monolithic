//! Port of upstream's `InMemoryUserStoreDiagnosticsTests`: the startup
//! warnings that make a 401 at login explainable.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use shared_kernel::{AppConfig, Environment};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn startup_log(environment: Environment, users: Value) -> String {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();

    let config = AppConfig::from_values(
        json!({ "Identity": { "InMemoryUsers": users } }),
        environment,
    );
    let keys: Arc<dyn module_identity::KeyMaterial> =
        Arc::new(module_identity::DevKeyMaterial::generate().unwrap());
    tracing::subscriber::with_default(subscriber, || {
        let _ = module_identity::IdentityRuntime::with_keys(&config, keys);
    });

    String::from_utf8(captured.0.lock().unwrap().clone()).unwrap()
}

#[test]
fn an_incomplete_entry_is_named_by_index_and_missing_field() {
    let log = startup_log(
        Environment::Development,
        json!([
            { "Username": "demo", "Password": "pw", "UserId": "user-1" },
            { "Username": "half" }
        ]),
    );

    assert!(log.contains("Ignoring Identity:InMemoryUsers:1 because it is missing required field(s): Password, UserId."), "{log}");
    assert!(log.contains("Loaded 1 in-memory login users and ignored 1 incomplete Identity:InMemoryUsers entries."), "{log}");
    assert!(log.contains("Effective Identity:InMemoryUsers:0 => Username='demo', UserId='user-1', Roles='(none)', Permissions='(none)', Tenant='(none)', PasswordLength=2."), "{log}");
}

#[test]
fn no_usable_entry_is_reported_and_so_is_none_at_all() {
    let none_usable = startup_log(Environment::Development, json!([{ "Username": "half" }]));
    assert!(
        none_usable.contains("none of the 1 configured Identity:InMemoryUsers entries are usable"),
        "{none_usable}"
    );
    assert!(
        none_usable.contains("Ignoring Identity:InMemoryUsers:0"),
        "the per-index warning comes first"
    );

    let empty = startup_log(Environment::Development, json!([]));
    assert!(
        empty.contains("no Identity:InMemoryUsers entries are configured."),
        "{empty}"
    );
}

#[test]
fn complete_entries_and_production_raise_no_warning_about_entries() {
    let complete = startup_log(
        Environment::Development,
        json!([{ "Username": "demo", "Password": "pw", "UserId": "u" }]),
    );
    assert!(!complete.contains("WARN"), "{complete}");

    let production = startup_log(
        Environment::Production,
        json!([{ "Username": "demo", "Password": "pw", "UserId": "u" }]),
    );
    assert!(
        production.contains("in-memory login is disabled in the Production environment"),
        "{production}"
    );
    assert!(!production.contains("Effective"), "{production}");
}

#[test]
fn a_pasted_command_is_flagged_without_echoing_the_password() {
    let log = startup_log(
        Environment::Development,
        json!([{ "Username": "demo", "Password": "hunter2dotnet user-secrets list", "UserId": "u" }]),
    );

    assert!(
        log.contains("contains command text ('user-secrets')"),
        "{log}"
    );
    assert!(!log.contains("hunter2"), "the value itself is never logged");

    assert_eq!(
        module_identity::suspicious_password_reason("correct horse").as_deref(),
        Some("contains 1 whitespace character(s)")
    );
}
