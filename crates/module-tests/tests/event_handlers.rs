//! Catalog's and Administration's `InvoiceFinalized` handlers, through the
//! inbox. Port of upstream's two `InvoiceFinalizedHandlerTests`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{TimeZone, Utc};
use identity_contracts::permissions::{ADMINISTRATION_READ, CATALOG_READ};
use module_tests::{Data, ModuleTestHost, Which};
use orders_contracts::events::{InvoiceFinalized, InvoiceFinalizedLine};
use rust_decimal::Decimal;
use test_support::auth::TestUser;
use test_support::http::TestRequest;

fn event(customer: Option<i32>, day: u32) -> InvoiceFinalized {
    InvoiceFinalized {
        event_id: uuid::Uuid::new_v4(),
        occurred_at: Utc::now(),
        invoice_id: 1,
        customer_id: customer,
        invoice_date: Utc.with_ymd_and_hms(2021, 1, day, 0, 0, 0).unwrap(),
        total: Decimal::new(297, 2),
        lines: vec![
            InvoiceFinalizedLine {
                track_id: 1,
                quantity: 2,
                unit_price: Decimal::new(99, 2),
            },
            InvoiceFinalizedLine {
                track_id: 1,
                quantity: 1,
                unit_price: Decimal::new(99, 2),
            },
            InvoiceFinalizedLine {
                track_id: 2,
                quantity: 1,
                unit_price: Decimal::new(99, 2),
            },
        ],
    }
}

#[tokio::test]
async fn catalog_counts_each_lines_track_once_per_event() {
    let host = ModuleTestHost::start(Which::Catalog, Data::Seeded).await;
    let reader = TestUser::with(&[CATALOG_READ]);
    let first = event(Some(1), 1);

    assert_eq!(host.deliver(&first).await, 1);
    assert_eq!(
        host.deliver(&first).await,
        0,
        "the inbox skips a redelivery"
    );

    let sales = host
        .send_as(&reader, TestRequest::get("/api/catalog/tracks/1/sales"))
        .await
        .json();
    assert_eq!(sales["TimesSold"], 3);
    assert_eq!(sales["LastSoldAt"], "2021-01-01T00:00:00Z");

    // A later invoice moves the date; an earlier one would not.
    host.deliver(&event(Some(1), 5)).await;
    host.deliver(&event(Some(1), 3)).await;
    let sales = host
        .send_as(&reader, TestRequest::get("/api/catalog/tracks/1/sales"))
        .await
        .json();
    assert_eq!(sales["TimesSold"], 9);
    assert_eq!(sales["LastSoldAt"], "2021-01-05T00:00:00Z");

    let inbox: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM catalog."InboxMessage" WHERE "HandlerName" = 'Catalog.Modules.Events.InvoiceFinalizedHandler'"#)
        .fetch_one(&mut host.connect().await)
        .await
        .unwrap();
    assert_eq!(inbox, 3);
}

#[tokio::test]
async fn administration_sums_the_customers_purchases_once_per_event() {
    let host = ModuleTestHost::start(Which::Administration, Data::Seeded).await;
    let reader = TestUser::with(&[ADMINISTRATION_READ]).admin();
    let first = event(Some(1), 1);

    assert_eq!(host.deliver(&first).await, 1);
    assert_eq!(host.deliver(&first).await, 0);

    let purchases = host
        .send_as(
            &reader,
            TestRequest::get("/api/admin/customers/1/purchases"),
        )
        .await
        .json();
    assert_eq!(purchases["InvoiceCount"], 1);
    assert_eq!(purchases["TotalSpent"].as_f64(), Some(2.97));
    assert_eq!(purchases["LastPurchaseAt"], "2021-01-01T00:00:00Z");
}

#[tokio::test]
async fn an_invoice_without_a_customer_changes_no_summary() {
    let host = ModuleTestHost::start(Which::Administration, Data::Seeded).await;

    assert_eq!(
        host.deliver(&event(None, 1)).await,
        1,
        "the inbox row still commits"
    );

    let rows: i64 =
        sqlx::query_scalar(r#"SELECT COUNT(*) FROM administration."CustomerPurchaseSummary""#)
            .fetch_one(&mut host.connect().await)
            .await
            .unwrap();
    assert_eq!(rows, 0);
}
