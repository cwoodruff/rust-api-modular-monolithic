//! What needs more than one module: an invoice finalized in Orders reaching
//! Catalog and Administration. Port of upstream's `FinalizedInvoiceFlowTests`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::common::http::TestRequest;

#[tokio::test]
async fn a_finalized_invoice_reaches_both_consumers() {
    let host = crate::common::host().await;
    let token = host.token("admin").await;

    let invoice = host
        .send(TestRequest::get("/api/orders/invoices/1").bearer(&token))
        .await
        .json();
    let customer = invoice["CustomerId"].as_i64().unwrap();
    let track = invoice["InvoiceLines"][0]["TrackId"].as_i64().unwrap();

    let finalized = host
        .send(TestRequest::post("/api/orders/invoices/1/finalize").bearer(&token))
        .await;
    assert_eq!(finalized.status, 202);
    assert_eq!(
        finalized.header("location").as_deref(),
        Some("/api/orders/invoices/1")
    );

    let before = host
        .send(TestRequest::get(&format!("/api/catalog/tracks/{track}/sales")).bearer(&token))
        .await
        .json();
    assert_eq!(
        before["TimesSold"], 0,
        "nothing is counted until the outbox is dispatched"
    );

    let processed = api_dispatch(&host).await;
    assert_eq!(processed, 1);

    let after = host
        .send(TestRequest::get(&format!("/api/catalog/tracks/{track}/sales")).bearer(&token))
        .await
        .json();
    assert_eq!(after["TimesSold"], 1);

    let purchases = host
        .send(
            TestRequest::get(&format!("/api/admin/customers/{customer}/purchases")).bearer(&token),
        )
        .await
        .json();
    assert_eq!(purchases["InvoiceCount"], 1);
    assert_eq!(purchases["TotalSpent"].as_f64(), invoice["Total"].as_f64());
}

/// Runs one dispatcher batch, as upstream's test calls `ProcessBatchAsync`.
async fn api_dispatch(host: &crate::common::TestHost) -> usize {
    host.host
        .dispatch_outbox_once()
        .await
        .expect("the batch should run")
}
