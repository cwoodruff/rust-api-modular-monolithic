//! Port of upstream's `BulkheadTests` (ADR-0013): a module's rate limit is its
//! own, and each module that has background work has its own queue.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::http::TestRequest;
use serde_json::json;
use shared_kernel::Environment;

#[tokio::test]
async fn exhausting_catalogs_budget_leaves_orders_untouched() {
    let host = common::host_with(
        Environment::Development,
        json!({ "RateLimiting": { "Policies": { "catalog:api": { "PermitLimit": 3, "WindowSeconds": 600 } } } }),
    )
    .await;

    let mut statuses = Vec::new();
    for _ in 0..5 {
        statuses.push(
            host.send(TestRequest::get("/api/catalog/health"))
                .await
                .status
                .as_u16(),
        );
    }
    assert_eq!(statuses, [200, 200, 200, 429, 429]);

    assert_eq!(
        host.send(TestRequest::get("/api/orders/health"))
            .await
            .status,
        200
    );
    assert_eq!(host.send(TestRequest::get("/")).await.status, 200);
}

#[tokio::test]
async fn the_work_queues_are_exactly_orders_and_reportings() {
    let host = common::host().await;

    let queues: Vec<String> = host
        .host
        .services()
        .hosted_services()
        .iter()
        .map(|service| service.name())
        .filter(|name| name.ends_with("WorkQueue"))
        .collect();

    assert_eq!(
        queues,
        ["OrdersWorkQueue", "ReportingWorkQueue"],
        "each module's queue is started, not just the first"
    );
}

#[tokio::test]
async fn filling_one_modules_cache_never_evicts_anothers() {
    let host = common::host_with(
        Environment::Development,
        json!({ "Caching": { "Modules": { "Catalog": { "SizeLimit": 2 } } } }),
    )
    .await;
    let token = host.token("admin").await;

    host.send(TestRequest::get("/api/orders/invoices/1").bearer(&token))
        .await;
    for id in 1..=5 {
        host.send(TestRequest::get(&format!("/api/catalog/albums/{id}")).bearer(&token))
            .await;
    }

    let caches = host.host.services().caches();
    assert!(
        caches["Catalog"].entry_count().await <= 2,
        "Catalog stays within its limit"
    );
    assert_eq!(
        caches["Orders"].entry_count().await,
        1,
        "Orders' entry survived Catalog's churn"
    );
}

#[tokio::test]
async fn a_request_is_counted_on_its_modules_meter() {
    let host = common::host().await;
    let token = host.token("demo").await;

    host.send(TestRequest::get("/api/catalog/albums/1").bearer(&token))
        .await;
    host.send(TestRequest::get("/api/catalog/albums/1").bearer(&token))
        .await;

    let meters = host.host.services().meters();
    let catalog = &meters["Catalog"];
    assert_eq!(catalog.name(), "ModularMonolith.Catalog");
    assert_eq!(catalog.requests_with_status(200), 2);
    assert!(catalog.total(shared_kernel::diagnostics::CACHE_HITS) >= 1);
    assert_eq!(
        meters["Orders"].total(shared_kernel::diagnostics::REQUESTS),
        0
    );
    assert!(
        catalog
            .measurements()
            .iter()
            .all(|reading| reading.tags[0] == ("module".to_owned(), "Catalog".to_owned()))
    );
}
