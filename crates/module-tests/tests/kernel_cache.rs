//! Port of upstream's `ModuleCacheTests`: one module filling its cache never
//! evicts another module's entries (ADR-0013).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use shared_kernel::caching::{CacheFacade, CacheKeyComposer, CacheOptions};
use shared_kernel::diagnostics::ModuleMeter;

#[tokio::test]
async fn going_past_one_modules_limit_evicts_within_that_module_only() {
    let keys = CacheKeyComposer::from_parts("mmapi", "test");
    let catalog = CacheFacade::for_module(
        CacheOptions::default(),
        "Catalog",
        3,
        Arc::new(ModuleMeter::new("Catalog")),
    );
    let orders = CacheFacade::for_module(
        CacheOptions::default(),
        "Orders",
        3,
        Arc::new(ModuleMeter::new("Orders")),
    );

    for id in 0..3 {
        orders
            .set(
                &keys.compose("orders", "invoice", "v1", &format!("by-id:{id}")),
                id,
                None,
            )
            .await;
    }
    for id in 0..10 {
        catalog
            .set(
                &keys.compose("catalog", "album", "v1", &format!("by-id:{id}")),
                id,
                None,
            )
            .await;
    }

    assert!(
        catalog.entry_count().await <= 3,
        "Catalog stays within its own limit"
    );
    assert_eq!(orders.entry_count().await, 3);
    for id in 0..3 {
        assert!(
            orders
                .contains(&keys.compose("orders", "invoice", "v1", &format!("by-id:{id}")))
                .await
        );
    }
}

#[tokio::test]
async fn hits_and_misses_are_counted_on_the_modules_meter() {
    let keys = CacheKeyComposer::from_parts("mmapi", "test");
    let meter = Arc::new(ModuleMeter::new("Catalog"));
    let cache = CacheFacade::for_module(CacheOptions::default(), "Catalog", 10, Arc::clone(&meter));
    let key = keys.compose("catalog", "album", "v1", "by-id:1");

    for _ in 0..3 {
        let _ = cache.get_or_add(&key, || async { Some(1) }, None).await;
    }

    assert_eq!(meter.total(shared_kernel::diagnostics::CACHE_MISSES), 1);
    assert_eq!(meter.total(shared_kernel::diagnostics::CACHE_HITS), 2);
}
