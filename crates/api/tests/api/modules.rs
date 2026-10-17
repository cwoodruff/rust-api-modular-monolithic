//! Catalog and Orders through the whole host: the authorization matrix, the
//! shapes each route serves, and the members upstream removed when the modules
//! stopped joining across schemas. Ported from the earlier suite.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::common::http::{TestRequest, TestResponse};
use crate::common::{PASSWORD, TestHost};
use axum::http::StatusCode;
use serde_json::{Value, json};
use shared_kernel::Environment;

fn users() -> Value {
    json!([
        { "Username": "catalog", "Password": PASSWORD, "UserId": "user-1", "Tenant": "tenant-1",
          "Roles": ["User"], "Permissions": ["catalog.read"] },
        { "Username": "orders", "Password": PASSWORD, "UserId": "user-2", "Tenant": "tenant-1",
          "Roles": ["User"], "Permissions": ["orders.read"] },
        { "Username": "both", "Password": PASSWORD, "UserId": "user-3", "Tenant": "tenant-1",
          "Roles": ["User"], "Permissions": ["catalog.read", "orders.read"] },
        { "Username": "tenantless", "Password": PASSWORD, "UserId": "user-4",
          "Roles": ["User"], "Permissions": ["catalog.read", "orders.read"] },
        { "Username": "music", "Password": PASSWORD, "UserId": "user-5", "Tenant": "tenant-1",
          "Roles": ["User"], "Permissions": ["music.read"] }
    ])
}

async fn host() -> TestHost {
    crate::common::host_with(
        Environment::Development,
        json!({ "Identity": { "InMemoryUsers": users() } }),
    )
    .await
}

async fn get(
    host: &TestHost,
    uri: &str,
    token: Option<&str>,
    tenant: Option<&str>,
) -> TestResponse {
    let mut request = TestRequest::get(uri);
    if let Some(token) = token {
        request = request.bearer(token);
    }
    if let Some(tenant) = tenant {
        request = request.header("X-Tenant-Id", tenant);
    }
    host.send(request).await
}

const CATALOG_ROUTES: [&str; 15] = [
    "/api/catalog/albums/1",
    "/api/catalog/albums",
    "/api/catalog/albums/artist/1",
    "/api/catalog/artists/1",
    "/api/catalog/artists",
    "/api/catalog/playlists/1",
    "/api/catalog/playlists",
    "/api/catalog/tracks/1",
    "/api/catalog/tracks/1/sales",
    "/api/catalog/tracks",
    "/api/catalog/tracks/album/1",
    "/api/catalog/tracks/artist/1",
    "/api/catalog/tracks/genre/1",
    "/api/catalog/tracks/mediatype/1",
    "/api/catalog/tracks/playlist/1",
];

const ORDERS_ROUTES: [&str; 7] = [
    "/api/orders/invoices/1",
    "/api/orders/invoices",
    "/api/orders/invoices/customer/1",
    "/api/orders/invoice-lines/1",
    "/api/orders/invoice-lines",
    "/api/orders/invoice-lines/invoice/1",
    "/api/orders/invoice-lines/track/1013",
];

#[tokio::test]
async fn every_data_route_refuses_an_anonymous_caller_with_a_challenge() {
    let host = host().await;

    for uri in CATALOG_ROUTES.iter().chain(&ORDERS_ROUTES) {
        let response = get(&host, uri, None, None).await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(
            response.header("www-authenticate").as_deref(),
            Some("Bearer"),
            "{uri}"
        );
    }
}

