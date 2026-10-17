//! Catalog on its own host. Port of upstream's `CatalogEndpointsTests`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use identity_contracts::permissions::{CATALOG_READ, ORDERS_READ};
use module_tests::{Data, ModuleTestHost, Which};
use test_support::auth::TestUser;
use test_support::http::TestRequest;

fn reader() -> TestUser {
    TestUser::with(&[CATALOG_READ])
}

#[tokio::test]
async fn reads_need_a_caller_the_permission_and_the_right_tenant() {
    let host = ModuleTestHost::start(Which::Catalog, Data::Seeded).await;

    let anonymous = host.send(TestRequest::get("/api/catalog/albums/1")).await;
    assert_eq!(anonymous.status, 401);
    assert_eq!(
        anonymous.header("www-authenticate").as_deref(),
        Some("Bearer")
    );

    let wrong_permission = host
        .send_as(
            &TestUser::with(&[ORDERS_READ]),
            TestRequest::get("/api/catalog/albums/1"),
        )
        .await;
    assert_eq!(wrong_permission.status, 403);

    let wrong_tenant = host
        .send_as(
            &reader().asking_for_tenant("tenant-2"),
            TestRequest::get("/api/catalog/albums/1"),
        )
        .await;
    assert_eq!(wrong_tenant.status, 403);
}

#[tokio::test]
async fn by_id_reads_return_the_seeded_rows_and_404_for_the_rest() {
    let host = ModuleTestHost::start(Which::Catalog, Data::Seeded).await;

    let album = host
        .send_as(&reader(), TestRequest::get("/api/catalog/albums/1"))
        .await
        .json();
    assert_eq!(album["Title"], "For Those About To Rock We Salute You");
    assert_eq!(album["ArtistName"], "AC/DC");
    assert_eq!(album["Artist"]["Name"], "AC/DC");
    assert_eq!(album["Tracks"][0]["AlbumName"], album["Title"]);
    assert!(
        album["Tracks"][0].get("GenreName").is_none(),
        "Catalog no longer joins into Administration"
    );

    let track = host
        .send_as(&reader(), TestRequest::get("/api/catalog/tracks/1"))
        .await
        .json();
    assert_eq!(track["GenreId"], 1);
    assert_eq!(track["AlbumName"], "For Those About To Rock We Salute You");

    for missing in [
        "/api/catalog/albums/999999",
        "/api/catalog/artists/999999",
        "/api/catalog/tracks/999999",
        "/api/catalog/playlists/999999",
    ] {
        assert_eq!(
            host.send_as(&reader(), TestRequest::get(missing))
                .await
                .status,
            404,
            "{missing}"
        );
    }
}

#[tokio::test]
async fn a_non_numeric_id_matches_no_route() {
    let host = ModuleTestHost::start(Which::Catalog, Data::Empty).await;

    // A failed `{id:int}` constraint is "no such route": 404 before authentication is asked.
    assert_eq!(
        host.send(TestRequest::get("/api/catalog/albums/abc"))
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn collections_return_every_seeded_row_with_and_without_the_slash() {
    let host = ModuleTestHost::start(Which::Catalog, Data::Seeded).await;

    let albums = host
        .send_as(&reader(), TestRequest::get("/api/catalog/albums"))
        .await
        .json();
    assert_eq!(albums.as_array().unwrap().len(), 347);
    assert!(
        albums[0]["ArtistName"].is_null(),
        "the collection route does not Include the artist"
    );

    let slashed = host
        .send_as(&reader(), TestRequest::get("/api/catalog/albums/"))
        .await;
    assert_eq!(slashed.status, 200);

    let by_artist = host
        .send_as(&reader(), TestRequest::get("/api/catalog/albums/artist/1"))
        .await
        .json();
    assert_eq!(
        by_artist[0]["ArtistName"], "AC/DC",
        "this route does Include it"
    );

    let playlists = host
        .send_as(&reader(), TestRequest::get("/api/catalog/playlists"))
        .await
        .json();
    assert_eq!(playlists.as_array().unwrap().len(), 18);

    let genre = host
        .send_as(&reader(), TestRequest::get("/api/catalog/tracks/genre/1"))
        .await
        .json();
    assert!(
        genre
            .as_array()
            .unwrap()
            .iter()
            .all(|track| track["GenreId"] == 1)
    );

    let none = host
        .send_as(
            &reader(),
            TestRequest::get("/api/catalog/tracks/album/999999"),
        )
        .await
        .json();
    assert_eq!(none, serde_json::json!([]));
}

#[tokio::test]
async fn an_unsold_track_reports_zero_and_an_unknown_one_404() {
    let host = ModuleTestHost::start(Which::Catalog, Data::Seeded).await;

    let sales = host
        .send_as(&reader(), TestRequest::get("/api/catalog/tracks/1/sales"))
        .await
        .json();
    assert_eq!(
        sales,
        serde_json::json!({ "TrackId": 1, "TimesSold": 0, "LastSoldAt": null })
    );

    assert_eq!(
        host.send_as(
            &reader(),
            TestRequest::get("/api/catalog/tracks/999999/sales")
        )
        .await
        .status,
        404
    );
}

#[tokio::test]
async fn a_repeated_read_returns_the_same_body() {
    let host = ModuleTestHost::start(Which::Catalog, Data::Seeded).await;

    let first = host
        .send_as(&reader(), TestRequest::get("/api/catalog/artists/1"))
        .await;
    let second = host
        .send_as(&reader(), TestRequest::get("/api/catalog/artists/1"))
        .await;

    assert_eq!(first.body, second.body);
}

#[tokio::test]
async fn concurrent_reads_of_one_key_all_succeed() {
    let host = std::sync::Arc::new(ModuleTestHost::start(Which::Catalog, Data::Seeded).await);

    let calls: Vec<_> = (0..16)
        .map(|_| {
            let host = std::sync::Arc::clone(&host);
            tokio::spawn(async move {
                host.send_as(&reader(), TestRequest::get("/api/catalog/playlists/1"))
                    .await
                    .status
            })
        })
        .collect();

    for call in calls {
        assert_eq!(call.await.unwrap(), 200);
    }
}

#[tokio::test]
async fn a_request_is_counted_on_the_catalog_meter() {
    let host = ModuleTestHost::start(Which::Catalog, Data::Seeded).await;

    host.send_as(&reader(), TestRequest::get("/api/catalog/albums/1"))
        .await;
    host.send(TestRequest::get("/api/catalog/albums/1")).await;

    let meter = &host.services().meters()["Catalog"];
    assert_eq!(meter.name(), "ModularMonolith.Catalog");
    assert_eq!(meter.requests_with_status(200), 1);
    assert_eq!(
        meter.requests_with_status(401),
        0,
        "authorization's refusals never reach the endpoint filter"
    );
    for reading in meter.measurements() {
        assert_eq!(reading.tags[0], ("module".to_owned(), "Catalog".to_owned()));
    }
}
