//! Administration endpoint tests, including the only write surface in the
//! application.
//!
//! Shapes here were confirmed by diffing against the running C# service.
//! Writes run against a per-test copy of the database.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use figment::Figment;
use figment::providers::Serialized;
use module_identity::{DevKeyMaterial, IdentityRuntime, KeyMaterial};
use serde_json::{Value, json};
use shared_kernel::{AppConfig, Environment};
use tower::ServiceExt;

fn repository_root() -> PathBuf {
    let mut current = Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        if current.join("appsettings.json").is_file() {
            return current.to_path_buf();
        }
        current = current
            .parent()
            .expect("appsettings.json should be at the root");
    }
}

/// A private copy of the database, removed when the test ends.
struct ScratchDatabase(PathBuf);

impl ScratchDatabase {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("chinook-admin-{}-{label}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::copy(repository_root().join("data/chinook.db"), &path)
            .expect("the database should be copyable");

        Self(path)
    }
}

impl Drop for ScratchDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Four logins covering the three-policy matrix Administration enforces.
fn users() -> Value {
    json!([
        // Everything: the admin role, both permissions, a tenant.
        { "username": "admin", "password": "pw", "userid": "admin-1", "tenant": "tenant-1",
          "roles": ["Admin"], "permissions": ["administration.read", "administration.write"] },
        // The role and read, but not write.
        { "username": "reader", "password": "pw", "userid": "user-1", "tenant": "tenant-1",
          "roles": ["Admin"], "permissions": ["administration.read"] },
        // Both permissions but not the role.
        { "username": "roleless", "password": "pw", "userid": "user-2", "tenant": "tenant-1",
          "roles": ["User"], "permissions": ["administration.read", "administration.write"] },
        // Everything except a tenant claim.
        { "username": "tenantless", "password": "pw", "userid": "user-3",
          "roles": ["Admin"], "permissions": ["administration.read", "administration.write"] }
    ])
}

fn config_for(database: &Path) -> AppConfig {
    AppConfig::from_figment(
        Figment::new().merge(Serialized::defaults(json!({
            "servicename": "ModularMonolith.Api",
            "connectionstrings": { "appdatabase": format!("Data Source={}", database.display()) },
            "identity": { "inmemoryusers": users() }
        }))),
        Environment::Development,
    )
}

async fn app_on(database: &Path) -> Router {
    let config = config_for(database);
    let keys: Arc<dyn KeyMaterial> = Arc::new(DevKeyMaterial::generate().unwrap());
    let identity = Arc::new(IdentityRuntime::with_keys(&config, keys));
    let state = api::build_state(config, &repository_root())
        .await
        .expect("state should build");

    api::build(state, identity)
}

/// A read-only app on the bundled database.
async fn app() -> Router {
    app_on(&repository_root().join("data/chinook.db")).await
}

struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl TestResponse {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected JSON, got {:?}: {error}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
    }
}

async fn send(app: &Router, request: Request<Body>) -> TestResponse {
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("the router should answer");
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();

    TestResponse {
        status,
        headers,
        body,
    }
}

async fn token(app: &Router, username: &str) -> String {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/identity/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "username": username, "password": "pw" }).to_string(),
        ))
        .unwrap();

    let response = send(app, request).await;
    assert_eq!(response.status, StatusCode::OK, "login as {username}");

    response.json()["access_token"].as_str().unwrap().to_owned()
}

async fn get(app: &Router, uri: &str, bearer: Option<&str>) -> TestResponse {
    let mut builder = Request::builder().uri(uri);
    if let Some(value) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {value}"));
    }

    send(app, builder.body(Body::empty()).unwrap()).await
}

async fn write(
    app: &Router,
    method: Method,
    uri: &str,
    bearer: &str,
    body: Option<&str>,
) -> TestResponse {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"));

    let payload = match body {
        Some(content) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(content.to_owned())
        }
        None => Body::empty(),
    };

    send(app, builder.body(payload).unwrap()).await
}

