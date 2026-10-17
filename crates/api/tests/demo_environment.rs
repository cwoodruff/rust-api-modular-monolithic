//! Port of upstream's `DemoEnvironmentTests` (4ad3c91).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::process::Command;

use shared_kernel::{AppConfig, Environment};

/// Set only in the child process this suite spawns.
const CHILD: &str = "RMM_USER_SECRETS_CHILD";

#[test]
fn demo_reads_the_compose_database_and_migrates_on_startup() {
    let config =
        AppConfig::load_without_user_secrets(&test_support::content_root(), Environment::Demo)
            .unwrap();

    assert!(
        config
            .get_string("ConnectionStrings:AppDatabase")
            .unwrap()
            .contains("Database=chinook")
    );
    assert!(
        config
            .get_string("ConnectionStrings:Reporting")
            .unwrap()
            .contains("Username=reporting")
    );
    assert!(config.get_or("Database:MigrateAndSeedOnStartup", false));
    assert!(Environment::Demo.migrates_on_startup());
    assert!(
        !Environment::Demo.validates_endpoints(),
        "the startup check is Development and Test only"
    );
}

/// User secrets load in Development and Demo, after `appsettings.{env}.json`
/// and before the environment — and not at all in Production.
///
/// The path comes from `USER_SECRETS_PATH`, which this process cannot set on
/// itself without `unsafe`; so the test reruns itself with the variable set.
#[test]
fn user_secrets_load_in_development_and_demo_only() {
    if std::env::var_os(CHILD).is_some() {
        let root = test_support::content_root();
        for environment in [Environment::Development, Environment::Demo] {
            let config = AppConfig::load_for(&root, environment.clone()).unwrap();
            assert_eq!(
                config
                    .get_string("Identity:InMemoryUsers:0:Username")
                    .as_deref(),
                Some("from-secrets"),
                "{}: the secret overrides appsettings.{0}.json's empty list",
                environment.name()
            );
            assert_eq!(
                config
                    .get_string("Identity:InMemoryUsers:0:Password")
                    .as_deref(),
                Some("from-environment"),
                "{}: an environment variable still beats a secret",
                environment.name()
            );
        }
        let production = AppConfig::load_for(&root, Environment::Production).unwrap();
        assert_eq!(
            production.get_string("Identity:InMemoryUsers:0:Username"),
            None
        );
        return;
    }

    let directory = std::env::temp_dir().join(format!("rmm-secrets-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let secrets = directory.join("secrets.json");
    std::fs::write(
        &secrets,
        r#"{ "Identity:InMemoryUsers:0:Username": "from-secrets", "Identity:InMemoryUsers:0:Password": "from-secrets" }"#,
    )
    .unwrap();

    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "user_secrets_load_in_development_and_demo_only",
            "--test-threads",
            "1",
        ])
        .env(CHILD, "1")
        .env("USER_SECRETS_PATH", &secrets)
        .env("Identity__InMemoryUsers__0__Password", "from-environment")
        .status()
        .unwrap();

    let _ = std::fs::remove_dir_all(&directory);
    assert!(
        status.success(),
        "the child run failed; rerun with {CHILD}=1 to see why"
    );
}
