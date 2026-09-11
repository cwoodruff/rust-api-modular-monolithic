//! Repository tests against the real Chinook database.
//!
//! Every test runs on its own copy of `data/chinook.db`, so the write tests can
//! insert and delete freely without touching the file the repository commits.
//! That mirrors the C# suite's `TestAuthHelpers`, which copies the database into
//! `TestData/chinook-{guid}.db` per factory.
//!
//! # A note on the bundled data
//!
//! The committed database is **not** pristine Chinook: its `Genre` table
//! carries leftovers from a previous run of the C# test suite. Genre 1's name
//! has been overwritten with `Updated_<guid>`, and genres 26 through 33 are
//! rows those tests created (`CacheTest_`, `TestGenre_`, `Concurrent_`,
//! `CharsetTest_`). Genre is the only entity with write endpoints, which is why
//! it is the only table affected.
//!
//! Tests here therefore assert genre *behavior* rather than genre *contents*,
//! and take their fixed expectations from the untouched tables.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use shared_data_sqlite::{
    SqliteAlbumRepository, SqliteArtistRepository, SqliteCustomerRepository,
    SqliteEmployeeRepository, SqliteGenreRepository, SqliteInvoiceLineRepository,
    SqliteInvoiceRepository, SqliteMediaTypeRepository, SqlitePlaylistRepository,
    SqliteTrackRepository, create_pool,
};
use shared_persistence::entities::Genre;
use shared_persistence::repositories::{
    AlbumRepository, ArtistRepository, CustomerRepository, EmployeeRepository, GenreRepository,
    InvoiceLineRepository, InvoiceRepository, MediaTypeRepository, PlaylistRepository, Repository,
    TrackRepository,
};
use sqlx::SqlitePool;

/// A private copy of the database, removed when the test ends.
struct ScratchDatabase {
    path: PathBuf,
}

impl ScratchDatabase {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("chinook-{}-{label}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::copy(bundled_database(), &path).expect("the database should be copyable");

        Self { path }
    }

    async fn pool(&self) -> SqlitePool {
        create_pool(&self.path)
            .await
            .expect("the scratch database should open")
    }
}

impl Drop for ScratchDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn bundled_database() -> PathBuf {
    let mut current = Path::new(env!("CARGO_MANIFEST_DIR"));

    loop {
        let candidate = current.join("data/chinook.db");
        if candidate.is_file() {
            return candidate;
        }
        current = current.parent().expect("the database should be bundled");
    }
}

/// Opens the bundled database directly, for read-only tests.
async fn read_only_pool() -> SqlitePool {
    create_pool(&bundled_database())
        .await
        .expect("the bundled database should open")
}

// ---------------------------------------------------------------------------
// Counts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_collection_returns_the_whole_table() {
    // Unbounded reads, as in the original — no paging anywhere.
    let pool = read_only_pool().await;

    assert_eq!(
        SqliteAlbumRepository::new(pool.clone())
            .get_all()
            .await
            .unwrap()
            .len(),
        347
    );
    assert_eq!(
        SqliteArtistRepository::new(pool.clone())
            .get_all()
            .await
            .unwrap()
            .len(),
        275
    );
    assert_eq!(
        SqliteTrackRepository::new(pool.clone())
            .get_all()
            .await
            .unwrap()
            .len(),
        3503
    );
    assert_eq!(
        SqlitePlaylistRepository::new(pool.clone())
            .get_all()
            .await
            .unwrap()
            .len(),
        18
    );
    assert_eq!(
        SqliteCustomerRepository::new(pool.clone())
            .get_all()
            .await
            .unwrap()
            .len(),
        59
    );
    assert_eq!(
        SqliteEmployeeRepository::new(pool.clone())
            .get_all()
            .await
            .unwrap()
            .len(),
        8
    );
    assert_eq!(
        SqliteInvoiceRepository::new(pool.clone())
            .get_all()
            .await
            .unwrap()
            .len(),
        458
    );
    assert_eq!(
        SqliteInvoiceLineRepository::new(pool.clone())
            .get_all()
            .await
            .unwrap()
            .len(),
        2662
    );
    assert_eq!(
        SqliteMediaTypeRepository::new(pool)
            .get_all()
            .await
            .unwrap()
            .len(),
        5
    );
}

