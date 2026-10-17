//! Port of upstream's `Samples/CatalogHostTests`: the sample on a database
//! holding only the `catalog` and `orders` schemas, with its feed switched off
//! so each test runs a batch itself.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use catalog_host::CatalogHost;
use module_catalog::CatalogModule;
use serde_json::json;
use shared_kernel::{AppConfig, Environment, Module, Services};
use sqlx::{Connection, PgConnection};
use test_support::http::TestRequest;
use test_support::postgres;

/// Orders' outbox table, as Orders' own migration creates it. Copied rather
/// than depended on: the sample must not reach Orders, even in its tests.
const OUTBOX_DDL: &str = r#"
CREATE SCHEMA IF NOT EXISTS orders;
CREATE TABLE orders."OutboxMessage" (
    "Id" uuid NOT NULL, "EventType" varchar(300) NOT NULL, "Payload" jsonb NOT NULL,
    "OccurredAt" timestamp with time zone NOT NULL, "ProcessedAt" timestamp with time zone NULL,
    "Attempts" integer NOT NULL, "LastError" varchar(2000) NULL,
    "DeadLetteredAt" timestamp with time zone NULL, "NextAttemptAt" timestamp with time zone NOT NULL,
    CONSTRAINT "PK_OutboxMessage" PRIMARY KEY ("Id"));
"#;

fn sample_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The seed's Catalog statements: every `INSERT INTO catalog.` and the
/// `setval` lines naming a catalog table. Port of `ChinookSeed.For`.
fn catalog_seed() -> String {
    let script = std::fs::read_to_string(
        test_support::content_root().join("data/chinook-postgres-seed.sql"),
    )
    .unwrap();
    let mut statements = Vec::new();
    let mut current = String::new();

    for line in script.lines() {
        if current.is_empty() && (line.trim().is_empty() || line.trim_start().starts_with("--")) {
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
            statement.contains("INSERT INTO catalog.") || statement.contains("('catalog.")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn config(database: &str, overrides: serde_json::Value) -> AppConfig {
    let server = postgres::server();
    AppConfig::load_without_user_secrets(&sample_root(), Environment::Development)
        .unwrap()
        .with_overrides(json!({
            "ConnectionStrings": { "AppDatabase": server.connection_string(database) },
            "CatalogHost": { "Feed": { "Enabled": false } },
            "Database": { "MaxPoolSize": 4 }
        }))
        .with_overrides(overrides)
}

async fn template() -> String {
    postgres::server()
        .template("catalog_host", |database| async move {
            let config = Arc::new(config(&database, json!({})));
            let mut services = Services::new(Arc::clone(&config));
            CatalogModule::default()
                .register_services(&mut services, &config)
                .unwrap();
            services
                .databases()
                .get("Catalog")
                .unwrap()
                .migrate()
                .await
                .unwrap();

            let mut connection = postgres::server().connect(&database).await;
            sqlx::raw_sql(OUTBOX_DDL)
                .execute(&mut connection)
                .await
                .unwrap();
            sqlx::raw_sql(&catalog_seed())
                .execute(&mut connection)
                .await
                .unwrap();
            connection.close().await.unwrap();
        })
        .await
}

struct Sample {
    host: CatalogHost,
    database: String,
}

async fn sample(overrides: serde_json::Value) -> Sample {
    let database = postgres::server().clone_of(&template().await).await;
    let host = catalog_host::build(config(&database, overrides), &sample_root())
        .expect("the sample should build");
    Sample { host, database }
}

impl Sample {
    async fn connect(&self) -> PgConnection {
        postgres::server().connect(&self.database).await
    }

    async fn insert_outbox(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        occurred_at: &str,
    ) -> uuid::Uuid {
        let id = uuid::Uuid::new_v4();
        let mut connection = self.connect().await;
        sqlx::query(&format!(
            r#"INSERT INTO orders."OutboxMessage" ("Id", "EventType", "Payload", "OccurredAt", "Attempts", "NextAttemptAt")
               VALUES ($1, $2, $3, {occurred_at}, 0, now())"#
        ))
        .bind(id)
        .bind(event_type)
        .bind(sqlx::types::Json(payload))
        .execute(&mut connection)
        .await
        .unwrap();
        id
    }
}

fn invoice_finalized(event_id: uuid::Uuid) -> serde_json::Value {
    json!({
        "eventId": event_id, "occurredAt": "2026-10-01T12:00:00+00:00", "invoiceId": 1, "customerId": 2,
        "invoiceDate": "2026-10-01T00:00:00Z", "total": 0.99,
        "lines": [{ "trackId": 1, "quantity": 1, "unitPrice": 0.99 }]
    })
}

#[tokio::test]
async fn health_reports_catalog_served_by_the_sample() {
    let sample = sample(json!({})).await;

    let response = TestRequest::get("/api/catalog/health")
        .send(&sample.host.router())
        .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.json()["module"], "Catalog");
    assert_eq!(response.json()["service"], "Catalog.Host");
}

#[tokio::test]
async fn a_data_route_without_a_token_is_unauthorized() {
    let sample = sample(json!({})).await;

    let response = TestRequest::get("/api/catalog/albums/1")
        .send(&sample.host.router())
        .await;

    assert_eq!(response.status, 401);
}

#[tokio::test]
async fn a_token_signed_by_the_published_key_is_accepted() {
    use rsa::pkcs8::{EncodePrivateKey, LineEnding};
    use rsa::traits::PublicKeyParts;

    let key = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let jwks = json!({ "keys": [{
        "kty": "RSA", "use": "sig", "alg": "RS256", "kid": "test-kid",
        "n": URL_SAFE_NO_PAD.encode(key.n().to_bytes_be()),
        "e": URL_SAFE_NO_PAD.encode(key.e().to_bytes_be())
    }]});

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/jwks.json",
            axum::routing::get(move || async move { axum::Json(jwks) }),
        );
        axum::serve(listener, app).await.unwrap();
    });

    let sample = sample(
        json!({ "CatalogHost": { "IdentityJwksUrl": format!("http://{address}/jwks.json") } }),
    )
    .await;

    let now = chrono::Utc::now().timestamp();
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test-kid".to_owned());
    let claims = json!({
        "sub": "user-1", "iss": "https://auth.local", "aud": ["modular-api", "modular-api"],
        "iat": now, "nbf": now, "exp": now + 900,
        "tenant": "tenant-1", "permissions": "catalog.read"
    });
    let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let token = jsonwebtoken::encode(
        &header,
        &claims,
        &jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
    )
    .unwrap();

    let response = TestRequest::get("/api/catalog/albums/1")
        .bearer(&token)
        .send(&sample.host.router())
        .await;

    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(response.json()["Id"], 1);

    let foreign = TestRequest::get("/api/catalog/albums/1")
        .bearer("not-a-valid-jwt")
        .send(&sample.host.router())
        .await;
    assert_eq!(foreign.status, 401);
}