/// Every Administration read route.
const READ_ROUTES: [&str; 12] = [
    "/api/admin/customers/1",
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

// ---------------------------------------------------------------------------
// The three-policy matrix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_read_route_refuses_an_anonymous_caller() {
    let app = app().await;

    for uri in READ_ROUTES {
        let response = get(&app, uri, None).await;

        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{uri}");
    }
}

#[tokio::test]
async fn a_full_administrator_can_read_everything() {
    let app = app().await;
    let bearer = token(&app, "admin").await;

    for uri in READ_ROUTES {
        assert_eq!(
            get(&app, uri, Some(&bearer)).await.status,
            StatusCode::OK,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn the_permission_alone_is_not_enough_without_the_role() {
    // Administration is the only module that also requires a role.
    let app = app().await;
    let bearer = token(&app, "roleless").await;

    for uri in READ_ROUTES {
        assert_eq!(
            get(&app, uri, Some(&bearer)).await.status,
            StatusCode::FORBIDDEN,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn the_role_alone_is_not_enough_without_a_tenant_claim() {
    let app = app().await;
    let bearer = token(&app, "tenantless").await;

    assert_eq!(
        get(&app, "/api/admin/genres/1", Some(&bearer)).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn the_read_permission_does_not_open_the_write_endpoints() {
    let app = app().await;
    let bearer = token(&app, "reader").await;

    assert_eq!(
        get(&app, "/api/admin/genres/1", Some(&bearer)).await.status,
        StatusCode::OK,
        "reading is allowed"
    );

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
            write(&app, method.clone(), uri, &bearer, body).await.status,
            StatusCode::FORBIDDEN,
            "{method} {uri} needs administration.write"
        );
    }
}

// ---------------------------------------------------------------------------
// Read shapes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_customer_by_id_omits_the_nine_columns_the_original_never_projects() {
    // Faithful to the C# query, whose projection has a `// ... other fields ...`
    // comment where these belong.
    let app = app().await;
    let bearer = token(&app, "admin").await;

    let customer = get(&app, "/api/admin/customers/1", Some(&bearer))
        .await
        .json();

    assert_eq!(customer["FirstName"], "Luís");
    assert_eq!(customer["SupportRepName"], "Jane Peacock");
    assert_eq!(customer["SupportRep"]["Id"], 3);
    assert_eq!(customer["Invoices"].as_array().map(Vec::len), Some(8));

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
        assert_eq!(customer[absent], json!(null), "{absent} is not projected");
    }
}

#[tokio::test]
async fn the_customer_collection_does_return_what_the_by_id_route_drops() {
    let app = app().await;
    let bearer = token(&app, "admin").await;

    let all = get(&app, "/api/admin/customers", Some(&bearer))
        .await
        .json();
    let first = all
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["Id"] == 1)
        .unwrap();

    assert_eq!(first["Email"], "luisg@embraer.com.br");
    assert_eq!(
        first["SupportRepId"],
        json!(null),
        "the entity conversion drops this one, unlike the by-id query"
    );
}

#[tokio::test]
async fn an_employee_carries_their_manager_as_a_name() {
    let app = app().await;
    let bearer = token(&app, "admin").await;

    let employee = get(&app, "/api/admin/employees/2", Some(&bearer))
        .await
        .json();

    assert_eq!(employee["FirstName"], "Nancy");
    assert_eq!(employee["ReportsToNavigation"], "Andrew Adams");
    assert_eq!(employee["HireDate"], "2002-05-01T00:00:00Z");
}

#[tokio::test]
async fn the_reports_to_route_returns_the_employee_rather_than_their_manager() {
    // The C# service passes the route's id straight to a plain FindAsync, so
    // despite the name this answers with the employee themselves. Reproduced
    // because the response is wire-visible.
    let app = app().await;
    let bearer = token(&app, "admin").await;

    let answered = get(&app, "/api/admin/employees/2/reports-to", Some(&bearer))
        .await
        .json();

    assert_eq!(answered["Id"], 2);
    assert_eq!(answered["FirstName"], "Nancy");
}

#[tokio::test]
async fn direct_reports_lists_the_employees_below_one_manager() {
    let app = app().await;
    let bearer = token(&app, "admin").await;

    let reports = get(&app, "/api/admin/employees/2/direct-reports", Some(&bearer))
        .await
        .json();

    assert_eq!(reports.as_array().map(Vec::len), Some(3));
    assert!(
        reports
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["ReportsTo"] == 2)
    );
}

#[tokio::test]
async fn a_genre_carries_an_empty_track_list() {
    // The repository returns the entity and the service converts it, so the
    // collection is never populated even though the genre has tracks.
    let app = app().await;
    let bearer = token(&app, "admin").await;

    let genre = get(&app, "/api/admin/genres/1", Some(&bearer)).await.json();

    assert_eq!(genre, json!({ "Id": 1, "Name": "Rock", "Tracks": [] }));
}

#[tokio::test]
async fn a_missing_record_is_a_404_and_a_missing_collection_is_an_empty_200() {
    let app = app().await;
    let bearer = token(&app, "admin").await;

    for uri in [
        "/api/admin/customers/999999",
        "/api/admin/employees/999999",
        "/api/admin/genres/999999",
        "/api/admin/media-types/999999",
    ] {
        assert_eq!(
            get(&app, uri, Some(&bearer)).await.status,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }

    let empty = get(
        &app,
        "/api/admin/customers/support-rep/999999",
        Some(&bearer),
    )
    .await;
    assert_eq!(empty.status, StatusCode::OK);
    assert_eq!(empty.json(), json!([]));
}

// ---------------------------------------------------------------------------
// The write surface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn creating_a_genre_answers_201_with_a_location_and_the_new_model() {
    let scratch = ScratchDatabase::new("create");
    let app = app_on(&scratch.0).await;
    let bearer = token(&app, "admin").await;

    let response = write(
        &app,
        Method::POST,
        "/api/admin/genres",
        &bearer,
        Some(r#"{"Name":"Shoegaze"}"#),
    )
    .await;

    assert_eq!(response.status, StatusCode::CREATED);

    let created = response.json();
    let id = created["Id"].as_i64().expect("a generated key");
    assert!(id > 0);
    assert_eq!(created["Name"], "Shoegaze");
    assert_eq!(created["Tracks"], json!([]));
    assert_eq!(
        response.header("location"),
        Some(format!("/api/admin/genres/{id}")),
        "the Location header names the new record"
    );
}

#[tokio::test]
async fn a_lowercase_member_name_binds_too() {
    // The host leaves the naming policy alone but keeps the web defaults'
    // case-insensitive matching.
    let scratch = ScratchDatabase::new("lowercase");
    let app = app_on(&scratch.0).await;
    let bearer = token(&app, "admin").await;

    let response = write(
        &app,
        Method::POST,
        "/api/admin/genres",
        &bearer,
        Some(r#"{"name":"Dream Pop"}"#),
    )
    .await;

    assert_eq!(response.status, StatusCode::CREATED);
    assert_eq!(response.json()["Name"], "Dream Pop");
}

#[tokio::test]
async fn a_created_genre_is_readable_immediately() {
    let scratch = ScratchDatabase::new("readback");
    let app = app_on(&scratch.0).await;
    let bearer = token(&app, "admin").await;

    let created = write(
        &app,
        Method::POST,
        "/api/admin/genres",
        &bearer,
        Some(r#"{"Name":"Post-Rock"}"#),
    )
    .await
    .json();
    let id = created["Id"].as_i64().unwrap();

    let read_back = get(&app, &format!("/api/admin/genres/{id}"), Some(&bearer)).await;

    assert_eq!(read_back.status, StatusCode::OK);
    assert_eq!(read_back.json()["Name"], "Post-Rock");
}

/// The F1 divergence, demonstrated.
///
/// The C# `RemoveByTagAsync` is a no-op, so after a write its collection
/// endpoint keeps serving the stale list for the rest of the entry's twenty
/// minutes. Verified against the running service: it answered 28 both before
/// and after a create, where this port answers 28 then 29.
#[tokio::test]
async fn a_write_invalidates_the_collection_the_original_leaves_stale() {
    let scratch = ScratchDatabase::new("invalidation");
    let app = app_on(&scratch.0).await;
    let bearer = token(&app, "admin").await;

    let before = get(&app, "/api/admin/genres", Some(&bearer)).await.json();
    let before_count = before.as_array().unwrap().len();

    write(
        &app,
        Method::POST,
        "/api/admin/genres",
        &bearer,
        Some(r#"{"Name":"Krautrock"}"#),
    )
    .await;

    let after = get(&app, "/api/admin/genres", Some(&bearer)).await.json();

    assert_eq!(
        after.as_array().unwrap().len(),
        before_count + 1,
        "the collection entry should have been invalidated, not served stale"
    );
}

#[tokio::test]
async fn updating_a_genre_answers_a_lowercase_echo() {
    // The PUT answers an anonymous object, so its members are lowercase —
    // unlike the POST, which answers a PascalCase API model.
    let scratch = ScratchDatabase::new("update");
    let app = app_on(&scratch.0).await;
    let bearer = token(&app, "admin").await;

    let created = write(
        &app,
        Method::POST,
        "/api/admin/genres",
        &bearer,
        Some(r#"{"Name":"Before"}"#),
    )
    .await
    .json();
    let id = created["Id"].as_i64().unwrap();

    let response = write(
        &app,
        Method::PUT,
        &format!("/api/admin/genres/{id}"),
        &bearer,
        Some(r#"{"Name":"After"}"#),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.json(), json!({ "id": id, "name": "After" }));

    let read_back = get(&app, &format!("/api/admin/genres/{id}"), Some(&bearer))
        .await
        .json();
    assert_eq!(
        read_back["Name"], "After",
        "the by-id entry was invalidated too"
    );
}

#[tokio::test]
async fn deleting_a_genre_answers_204_and_removes_it() {
    let scratch = ScratchDatabase::new("delete");
    let app = app_on(&scratch.0).await;
    let bearer = token(&app, "admin").await;

    let created = write(
        &app,
        Method::POST,
        "/api/admin/genres",
        &bearer,
        Some(r#"{"Name":"Temporary"}"#),
    )
    .await
    .json();
    let id = created["Id"].as_i64().unwrap();

    let deleted = write(
        &app,
        Method::DELETE,
        &format!("/api/admin/genres/{id}"),
        &bearer,
        None,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert!(deleted.body.is_empty());

    assert_eq!(
        get(&app, &format!("/api/admin/genres/{id}"), Some(&bearer))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn writing_to_a_missing_genre_is_a_404_rather_than_a_500() {
    // The repository reports false for a missing row, which is what lets the
    // endpoint answer 404.
    let scratch = ScratchDatabase::new("missing");
    let app = app_on(&scratch.0).await;
    let bearer = token(&app, "admin").await;

    assert_eq!(
        write(
            &app,
            Method::PUT,
            "/api/admin/genres/999999",
            &bearer,
            Some(r#"{"Name":"Nope"}"#)
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        write(
            &app,
            Method::DELETE,
            "/api/admin/genres/999999",
            &bearer,
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_blank_name_fails_validation_with_the_originals_document() {
    let scratch = ScratchDatabase::new("blank");
    let app = app_on(&scratch.0).await;
    let bearer = token(&app, "admin").await;

    let response = write(
        &app,
        Method::POST,
        "/api/admin/genres",
        &bearer,
        Some(r#"{"Name":null}"#),
    )
    .await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json(),
        json!({
            "type": "https://www.rfc-editor.org/rfc/rfc9110#section-15.5.1",
            "title": "Request validation failed.",
            "status": 400,
            "detail": "One or more validation errors occurred.",
            "errors": { "Name": ["'Name' must not be empty."] },
            "traceId": response.json()["traceId"]
        })
    );
}

#[tokio::test]
async fn an_over_long_name_reports_the_length_rule() {
    let scratch = ScratchDatabase::new("toolong");
    let app = app_on(&scratch.0).await;
    let bearer = token(&app, "admin").await;

    let body = format!(r#"{{"Name":"{}"}}"#, "x".repeat(121));
    let response = write(
        &app,
        Method::POST,
        "/api/admin/genres",
        &bearer,
        Some(&body),
    )
    .await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json()["errors"]["Name"],
        json!([
            "The length of 'Name' must be 120 characters or fewer. You entered 121 characters."
        ])
    );
}

#[tokio::test]
async fn a_name_of_exactly_120_characters_is_accepted() {
    let scratch = ScratchDatabase::new("boundary");
    let app = app_on(&scratch.0).await;
    let bearer = token(&app, "admin").await;

    let body = format!(r#"{{"Name":"{}"}}"#, "x".repeat(120));

    assert_eq!(
        write(
            &app,
            Method::POST,
            "/api/admin/genres",
            &bearer,
            Some(&body)
        )
        .await
        .status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn an_update_is_validated_the_same_way_a_create_is() {
    let scratch = ScratchDatabase::new("updatevalid");
    let app = app_on(&scratch.0).await;
    let bearer = token(&app, "admin").await;

    let body = format!(r#"{{"Name":"{}"}}"#, "x".repeat(121));
    let response = write(
        &app,
        Method::PUT,
        "/api/admin/genres/1",
        &bearer,
        Some(&body),
    )
    .await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.json()["title"], "Request validation failed.");
}

#[tokio::test]
async fn unparseable_json_is_a_malformed_request_not_a_validation_failure() {
    let scratch = ScratchDatabase::new("badjson");
    let app = app_on(&scratch.0).await;
    let bearer = token(&app, "admin").await;

    let response = write(
        &app,
        Method::POST,
        "/api/admin/genres",
        &bearer,
        Some("{not json"),
    )
    .await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST);

    let document = response.json();
    assert_eq!(document["title"], "Malformed request.");
    assert_eq!(
        document["type"],
        "https://www.rfc-editor.org/rfc/rfc9110#section-15.5.1"
    );
    // The detail is the parser's own diagnostic and names framework
    // internals, so it is the one member that does not match the original.
    assert!(document["detail"].as_str().is_some_and(|d| !d.is_empty()));
}

#[tokio::test]
async fn the_write_endpoints_refuse_an_anonymous_caller() {
    let app = app().await;

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
        let mut builder = Request::builder().method(method.clone()).uri(uri);
        let payload = match body {
            Some(content) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                Body::from(content)
            }
            None => Body::empty(),
        };

        let response = send(&app, builder.body(payload).unwrap()).await;

        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

#[tokio::test]
async fn a_write_leaves_the_bundled_database_untouched() {
    let scratch = ScratchDatabase::new("isolation");
    let scratch_app = app_on(&scratch.0).await;
    let bearer = token(&scratch_app, "admin").await;

    let bundled = app().await;
    let bundled_bearer = token(&bundled, "admin").await;
    let before = get(&bundled, "/api/admin/genres", Some(&bundled_bearer))
        .await
        .json();

    write(
        &scratch_app,
        Method::POST,
        "/api/admin/genres",
        &bearer,
        Some(r#"{"Name":"Isolated"}"#),
    )
    .await;

    // A fresh app, so nothing is served from the first one's cache.
    let after_app = app().await;
    let after_bearer = token(&after_app, "admin").await;
    let after = get(&after_app, "/api/admin/genres", Some(&after_bearer))
        .await
        .json();

    assert_eq!(
        before.as_array().unwrap().len(),
        after.as_array().unwrap().len(),
        "the committed database should not have changed"
    );
}
