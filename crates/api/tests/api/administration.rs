//! Administration through the whole host: the three-policy matrix, the read
//! shapes and their quirks, and the Genre write surface. Ported from the
//! earlier suite, with upstream's changes since: Customer lost `Invoices`,
//! Genre lost `Tracks`, reports-to answers the manager, and the validation
//! filter now refuses a blank name.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::common::http::{TestRequest, TestResponse};
use crate::common::{PASSWORD, TestHost};
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use shared_kernel::Environment;

fn users() -> Value {
    json!([
        { "Username": "admin", "Password": PASSWORD, "UserId": "admin-1", "Tenant": "tenant-1",
          "Roles": ["Admin"], "Permissions": ["administration.read", "administration.write"] },
        { "Username": "reader", "Password": PASSWORD, "UserId": "user-1", "Tenant": "tenant-1",
          "Roles": ["Admin"], "Permissions": ["administration.read"] },
        { "Username": "roleless", "Password": PASSWORD, "UserId": "user-2", "Tenant": "tenant-1",
          "Roles": ["User"], "Permissions": ["administration.read", "administration.write"] },
        { "Username": "tenantless", "Password": PASSWORD, "UserId": "user-3",
          "Roles": ["Admin"], "Permissions": ["administration.read", "administration.write"] }
    ])
}

async fn host() -> TestHost {
    crate::common::host_with(
        Environment::Development,
        json!({ "Identity": { "InMemoryUsers": users() } }),
    )
    .await
}

async fn get(host: &TestHost, uri: &str, token: Option<&str>) -> TestResponse {
    let mut request = TestRequest::get(uri);
    if let Some(token) = token {
        request = request.bearer(token);
    }
    host.send(request).await
}

async fn write(
    host: &TestHost,
    method: Method,
    uri: &str,
    token: &str,
    body: Option<&str>,
) -> TestResponse {
    let mut request = TestRequest::new(method, uri).bearer(token);
    if let Some(body) = body {
        request = request.raw("application/json", body);
    }
    host.send(request).await
}

const READ_ROUTES: [&str; 13] = [
    "/api/admin/customers/1",
    "/api/admin/customers/1/purchases",
    "/api/admin/customers",
    "/api/admin/customers/support-rep/3",
    "/api/admin/employees/1",
    "/api/admin/employees",
    "/api/admin/employees/2/direct-reports",
    "/api/admin/employees/2/reports-to",
    "/api/admin/genres/1",
    "/api/admin/genres",
    "/api/admin/media-types/1",
    "/api/admin/media-types",
    "/api/admin/customers/",
];