#[tokio::test]
async fn the_feed_reads_by_cursor_and_the_inbox_keeps_a_rerun_from_counting_twice() {
    let sample = sample(json!({})).await;
    let id = uuid::Uuid::new_v4();
    {
        let mut connection = sample.connect().await;
        sqlx::query(
            r#"INSERT INTO orders."OutboxMessage" ("Id", "EventType", "Payload", "OccurredAt", "Attempts", "NextAttemptAt")
               VALUES ($1, 'Orders.Contracts.Events.InvoiceFinalized', $2, now() - interval '1 minute', 0, now())"#,
        )
        .bind(id)
        .bind(sqlx::types::Json(invoice_finalized(id)))
        .execute(&mut connection)
        .await
        .unwrap();
    }

    assert_eq!(sample.host.feed().process_batch().await.unwrap(), 1);
    assert_eq!(
        sample.host.feed().process_batch().await.unwrap(),
        0,
        "the cursor has moved past it"
    );

    let mut connection = sample.connect().await;
    sqlx::query(
        "UPDATE catalog_host.orders_outbox_cursor SET occurred_at = '-infinity' WHERE id = 1",
    )
    .execute(&mut connection)
    .await
    .unwrap();

    assert_eq!(
        sample.host.feed().process_batch().await.unwrap(),
        1,
        "a reset cursor reads the row again"
    );

    let sold: i32 =
        sqlx::query_scalar(r#"SELECT "TimesSold" FROM catalog."TrackSales" WHERE "TrackId" = 1"#)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(sold, 1, "the inbox applied the event once");

    let processed: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar(r#"SELECT "ProcessedAt" FROM orders."OutboxMessage" WHERE "Id" = $1"#)
            .bind(id)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(processed, None, "the feed never writes Orders' outbox");
}

#[tokio::test]
async fn a_row_inside_the_settle_window_waits() {
    let sample = sample(json!({})).await;
    let id = uuid::Uuid::new_v4();
    sample
        .insert_outbox(
            "Orders.Contracts.Events.InvoiceFinalized",
            invoice_finalized(id),
            "now()",
        )
        .await;

    assert_eq!(sample.host.feed().process_batch().await.unwrap(), 0);
}

#[tokio::test]
async fn a_type_catalog_does_not_handle_is_passed_over() {
    let sample = sample(json!({})).await;
    sample
        .insert_outbox(
            "Orders.Contracts.Events.SomethingElse",
            json!({}),
            "now() - interval '1 minute'",
        )
        .await;

    assert_eq!(sample.host.feed().process_batch().await.unwrap(), 1);
}
