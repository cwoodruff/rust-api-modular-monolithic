//! One PostgreSQL server per test process, and a database per host cloned
//! from a template. Port of upstream's `PostgresFixture` and `PostgresServer`.
//!
//! The server is a `postgres:17-alpine` container named `rmm-test-postgres`,
//! with durability switched off (`fsync`, `synchronous_commit`,
//! `full_page_writes`) as upstream starts its own. It is shared by every test
//! process and left running between runs; databases a run leaves behind are
//! dropped by a later one once they are six hours old. Set
//! `TEST_POSTGRES_URL` to a superuser URL —
//! `postgres://postgres:postgres@localhost:5432/postgres` — to use another
//! server instead; it should be a throwaway one.
//!
//! A template is migrated and seeded once per process; every host then gets a
//! `CREATE DATABASE … TEMPLATE … STRATEGY FILE_COPY` clone of its own, so tests
//! run in parallel without seeing each other's writes. Clones are serialized,
//! because PostgreSQL refuses to copy a template while another copy is
//! reading it.
//!
//! Connections here are opened per operation and closed after it. A pool
//! belongs to the runtime that made it, and every `#[tokio::test]` has a
//! runtime of its own.

use std::collections::HashSet;
use std::future::Future;
use std::sync::OnceLock;

use sqlx::{Connection, PgConnection};
use tokio::sync::Mutex;

/// The login the Reporting module reads as, created on every server.
pub const REPORTING_LOGIN: &str = "reporting";

/// Its password.
pub const REPORTING_PASSWORD: &str = "reporting";

/// Where the server is and how to reach it as its superuser.
#[derive(Debug, Clone)]
pub struct PostgresServer {
    host: String,
    port: u16,
    user: String,
    password: String,
}

static SERVER: OnceLock<PostgresServer> = OnceLock::new();
static CLONE_LOCK: Mutex<()> = Mutex::const_new(());
static TEMPLATES: Mutex<Option<HashSet<String>>> = Mutex::const_new(None);
static ROLES: Mutex<bool> = Mutex::const_new(false);

/// The container every test process shares.
pub const CONTAINER_NAME: &str = "rmm-test-postgres";

/// Databases older than this are dropped when a process first asks for a
/// template: whatever left them has long finished.
const STALE_AFTER_SECONDS: u64 = 6 * 60 * 60;

