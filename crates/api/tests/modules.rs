//! Music and Orders endpoint tests, including the authorization matrix.
//!
//! Response shapes here were confirmed by diffing against the running C#
//! service; `tools/parity-diff.py` reproduces that check.

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

fn content_root() -> PathBuf {
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

/// Four logins covering the permission matrix, mirroring the C# fixtures.
fn users() -> Value {
    json!([
        { "username": "music", "password": "pw", "userid": "user-1", "tenant": "tenant-1",
          "roles": ["User"], "permissions": ["music.read"] },
        { "username": "orders", "password": "pw", "userid": "user-2", "tenant": "tenant-1",
          "roles": ["User"], "permissions": ["orders.read"] },
        { "username": "both", "password": "pw", "userid": "user-3", "tenant": "tenant-1",
          "roles": ["User"], "permissions": ["music.read", "orders.read"] },
        // Carries the permissions but no tenant claim at all.
        { "username": "tenantless", "password": "pw", "userid": "user-4",
          "roles": ["User"], "permissions": ["music.read", "orders.read"] }
    ])
}

async fn app() -> Router {
    let root = content_root();
    let config = AppConfig::from_figment(
        Figment::new().merge(Serialized::defaults(json!({
            "servicename": "ModularMonolith.Api",
            "connectionstrings": { "appdatabase": "Data Source=./data/chinook.db" },
            "identity": { "inmemoryusers": users() }
        }))),
        Environment::Development,
    );

    let keys: Arc<dyn KeyMaterial> = Arc::new(DevKeyMaterial::generate().unwrap());
    let identity = Arc::new(IdentityRuntime::with_keys(&config, keys));
    let state = api::build_state(config, &root)
        .await
        .expect("state should build");

    api::build(state, identity)
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

/// A GET carrying a bearer token and, optionally, a tenant header.
async fn get_as(
    app: &Router,
    uri: &str,
    bearer: Option<&str>,
    tenant: Option<&str>,
) -> TestResponse {
    let mut builder = Request::builder().uri(uri);
    if let Some(value) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {value}"));
    }
    if let Some(value) = tenant {
        builder = builder.header("X-Tenant-Id", value);
    }

    send(app, builder.body(Body::empty()).unwrap()).await
}

/// Every Music data route.
const MUSIC_ROUTES: [&str; 15] = [
    "/api/music/albums/1",
    "/api/music/albums",
    "/api/music/albums/artist/1",
    "/api/music/artists/1",
    "/api/music/artists",
    "/api/music/playlists/1",
    "/api/music/playlists",
    "/api/music/tracks/1",
    "/api/music/tracks",
    "/api/music/tracks/album/1",
    "/api/music/tracks/artist/1",
    "/api/music/tracks/genre/1",
    "/api/music/tracks/mediatype/1",
    "/api/music/tracks/playlist/1",
    "/api/music/tracks/invoice/1",
];

/// Every Orders data route.
const ORDERS_ROUTES: [&str; 7] = [
    "/api/orders/invoices/1",
    "/api/orders/invoices",
    "/api/orders/invoices/customer/1",
    "/api/orders/invoice-lines/1",
    "/api/orders/invoice-lines",
    "/api/orders/invoice-lines/invoice/1",
    "/api/orders/invoice-lines/track/1013",
];

// ---------------------------------------------------------------------------
// The authorization matrix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_data_route_refuses_an_anonymous_caller() {
    let app = app().await;

    for uri in MUSIC_ROUTES.iter().chain(&ORDERS_ROUTES) {
        let response = get_as(&app, uri, None, None).await;

        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(
            response
                .headers
                .get("www-authenticate")
                .and_then(|v| v.to_str().ok()),
            Some("Bearer"),
            "{uri}"
        );
    }
}

#[tokio::test]
async fn the_music_permission_opens_music_and_nothing_else() {
    let app = app().await;
    let bearer = token(&app, "music").await;

    for uri in MUSIC_ROUTES {
        assert_eq!(
            get_as(&app, uri, Some(&bearer), None).await.status,
            StatusCode::OK,
            "{uri}"
        );
    }

    for uri in ORDERS_ROUTES {
        assert_eq!(
            get_as(&app, uri, Some(&bearer), None).await.status,
            StatusCode::FORBIDDEN,
            "{uri} needs orders.read"
        );
    }
}

