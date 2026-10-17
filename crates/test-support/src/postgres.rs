//! One PostgreSQL server per test process, and a database per host cloned
//! from a template. Port of upstream's `PostgresFixture` and `PostgresServer`.
//!
//! The server is a `postgres:17-alpine` container started on first use with
//! durability switched off (`fsync`, `synchronous_commit`, `full_page_writes`),
//! as upstream starts its own. Set `TEST_POSTGRES_URL` to a superuser URL —
//! `postgres://postgres:postgres@localhost:5432/postgres` — to use a running
//! server instead; it should be a throwaway one, because nothing here drops
//! the databases it creates.
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
use testcontainers::ImageExt;
use testcontainers::runners::SyncRunner;
use testcontainers_modules::postgres::Postgres;
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
static CONTAINER: OnceLock<testcontainers::Container<Postgres>> = OnceLock::new();
static CLONE_LOCK: Mutex<()> = Mutex::const_new(());
static TEMPLATES: Mutex<Option<HashSet<String>>> = Mutex::const_new(None);
static ROLES: Mutex<bool> = Mutex::const_new(false);

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

        // The blocking runner owns a runtime of its own, which cannot start
        // inside a test's; a plain thread keeps them apart.
        let (host, port) = std::thread::spawn(|| {
            let container = Postgres::default()
                .with_tag("17-alpine")
                .with_cmd([
                    "postgres",
                    "-c",
                    "fsync=off",
                    "-c",
                    "synchronous_commit=off",
                    "-c",
                    "full_page_writes=off",
                    "-c",
                    "max_connections=500",
                ])
                .start()
                .expect("Docker should start postgres:17-alpine; or set TEST_POSTGRES_URL");

            // Both lookups block on the runner's own runtime, so they run here
            // too, and the container is parked in a static that is never
            // dropped: the reaper removes it when the process exits.
            let host = container
                .get_host()
                .expect("the container should have a host")
                .to_string();
            let port = container
                .get_host_port_ipv4(5432)
                .expect("the container should publish 5432");
            let _ = CONTAINER.set(container);
            (host, port)
        })
        .join()
        .expect("the container thread should not panic");

        PostgresServer {
            host,
            port,
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
        self.admin(&format!(
            "DO $$ BEGIN
               IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'reporting_reader') THEN
                 CREATE ROLE reporting_reader NOLOGIN;
               END IF;
               IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{REPORTING_LOGIN}') THEN
                 CREATE ROLE {REPORTING_LOGIN} LOGIN PASSWORD '{REPORTING_PASSWORD}' IN ROLE reporting_reader;
               END IF;
             END $$;"
        ))
        .await;
        *created = true;
    }

    /// The template named `name`, built by `build` the first time any test in
    /// this process asks for it.
    ///
    /// `build` receives the template's database name and must leave it
    /// migrated and seeded as every clone should start.
    pub async fn template<F, Fut>(&self, name: &str, build: F) -> String
    where
        F: FnOnce(String) -> Fut,
        Fut: Future<Output = ()>,
    {
        let database = format!("tpl_{name}_{}", std::process::id());
        let mut built = TEMPLATES.lock().await;
        let built = built.get_or_insert_with(HashSet::new);

        if built.contains(&database) {
            return database;
        }

        self.ensure_roles().await;
        self.admin(&format!(
            r#"DROP DATABASE IF EXISTS "{database}" WITH (FORCE)"#
        ))
        .await;
        self.admin(&format!(r#"CREATE DATABASE "{database}""#))
            .await;
        build(database.clone()).await;

        built.insert(database.clone());
        database
    }

    /// A fresh database copied from `template`.
    pub async fn clone_of(&self, template: &str) -> String {
        let database = format!("test_{}", uuid::Uuid::new_v4().simple());
        let _serialized = CLONE_LOCK.lock().await;
        self.admin(&format!(
            r#"CREATE DATABASE "{database}" TEMPLATE "{template}" STRATEGY FILE_COPY"#
        ))
        .await;
        database
    }

    /// A fresh, empty database.
    pub async fn empty(&self) -> String {
        self.ensure_roles().await;
        let database = format!("empty_{}", uuid::Uuid::new_v4().simple());
        self.admin(&format!(r#"CREATE DATABASE "{database}""#))
            .await;
        database
    }
}
