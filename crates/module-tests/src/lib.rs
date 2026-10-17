//! Port of `ModularMonolith.Module.Tests`: each module on a host of its own
//! (ADR-0016).
//!
//! [`ModuleTestHost`] registers the kernel's services and one module — nothing
//! else — against a database cloned from a template holding only that
//! module's schema (Reporting's also holds the three its views read). A
//! header-based scheme stands in for Identity, with Identity's own policy
//! rules, and [`ModuleTestHost::deliver`] feeds the module's event handlers
//! through its inbox, upstream's `DeliverAsync`. The outbox loop and the
//! integrity schedule are off; tests drive both by hand.
//!
//! The suites live under `tests/`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::missing_panics_doc)]

use std::sync::Arc;

use axum::Router;
use serde_json::json;
use shared_kernel::clock::{self, Clock};
use shared_kernel::endpoints::{Endpoints, HostInfo};
use shared_kernel::events::{IntegrationEvent, inbox};
use shared_kernel::hosting::RunningServices;
use shared_kernel::persistence::ModuleDatabase;
use shared_kernel::{AppConfig, Environment, Module, Services};
use test_support::auth::{HeaderAuthenticator, TestPolicies, TestUser, add_test_authentication};
use test_support::http::{TestRequest, TestResponse};
use test_support::postgres;

/// The order schemas migrate in, as the host's `DbSeeder` orders them.
const MIGRATION_ORDER: [&str; 4] = ["administration", "catalog", "orders", "reporting"];

/// Which module a host carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// Catalog: schema `catalog`.
    Catalog,
    /// Orders: schema `orders`.
    Orders,
    /// Administration: schema `administration`.
    Administration,
    /// Reporting: its own schema plus the three its views read.
    Reporting,
}

impl Which {
    fn name(self) -> &'static str {
        match self {
            Self::Catalog => "catalog",
            Self::Orders => "orders",
            Self::Administration => "administration",
            Self::Reporting => "reporting",
        }
    }

    fn module(self) -> Box<dyn Module> {
        match self {
            Self::Catalog => Box::new(module_catalog::CatalogModule::default()),
            Self::Orders => Box::new(module_orders::OrdersModule::default()),
            Self::Administration => Box::new(module_admin::AdministrationModule::default()),
            Self::Reporting => Box::new(module_reporting::ReportingModule::default()),
        }
    }

    /// The modules whose schemas this host's database holds. Port of `ModuleData`.
    fn schema_modules(self) -> Vec<Self> {
        match self {
            Self::Reporting => vec![
                Self::Administration,
                Self::Catalog,
                Self::Orders,
                Self::Reporting,
            ],
            other => vec![other],
        }
    }

    fn schemas(self) -> Vec<&'static str> {
        self.schema_modules().into_iter().map(Self::name).collect()
    }
}

/// Whether the clone starts with the Chinook slice for its schemas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Data {
    /// The seed's rows for the host's schemas.
    Seeded,
    /// Migrated tables, no rows.
    Empty,
}

fn module_config(database: &str) -> AppConfig {
    let server = postgres::server();
    test_support::config(
        Environment::Development,
        json!({
            "ConnectionStrings": {
                "AppDatabase": server.connection_string(database),
                "Reporting": server.reporting_connection_string(database)
            },
            "Database": { "MigrateAndSeedOnStartup": false, "MaxPoolSize": 4 },
            "Outbox": { "Enabled": false },
            "Reporting": { "Integrity": { "Enabled": false } }
        }),
    )
}

/// The seed statements for `schemas`. Port of `ChinookSeed.For`.
///
/// A statement ends at a line ending in `;`; leading `--` lines are dropped.
/// A statement is kept when it inserts into one of the schemas, or is a
/// `setval` naming one.
#[must_use]
pub fn seed_for(schemas: &[&str]) -> Vec<String> {
    let script = std::fs::read_to_string(
        test_support::content_root().join("data/chinook-postgres-seed.sql"),
    )
    .expect("the seed script should be readable");

    let mut statements = Vec::new();
    let mut current = String::new();
    for line in script.lines() {
        if current.is_empty() && (line.trim_start().starts_with("--") || line.trim().is_empty()) {
            continue;
        }
        current.push_str(line);
        current.push('\n');
        if line.trim_end().ends_with(';') {
            statements.push(std::mem::take(&mut current));
        }
    }

    statements
        .into_iter()
        .filter(|statement| {
            schemas.iter().any(|schema| {
                statement.contains(&format!("INSERT INTO {schema}."))
                    || statement.contains(&format!("('{schema}."))
            })
        })
        .collect()
}

