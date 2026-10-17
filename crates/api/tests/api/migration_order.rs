//! Port of upstream's `MigrationOrderTests` (2efb158): schemas migrate
//! Administration, Catalog, Orders, then Reporting, and an unknown schema
//! stops the host rather than migrating out of order.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use api::db_seeder::{MIGRATION_ORDER, in_migration_order};
use shared_kernel::persistence::ModuleDatabase;

static EMPTY: sqlx::migrate::Migrator = sqlx::migrate!("tests/empty_migrations");

#[tokio::test]
async fn the_real_databases_migrate_in_upstreams_order() {
    let host = crate::common::host().await;
    let databases = host.host.services().databases().all();

    let ordered: Vec<&str> = in_migration_order(databases)
        .unwrap()
        .iter()
        .map(ModuleDatabase::schema)
        .collect();
    assert_eq!(ordered, MIGRATION_ORDER);
    assert_eq!(
        ordered,
        ["administration", "catalog", "orders", "reporting"]
    );
}

#[tokio::test]
async fn a_stray_schema_is_refused_by_name() {
    let stray = ModuleDatabase::unconnected("Stray", "stray", &EMPTY);
    let catalog = ModuleDatabase::unconnected("Catalog", "catalog", &EMPTY);

    let error = in_migration_order(vec![catalog, stray])
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("No migration order for:"), "{error}");
    assert!(error.contains("StrayDbContext"), "{error}");
}

#[tokio::test]
async fn each_schema_keeps_its_own_migration_history() {
    let host = crate::common::host().await;

    for database in host.host.services().databases().all() {
        let applied = database.applied_migrations().await.unwrap();
        assert!(
            !applied.is_empty(),
            "{} has its own history table",
            database.schema()
        );
    }
}