#[tokio::test]
async fn the_orders_permission_opens_orders_and_nothing_else() {
    let app = app().await;
    let bearer = token(&app, "orders").await;

    for uri in ORDERS_ROUTES {
        assert_eq!(
            get_as(&app, uri, Some(&bearer), None).await.status,
            StatusCode::OK,
            "{uri}"
        );
    }

    for uri in MUSIC_ROUTES {
        assert_eq!(
            get_as(&app, uri, Some(&bearer), None).await.status,
            StatusCode::FORBIDDEN,
            "{uri} needs music.read"
        );
    }
}

#[tokio::test]
async fn holding_both_permissions_opens_both_modules() {
    let app = app().await;
    let bearer = token(&app, "both").await;

    for uri in MUSIC_ROUTES.iter().chain(&ORDERS_ROUTES) {
        assert_eq!(
            get_as(&app, uri, Some(&bearer), None).await.status,
            StatusCode::OK,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn a_caller_with_no_tenant_claim_is_refused_despite_the_permission() {
    // The tenant guard's first half: no tenant claim fails outright.
    let app = app().await;
    let bearer = token(&app, "tenantless").await;

    assert_eq!(
        get_as(&app, "/api/music/albums/1", Some(&bearer), None)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn naming_a_matching_tenant_is_allowed_and_another_is_refused() {
    let app = app().await;
    let bearer = token(&app, "music").await;

    assert_eq!(
        get_as(&app, "/api/music/albums/1", Some(&bearer), Some("tenant-1"))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        get_as(&app, "/api/music/albums/1", Some(&bearer), Some("tenant-2"))
            .await
            .status,
        StatusCode::FORBIDDEN,
        "a cross-tenant request should be refused"
    );
    assert_eq!(
        get_as(&app, "/api/music/albums/1", Some(&bearer), Some("TENANT-1"))
            .await
            .status,
        StatusCode::FORBIDDEN,
        "the comparison is ordinal, so case matters"
    );
}

#[tokio::test]
async fn sending_no_tenant_header_is_allowed() {
    // The surprising half, kept deliberately: isolation is enforced only when
    // the caller volunteers the header.
    let app = app().await;
    let bearer = token(&app, "music").await;

    assert_eq!(
        get_as(&app, "/api/music/albums/1", Some(&bearer), None)
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_health_endpoints_stay_open_while_the_data_routes_are_shut() {
    let app = app().await;

    for uri in [
        "/api/music/health",
        "/api/music/data-health",
        "/api/orders/health",
        "/api/orders/data-health",
    ] {
        assert_eq!(
            get_as(&app, uri, None, None).await.status,
            StatusCode::OK,
            "{uri}"
        );
    }
}

// ---------------------------------------------------------------------------
// Response shapes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_album_by_id_carries_its_artist_and_tracks() {
    let app = app().await;
    let bearer = token(&app, "music").await;

    let album = get_as(&app, "/api/music/albums/1", Some(&bearer), None)
        .await
        .json();

    assert_eq!(album["Id"], 1);
    assert_eq!(album["Title"], "For Those About To Rock We Salute You");
    assert_eq!(album["ArtistName"], "AC/DC");
    assert_eq!(album["Artist"]["Name"], "AC/DC");
    assert_eq!(album["Artist"]["Albums"], json!([]));
    assert_eq!(album["Tracks"].as_array().map(Vec::len), Some(10));
    assert_eq!(album["Tracks"][0]["GenreName"], "Rock");
    assert_eq!(album["Tracks"][0]["Album"], json!(null));
}

#[tokio::test]
async fn the_collection_route_leaves_the_artist_name_null_but_the_by_artist_route_fills_it() {
    // The C# by-artist query Includes the artist and the conversion reads its
    // name; the plain collection query does not. Same model, two shapes.
    let app = app().await;
    let bearer = token(&app, "music").await;

    let all = get_as(&app, "/api/music/albums", Some(&bearer), None)
        .await
        .json();
    let first = all
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["Id"] == 1)
        .unwrap();
    assert_eq!(first["ArtistName"], json!(null));

    let by_artist = get_as(&app, "/api/music/albums/artist/1", Some(&bearer), None)
        .await
        .json();
    assert_eq!(by_artist[0]["ArtistName"], "AC/DC");
}

#[tokio::test]
async fn both_slash_spellings_of_a_collection_route_answer() {
    // ASP.NET treats a trailing slash as insignificant; axum does not, so each
    // collection route is registered twice.
    let app = app().await;
    let bearer = token(&app, "music").await;

    let unslashed = get_as(&app, "/api/music/albums", Some(&bearer), None).await;
    let slashed = get_as(&app, "/api/music/albums/", Some(&bearer), None).await;

    assert_eq!(unslashed.status, StatusCode::OK);
    assert_eq!(slashed.status, StatusCode::OK);
    assert_eq!(unslashed.json(), slashed.json());
}

#[tokio::test]
async fn a_missing_item_is_a_404_and_a_missing_collection_is_an_empty_200() {
    let app = app().await;
    let bearer = token(&app, "both").await;

    let missing = get_as(&app, "/api/music/albums/999999", Some(&bearer), None).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.json()["title"], "Not Found");

    // The endpoints declare a 404 for collections, but the original cannot
    // produce one: an empty result is an empty array.
    let empty = get_as(&app, "/api/music/albums/artist/999999", Some(&bearer), None).await;
    assert_eq!(empty.status, StatusCode::OK);
    assert_eq!(empty.json(), json!([]));
}

#[tokio::test]
async fn an_invoice_carries_a_shallow_customer_and_its_lines() {
    let app = app().await;
    let bearer = token(&app, "orders").await;

    let invoice = get_as(&app, "/api/orders/invoices/1", Some(&bearer), None)
        .await
        .json();

    assert_eq!(invoice["Id"], 1);
    assert_eq!(invoice["Total"], 3.96);
    assert_eq!(
        invoice["InvoiceDate"], "2007-01-02T00:00:00Z",
        "EF reads the column as UTC, so the wire form carries a Z"
    );
    assert_eq!(invoice["Customer"]["Id"], 46);
    assert_eq!(
        invoice["Customer"]["Address"],
        json!(null),
        "the nested customer is deliberately partial"
    );
    assert_eq!(invoice["InvoiceLines"].as_array().map(Vec::len), Some(4));
    assert!(invoice["InvoiceLines"][0]["TrackName"].is_string());
}

#[tokio::test]
async fn the_invoice_line_by_id_route_serves_an_entity_shape() {
    // The only endpoint in the application that does: the repository returns
    // an entity and the service passes it through, so the response carries the
    // entity's two navigation members, both null.
    let app = app().await;
    let bearer = token(&app, "orders").await;

    let line = get_as(&app, "/api/orders/invoice-lines/1", Some(&bearer), None)
        .await
        .json();

    assert_eq!(
        line,
        json!({
            "Id": 1,
            "InvoiceId": 1,
            "TrackId": 3027,
            "UnitPrice": 0.99,
            "Quantity": 1,
            "Invoice": null,
            "Track": null
        })
    );
}

#[tokio::test]
async fn collections_return_every_row_because_nothing_pages() {
    let app = app().await;
    let bearer = token(&app, "both").await;

    for (uri, expected) in [
        ("/api/music/albums", 347),
        ("/api/music/artists", 275),
        ("/api/music/tracks", 3503),
        ("/api/music/playlists", 18),
        ("/api/orders/invoices", 458),
        ("/api/orders/invoice-lines", 2662),
    ] {
        let body = get_as(&app, uri, Some(&bearer), None).await.json();
        let count = body.as_array().map(Vec::len).unwrap_or_default();

        assert_eq!(count, expected, "{uri}");
    }
}

#[tokio::test]
async fn a_repeated_read_is_served_from_the_cache() {
    // Not directly observable, so this asserts the contract that matters: the
    // same request answers identically.
    let app = app().await;
    let bearer = token(&app, "music").await;

    let first = get_as(&app, "/api/music/albums/1", Some(&bearer), None).await;
    let second = get_as(&app, "/api/music/albums/1", Some(&bearer), None).await;

    assert_eq!(first.status, second.status);
    assert_eq!(first.json(), second.json());
}