fn docker(arguments: &[&str]) -> Option<String> {
    let output = std::process::Command::new("docker")
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Starts the shared container if it is not running, and returns its port.
///
/// One container serves every test process, named [`CONTAINER_NAME`]: a
/// container per process was never stopped, because a static is never
/// dropped, and nothing reaped them. `docker rm -f rmm-test-postgres` removes
/// it; the next test run starts it again.
fn shared_container_port() -> u16 {
    let running =
        docker(&["ps", "-q", "--filter", &format!("name=^{CONTAINER_NAME}$")]).unwrap_or_default();

    if running.is_empty() {
        let _ = docker(&["rm", "-f", CONTAINER_NAME]);
        // A concurrent process may win this race; its container is as good.
        let _ = docker(&[
            "run",
            "-d",
            "--name",
            CONTAINER_NAME,
            "--label",
            "org.testcontainers.managed-by=rust-modular-monolith",
            "-e",
            "POSTGRES_PASSWORD=postgres",
            "-p",
            "127.0.0.1::5432",
            "postgres:17-alpine",
            "postgres",
            "-c",
            "fsync=off",
            "-c",
            "synchronous_commit=off",
            "-c",
            "full_page_writes=off",
            "-c",
            "max_connections=500",
        ]);
    }

    for _ in 0..120 {
        let ready = docker(&[
            "exec",
            CONTAINER_NAME,
            "pg_isready",
            "-U",
            "postgres",
            "-h",
            "127.0.0.1",
        ])
        .is_some();
        if ready
            && let Some(mapping) = docker(&["port", CONTAINER_NAME, "5432/tcp"])
            && let Some(port) = mapping
                .lines()
                .next()
                .and_then(|line| line.rsplit(':').next())
                .and_then(|port| port.parse().ok())
        {
            return port;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    panic!("Docker should run postgres:17-alpine as {CONTAINER_NAME}; or set TEST_POSTGRES_URL");
}

/// This process's stamp: when it first asked for a database. Every name it
/// creates carries it, so a later process can tell what is stale.
fn process_stamp() -> u64 {
    static STAMP: OnceLock<u64> = OnceLock::new();
    *STAMP.get_or_init(unix_seconds)
}

/// A fingerprint of everything a template's contents depend on.
fn fingerprint() -> String {
    static FINGERPRINT: OnceLock<String> = OnceLock::new();
    FINGERPRINT
        .get_or_init(|| {
            use sha2::{Digest, Sha256};

            let root = crate::content_root();
            let mut files: Vec<std::path::PathBuf> = vec![
                root.join("data/chinook-postgres-seed.sql"),
                root.join("crates/api/src/db_seeder.rs"),
                root.join("crates/api/tests/api/common.rs"),
                root.join("crates/module-tests/src/lib.rs"),
                root.join("crates/test-support/src/postgres.rs"),
            ];
            for parent in ["crates", "samples"] {
                let Ok(crates) = std::fs::read_dir(root.join(parent)) else {
                    continue;
                };
                for entry in crates.flatten() {
                    if let Ok(migrations) = std::fs::read_dir(entry.path().join("migrations")) {
                        files.extend(migrations.flatten().map(|migration| migration.path()));
                    }
                }
            }
            files.sort();

            let mut hasher = Sha256::new();
            for file in files {
                hasher.update(file.to_string_lossy().as_bytes());
                hasher.update(std::fs::read(&file).unwrap_or_default());
            }
            hasher
                .finalize()
                .iter()
                .take(6)
                .map(|byte| format!("{byte:02x}"))
                .collect()
        })
        .clone()
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The server, started on first use.
///
/// # Panics
///
/// When Docker is unavailable and no `TEST_POSTGRES_URL` is set — the
/// database suites need one or the other.
pub fn server() -> &'static PostgresServer {
    SERVER.get_or_init(|| {
        if let Ok(url) = std::env::var("TEST_POSTGRES_URL") {
            return PostgresServer::from_url(&url);
        }

        PostgresServer {
            host: "127.0.0.1".to_owned(),
            port: shared_container_port(),
            user: "postgres".to_owned(),
            password: "postgres".to_owned(),
        }
    })
}

impl PostgresServer {
    fn from_url(url: &str) -> Self {
        let options: sqlx::postgres::PgConnectOptions = url
            .parse()
            .expect("TEST_POSTGRES_URL should be a postgres:// URL");
        let password = url
            .split_once("://")
            .and_then(|(_, rest)| rest.split_once('@'))
            .and_then(|(credentials, _)| credentials.split_once(':'))
            .map(|(_, password)| password.to_owned())
            .unwrap_or_default();

        Self {
            host: options.get_host().to_owned(),
            port: options.get_port(),
            user: options.get_username().to_owned(),
            password,
        }
    }

    /// An Npgsql-style connection string to `database` as the superuser.
    #[must_use]
    pub fn connection_string(&self, database: &str) -> String {
        format!(
            "Host={};Port={};Database={database};Username={};Password={}",
            self.host, self.port, self.user, self.password
        )
    }

    /// The same, as the read-only Reporting login.
    #[must_use]
    pub fn reporting_connection_string(&self, database: &str) -> String {
        format!(
            "Host={};Port={};Database={database};Username={REPORTING_LOGIN};Password={REPORTING_PASSWORD}",
            self.host, self.port
        )
    }

    fn url(&self, database: &str) -> String {
        format!(
            "postgres://{}:{}@{}:{}/{database}",
            self.user, self.password, self.host, self.port
        )
    }

    /// Opens one connection to `database` as the superuser.
    ///
    /// # Panics
    ///
    /// When the server refuses.
    pub async fn connect(&self, database: &str) -> PgConnection {
        PgConnection::connect(&self.url(database))
            .await
            .expect("the test server should accept a connection")
    }

    async fn admin(&self, statement: &str) {
        let mut connection = self.connect("postgres").await;
        sqlx::raw_sql(statement)
            .execute(&mut connection)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
        let _ = connection.close().await;
    }

    /// Creates the roles every database on the server relies on: the
    /// `reporting_reader` role Reporting's migration grants to, and the
    /// `reporting` login in it. Roles are server-wide, so once is enough.
    async fn ensure_roles(&self) {
        // Once per process, and never two at once: concurrent `CREATE ROLE`s
        // race on pg_authid's unique index even behind `IF NOT EXISTS`.
        let mut created = ROLES.lock().await;
        if *created {
            return;
        }
        // Another process may be creating them at the same moment; whichever
        // loses the race finds them there.
        self.admin(&format!(
            "DO $$ BEGIN
               BEGIN
                 CREATE ROLE reporting_reader NOLOGIN;
               EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL;
               END;
               BEGIN
                 CREATE ROLE {REPORTING_LOGIN} LOGIN PASSWORD '{REPORTING_PASSWORD}' IN ROLE reporting_reader;
               EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL;
               END;
             END $$;"
        ))
        .await;
        self.drop_stale_databases().await;
        *created = true;
    }

    /// Drops what earlier processes left behind: every test and empty
    /// database whose stamp is more than six hours old. Templates are kept;
    /// a changed fingerprint replaces them.
    async fn drop_stale_databases(&self) {
        let mut connection = self.connect("postgres").await;
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT datname FROM pg_database WHERE datname ~ '^(test|empty)_' AND NOT datistemplate",
        )
        .fetch_all(&mut connection)
        .await
        .unwrap_or_default();
        let _ = connection.close().await;

        let cutoff = unix_seconds().saturating_sub(STALE_AFTER_SECONDS);
        for name in names {
            let stamp = name.split('_').find_map(|part| {
                part.parse::<u64>()
                    .ok()
                    .filter(|value| *value > 1_000_000_000)
            });
            if stamp.is_some_and(|stamp| stamp < cutoff) {
                self.admin(&format!(r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#))
                    .await;
            }
        }
    }

    /// The template named `name`, built by `build` only if no process has
    /// built this version of it before.
    ///
    /// The name carries a fingerprint of everything a template's contents
    /// depend on — every migration, the seed, and the two places templates
    /// are built from — so a template survives between runs until one of them
    /// changes. Building and checking happen under a server-wide advisory
    /// lock, so two test processes never build the same template at once, and
    /// a template is used only once it is marked ready.
    ///
    /// `build` receives the template's database name and must leave it
    /// migrated and seeded as every clone should start.
    pub async fn template<F, Fut>(&self, name: &str, build: F) -> String
    where
        F: FnOnce(String) -> Fut,
        Fut: Future<Output = ()>,
    {
        let database = format!("tpl_{name}_{}", fingerprint());
        let mut built = TEMPLATES.lock().await;
        let built = built.get_or_insert_with(HashSet::new);

        if built.contains(&database) {
            return database;
        }

        self.ensure_roles().await;

        let mut lock = self.connect("postgres").await;
        sqlx::query("SELECT pg_advisory_lock(hashtext('rmm-test-templates'))")
            .execute(&mut lock)
            .await
            .expect("the template lock should be granted");

        let ready: Option<String> = sqlx::query_scalar(
            "SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = $1",
        )
        .bind(&database)
        .fetch_optional(&mut lock)
        .await
        .expect("pg_database should be readable")
        .flatten();

        if ready.as_deref() != Some("ready") {
            // Earlier versions of this template are of no further use.
            let outdated: Vec<String> = sqlx::query_scalar(
                "SELECT datname FROM pg_database WHERE datname LIKE $1 AND datname <> $2",
            )
            .bind(format!("tpl\\_{name}\\_%"))
            .bind(&database)
            .fetch_all(&mut lock)
            .await
            .unwrap_or_default();
            for old in outdated
                .into_iter()
                .chain(std::iter::once(database.clone()))
            {
                self.admin(&format!(r#"DROP DATABASE IF EXISTS "{old}" WITH (FORCE)"#))
                    .await;
            }

            self.admin(&format!(r#"CREATE DATABASE "{database}""#))
                .await;
            build(database.clone()).await;
            self.admin(&format!(r#"COMMENT ON DATABASE "{database}" IS 'ready'"#))
                .await;
        }

        let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('rmm-test-templates'))")
            .execute(&mut lock)
            .await;
        let _ = lock.close().await;

        built.insert(database.clone());
        database
    }

    /// A fresh database copied from `template`.
    ///
    /// Serialized across every process on the server, not just this one:
    /// PostgreSQL refuses to copy a template another copy is reading.
    pub async fn clone_of(&self, template: &str) -> String {
        let database = format!("test_{}_{}", process_stamp(), uuid::Uuid::new_v4().simple());
        let _serialized = CLONE_LOCK.lock().await;

        let mut lock = self.connect("postgres").await;
        sqlx::query("SELECT pg_advisory_lock(hashtext('rmm-test-clones'))")
            .execute(&mut lock)
            .await
            .expect("the clone lock should be granted");
        let created = sqlx::raw_sql(&format!(
            r#"CREATE DATABASE "{database}" TEMPLATE "{template}" STRATEGY FILE_COPY"#
        ))
        .execute(&mut lock)
        .await;
        let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('rmm-test-clones'))")
            .execute(&mut lock)
            .await;
        let _ = lock.close().await;

        created.unwrap_or_else(|error| panic!("cloning {template}: {error}"));
        database
    }

    /// A fresh, empty database.
    pub async fn empty(&self) -> String {
        self.ensure_roles().await;
        let database = format!(
            "empty_{}_{}",
            process_stamp(),
            uuid::Uuid::new_v4().simple()
        );
        self.admin(&format!(r#"CREATE DATABASE "{database}""#))
            .await;
        database
    }
}