async fn template(which: Which, data: Data) -> String {
    let name = format!(
        "module_{}_{}",
        which.name(),
        if data == Data::Seeded {
            "seeded"
        } else {
            "empty"
        }
    );

    postgres::server()
        .template(&name, |database| async move {
            let config = Arc::new(module_config(&database));
            let mut services = Services::new(Arc::clone(&config));
            for owner in which.schema_modules() {
                owner
                    .module()
                    .register_services(&mut services, &config)
                    .expect("a module should register");
            }

            let mut databases = services.databases().all();
            databases.sort_by_key(|database| {
                MIGRATION_ORDER
                    .iter()
                    .position(|schema| *schema == database.schema())
            });
            for database in &databases {
                database
                    .migrate()
                    .await
                    .expect("a module's migrations should apply");
            }

            if data == Data::Seeded {
                let mut connection = postgres::server().connect(&database).await;
                for statement in seed_for(&which.schemas()) {
                    sqlx::raw_sql(&statement)
                        .execute(&mut connection)
                        .await
                        .expect("a seed statement should load");
                }
            }
        })
        .await
}

/// One module on a host of its own. Port of `ModuleTestHost<TModule>`.
pub struct ModuleTestHost {
    router: Router,
    services: Services,
    database_name: String,
    _running: RunningServices,
}

impl std::fmt::Debug for ModuleTestHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModuleTestHost")
            .field("database", &self.database_name)
            .finish_non_exhaustive()
    }
}

impl ModuleTestHost {
    /// `which` on a fresh clone, on the system clock.
    pub async fn start(which: Which, data: Data) -> Self {
        Self::start_with_clock(which, data, clock::system()).await
    }

    /// `which` on a fresh clone, telling time with `clock`.
    pub async fn start_with_clock(which: Which, data: Data, clock: Arc<dyn Clock>) -> Self {
        let template = template(which, data).await;
        let database_name = postgres::server().clone_of(&template).await;
        let config = Arc::new(module_config(&database_name));

        let mut services = Services::with_clock(Arc::clone(&config), clock);
        add_test_authentication(&mut services);
        let mut module = which.module();
        module
            .register_services(&mut services, &config)
            .expect("the module should register");

        let mut endpoints = Endpoints::new(
            Arc::new(TestPolicies),
            HostInfo {
                environment: Environment::Development,
                service_name: config.service_name(),
            },
        );
        module.map_endpoints(&mut endpoints);
        let (router, _) = endpoints.into_parts();

        let router = router.layer(axum::middleware::from_fn(|request, next| {
            shared_kernel::auth::authenticate(Arc::new(HeaderAuthenticator), request, next)
        }));

        let running = RunningServices::start(services.hosted_services());

        Self {
            router,
            services,
            database_name,
            _running: running,
        }
    }

    /// What the module registered.
    #[must_use]
    pub fn services(&self) -> &Services {
        &self.services
    }

    /// The router.
    pub fn router(&self) -> &Router {
        &self.router
    }

    /// The clone's name.
    #[must_use]
    pub fn database_name(&self) -> &str {
        &self.database_name
    }

    /// One database the host registered, by module name.
    #[must_use]
    pub fn database(&self, module: &str) -> ModuleDatabase {
        self.services
            .databases()
            .get(module)
            .expect("the module registered a database")
    }

    /// A superuser connection to the clone, for looking underneath.
    pub async fn connect(&self) -> sqlx::PgConnection {
        postgres::server().connect(&self.database_name).await
    }

    /// A connection as the read-only Reporting login.
    pub async fn connect_as_reporting(&self) -> sqlx::PgConnection {
        use sqlx::Connection;
        let server = postgres::server();
        let options = shared_kernel::persistence::parse_connection_string(
            &server.reporting_connection_string(&self.database_name),
        )
        .unwrap();
        sqlx::PgConnection::connect_with(&options).await.unwrap()
    }

    /// Sends `request` anonymously.
    pub async fn send(&self, request: TestRequest) -> TestResponse {
        request.send(&self.router).await
    }

    /// Sends `request` as `user`.
    pub async fn send_as(&self, user: &TestUser, request: TestRequest) -> TestResponse {
        request.headers(user.headers()).send(&self.router).await
    }

    /// Feeds `event` to every handler subscribed to it, through its inbox.
    /// Returns how many handlers applied it. Port of `DeliverAsync`.
    pub async fn deliver<E: IntegrationEvent>(&self, event: &E) -> usize {
        let mut applied = 0;
        let event: Arc<E> =
            Arc::new(serde_json::from_value(serde_json::to_value(event).unwrap()).unwrap());

        for subscription in self.services.subscriptions().for_event(E::EVENT_TYPE) {
            let database = self.database(subscription.module_key);
            let event = Arc::clone(&event);
            let handler = subscription.clone();
            let ran = inbox::run(
                database.pool(),
                database.schema(),
                event.event_id(),
                subscription.handler_name,
                chrono::Utc::now(),
                move |connection| {
                    Box::pin(async move {
                        let event: &(dyn std::any::Any + Send + Sync) = event.as_ref();
                        handler.handle(event, connection).await
                    })
                },
            )
            .await
            .expect("the handler should succeed");

            if ran {
                applied += 1;
            }
        }

        applied
    }
}
