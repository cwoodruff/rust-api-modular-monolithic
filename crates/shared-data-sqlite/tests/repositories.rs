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

use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use shared_data_sqlite::{
    SqliteAlbumRepository, SqliteArtistRepository, SqliteCustomerRepository,
    SqliteEmployeeRepository, SqliteGenreRepository, SqliteInvoiceLineRepository,
    SqliteInvoiceRepository, SqliteMediaTypeRepository, SqlitePlaylistRepository,
    SqliteTrackRepository, create_pool,
};
use shared_persistence::entities::{
    Album, Artist, Customer, Employee, Genre, Invoice, InvoiceLine, MediaType, Playlist, Track,
};
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
        "2007-01-02 00:00:00 UTC",
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

// ---------------------------------------------------------------------------
// Writes — every entity, not just the one with endpoints
// ---------------------------------------------------------------------------
//
// Nine of the ten repositories expose `add`, `update` and `delete` that no
// route reaches, so nothing exercised them and nothing would have noticed them
// breaking. They did break: an `Employee` or `Invoice` written here went out
// through sqlx's `DateTime` encoder as RFC 3339, which `rows::timestamp`
// could not parse, so a birth date came back `None` and an invoice date came
// back as the Unix epoch. The round-trip assertions below are what catch that
// class of fault, which is why each one reads the row back rather than
// trusting what `add` returned.

/// An instant with no fractional part, which is what the file's rows carry.
fn instant(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
        .single()
        .expect("a valid instant")
}

fn money(value: &str) -> Decimal {
    value.parse().expect("a valid decimal")
}

#[tokio::test]
async fn an_artist_can_be_written_read_back_updated_and_deleted() {
    let scratch = ScratchDatabase::new("artist-writes");
    let repository = SqliteArtistRepository::new(scratch.pool().await);

    let created = repository
        .add(Artist {
            id: 0,
            name: Some("Slowdive".to_owned()),
        })
        .await
        .unwrap();

    assert!(created.id > 0);
    assert_eq!(created.name.as_deref(), Some("Slowdive"));

    assert!(
        repository
            .update(Artist {
                id: created.id,
                name: Some("Ride".to_owned()),
            })
            .await
            .unwrap()
    );

    let reread = repository
        .get_all()
        .await
        .unwrap()
        .into_iter()
        .find(|artist| artist.id == created.id)
        .expect("the updated artist should still be there");
    assert_eq!(reread.name.as_deref(), Some("Ride"));

    assert!(repository.delete(created.id).await.unwrap());
    assert!(!repository.entity_exists(created.id).await.unwrap());
}

#[tokio::test]
async fn an_album_can_be_written_read_back_updated_and_deleted() {
    let scratch = ScratchDatabase::new("album-writes");
    let repository = SqliteAlbumRepository::new(scratch.pool().await);

    let created = repository
        .add(Album {
            id: 0,
            title: Some("Souvlaki".to_owned()),
            artist_id: Some(1),
        })
        .await
        .unwrap();

    assert!(created.id > 0);
    assert_eq!(created.artist_id, Some(1));

    assert!(
        repository
            .update(Album {
                id: created.id,
                title: Some("Pygmalion".to_owned()),
                artist_id: Some(2),
            })
            .await
            .unwrap()
    );

    let reread = repository
        .get_all()
        .await
        .unwrap()
        .into_iter()
        .find(|album| album.id == created.id)
        .expect("the updated album should still be there");
    assert_eq!(reread.title.as_deref(), Some("Pygmalion"));
    assert_eq!(reread.artist_id, Some(2));

    assert!(repository.delete(created.id).await.unwrap());
    assert!(!repository.entity_exists(created.id).await.unwrap());
}

#[tokio::test]
async fn a_track_round_trips_its_money_and_its_three_foreign_keys() {
    let scratch = ScratchDatabase::new("track-writes");
    let repository = SqliteTrackRepository::new(scratch.pool().await);

    let written = Track {
        id: 0,
        name: Some("Alison".to_owned()),
        album_id: Some(1),
        media_type_id: Some(1),
        genre_id: Some(1),
        composer: Some("Halstead".to_owned()),
        milliseconds: Some(233_000),
        bytes: Some(7_654_321),
        unit_price: Some(money("0.99")),
    };

    let created = repository.add(written.clone()).await.unwrap();

    assert!(created.id > 0);
    assert_eq!(
        Track {
            id: 0,
            ..created.clone()
        },
        written,
        "every scalar should survive the insert"
    );

    let updated = Track {
        unit_price: Some(money("1.29")),
        milliseconds: Some(240_000),
        ..created.clone()
    };
    assert!(repository.update(updated.clone()).await.unwrap());

    let reread = repository
        .get_all()
        .await
        .unwrap()
        .into_iter()
        .find(|track| track.id == created.id)
        .expect("the updated track should still be there");
    assert_eq!(
        reread, updated,
        "the money column is declared decimal and stored real; it must not drift"
    );

    assert!(repository.delete(created.id).await.unwrap());
    assert!(!repository.entity_exists(created.id).await.unwrap());
}

