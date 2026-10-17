//! Orders' outbox, end to end on its own schema. Port of upstream's `OrdersOutboxTests`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use identity_contracts::permissions::{ORDERS_READ, ORDERS_WRITE};
use module_tests::{Data, ModuleTestHost, Which};
use test_support::auth::TestUser;
use test_support::http::TestRequest;

fn writer() -> TestUser {
    TestUser::with(&[ORDERS_READ, ORDERS_WRITE])
}

async fn outbox_rows(host: &ModuleTestHost) -> Vec<(String, serde_json::Value)> {
    let mut connection = host.connect().await;
    sqlx::query_as::<_, (String, sqlx::types::Json<serde_json::Value>)>(
        r#"SELECT "EventType", "Payload" FROM orders."OutboxMessage""#,
    )
    .fetch_all(&mut connection)
    .await
    .unwrap()
    .into_iter()
    .map(|(event_type, payload)| (event_type, payload.0))
    .collect()
}

#[tokio::test]
async fn finalize_writes_exactly_one_outbox_row_with_the_invoice_change() {
    let host = ModuleTestHost::start(Which::Orders, Data::Seeded).await;

    assert_eq!(
        host.send_as(
            &writer(),
            TestRequest::post("/api/orders/invoices/1/finalize")
        )
        .await
        .status,
        202
    );

    let rows = outbox_rows(&host).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "Orders.Contracts.Events.InvoiceFinalized");
    assert_eq!(rows[0].1["invoiceId"], 1);
    assert_eq!(
        rows[0].1["customerId"], 46,
        "invoice 1 bills customer 46 in the seed"
    );
    assert!(!rows[0].1["lines"].as_array().unwrap().is_empty());

    assert_eq!(
        host.services().meters()["Orders"].total(shared_kernel::diagnostics::OUTBOX_PUBLISHED),
        1
    );
}

#[tokio::test]
async fn finalizing_twice_publishes_once() {
    let host = ModuleTestHost::start(Which::Orders, Data::Seeded).await;

    host.send_as(
        &writer(),
        TestRequest::post("/api/orders/invoices/3/finalize"),
    )
    .await;
    assert_eq!(
        host.send_as(
            &writer(),
            TestRequest::post("/api/orders/invoices/3/finalize")
        )
        .await
        .status,
        409
    );

    assert_eq!(outbox_rows(&host).await.len(), 1);
}

#[tokio::test]
async fn when_the_outbox_write_fails_the_invoice_change_rolls_back() {
    let host = ModuleTestHost::start(Which::Orders, Data::Seeded).await;
    let mut connection = host.connect().await;
    sqlx::query(r#"ALTER TABLE orders."OutboxMessage" RENAME TO "OutboxMessageGone""#)
        .execute(&mut connection)
        .await
        .unwrap();

    let failed = host
        .send_as(
            &writer(),
            TestRequest::post("/api/orders/invoices/4/finalize"),
        )
        .await;
    assert_eq!(failed.status, 500);

    let status: String =
        sqlx::query_scalar(r#"SELECT "Status" FROM orders."Invoice" WHERE "Id" = 4"#)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(
        status, "Draft",
        "the status change and the outbox row commit together or not at all"
    );
}

#[tokio::test]
async fn a_dead_letter_is_listed_and_a_retry_sends_it_back_for_delivery() {
    let host = ModuleTestHost::start(Which::Orders, Data::Empty).await;
    let id = uuid::Uuid::new_v4();
    let mut connection = host.connect().await;
    sqlx::query(
        r#"INSERT INTO orders."OutboxMessage" ("Id", "EventType", "Payload", "OccurredAt", "Attempts", "LastError", "DeadLetteredAt", "NextAttemptAt")
           VALUES ($1, 'Orders.Contracts.Events.InvoiceFinalized', '{}'::jsonb, now(), 6, 'boom', now(), now())"#,
    )
    .bind(id)
    .execute(&mut connection)
    .await
    .unwrap();

    let admin = TestUser::with(&[]).admin();
    let listed = host
        .send_as(&admin, TestRequest::get("/api/orders/outbox/dead-letters"))
        .await
        .json();
    assert_eq!(listed[0]["Id"], id.to_string());
    assert_eq!(listed[0]["Attempts"], 6);
    assert_eq!(listed[0]["LastError"], "boom");
    assert!(
        listed[0]["DeadLetteredAt"]
            .as_str()
            .unwrap()
            .ends_with("+00:00"),
        "a DateTimeOffset"
    );

    let retried = host
        .send_as(
            &admin,
            TestRequest::post(&format!("/api/orders/outbox/dead-letters/{id}/retry")),
        )
        .await;
    assert_eq!(retried.status, 204);

    let (attempts, dead, last_error): (i32, Option<chrono::DateTime<chrono::Utc>>, Option<String>) =
        sqlx::query_as(r#"SELECT "Attempts", "DeadLetteredAt", "LastError" FROM orders."OutboxMessage" WHERE "Id" = $1"#)
            .bind(id)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(attempts, 0);
    assert!(dead.is_none());
    assert_eq!(
        last_error.as_deref(),
        Some("boom"),
        "the last error is kept"
    );

    let after = host
        .send_as(&admin, TestRequest::get("/api/orders/outbox/dead-letters"))
        .await
        .json();
    assert_eq!(after, serde_json::json!([]));
}