#[tokio::test]
async fn each_read_permission_opens_its_own_module_and_nothing_else() {
    let host = host().await;
    let catalog = host.token("catalog").await;
    let orders = host.token("orders").await;

    for uri in CATALOG_ROUTES {
        assert_eq!(
            get(&host, uri, Some(&catalog), None).await.status,
            StatusCode::OK,
            "{uri}"
        );
    }
    for uri in ORDERS_ROUTES {
        assert_eq!(
            get(&host, uri, Some(&catalog), None).await.status,
            StatusCode::FORBIDDEN,
            "{uri}"
        );
        assert_eq!(
            get(&host, uri, Some(&orders), None).await.status,
            StatusCode::OK,
            "{uri}"
        );
    }
    for uri in CATALOG_ROUTES {
        assert_eq!(
            get(&host, uri, Some(&orders), None).await.status,
            StatusCode::FORBIDDEN,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn the_pre_rename_permission_opens_nothing() {
    let host = host().await;
    let music = host.token("music").await;

    assert_eq!(
        get(&host, "/api/catalog/albums/1", Some(&music), None)
            .await
            .status,
        StatusCode::FORBIDDEN,
        "music.read became catalog.read upstream"
    );
    assert_eq!(
        get(&host, "/api/music/albums/1", Some(&music), None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_tenant_rules_hold() {
    let host = host().await;
    let tenantless = host.token("tenantless").await;
    let catalog = host.token("catalog").await;

    assert_eq!(
        get(&host, "/api/catalog/albums/1", Some(&tenantless), None)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&host, "/api/catalog/albums/1", Some(&catalog), None)
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        get(
            &host,
            "/api/catalog/albums/1",
            Some(&catalog),
            Some("tenant-1")
        )
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(
        get(
            &host,
            "/api/catalog/albums/1",
            Some(&catalog),
            Some("tenant-2")
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(
            &host,
            "/api/catalog/albums/1",
            Some(&catalog),
            Some("TENANT-1")
        )
        .await
        .status,
        StatusCode::FORBIDDEN,
        "the comparison is ordinal"
    );
}

#[tokio::test]
async fn an_album_by_id_carries_its_artist_and_tracks_without_cross_module_names() {
    let host = host().await;
    let token = host.token("catalog").await;

    let album = get(&host, "/api/catalog/albums/1", Some(&token), None)
        .await
        .json();
    assert_eq!(album["Title"], "For Those About To Rock We Salute You");
    assert_eq!(album["ArtistName"], "AC/DC");
    assert_eq!(
        album["Artist"],
        json!({ "Id": 1, "Name": "AC/DC", "Albums": [] })
    );
    assert_eq!(album["Tracks"].as_array().map(Vec::len), Some(10));

    let track = &album["Tracks"][0];
    assert_eq!(track["AlbumName"], "For Those About To Rock We Salute You");
    assert_eq!(track["GenreId"], 1);
    for removed in [
        "GenreName",
        "MediaTypeName",
        "Genre",
        "MediaType",
        "InvoiceLines",
    ] {
        assert!(
            track.get(removed).is_none(),
            "{removed} left with ADR-0005 to ADR-0007"
        );
    }
}

#[tokio::test]
async fn a_track_by_id_names_its_album_and_a_listed_one_does_not() {
    let host = host().await;
    let token = host.token("catalog").await;

    let track = get(&host, "/api/catalog/tracks/1", Some(&token), None)
        .await
        .json();
    assert_eq!(
        track,
        json!({
            "Id": 1, "Name": "For Those About To Rock (We Salute You)", "AlbumId": 1, "MediaTypeId": 1,
            "GenreId": 1, "Composer": "Angus Young, Malcolm Young, Brian Johnson", "Milliseconds": 343_719,
            "Bytes": 11_170_334, "UnitPrice": 0.99, "Album": null, "Playlists": [],
            "AlbumName": "For Those About To Rock We Salute You"
        })
    );

    let by_album = get(&host, "/api/catalog/tracks/album/1", Some(&token), None)
        .await
        .json();
    assert_eq!(by_album[0]["AlbumName"], json!(null));
}

#[tokio::test]
async fn the_invoice_route_into_catalog_is_gone() {
    let host = host().await;
    let token = host.token("both").await;

    assert_eq!(
        get(&host, "/api/catalog/tracks/invoice/1", Some(&token), None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_collection_route_leaves_the_artist_name_null_but_the_by_artist_route_fills_it() {
    let host = host().await;
    let token = host.token("catalog").await;

    let all = get(&host, "/api/catalog/albums", Some(&token), None)
        .await
        .json();
    assert_eq!(all[0]["ArtistName"], json!(null));

    let by_artist = get(&host, "/api/catalog/albums/artist/1", Some(&token), None)
        .await
        .json();
    assert_eq!(by_artist[0]["ArtistName"], "AC/DC");
}

#[tokio::test]
async fn both_slash_spellings_of_a_collection_route_answer_the_same() {
    let host = host().await;
    let token = host.token("catalog").await;

    let unslashed = get(&host, "/api/catalog/playlists", Some(&token), None).await;
    let slashed = get(&host, "/api/catalog/playlists/", Some(&token), None).await;
    assert_eq!(unslashed.status, StatusCode::OK);
    assert_eq!(unslashed.json(), slashed.json());
}

#[tokio::test]
async fn a_missing_item_is_a_404_and_a_missing_collection_is_an_empty_200() {
    let host = host().await;
    let token = host.token("both").await;

    let missing = get(&host, "/api/catalog/albums/999999", Some(&token), None).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.json()["title"], "Not Found");

    let unknown_sales = get(
        &host,
        "/api/catalog/tracks/999999/sales",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(unknown_sales.status, StatusCode::NOT_FOUND);

    let empty = get(
        &host,
        "/api/catalog/albums/artist/999999",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(empty.json(), json!([]));
}

#[tokio::test]
async fn an_unsold_track_reports_zero() {
    let host = host().await;
    let token = host.token("catalog").await;

    let sales = get(&host, "/api/catalog/tracks/1/sales", Some(&token), None)
        .await
        .json();
    assert_eq!(
        sales,
        json!({ "TrackId": 1, "TimesSold": 0, "LastSoldAt": null })
    );
}

#[tokio::test]
async fn an_invoice_carries_its_status_and_lines_but_no_customer() {
    let host = host().await;
    let token = host.token("orders").await;

    let invoice = get(&host, "/api/orders/invoices/1", Some(&token), None)
        .await
        .json();
    assert_eq!(invoice["Total"], 3.96);
    assert_eq!(invoice["InvoiceDate"], "2007-01-02T00:00:00Z");
    assert_eq!(invoice["CustomerId"], 46);
    assert_eq!(
        invoice["Status"], "Draft",
        "the seed leaves every invoice a draft"
    );
    assert!(invoice.get("Customer").is_none(), "ADR-0004");
    assert_eq!(invoice["InvoiceLines"].as_array().map(Vec::len), Some(4));
    assert!(
        invoice["InvoiceLines"][0].get("TrackName").is_none(),
        "ADR-0005"
    );

    let listed = get(&host, "/api/orders/invoices", Some(&token), None)
        .await
        .json();
    assert_eq!(listed[0]["Status"], "Draft");
    assert_eq!(listed[0]["InvoiceLines"], json!([]));
}

#[tokio::test]
async fn the_invoice_line_by_id_route_serves_its_shape() {
    let host = host().await;
    let token = host.token("orders").await;

    let line = get(&host, "/api/orders/invoice-lines/1", Some(&token), None)
        .await
        .json();
    assert_eq!(
        line,
        json!({ "Id": 1, "InvoiceId": 1, "TrackId": 3027, "UnitPrice": 0.99, "Quantity": 1, "Invoice": null })
    );
}

#[tokio::test]
async fn collections_return_every_row_because_nothing_pages() {
    let host = host().await;
    let token = host.token("both").await;

    for (uri, expected) in [
        ("/api/catalog/albums", 347),
        ("/api/catalog/artists", 275),
        ("/api/catalog/tracks", 3503),
        ("/api/catalog/playlists", 18),
        ("/api/orders/invoices", 458),
        ("/api/orders/invoice-lines", 2662),
    ] {
        let body = get(&host, uri, Some(&token), None).await.json();
        assert_eq!(body.as_array().map(Vec::len), Some(expected), "{uri}");
    }
}

#[tokio::test]
async fn a_repeated_read_is_the_same_answer() {
    let host = host().await;
    let token = host.token("catalog").await;

    let first = get(&host, "/api/catalog/albums/1", Some(&token), None).await;
    let second = get(&host, "/api/catalog/albums/1", Some(&token), None).await;
    assert_eq!(first.json(), second.json());
}