#[tokio::test]
async fn the_three_policy_matrix_holds() {
    let host = host().await;
    let admin = host.token("admin").await;
    let roleless = host.token("roleless").await;
    let tenantless = host.token("tenantless").await;

    for uri in READ_ROUTES {
        assert_eq!(
            get(&host, uri, None).await.status,
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
        assert_eq!(
            get(&host, uri, Some(&admin)).await.status,
            StatusCode::OK,
            "{uri}"
        );
        assert_eq!(
            get(&host, uri, Some(&roleless)).await.status,
            StatusCode::FORBIDDEN,
            "{uri} needs the role"
        );
    }
    assert_eq!(
        get(&host, "/api/admin/genres/1", Some(&tenantless))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn the_read_permission_does_not_open_the_write_endpoints() {
    let host = host().await;
    let reader = host.token("reader").await;

    for (method, uri, body) in [
        (
            Method::POST,
            "/api/admin/genres",
            Some(r#"{"Name":"Nope"}"#),
        ),
        (
            Method::PUT,
            "/api/admin/genres/1",
            Some(r#"{"Name":"Nope"}"#),
        ),
        (Method::DELETE, "/api/admin/genres/1", None),
    ] {
        assert_eq!(
            write(&host, method.clone(), uri, &reader, body)
                .await
                .status,
            StatusCode::FORBIDDEN,
            "{method} {uri}"
        );
    }

    for (method, uri) in [
        (Method::POST, "/api/admin/genres"),
        (Method::PUT, "/api/admin/genres/1"),
        (Method::DELETE, "/api/admin/genres/1"),
    ] {
        let response = host
            .send(TestRequest::new(method.clone(), uri).raw("application/json", r#"{"Name":"x"}"#))
            .await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

#[tokio::test]
async fn a_customer_by_id_carries_only_what_upstreams_projection_selects() {
    let host = host().await;
    let admin = host.token("admin").await;

    let customer = get(&host, "/api/admin/customers/1", Some(&admin))
        .await
        .json();
    assert_eq!(customer["FirstName"], "Luís");
    assert_eq!(customer["SupportRepId"], 3);
    assert_eq!(customer["SupportRepName"], "Jane Peacock");
    assert_eq!(customer["SupportRep"]["Id"], 3);
    assert!(
        customer.get("Invoices").is_none(),
        "ADR-0004 removed the customer's invoices"
    );
    for absent in [
        "Company",
        "Address",
        "City",
        "State",
        "Country",
        "PostalCode",
        "Phone",
        "Fax",
        "Email",
    ] {
        assert_eq!(
            customer[absent],
            json!(null),
            "{absent} is not projected (P17)"
        );
    }

    let all = get(&host, "/api/admin/customers", Some(&admin))
        .await
        .json();
    let first = all
        .as_array()
        .unwrap()
        .iter()
        .find(|customer| customer["Id"] == 1)
        .unwrap();
    assert_eq!(first["Email"], "luisg@embraer.com.br");
    assert_eq!(
        first["SupportRepId"],
        json!(null),
        "the list does not map SupportRepId"
    );
}

#[tokio::test]
async fn employees_name_their_manager_and_reports_to_answers_the_manager() {
    let host = host().await;
    let admin = host.token("admin").await;

    let employee = get(&host, "/api/admin/employees/2", Some(&admin))
        .await
        .json();
    assert_eq!(employee["FirstName"], "Nancy");
    assert_eq!(employee["ReportsToNavigation"], "Andrew Adams");
    assert_eq!(employee["HireDate"], "2002-05-01T00:00:00Z");
    assert_eq!(employee["Customers"], json!([]));

    let manager = get(&host, "/api/admin/employees/2/reports-to", Some(&admin)).await;
    assert_eq!(manager.status, StatusCode::OK);
    assert_eq!(
        manager.json()["Id"],
        1,
        "665e49c: the manager, not the employee"
    );

    let top = get(&host, "/api/admin/employees/1/reports-to", Some(&admin)).await;
    assert_eq!(
        top.status,
        StatusCode::NOT_FOUND,
        "the top of the chain reports to no one"
    );

    let reports = get(&host, "/api/admin/employees/2/direct-reports", Some(&admin))
        .await
        .json();
    assert_eq!(reports.as_array().map(Vec::len), Some(3));
}

#[tokio::test]
async fn genres_and_media_types_are_id_and_name_only() {
    let host = host().await;
    let admin = host.token("admin").await;

    assert_eq!(
        get(&host, "/api/admin/genres/1", Some(&admin)).await.json(),
        json!({ "Id": 1, "Name": "Rock" })
    );
    assert_eq!(
        get(&host, "/api/admin/media-types/1", Some(&admin))
            .await
            .json(),
        json!({ "Id": 1, "Name": "MPEG audio file" })
    );
}

#[tokio::test]
async fn a_missing_record_is_a_404_and_a_missing_collection_is_an_empty_200() {
    let host = host().await;
    let admin = host.token("admin").await;

    for uri in [
        "/api/admin/customers/9999",
        "/api/admin/employees/9999",
        "/api/admin/genres/9999",
        "/api/admin/media-types/9999",
        "/api/admin/customers/9999/purchases",
    ] {
        assert_eq!(
            get(&host, uri, Some(&admin)).await.status,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
    assert_eq!(
        get(&host, "/api/admin/customers/support-rep/9999", Some(&admin))
            .await
            .json(),
        json!([])
    );
}

#[tokio::test]
async fn a_customer_with_no_counted_purchases_reports_zeros() {
    let host = host().await;
    let admin = host.token("admin").await;

    let purchases = get(&host, "/api/admin/customers/1/purchases", Some(&admin))
        .await
        .json();
    assert_eq!(purchases["CustomerId"], 1);
    assert_eq!(purchases["TotalSpent"].as_f64(), Some(0.0));
    assert_eq!(purchases["InvoiceCount"], 0);
    assert_eq!(purchases["LastPurchaseAt"], json!(null));
}

#[tokio::test]
async fn the_genre_write_surface_round_trips() {
    let host = host().await;
    let admin = host.token("admin").await;

    let before = get(&host, "/api/admin/genres", Some(&admin))
        .await
        .json()
        .as_array()
        .unwrap()
        .len();

    let created = write(
        &host,
        Method::POST,
        "/api/admin/genres",
        &admin,
        Some(r#"{"Name":"Shoegaze"}"#),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let id = created.json()["Id"].as_i64().unwrap();
    assert_eq!(created.json(), json!({ "Id": id, "Name": "Shoegaze" }));
    assert_eq!(
        created.header("location"),
        Some(format!("/api/admin/genres/{id}"))
    );

    let lowercase = write(
        &host,
        Method::POST,
        "/api/admin/genres",
        &admin,
        Some(r#"{"name":"Dream Pop"}"#),
    )
    .await;
    assert_eq!(lowercase.status, StatusCode::CREATED);
    assert_eq!(lowercase.json()["Name"], "Dream Pop");

    let after = get(&host, "/api/admin/genres", Some(&admin))
        .await
        .json()
        .as_array()
        .unwrap()
        .len();
    assert_eq!(
        after,
        before + 2,
        "F1: the collection is invalidated, not served stale"
    );

    let updated = write(
        &host,
        Method::PUT,
        &format!("/api/admin/genres/{id}"),
        &admin,
        Some(r#"{"Name":"After"}"#),
    )
    .await;
    assert_eq!(updated.status, StatusCode::OK);
    assert_eq!(updated.json(), json!({ "id": id, "name": "After" }));
    assert_eq!(
        get(&host, &format!("/api/admin/genres/{id}"), Some(&admin))
            .await
            .json()["Name"],
        "After"
    );

    let deleted = write(
        &host,
        Method::DELETE,
        &format!("/api/admin/genres/{id}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert!(deleted.body.is_empty());
    assert_eq!(
        get(&host, &format!("/api/admin/genres/{id}"), Some(&admin))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        write(
            &host,
            Method::DELETE,
            &format!("/api/admin/genres/{id}"),
            &admin,
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        write(
            &host,
            Method::PUT,
            "/api/admin/genres/9999",
            &admin,
            Some(r#"{"Name":"Ghost"}"#)
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn invalid_names_fail_validation_with_upstreams_document() {
    let host = host().await;
    let admin = host.token("admin").await;

    for body in [
        r#"{"Name":null}"#,
        r#"{"Name":""}"#,
        r#"{"Name":"   "}"#,
        "{}",
    ] {
        let response = write(&host, Method::POST, "/api/admin/genres", &admin, Some(body)).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{body}");
        let document = response.json();
        assert_eq!(
            document,
            json!({
                "type": "https://www.rfc-editor.org/rfc/rfc9110#section-15.5.1",
                "title": "Request validation failed.",
                "status": 400,
                "detail": "One or more validation errors occurred.",
                "errors": { "Name": ["'Name' must not be empty."] },
                "traceId": document["traceId"]
            }),
            "{body}"
        );
    }

    let long = format!(r#"{{"Name":"{}"}}"#, "x".repeat(121));
    let too_long = write(
        &host,
        Method::POST,
        "/api/admin/genres",
        &admin,
        Some(&long),
    )
    .await;
    assert_eq!(
        too_long.json()["errors"]["Name"],
        json!([
            "The length of 'Name' must be 120 characters or fewer. You entered 121 characters."
        ])
    );

    let update = write(
        &host,
        Method::PUT,
        "/api/admin/genres/9999",
        &admin,
        Some(&long),
    )
    .await;
    assert_eq!(
        update.status,
        StatusCode::BAD_REQUEST,
        "the filter runs before the existence check"
    );

    let exact = format!(r#"{{"Name":"{}"}}"#, "x".repeat(120));
    assert_eq!(
        write(
            &host,
            Method::POST,
            "/api/admin/genres",
            &admin,
            Some(&exact)
        )
        .await
        .status,
        StatusCode::CREATED
    );

    let malformed = write(
        &host,
        Method::POST,
        "/api/admin/genres",
        &admin,
        Some(r#"{"Name":"#),
    )
    .await;
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
    assert_eq!(malformed.json()["title"], "Malformed request.");
}

#[tokio::test]
async fn deleting_a_genre_still_used_by_tracks_succeeds() {
    // ADR-0006: no foreign key crosses into Administration any more, so this
    // used to be a 500 and is now a 204, leaving the tracks an orphan finding.
    let host = host().await;
    let admin = host.token("admin").await;

    assert_eq!(
        write(&host, Method::DELETE, "/api/admin/genres/25", &admin, None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
}
