//! Each module's database, ported from `SharedKernel.Persistence.ModuleDbContextOptions`.
//!
//! Upstream gives every module a `DbContext` of its own, in a schema of its own,
//! with its migration history in `<schema>.__EFMigrationsHistory`. Here a
//! module gets a [`ModuleDatabase`]: a connection pool, its schema name, and the
//! migrations its own crate embeds with `sqlx::migrate!`. Migrations run on a
//! connection whose `search_path` is the module's schema, so sqlx's history
//! table lands at `<schema>._sqlx_migrations` — one per module, as upstream.
//!
//! This is the only file in the kernel that names the PostgreSQL driver's own
//! types. Everything else — the outbox, the inbox, the health checks — goes
//! through the aliases below, which is the Rust form of upstream's fence
//! "Npgsql only from `SharedKernel.Persistence`" (ADR-0012).

use std::str::FromStr;
use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use sqlx::{ConnectOptions, Connection};

use crate::AppConfig;

/// The database engine every module uses.
pub type Db = sqlx::Postgres;

/// A module's connection pool.
pub type DbPool = sqlx::PgPool;

/// One connection, as a transaction or a pool hands it out.
pub type DbConnection = sqlx::PgConnection;

/// A transaction on a module's pool.
pub type DbTransaction = sqlx::Transaction<'static, Db>;

/// A row as the driver returns it.
pub type DbRow = sqlx::postgres::PgRow;

/// The connection string every module shares unless told otherwise.
pub const CONNECTION_NAME: &str = "AppDatabase";

/// What a host without a connection string is told, verbatim from upstream.
///
/// It always names `AppDatabase`, even when the missing string is Reporting's:
/// upstream interpolates the constant, not the argument, and the message is
/// kept as it reads there.
pub const MISSING_CONNECTION_STRING: &str = "ConnectionStrings:AppDatabase is not configured. \
     For local development run 'docker compose up -d' and use the Development environment, \
     or set ConnectionStrings__AppDatabase.";

/// The connections a pool opens at most, unless `Database:MaxPoolSize` says otherwise.
pub const DEFAULT_MAX_POOL_SIZE: u32 = 10;

/// Why a module's database could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    /// No connection string under the name the module asked for.
    #[error("{MISSING_CONNECTION_STRING}")]
    MissingConnectionString,

    /// The connection string could not be read.
    #[error("ConnectionStrings:{name} is not a valid PostgreSQL connection string: {reason}")]
    InvalidConnectionString {
        /// The connection-string name.
        name: String,
        /// What was wrong with it.
        reason: String,
    },

    /// The database refused.
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),

    /// A migration failed.
    #[error("migrating schema {schema} failed")]
    Migration {
        /// The schema being migrated.
        schema: &'static str,
        /// The cause.
        #[source]
        source: sqlx::migrate::MigrateError,
    },
}

/// A module's database: its pool, its schema, its migrations.
///
/// Port of `AddModuleDbContext<TContext>(moduleName, schema, connectionName,
/// migrationConnectionName)`. Cloning is cheap — the pools are reference counted.
#[derive(Clone)]
pub struct ModuleDatabase {
    module: &'static str,
    schema: &'static str,
    pool: DbPool,
    migration_options: PgConnectOptions,
    migrator: &'static Migrator,
}

impl std::fmt::Debug for ModuleDatabase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModuleDatabase")
            .field("module", &self.module)
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

impl ModuleDatabase {
    /// Opens a module's pool on `ConnectionStrings:AppDatabase`.
    ///
    /// The pool is lazy: nothing connects until the first query, so a host
    /// whose database is down still starts and reports `Degraded`.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::MissingConnectionString`] when the string is absent.
    pub fn connect(
        config: &AppConfig,
        module: &'static str,
        schema: &'static str,
        migrator: &'static Migrator,
    ) -> Result<Self, PersistenceError> {
        Self::connect_as(config, module, schema, migrator, CONNECTION_NAME, None)
    }

