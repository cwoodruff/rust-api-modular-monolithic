//! Port of upstream's `EndpointFilterTests`: a group's filters run before an
//! endpoint's, each in the order added; and the validation filter refuses a
//! bad body before the handler runs.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use crate::common::http::TestRequest;
use axum::http::StatusCode;
use serde_json::json;
use shared_kernel::endpoints::{Endpoints, HostInfo, filter};
use shared_kernel::{Environment, validation};

#[tokio::test]
async fn group_filters_run_before_endpoint_filters_each_in_order() {
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    let mut endpoints = Endpoints::new(
        Arc::new(test_support::auth::TestPolicies),
        HostInfo {
            environment: Environment::Development,
            service_name: "test".to_owned(),
        },
    );

    {
        let record = |label: &'static str| {
            let order = Arc::clone(&order);
            filter(move |request, next| {
                order.lock().unwrap().push(label);
                next.run(request)
            })
        };

        let mut group = endpoints.map_group("/t", "Test", ());
        group
            .add_filter(record("group 1"))
            .add_filter(record("group 2"));
        let handler_order = Arc::clone(&order);
        group
            .map_get("/x", move || {
                handler_order.lock().unwrap().push("handler");
                async { "ok" }
            })
            .add_filter(record("endpoint 1"))
            .add_filter(record("endpoint 2"));
    }

    let (router, _) = endpoints.into_parts();
    let response = TestRequest::get("/t/x").send(&router).await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        *order.lock().unwrap(),
        ["group 1", "group 2", "endpoint 1", "endpoint 2", "handler"]
    );
}

#[tokio::test]
async fn an_invalid_body_is_refused_before_the_handler_runs() {
    let host = crate::common::host().await;
    let token = host.token("admin").await;
    let count = || async {
        host.send(TestRequest::get("/api/admin/genres").bearer(&token))
            .await
            .json()
            .as_array()
            .unwrap()
            .len()
    };
    let before = count().await;

    let response = host
        .send(
            TestRequest::post("/api/admin/genres")
                .bearer(&token)
                .json(&json!({ "name": "x".repeat(121) })),
        )
        .await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        response.header("content-type").as_deref(),
        Some("application/problem+json")
    );
    let document = response.json();
    assert_eq!(document["title"], validation::TITLE);
    assert_eq!(document["detail"], validation::DETAIL);
    assert_eq!(document["type"], validation::TYPE);
    assert_eq!(document["status"], 400);
    assert!(
        document["traceId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert_eq!(
        document["errors"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["Name"]
    );
    assert_eq!(count().await, before, "the handler never ran");
}
