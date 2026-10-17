//! Orders on its own host. Port of upstream's `OrdersEndpointsTests`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use identity_contracts::permissions::{ORDERS_READ, ORDERS_WRITE};
use module_tests::{Data, ModuleTestHost, Which};
use test_support::auth::TestUser;
use test_support::http::TestRequest;

fn reader() -> TestUser {
    TestUser::with(&[ORDERS_READ])
}

fn writer() -> TestUser {
    TestUser::with(&[ORDERS_READ, ORDERS_WRITE])
}

#[tokio::test]
async fn reads_need_a_caller_the_permission_and_the_right_tenant() {
    let host = ModuleTestHost::start(Which::Orders, Data::Seeded).await;

    assert_eq!(
        host.send(TestRequest::get("/api/orders/invoices/1"))
            .await
            .status,
        401
    );
    assert_eq!(
        host.send_as(
            &TestUser::with(&[]),
            TestRequest::get("/api/orders/invoices/1")
        )
        .await
        .status,
        403
    );
    assert_eq!(
        host.send_as(
            &reader().asking_for_tenant("other"),
            TestRequest::get("/api/orders/invoices/1")
        )
        .await
        .status,
        403
    );
}

#[tokio::test]
async fn an_invoice_carries_its_status_and_lines_but_no_customer() {
    let host = ModuleTestHost::start(Which::Orders, Data::Seeded).await;

    let invoice = host
        .send_as(&reader(), TestRequest::get("/api/orders/invoices/1"))
        .await
        .json();
    assert_eq!(
        invoice["Status"], "Draft",
        "the seed's invoices all start as Draft"
    );
    assert_eq!(invoice["InvoiceDate"], "2007-01-02T00:00:00Z");
    assert!(
        invoice.get("Customer").is_none(),
        "Orders no longer joins into Administration"
    );
    assert!(!invoice["InvoiceLines"].as_array().unwrap().is_empty());
    assert!(invoice["InvoiceLines"][0].get("TrackName").is_none());

    let line = host
        .send_as(&reader(), TestRequest::get("/api/orders/invoice-lines/1"))
        .await
        .json();
    assert_eq!(line["InvoiceId"], 1);
    assert!(line["Invoice"].is_null());

    let all = host
        .send_as(&reader(), TestRequest::get("/api/orders/invoices/"))
        .await
        .json();
    assert_eq!(all.as_array().unwrap().len(), 458);
    assert_eq!(all[0]["InvoiceLines"], serde_json::json!([]));

    assert_eq!(
        host.send_as(&reader(), TestRequest::get("/api/orders/invoices/999999"))
            .await
            .status,
        404
    );
    assert_eq!(
        host.send_as(
            &reader(),
            TestRequest::get("/api/orders/invoice-lines/999999")
        )
        .await
        .status,
        404
    );

    let by_customer = host
        .send_as(
            &reader(),
            TestRequest::get("/api/orders/invoices/customer/2"),
        )
        .await
        .json();
    assert!(
        by_customer
            .as_array()
            .unwrap()
            .iter()
            .all(|invoice| invoice["CustomerId"] == 2)
    );
}

#[tokio::test]
async fn finalize_accepts_then_conflicts() {
    let host = ModuleTestHost::start(Which::Orders, Data::Seeded).await;

    let accepted = host
        .send_as(
            &writer(),
            TestRequest::post("/api/orders/invoices/1/finalize"),
        )
        .await;
    assert_eq!(accepted.status, 202);
    assert_eq!(
        accepted.header("location").as_deref(),
        Some("/api/orders/invoices/1")
    );
    assert_eq!(
        accepted.json(),
        serde_json::json!({ "invoiceId": 1, "status": "Finalized", "salesCountersUpdate": "eventual" })
    );

    let again = host
        .send_as(
            &writer(),
            TestRequest::post("/api/orders/invoices/1/finalize"),
        )
        .await;
    assert_eq!(again.status, 409);
    assert_eq!(again.json()["title"], "Conflict");

    let invoice = host
        .send_as(&reader(), TestRequest::get("/api/orders/invoices/1"))
        .await
        .json();
    assert_eq!(
        invoice["Status"], "Finalized",
        "the by-id entry was evicted on finalize"
    );

    assert_eq!(
        host.send_as(
            &writer(),
            TestRequest::post("/api/orders/invoices/999999/finalize")
        )
        .await
        .status,
        404
    );
    assert_eq!(
        host.send_as(
            &reader(),
            TestRequest::post("/api/orders/invoices/2/finalize")
        )
        .await
        .status,
        403
    );
}

#[tokio::test]
async fn dead_letters_are_for_admins_only() {
    let host = ModuleTestHost::start(Which::Orders, Data::Empty).await;

    assert_eq!(
        host.send_as(
            &reader(),
            TestRequest::get("/api/orders/outbox/dead-letters")
        )
        .await
        .status,
        403
    );

    let admin = TestUser::with(&[]).admin();
    let listed = host
        .send_as(&admin, TestRequest::get("/api/orders/outbox/dead-letters"))
        .await;
    assert_eq!(listed.status, 200);
    assert_eq!(listed.json(), serde_json::json!([]));

    let unknown = format!(
        "/api/orders/outbox/dead-letters/{}/retry",
        uuid::Uuid::new_v4()
    );
    assert_eq!(
        host.send_as(&admin, TestRequest::post(&unknown))
            .await
            .status,
        404
    );
    assert_eq!(
        host.send_as(
            &admin,
            TestRequest::post("/api/orders/outbox/dead-letters/not-a-guid/retry")
        )
        .await
        .status,
        404
    );
}