    /// Opens a module's pool on a named connection string, migrating through
    /// another.
    ///
    /// Reporting reads as a login that may not create anything, and migrates
    /// as the owner (ADR-0014): `connection_name` is `Reporting`,
    /// `migration_connection_name` is `AppDatabase`.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::MissingConnectionString`] when either string is absent.
    pub fn connect_as(
        config: &AppConfig,
        module: &'static str,
        schema: &'static str,
        migrator: &'static Migrator,
        connection_name: &str,
        migration_connection_name: Option<&str>,
    ) -> Result<Self, PersistenceError> {
        let options = connect_options(config, connection_name)?;
        let migration_options = match migration_connection_name {
            Some(name) if name != connection_name => connect_options(config, name)?,
            _ => options.clone(),
        };

        let max_connections = config
            .get_string("Database:MaxPoolSize")
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_MAX_POOL_SIZE);

        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(5))
            .connect_lazy_with(options);

        Ok(Self {
            module,
            schema,
            pool,
            migration_options,
            migrator,
        })
    }

    /// A database that has never connected and never will unless queried:
    /// a lazy pool on the default local server. For tests of code that holds
    /// a database but does not use it, such as a handler over a fake service.
    #[must_use]
    pub fn unconnected(
        module: &'static str,
        schema: &'static str,
        migrator: &'static Migrator,
    ) -> Self {
        let options = PgConnectOptions::new_without_pgpass();
        Self {
            module,
            schema,
            pool: PgPoolOptions::new().connect_lazy_with(options.clone()),
            migration_options: options,
            migrator,
        }
    }

    /// The module this database belongs to.
    #[must_use]
    pub fn module(&self) -> &'static str {
        self.module
    }

    /// The module's schema, such as `catalog`.
    #[must_use]
    pub fn schema(&self) -> &'static str {
        self.schema
    }

    /// The module's pool.
    #[must_use]
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// What upstream's health check calls this database: `CatalogDbContext`.
    #[must_use]
    pub fn context_name(&self) -> String {
        format!("{}DbContext", self.module)
    }

    /// Applies the module's migrations to its own schema.
    ///
    /// Port of `Database.Migrate()`. The schema is created first, because sqlx
    /// creates its history table before running the first migration and needs
    /// somewhere to put it.
    ///
    /// # Errors
    ///
    /// Any connection or migration failure.
    pub async fn migrate(&self) -> Result<(), PersistenceError> {
        let mut connection = self.migration_options.connect().await?;

        sqlx::query(&format!(r#"CREATE SCHEMA IF NOT EXISTS "{}""#, self.schema))
            .execute(&mut connection)
            .await?;
        sqlx::query(&format!(r#"SET search_path TO "{}""#, self.schema))
            .execute(&mut connection)
            .await?;

        let result = self.migrator.run(&mut connection).await;
        connection.close().await?;

        result.map_err(|source| PersistenceError::Migration {
            schema: self.schema,
            source,
        })
    }

    /// The versions applied to this schema, in order.
    ///
    /// Port of `dotnet ef migrations list`, for the tests that check the
    /// history table really is per schema.
    ///
    /// # Errors
    ///
    /// Any query failure, including a schema not migrated yet.
    pub async fn applied_migrations(&self) -> Result<Vec<i64>, PersistenceError> {
        let versions = sqlx::query_scalar(&format!(
            r#"SELECT version FROM "{}"._sqlx_migrations ORDER BY version"#,
            self.schema
        ))
        .fetch_all(&self.pool)
        .await?;

        Ok(versions)
    }

    /// Whether a connection can be opened and used.
    ///
    /// Port of `Database.CanConnectAsync`. Never fails: any error is `false`,
    /// and is logged so a degraded host has something to diagnose from (F9).
    pub async fn can_connect(&self) -> bool {
        can_connect(&self.pool).await
    }
}

impl From<sqlx::Error> for crate::data::DataError {
    fn from(error: sqlx::Error) -> Self {
        Self::database(error)
    }
}

/// Whether a pool can hand out a working connection.
pub async fn can_connect(pool: &DbPool) -> bool {
    match tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(pool),
    )
    .await
    {
        Ok(Ok(_)) => true,
        Ok(Err(error)) => {
            tracing::warn!(%error, "database connectivity check failed");
            false
        }
        Err(_) => {
            tracing::warn!("database connectivity check timed out");
            false
        }
    }
}

/// Whether `ConnectionStrings:AppDatabase` reaches a database.
///
/// Port of `ModuleDbContextOptions.CanConnectAsync(config)`, which Identity —
/// a module with no tables — uses for its `/data-health`. A missing or invalid
/// string is `false`, as upstream's catch-all makes it.
pub async fn can_connect_with(config: &AppConfig) -> bool {
    let Ok(options) = connect_options(config, CONNECTION_NAME) else {
        return false;
    };

    match tokio::time::timeout(Duration::from_secs(5), options.connect()).await {
        Ok(Ok(connection)) => {
            let _ = connection.close().await;
            true
        }
        Ok(Err(error)) => {
            tracing::warn!(%error, "database connectivity check failed");
            false
        }
        Err(_) => false,
    }
}

/// Reads `ConnectionStrings:<name>` into driver options.
///
/// # Errors
///
/// Missing or unreadable strings.
pub fn connect_options(
    config: &AppConfig,
    name: &str,
) -> Result<PgConnectOptions, PersistenceError> {
    let value = config
        .get_string(&format!("ConnectionStrings:{name}"))
        .filter(|value| !value.trim().is_empty())
        .ok_or(PersistenceError::MissingConnectionString)?;

    parse_connection_string(&value).map_err(|reason| PersistenceError::InvalidConnectionString {
        name: name.to_owned(),
        reason,
    })
}

/// Parses either an ADO.NET-style string or a `postgres://` URL.
///
/// The configuration files are upstream's, so their strings are Npgsql's
/// `Host=localhost;Port=5432;Database=chinook;Username=chinook;Password=chinook`.
/// Accepting that form keeps one `appsettings.json` meaning the same thing to
/// both implementations, and an existing `ConnectionStrings__AppDatabase`
/// variable working unchanged.
///
/// # Errors
///
/// A description of the first key or value that could not be read.
pub fn parse_connection_string(value: &str) -> Result<PgConnectOptions, String> {
    let trimmed = value.trim();

    if trimmed.starts_with("postgres://") || trimmed.starts_with("postgresql://") {
        return PgConnectOptions::from_str(trimmed).map_err(|error| error.to_string());
    }

    let mut options = PgConnectOptions::new_without_pgpass().ssl_mode(PgSslMode::Prefer);

    for pair in trimmed.split(';').filter(|pair| !pair.trim().is_empty()) {
        let (key, raw) = pair
            .split_once('=')
            .ok_or_else(|| format!("'{pair}' is not a key=value pair"))?;
        let raw = raw.trim();

        options = match key.trim().to_ascii_lowercase().replace(' ', "").as_str() {
            "host" | "server" => options.host(raw),
            "port" => options.port(raw.parse().map_err(|_| format!("'{raw}' is not a port"))?),
            "database" | "initialcatalog" => options.database(raw),
            "username" | "user" | "userid" | "uid" => options.username(raw),
            "password" | "pwd" => options.password(raw),
            "sslmode" => options.ssl_mode(
                PgSslMode::from_str(&raw.to_ascii_lowercase())
                    .map_err(|error| error.to_string())?,
            ),
            "applicationname" => options.application_name(raw),
            // Npgsql pooling and timeout keys: the pool is configured separately.
            "pooling" | "maxpoolsize" | "maximumpoolsize" | "minpoolsize" | "minimumpoolsize"
            | "timeout" | "commandtimeout" | "includeerrordetail" => options,
            other => return Err(format!("unsupported key '{other}'")),
        };
    }

    Ok(options.disable_statement_logging())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn an_npgsql_string_is_read_key_by_key() {
        let options = parse_connection_string(
            "Host=db.local;Port=6543;Database=chinook;Username=reporting;Password=pw;Pooling=true;MaxPoolSize=10",
        )
        .expect("the string should parse");

        assert_eq!(options.get_host(), "db.local");
        assert_eq!(options.get_port(), 6543);
        assert_eq!(options.get_database(), Some("chinook"));
        assert_eq!(options.get_username(), "reporting");
    }

    #[test]
    fn a_url_is_accepted_too() {
        let options = parse_connection_string("postgres://u:p@localhost:5432/chinook").unwrap();

        assert_eq!(options.get_database(), Some("chinook"));
    }

    #[test]
    fn an_unknown_key_is_refused_rather_than_ignored() {
        assert!(parse_connection_string("Host=x;Flavour=mint").is_err());
    }

    #[test]
    fn the_missing_string_message_is_upstreams() {
        assert_eq!(
            PersistenceError::MissingConnectionString.to_string(),
            "ConnectionStrings:AppDatabase is not configured. For local development run 'docker compose up -d' and use the Development environment, or set ConnectionStrings__AppDatabase."
        );
    }
}
