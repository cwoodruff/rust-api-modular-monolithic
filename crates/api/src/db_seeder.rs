//! Migrations and the Chinook seed, ported from `DbSeeder`.
//!
//! Every module's database migrates in a fixed order — Administration,
//! Catalog, Orders, then Reporting, whose views read the other three — and the
//! seed loads once, when `catalog."Track"` is empty. Development, Test and Demo
//! do this on startup unless `Database:MigrateAndSeedOnStartup` is false; other
//! environments migrate as a deployment step.

use std::path::Path;

use shared_kernel::persistence::{ModuleDatabase, PersistenceError};

/// The seed, relative to the content root.
pub const SEED_SCRIPT_RELATIVE_PATH: &str = "data/chinook-postgres-seed.sql";

/// The order schemas migrate in.
pub const MIGRATION_ORDER: [&str; 4] = ["administration", "catalog", "orders", "reporting"];

/// Why migrating or seeding failed.
#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    /// A database has a schema the order does not name.
    #[error("No migration order for: {0}")]
    NoMigrationOrder(String),
    /// A migration failed.
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
    /// The seed could not be read.
    #[error("could not read the seed script {path}")]
    Read {
        /// Where it was looked for.
        path: String,
        /// The cause.
        #[source]
        source: std::io::Error,
    },
    /// The seed failed to load.
    #[error("loading the seed script failed")]
    Load(#[from] sqlx::Error),
}

/// Sorts `databases` into [`MIGRATION_ORDER`].
///
/// # Errors
///
/// [`SeedError::NoMigrationOrder`] naming every module whose schema is not in it.
pub fn in_migration_order(
    mut databases: Vec<ModuleDatabase>,
) -> Result<Vec<ModuleDatabase>, SeedError> {
    let strays: Vec<String> = databases
        .iter()
        .filter(|database| !MIGRATION_ORDER.contains(&database.schema()))
        .map(ModuleDatabase::context_name)
        .collect();

    if !strays.is_empty() {
        return Err(SeedError::NoMigrationOrder(strays.join(", ")));
    }

    databases.sort_by_key(|database| {
        MIGRATION_ORDER
            .iter()
            .position(|schema| *schema == database.schema())
    });
    Ok(databases)
}

/// Migrates every database in order, then loads the seed if Catalog is empty.
///
/// The script runs as one command in one transaction, as upstream runs it, so
/// a failure part-way leaves nothing behind.
///
/// # Errors
///
/// Any migration or seed failure.
pub async fn migrate_and_seed(
    databases: Vec<ModuleDatabase>,
    content_root: &Path,
) -> Result<(), SeedError> {
    let ordered = in_migration_order(databases)?;

    for database in &ordered {
        tracing::info!(schema = database.schema(), "migrating");
        database.migrate().await?;
    }

    let Some(catalog) = ordered
        .iter()
        .find(|database| database.schema() == "catalog")
    else {
        return Ok(());
    };

    let seeded: bool = sqlx::query_scalar(r#"SELECT EXISTS (SELECT 1 FROM catalog."Track")"#)
        .fetch_one(catalog.pool())
        .await?;
    if seeded {
        return Ok(());
    }

    let path = content_root.join(SEED_SCRIPT_RELATIVE_PATH);
    let script = std::fs::read_to_string(&path).map_err(|source| SeedError::Read {
        path: path.display().to_string(),
        source,
    })?;

    tracing::info!(path = %path.display(), "loading the Chinook seed");

    // The owner's connection: the first database in order migrates as it.
    let owner = &ordered[0];
    let mut transaction = owner.pool().begin().await?;
    sqlx::raw_sql(&script).execute(&mut *transaction).await?;
    transaction.commit().await?;

    Ok(())
}