// ---------------------------------------------------------------------------
// Music
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_album_by_id_carries_its_artist_and_tracks() {
    let repository = SqliteAlbumRepository::new(read_only_pool().await);

    let album = repository
        .get_by_id(1)
        .await
        .unwrap()
        .expect("album 1 exists");

    assert_eq!(album.id, 1);
    assert_eq!(
        album.title.as_deref(),
        Some("For Those About To Rock We Salute You")
    );
    assert_eq!(album.artist_id, Some(1));
    assert_eq!(album.artist_name.as_deref(), Some("AC/DC"));

    let artist = album.artist.as_ref().expect("the artist is nested");
    assert_eq!(artist.name.as_deref(), Some("AC/DC"));
    assert!(
        artist.albums.is_empty(),
        "the nested artist's own albums stay empty, as the original leaves them"
    );

    assert_eq!(album.tracks.len(), 10);
    let first = &album.tracks[0];
    assert_eq!(
        first.name.as_deref(),
        Some("For Those About To Rock (We Salute You)")
    );
    assert_eq!(
        first.album_name.as_deref(),
        Some("For Those About To Rock We Salute You"),
        "tracks carry the album's title denormalized"
    );
    assert!(first.genre_name.is_some());
    assert!(first.media_type_name.is_some());
    assert!(
        first.album.is_none() && first.genre.is_none() && first.media_type.is_none(),
        "nested objects stay null to avoid cycles"
    );
    assert!(first.playlists.is_empty() && first.invoice_lines.is_empty());
}

#[tokio::test]
async fn a_missing_album_is_absent_rather_than_an_error() {
    let repository = SqliteAlbumRepository::new(read_only_pool().await);

    assert!(repository.get_by_id(999_999).await.unwrap().is_none());
}

#[tokio::test]
async fn albums_can_be_listed_by_artist() {
    let repository = SqliteAlbumRepository::new(read_only_pool().await);

    assert_eq!(repository.get_by_artist_id(1).await.unwrap().len(), 2);
    assert!(
        repository
            .get_by_artist_id(999_999)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn an_artist_by_id_carries_albums_each_with_their_tracks() {
    let repository = SqliteArtistRepository::new(read_only_pool().await);

    let artist = repository
        .get_by_id(1)
        .await
        .unwrap()
        .expect("artist 1 exists");

    assert_eq!(artist.name.as_deref(), Some("AC/DC"));
    assert_eq!(artist.albums.len(), 2);

    let total_tracks: usize = artist.albums.iter().map(|album| album.tracks.len()).sum();
    assert_eq!(total_tracks, 18, "every album's tracks should be grouped");

    for album in &artist.albums {
        assert_eq!(
            album.artist_name.as_deref(),
            Some("AC/DC"),
            "albums carry the artist's name denormalized"
        );
        assert!(
            album.artist.is_none(),
            "the album is already inside its artist, so the back-reference stays null"
        );
        for track in &album.tracks {
            assert_eq!(track.album_id, Some(album.id));
            assert_eq!(track.album_name, album.title);
        }
    }
}

#[tokio::test]
async fn a_track_by_id_carries_its_denormalized_names() {
    let repository = SqliteTrackRepository::new(read_only_pool().await);

    let track = repository
        .get_by_id(1)
        .await
        .unwrap()
        .expect("track 1 exists");

    assert_eq!(
        track.name.as_deref(),
        Some("For Those About To Rock (We Salute You)")
    );
    assert_eq!(
        track.composer.as_deref(),
        Some("Angus Young, Malcolm Young, Brian Johnson")
    );
    assert_eq!(track.unit_price, Some("0.99".parse().unwrap()));
    assert_eq!(
        track.album_name.as_deref(),
        Some("For Those About To Rock We Salute You")
    );
    assert!(track.genre_name.is_some());
    assert!(track.media_type_name.is_some());
}

#[tokio::test]
async fn tracks_can_be_reached_by_every_relationship() {
    let repository = SqliteTrackRepository::new(read_only_pool().await);

    assert_eq!(repository.get_by_album_id(1).await.unwrap().len(), 10);
    assert_eq!(repository.get_by_genre_id(1).await.unwrap().len(), 1297);
    assert_eq!(
        repository.get_by_media_type_id(1).await.unwrap().len(),
        3034
    );
    assert_eq!(repository.get_by_artist_id(1).await.unwrap().len(), 18);
    assert_eq!(repository.get_by_playlist_id(1).await.unwrap().len(), 3290);
}

#[tokio::test]
async fn tracks_on_an_invoice_are_found_without_duplicates() {
    // The EXISTS subquery the original uses: a join would repeat a track that
    // appears on the same invoice more than once.
    let repository = SqliteTrackRepository::new(read_only_pool().await);

    let tracks = repository.get_by_invoice_id(1).await.unwrap();

    let ids: Vec<i32> = tracks.iter().map(|track| track.id).collect();
    assert_eq!(ids, vec![1013, 1474, 3027, 3485]);

    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "no track should repeat");
}

#[tokio::test]
async fn a_playlist_by_id_returns_its_tracks_in_key_order() {
    let repository = SqlitePlaylistRepository::new(read_only_pool().await);

    let playlist = repository
        .get_by_id(1)
        .await
        .unwrap()
        .expect("playlist 1 exists");

    assert_eq!(playlist.name.as_deref(), Some("Music"));
    assert_eq!(playlist.tracks.len(), 3290);

    let ids: Vec<i32> = playlist.tracks.iter().map(|track| track.id).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "the original orders these explicitly");

    assert!(playlist.tracks[0].album_name.is_some());
}

