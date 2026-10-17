//! Port of upstream's `ConfigurationTests` (1f0e053): every per-module limit
//! the host registers has its value written in `appsettings.json`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::collections::BTreeSet;

use serde_json::Value;
use shared_kernel::traffic_control::policies;

fn appsettings() -> Value {
    let path = test_support::content_root().join("appsettings.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn every_registered_limit_has_its_value_in_appsettings() {
    let host = common::host().await;
    let services = host.host.services();
    let file = appsettings();

    let registered: BTreeSet<&str> = services.rate_limiters().keys().copied().collect();
    assert_eq!(
        registered,
        policies::ALL.into_iter().collect::<BTreeSet<_>>(),
        "exactly the six policies"
    );
    for policy in policies::ALL {
        let section = &file["RateLimiting"]["Policies"][policy];
        for member in ["PermitLimit", "WindowSeconds", "QueueLimit"] {
            assert!(
                section[member].is_number(),
                "RateLimiting:Policies:{policy}:{member}"
            );
        }
        let options = services.rate_limiters()[policy].options();
        assert_eq!(
            u64::from(options.permit_limit),
            section["PermitLimit"].as_u64().unwrap(),
            "{policy}"
        );
    }

    let caches: Vec<&str> = services.caches().keys().map(String::as_str).collect();
    assert_eq!(caches, ["Administration", "Catalog", "Orders"]);
    for module in caches {
        let limit = &file["Caching"]["Modules"][module]["SizeLimit"];
        assert!(limit.is_number(), "Caching:Modules:{module}:SizeLimit");
        assert_eq!(services.caches()[module].size_limit(), limit.as_u64());
    }

    let queues: Vec<&str> = services.work_queues().keys().map(String::as_str).collect();
    assert_eq!(queues, ["Orders", "Reporting"]);
    for module in queues {
        assert!(
            file["Concurrency"][module]["QueueCapacity"].is_number(),
            "Concurrency:{module}:QueueCapacity"
        );
    }

    let gates: Vec<&str> = services.gates().keys().map(String::as_str).collect();
    assert_eq!(gates, ["Reporting"]);
    assert!(file["Concurrency"]["Reporting"]["MaxConcurrentExpensive"].is_number());
}