#[tokio::test]
async fn a_playlist_can_be_written_read_back_updated_and_deleted() {
    let scratch = ScratchDatabase::new("playlist-writes");
    let repository = SqlitePlaylistRepository::new(scratch.pool().await);

    let created = repository
        .add(Playlist {
            id: 0,
            name: Some("Late Night".to_owned()),
        })
        .await
        .unwrap();

    assert!(
        repository
            .update(Playlist {
                id: created.id,
                name: Some("Early Morning".to_owned()),
            })
            .await
            .unwrap()
    );

    let reread = repository
        .get_by_id(created.id)
        .await
        .unwrap()
        .expect("the updated playlist should be readable");
    assert_eq!(reread.name.as_deref(), Some("Early Morning"));
    assert!(reread.tracks.is_empty());

    assert!(repository.delete(created.id).await.unwrap());
    assert!(!repository.entity_exists(created.id).await.unwrap());
}

#[tokio::test]
async fn a_media_type_can_be_written_read_back_updated_and_deleted() {
    let scratch = ScratchDatabase::new("mediatype-writes");
    let repository = SqliteMediaTypeRepository::new(scratch.pool().await);

    let created = repository
        .add(MediaType {
            id: 0,
            name: Some("Opus audio file".to_owned()),
        })
        .await
        .unwrap();

    assert!(
        repository
            .update(MediaType {
                id: created.id,
                name: Some("FLAC audio file".to_owned()),
            })
            .await
            .unwrap()
    );

    let reread = repository
        .get_by_id(created.id)
        .await
        .unwrap()
        .expect("the updated media type should be readable");
    assert_eq!(reread.name.as_deref(), Some("FLAC audio file"));

    assert!(repository.delete(created.id).await.unwrap());
    assert!(!repository.entity_exists(created.id).await.unwrap());
}

#[tokio::test]
async fn a_customer_round_trips_all_twelve_of_its_columns() {
    let scratch = ScratchDatabase::new("customer-writes");
    let repository = SqliteCustomerRepository::new(scratch.pool().await);

    let written = Customer {
        id: 0,
        first_name: Some("Ada".to_owned()),
        last_name: Some("Lovelace".to_owned()),
        company: Some("Analytical Engines".to_owned()),
        address: Some("12 Marylebone".to_owned()),
        city: Some("London".to_owned()),
        state: None,
        country: Some("United Kingdom".to_owned()),
        postal_code: Some("W1U 5AA".to_owned()),
        phone: Some("+44 20 7946 0000".to_owned()),
        fax: None,
        email: Some("ada@example.com".to_owned()),
        support_rep_id: Some(3),
    };

    let created = repository.add(written.clone()).await.unwrap();

    assert!(created.id > 0);
    assert_eq!(
        Customer {
            id: 0,
            ..created.clone()
        },
        written
    );

    let updated = Customer {
        city: Some("Oxford".to_owned()),
        state: Some("Oxfordshire".to_owned()),
        support_rep_id: Some(4),
        ..created.clone()
    };
    assert!(repository.update(updated.clone()).await.unwrap());

    let reread = repository
        .get_all()
        .await
        .unwrap()
        .into_iter()
        .find(|customer| customer.id == created.id)
        .expect("the updated customer should still be there");
    assert_eq!(reread, updated);

    assert!(repository.delete(created.id).await.unwrap());
    assert!(!repository.entity_exists(created.id).await.unwrap());
}