#[tokio::test]
async fn playlists_can_be_listed_by_track() {
    let repository = SqlitePlaylistRepository::new(read_only_pool().await);

    assert!(!repository.get_by_track_id(1).await.unwrap().is_empty());
    assert!(
        repository
            .get_by_track_id(999_999)
            .await
            .unwrap()
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// Administration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_customer_by_id_omits_the_nine_unprojected_columns() {
    // Faithful to the original, whose projection has a `// ... other fields ...`
    // comment where these columns should be. The row has values for all of
    // them; this endpoint just never asks for them.
    let repository = SqliteCustomerRepository::new(read_only_pool().await);

    let customer = repository
        .get_by_id(1)
        .await
        .unwrap()
        .expect("customer 1 exists");

    assert_eq!(customer.first_name.as_deref(), Some("Luís"));
    assert_eq!(customer.last_name.as_deref(), Some("Gonçalves"));

    for (field, value) in [
        ("Company", &customer.company),
        ("Address", &customer.address),
        ("City", &customer.city),
        ("State", &customer.state),
        ("Country", &customer.country),
        ("PostalCode", &customer.postal_code),
        ("Phone", &customer.phone),
        ("Fax", &customer.fax),
        ("Email", &customer.email),
    ] {
        assert_eq!(
            *value, None,
            "{field} is not projected by the original's by-id query"
        );
    }

    assert_eq!(customer.support_rep_id, Some(3));
    assert_eq!(customer.support_rep_name.as_deref(), Some("Jane Peacock"));

    let rep = customer.support_rep.as_ref().expect("the rep is nested");
    assert_eq!(rep.id, 3);
    assert_eq!(rep.first_name.as_deref(), Some("Jane"));

    assert_eq!(customer.invoices.len(), 8);
    assert!(
        customer
            .invoices
            .iter()
            .all(|invoice| invoice.customer.is_none()),
        "no back-reference to the customer"
    );
}

#[tokio::test]
async fn the_collection_endpoint_does_return_what_the_by_id_query_drops() {
    // The same customer looks different depending on which endpoint served it,
    // because the collection path goes through the entity instead.
    let repository = SqliteCustomerRepository::new(read_only_pool().await);

    let customers = repository.get_all().await.unwrap();
    let first = customers
        .iter()
        .find(|customer| customer.id == 1)
        .expect("customer 1 exists");

    assert_eq!(first.email.as_deref(), Some("luisg@embraer.com.br"));
    assert_eq!(first.city.as_deref(), Some("São José dos Campos"));
}

#[tokio::test]
async fn customers_can_be_listed_by_support_rep() {
    let repository = SqliteCustomerRepository::new(read_only_pool().await);

    let supported = repository.get_by_support_rep_id(3).await.unwrap();

    assert!(!supported.is_empty());
    assert!(
        supported
            .iter()
            .all(|customer| customer.support_rep_id == Some(3))
    );
}

#[tokio::test]
async fn an_employee_by_id_flattens_their_manager_to_a_name() {
    let repository = SqliteEmployeeRepository::new(read_only_pool().await);

    let employee = repository
        .get_by_id(2)
        .await
        .unwrap()
        .expect("employee 2 exists");

    assert_eq!(employee.first_name.as_deref(), Some("Nancy"));
    assert_eq!(employee.title.as_deref(), Some("Sales Manager"));
    assert_eq!(employee.reports_to, Some(1));
    assert_eq!(
        employee.reports_to_navigation.as_deref(),
        Some("Andrew Adams"),
        "the API model carries a name here, not an object"
    );
    assert!(employee.hire_date.is_some());
    assert!(employee.customers.is_empty());
    assert!(employee.inverse_reports_to_navigation.is_empty());
}

#[tokio::test]
async fn the_most_senior_employee_has_no_manager_name() {
    let repository = SqliteEmployeeRepository::new(read_only_pool().await);

    let employee = repository
        .get_by_id(1)
        .await
        .unwrap()
        .expect("employee 1 exists");

    assert_eq!(employee.reports_to, None);
    assert_eq!(employee.reports_to_navigation, None);
}

#[tokio::test]
async fn direct_reports_and_the_reports_to_lookup_behave_as_written() {
    let repository = SqliteEmployeeRepository::new(read_only_pool().await);

    assert_eq!(repository.get_direct_reports(2).await.unwrap().len(), 3);

    // Named for the manager relationship, but it looks up the key it is given.
    let found = repository
        .get_reports_to(2)
        .await
        .unwrap()
        .expect("employee 2 exists");
    assert_eq!(found.id, 2);
    assert_eq!(found.first_name.as_deref(), Some("Nancy"));
}

#[tokio::test]
async fn media_types_read_back_as_entities() {
    let repository = SqliteMediaTypeRepository::new(read_only_pool().await);

    let media_type = repository
        .get_by_id(1)
        .await
        .unwrap()
        .expect("media type 1 exists");

    assert_eq!(media_type.id, 1);
    assert!(media_type.name.is_some());
    assert!(repository.get_by_id(999_999).await.unwrap().is_none());
}

// ---------------------------------------------------------------------------
// Orders
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_invoice_by_id_carries_a_shallow_customer_and_its_lines() {
    let repository = SqliteInvoiceRepository::new(read_only_pool().await);

    let invoice = repository
        .get_by_id(1)
        .await
        .unwrap()
        .expect("invoice 1 exists");

    assert_eq!(invoice.customer_id, Some(46));
    assert_eq!(invoice.total, "3.96".parse().unwrap());
    assert_eq!(
        invoice.invoice_date.to_string(),
        "2007-01-02 00:00:00",
        "the stored offset must not shift the date"
    );

    let customer = invoice.customer.as_ref().expect("the customer is nested");
    assert_eq!(customer.id, 46);
    assert!(customer.first_name.is_some());
    assert!(
        customer.address.is_none() && customer.city.is_none(),
        "the nested customer is deliberately partial"
    );
    assert!(
        customer.invoices.is_empty() && customer.support_rep.is_none(),
        "the nested customer stays shallow to avoid a cycle"
    );

    assert_eq!(invoice.invoice_lines.len(), 4);
    assert!(
        invoice
            .invoice_lines
            .iter()
            .all(|line| line.track_name.is_some()),
        "lines carry the track's name denormalized"
    );
    assert!(
        invoice
            .invoice_lines
            .iter()
            .all(|line| line.invoice.is_none() && line.track.is_none())
    );
}

#[tokio::test]
async fn invoices_can_be_listed_by_customer() {
    let repository = SqliteInvoiceRepository::new(read_only_pool().await);

    assert_eq!(repository.get_by_customer_id(1).await.unwrap().len(), 8);
}

#[tokio::test]
async fn invoice_lines_read_back_as_entities() {
    let repository = SqliteInvoiceLineRepository::new(read_only_pool().await);

    let line = repository
        .get_by_id(1)
        .await
        .unwrap()
        .expect("line 1 exists");

    assert_eq!(line.id, 1);
    assert!(line.unit_price.is_some());
    assert!(line.quantity.is_some());

    assert_eq!(repository.get_by_invoice_id(1).await.unwrap().len(), 4);
    assert!(!repository.get_by_track_id(1013).await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Writes — the Genre surface, the only one any endpoint reaches
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_genre_can_be_created_read_updated_and_deleted() {
    let scratch = ScratchDatabase::new("genre-crud");
    let repository = SqliteGenreRepository::new(scratch.pool().await);

    let created = repository
        .add(Genre {
            id: 0,
            name: Some("Shoegaze".to_owned()),
        })
        .await
        .unwrap();

    assert!(created.id > 0, "the key should be generated");
    assert_eq!(created.name.as_deref(), Some("Shoegaze"));

    let found = repository
        .get_by_id(created.id)
        .await
        .unwrap()
        .expect("the new genre should be readable");
    assert_eq!(found.name.as_deref(), Some("Shoegaze"));
    assert!(repository.entity_exists(created.id).await.unwrap());

    let updated = repository
        .update(Genre {
            id: created.id,
            name: Some("Dream Pop".to_owned()),
        })
        .await
        .unwrap();
    assert!(updated);
    assert_eq!(
        repository
            .get_by_id(created.id)
            .await
            .unwrap()
            .and_then(|genre| genre.name)
            .as_deref(),
        Some("Dream Pop")
    );

    assert!(repository.delete(created.id).await.unwrap());
    assert!(repository.get_by_id(created.id).await.unwrap().is_none());
    assert!(!repository.entity_exists(created.id).await.unwrap());
}

#[tokio::test]
async fn updating_or_deleting_a_missing_genre_reports_false_rather_than_failing() {
    // This is what lets the endpoint answer 404 instead of 500.
    let scratch = ScratchDatabase::new("genre-missing");
    let repository = SqliteGenreRepository::new(scratch.pool().await);

    assert!(
        !repository
            .update(Genre {
                id: 999_999,
                name: Some("Nowhere".to_owned()),
            })
            .await
            .unwrap()
    );
    assert!(!repository.delete(999_999).await.unwrap());
    assert!(!repository.entity_exists(999_999).await.unwrap());
}

#[tokio::test]
async fn a_write_leaves_the_bundled_database_untouched() {
    // The scratch copy is the whole point: the committed file must not change.
    let scratch = ScratchDatabase::new("isolation");
    let repository = SqliteGenreRepository::new(scratch.pool().await);

    let before = SqliteGenreRepository::new(read_only_pool().await)
        .get_all()
        .await
        .unwrap()
        .len();

    repository
        .add(Genre {
            id: 0,
            name: Some("Isolated".to_owned()),
        })
        .await
        .unwrap();

    let after = SqliteGenreRepository::new(read_only_pool().await)
        .get_all()
        .await
        .unwrap()
        .len();

    assert_eq!(
        before, after,
        "the bundled database should not have changed"
    );
}

#[tokio::test]
async fn a_created_genre_round_trips_through_the_collection() {
    let scratch = ScratchDatabase::new("genre-collection");
    let repository = SqliteGenreRepository::new(scratch.pool().await);

    let before = repository.get_all().await.unwrap().len();
    let created = repository
        .add(Genre {
            id: 0,
            name: Some("Post-Rock".to_owned()),
        })
        .await
        .unwrap();

    let after = repository.get_all().await.unwrap();

    assert_eq!(after.len(), before + 1);
    assert!(
        after.iter().any(|genre| genre.id == created.id),
        "the new genre should appear in the collection"
    );
}
