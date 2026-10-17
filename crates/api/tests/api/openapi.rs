//! Port of upstream's `OpenApiDocumentTests`: one document per module, each
//! holding only that module's paths, and `v1` holding them all.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::common::http::TestRequest;
use axum::http::StatusCode;

const DOCUMENTS: [(&str, &str); 5] = [
    ("catalog", "/api/catalog/"),
    ("orders", "/api/orders/"),
    ("admin", "/api/admin/"),
    ("identity", "/api/identity/"),
    ("reporting", "/api/reporting/"),
];

#[tokio::test]
async fn every_module_document_holds_only_its_modules_paths_and_v1_holds_them_all() {
    let host = crate::common::host().await;

    let v1 = host
        .send(TestRequest::get("/swagger/v1/swagger.json"))
        .await;
    assert_eq!(v1.status, StatusCode::OK);
    let all = v1.json()["paths"].as_object().unwrap().clone();
    assert!(all.contains_key("/"), "the root is in v1");
    assert!(all.contains_key("/healthz"));

    for (document, prefix) in DOCUMENTS {
        let response = host
            .send(TestRequest::get(&format!(
                "/swagger/{document}/swagger.json"
            )))
            .await;
        assert_eq!(response.status, StatusCode::OK, "{document}");
        let spec = response.json();
        let paths = spec["paths"].as_object().unwrap();
        assert!(!paths.is_empty(), "{document}");
        for path in paths.keys() {
            assert!(path.starts_with(prefix), "{document} holds {path}");
            assert!(all.contains_key(path), "v1 lacks {path}");
        }
        assert!(
            spec["info"]["title"]
                .as_str()
                .unwrap()
                .starts_with("Modular Monolith API: ")
        );
    }

    let finalize = &all["/api/orders/invoices/{id}/finalize"]["post"];
    assert_eq!(finalize["operationId"], "OrdersFinalizeInvoice");
    assert!(
        finalize["description"]
            .as_str()
            .unwrap()
            .contains("salesCountersUpdate")
    );
    assert!(finalize["responses"]["202"].is_object());

    assert_eq!(
        host.send(TestRequest::get("/swagger/nope/swagger.json"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}