#[tokio::test]
async fn an_employees_dates_survive_being_written_and_read_back() {
    // The round trip the timestamp encoding exists for. Before it, both dates
    // came back `None` and the failure was invisible: the insert succeeded and
    // every other column was right.
    let scratch = ScratchDatabase::new("employee-writes");
    let repository = SqliteEmployeeRepository::new(scratch.pool().await);

    let written = Employee {
        id: 0,
        last_name: Some("Hopper".to_owned()),
        first_name: Some("Grace".to_owned()),
        title: Some("Rear Admiral".to_owned()),
        reports_to: Some(1),
        birth_date: Some(instant(1906, 12, 9)),
        hire_date: Some(instant(1944, 7, 2)),
        address: Some("1 Navy Yard".to_owned()),
        city: Some("Arlington".to_owned()),
        state: Some("VA".to_owned()),
        country: Some("USA".to_owned()),
        postal_code: Some("22202".to_owned()),
        phone: Some("+1 (703) 555-0100".to_owned()),
        fax: None,
        email: Some("grace@example.com".to_owned()),
    };

    let created = repository.add(written.clone()).await.unwrap();

    assert_eq!(
        created.birth_date,
        Some(instant(1906, 12, 9)),
        "the insert should hand back the date it was given"
    );

    let reread = repository
        .get_reports_to(created.id)
        .await
        .unwrap()
        .expect("the new employee should be readable");
    assert_eq!(
        Employee {
            id: 0,
            ..reread.clone()
        },
        written,
        "every column, dates included, should survive the round trip"
    );

    let updated = Employee {
        hire_date: Some(instant(1949, 3, 14)),
        title: Some("Systems Engineer".to_owned()),
        ..created.clone()
    };
    assert!(repository.update(updated.clone()).await.unwrap());

    let after_update = repository
        .get_reports_to(created.id)
        .await
        .unwrap()
        .expect("the updated employee should be readable");
    assert_eq!(after_update, updated);
    assert_eq!(after_update.hire_date, Some(instant(1949, 3, 14)));
    assert_eq!(
        after_update.birth_date,
        Some(instant(1906, 12, 9)),
        "an update must not quietly blank the column it rewrote unchanged"
    );

    assert!(repository.delete(created.id).await.unwrap());
    assert!(!repository.entity_exists(created.id).await.unwrap());
}

#[tokio::test]
async fn an_employee_written_with_no_dates_reads_back_with_none() {
    // The other half: a genuinely absent date must stay absent rather than
    // arriving as some default.
    let scratch = ScratchDatabase::new("employee-null-dates");
    let repository = SqliteEmployeeRepository::new(scratch.pool().await);

    let created = repository
        .add(Employee {
            id: 0,
            last_name: Some("Unknown".to_owned()),
            first_name: Some("Dates".to_owned()),
            ..Employee::default()
        })
        .await
        .unwrap();

    let reread = repository
        .get_reports_to(created.id)
        .await
        .unwrap()
        .expect("the new employee should be readable");

    assert_eq!(reread.birth_date, None);
    assert_eq!(reread.hire_date, None);
}

#[tokio::test]
async fn an_invoices_date_and_total_survive_being_written_and_read_back() {
    // `invoice_date` is not optional, so a timestamp that failed to parse came
    // back as the Unix epoch rather than as `None` — a wrong answer that looks
    // like a real one.
    let scratch = ScratchDatabase::new("invoice-writes");
    let repository = SqliteInvoiceRepository::new(scratch.pool().await);

    let written = Invoice {
        id: 0,
        customer_id: Some(1),
        invoice_date: instant(2011, 5, 22),
        billing_address: Some("12 Marylebone".to_owned()),
        billing_city: Some("London".to_owned()),
        billing_state: None,
        billing_country: Some("United Kingdom".to_owned()),
        billing_postal_code: Some("W1U 5AA".to_owned()),
        total: money("13.86"),
    };

    let created = repository.add(written.clone()).await.unwrap();

    assert!(created.id > 0);
    assert_eq!(
        Invoice {
            id: 0,
            ..created.clone()
        },
        written
    );

    let reread = repository
        .get_by_customer_id(1)
        .await
        .unwrap()
        .into_iter()
        .find(|invoice| invoice.id == created.id)
        .expect("the new invoice should be readable");
    assert_eq!(
        reread.invoice_date,
        instant(2011, 5, 22),
        "a date that failed to parse would silently read as the epoch"
    );
    assert_ne!(reread.invoice_date, DateTime::UNIX_EPOCH);
    assert_eq!(reread.total, money("13.86"));

    let updated = Invoice {
        invoice_date: instant(2011, 6, 1),
        total: money("21.00"),
        ..created.clone()
    };
    assert!(repository.update(updated.clone()).await.unwrap());

    let after_update = repository
        .get_by_customer_id(1)
        .await
        .unwrap()
        .into_iter()
        .find(|invoice| invoice.id == created.id)
        .expect("the updated invoice should be readable");
    assert_eq!(after_update, updated);

    assert!(repository.delete(created.id).await.unwrap());
    assert!(!repository.entity_exists(created.id).await.unwrap());
}

#[tokio::test]
async fn an_invoice_line_can_be_written_read_back_updated_and_deleted() {
    let scratch = ScratchDatabase::new("invoiceline-writes");
    let repository = SqliteInvoiceLineRepository::new(scratch.pool().await);

    let written = InvoiceLine {
        id: 0,
        invoice_id: Some(1),
        track_id: Some(1),
        unit_price: Some(money("0.99")),
        quantity: Some(2),
    };

    let created = repository.add(written.clone()).await.unwrap();

    assert!(created.id > 0);
    assert_eq!(
        InvoiceLine {
            id: 0,
            ..created.clone()
        },
        written
    );

    let updated = InvoiceLine {
        quantity: Some(5),
        unit_price: Some(money("1.99")),
        ..created.clone()
    };
    assert!(repository.update(updated.clone()).await.unwrap());

    let reread = repository
        .get_by_id(created.id)
        .await
        .unwrap()
        .expect("the updated line should be readable");
    assert_eq!(reread, updated);

    assert!(repository.delete(created.id).await.unwrap());
    assert!(repository.get_by_id(created.id).await.unwrap().is_none());
}

#[tokio::test]
async fn updating_or_deleting_a_missing_row_reports_false_for_every_entity() {
    // Each of these is what lets an endpoint answer 404 rather than 500, and
    // only Genre had a test for it.
    let scratch = ScratchDatabase::new("missing-rows");
    let pool = scratch.pool().await;
    let absent = 999_999;

    assert!(
        !SqliteArtistRepository::new(pool.clone())
            .update(Artist {
                id: absent,
                name: Some("Nobody".to_owned()),
            })
            .await
            .unwrap()
    );
    assert!(
        !SqliteAlbumRepository::new(pool.clone())
            .update(Album {
                id: absent,
                title: Some("Nothing".to_owned()),
                artist_id: Some(1),
            })
            .await
            .unwrap()
    );
    assert!(
        !SqliteTrackRepository::new(pool.clone())
            .update(Track {
                id: absent,
                name: Some("Nothing".to_owned()),
                ..Track::default()
            })
            .await
            .unwrap()
    );
    assert!(
        !SqlitePlaylistRepository::new(pool.clone())
            .update(Playlist {
                id: absent,
                name: Some("Nothing".to_owned()),
            })
            .await
            .unwrap()
    );
    assert!(
        !SqliteMediaTypeRepository::new(pool.clone())
            .update(MediaType {
                id: absent,
                name: Some("Nothing".to_owned()),
            })
            .await
            .unwrap()
    );
    assert!(
        !SqliteCustomerRepository::new(pool.clone())
            .update(Customer {
                id: absent,
                ..Customer::default()
            })
            .await
            .unwrap()
    );
    assert!(
        !SqliteEmployeeRepository::new(pool.clone())
            .update(Employee {
                id: absent,
                ..Employee::default()
            })
            .await
            .unwrap()
    );
    assert!(
        !SqliteInvoiceRepository::new(pool.clone())
            .update(Invoice {
                id: absent,
                customer_id: None,
                invoice_date: instant(2011, 5, 22),
                billing_address: None,
                billing_city: None,
                billing_state: None,
                billing_country: None,
                billing_postal_code: None,
                total: money("0.00"),
            })
            .await
            .unwrap()
    );
    assert!(
        !SqliteInvoiceLineRepository::new(pool.clone())
            .update(InvoiceLine {
                id: absent,
                ..InvoiceLine::default()
            })
            .await
            .unwrap()
    );

    for deleted in [
        SqliteArtistRepository::new(pool.clone())
            .delete(absent)
            .await,
        SqliteAlbumRepository::new(pool.clone())
            .delete(absent)
            .await,
        SqliteTrackRepository::new(pool.clone())
            .delete(absent)
            .await,
        SqlitePlaylistRepository::new(pool.clone())
            .delete(absent)
            .await,
        SqliteMediaTypeRepository::new(pool.clone())
            .delete(absent)
            .await,
        SqliteCustomerRepository::new(pool.clone())
            .delete(absent)
            .await,
        SqliteEmployeeRepository::new(pool.clone())
            .delete(absent)
            .await,
        SqliteInvoiceRepository::new(pool.clone())
            .delete(absent)
            .await,
        SqliteInvoiceLineRepository::new(pool).delete(absent).await,
    ] {
        assert!(!deleted.unwrap(), "deleting a missing row reports false");
    }
}
