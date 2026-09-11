//! Decoding SQLite rows into the shared entity types.
//!
//! The Chinook file predates this port and its column affinities are loose, so
//! each accessor here is deliberately tolerant: money is declared
//! `decimal(10, 2)` but stored as `real`, and timestamps are declared `datetime`
//! but stored as text carrying a UTC offset.

use chrono::{DateTime, NaiveDateTime};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use shared_persistence::api_models::TrackApiModel;
use shared_persistence::entities::{
    Album, Artist, Customer, Employee, Genre, Invoice, InvoiceLine, MediaType, Playlist, Track,
};
use sqlx::sqlite::SqliteRow;
use sqlx::{Error, Row};

/// Reads an `INTEGER` column as the `i32` the entities use.
pub(crate) fn int(row: &SqliteRow, column: &str) -> Result<Option<i32>, Error> {
    let value: Option<i64> = row.try_get(column)?;

    #[allow(clippy::cast_possible_truncation)]
    Ok(value.map(|number| number as i32))
}

/// Reads a required key column.
pub(crate) fn id(row: &SqliteRow, column: &str) -> Result<i32, Error> {
    Ok(int(row, column)?.unwrap_or_default())
}

/// Reads a `TEXT` column.
pub(crate) fn text(row: &SqliteRow, column: &str) -> Result<Option<String>, Error> {
    row.try_get(column)
}

/// Reads a money column.
///
/// Declared `decimal(10, 2)`, stored as `real`. The integer and text branches
/// cover rows a different writer might have produced.
pub(crate) fn decimal(row: &SqliteRow, column: &str) -> Result<Option<Decimal>, Error> {
    if let Ok(value) = row.try_get::<Option<f64>, _>(column) {
        return Ok(value.and_then(Decimal::from_f64));
    }

    if let Ok(value) = row.try_get::<Option<i64>, _>(column) {
        return Ok(value.map(Decimal::from));
    }

    let value: Option<String> = row.try_get(column)?;
    Ok(value.and_then(|raw| raw.parse().ok()))
}

/// Renders a money value for binding.
///
/// The columns are declared `decimal(10, 2)` but hold `real`, so a write has to
/// go back out as a float to match what every existing row looks like.
pub(crate) fn money(value: Option<Decimal>) -> Option<f64> {
    use rust_decimal::prelude::ToPrimitive;

    value.and_then(|amount| amount.to_f64())
}

/// Reads a timestamp column.
pub(crate) fn timestamp(row: &SqliteRow, column: &str) -> Result<Option<NaiveDateTime>, Error> {
    let value: Option<String> = row.try_get(column)?;
    Ok(value.as_deref().and_then(parse_timestamp))
}

/// Parses the timestamp formats this database is written in.
///
/// The stored form is `2007-01-02 00:00:00.000000 +00:00` — EF Core's SQLite
/// encoding. The offset is dropped rather than applied: the original reads the
/// column into a `DateTime` and serializes the wall-clock value, so shifting it
/// would move every date on the wire.
fn parse_timestamp(value: &str) -> Option<NaiveDateTime> {
    let trimmed = value.trim();

    if let Ok(with_offset) = DateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S%.f %:z") {
        return Some(with_offset.naive_local());
    }

    for format in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, format) {
            return Some(naive);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Entity mappers
// ---------------------------------------------------------------------------

pub(crate) fn artist(row: &SqliteRow) -> Result<Artist, Error> {
    Ok(Artist {
        id: id(row, "Id")?,
        name: text(row, "Name")?,
    })
}

pub(crate) fn album(row: &SqliteRow) -> Result<Album, Error> {
    Ok(Album {
        id: id(row, "Id")?,
        title: text(row, "Title")?,
        artist_id: int(row, "ArtistId")?,
    })
}

pub(crate) fn genre(row: &SqliteRow) -> Result<Genre, Error> {
    Ok(Genre {
        id: id(row, "Id")?,
        name: text(row, "Name")?,
    })
}

pub(crate) fn media_type(row: &SqliteRow) -> Result<MediaType, Error> {
    Ok(MediaType {
        id: id(row, "Id")?,
        name: text(row, "Name")?,
    })
}

pub(crate) fn playlist(row: &SqliteRow) -> Result<Playlist, Error> {
    Ok(Playlist {
        id: id(row, "Id")?,
        name: text(row, "Name")?,
    })
}

pub(crate) fn track(row: &SqliteRow) -> Result<Track, Error> {
    Ok(Track {
        id: id(row, "Id")?,
        name: text(row, "Name")?,
        album_id: int(row, "AlbumId")?,
        media_type_id: int(row, "MediaTypeId")?,
        genre_id: int(row, "GenreId")?,
        composer: text(row, "Composer")?,
        milliseconds: int(row, "Milliseconds")?,
        bytes: int(row, "Bytes")?,
        unit_price: decimal(row, "UnitPrice")?,
    })
}

/// The projection every track-shaped API model is read from.
///
/// The three joins supply the denormalized `AlbumName`, `GenreName`, and
/// `MediaTypeName` the model carries. They are `LEFT` joins because all three
/// foreign keys are nullable, and an inner join would silently drop tracks that
/// belong to no album.
pub(crate) const TRACK_MODEL_SELECT: &str = r#"
    SELECT t."Id", t."Name", t."AlbumId", t."MediaTypeId", t."GenreId", t."Composer",
           t."Milliseconds", t."Bytes", t."UnitPrice",
           al."Title" AS "AlbumName", g."Name" AS "GenreName", m."Name" AS "MediaTypeName"
    FROM "Track" t
    LEFT JOIN "Album" al ON al."Id" = t."AlbumId"
    LEFT JOIN "Genre" g ON g."Id" = t."GenreId"
    LEFT JOIN "MediaType" m ON m."Id" = t."MediaTypeId"
"#;

/// Maps a row from [`TRACK_MODEL_SELECT`] into a track API model.
///
/// Nested objects stay null and nested collections stay empty, reproducing the
/// original's explicit cycle avoidance — every C# projection sets `Album`,
/// `Genre`, and `MediaType` to null and the two collections to empty lists.
pub(crate) fn track_model(row: &SqliteRow) -> Result<TrackApiModel, Error> {
    Ok(TrackApiModel {
        id: id(row, "Id")?,
        name: text(row, "Name")?,
        album_id: int(row, "AlbumId")?,
        media_type_id: int(row, "MediaTypeId")?,
        genre_id: int(row, "GenreId")?,
        composer: text(row, "Composer")?,
        milliseconds: int(row, "Milliseconds")?,
        bytes: int(row, "Bytes")?,
        unit_price: decimal(row, "UnitPrice")?,
        album_name: text(row, "AlbumName")?,
        genre_name: text(row, "GenreName")?,
        media_type_name: text(row, "MediaTypeName")?,
        ..TrackApiModel::default()
    })
}

pub(crate) fn customer(row: &SqliteRow) -> Result<Customer, Error> {
    Ok(Customer {
        id: id(row, "Id")?,
        first_name: text(row, "FirstName")?,
        last_name: text(row, "LastName")?,
        company: text(row, "Company")?,
        address: text(row, "Address")?,
        city: text(row, "City")?,
        state: text(row, "State")?,
        country: text(row, "Country")?,
        postal_code: text(row, "PostalCode")?,
        phone: text(row, "Phone")?,
        fax: text(row, "Fax")?,
        email: text(row, "Email")?,
        support_rep_id: int(row, "SupportRepId")?,
    })
}

pub(crate) fn employee(row: &SqliteRow) -> Result<Employee, Error> {
    Ok(Employee {
        id: id(row, "Id")?,
        last_name: text(row, "LastName")?,
        first_name: text(row, "FirstName")?,
        title: text(row, "Title")?,
        reports_to: int(row, "ReportsTo")?,
        birth_date: timestamp(row, "BirthDate")?,
        hire_date: timestamp(row, "HireDate")?,
        address: text(row, "Address")?,
        city: text(row, "City")?,
        state: text(row, "State")?,
        country: text(row, "Country")?,
        postal_code: text(row, "PostalCode")?,
        phone: text(row, "Phone")?,
        fax: text(row, "Fax")?,
        email: text(row, "Email")?,
    })
}

pub(crate) fn invoice(row: &SqliteRow) -> Result<Invoice, Error> {
    Ok(Invoice {
        id: id(row, "Id")?,
        customer_id: int(row, "CustomerId")?,
        invoice_date: timestamp(row, "InvoiceDate")?
            .unwrap_or_else(|| DateTime::UNIX_EPOCH.naive_utc()),
        billing_address: text(row, "BillingAddress")?,
        billing_city: text(row, "BillingCity")?,
        billing_state: text(row, "BillingState")?,
        billing_country: text(row, "BillingCountry")?,
        billing_postal_code: text(row, "BillingPostalCode")?,
        total: decimal(row, "Total")?.unwrap_or_default(),
    })
}

pub(crate) fn invoice_line(row: &SqliteRow) -> Result<InvoiceLine, Error> {
    Ok(InvoiceLine {
        id: id(row, "Id")?,
        invoice_id: int(row, "InvoiceId")?,
        track_id: int(row, "TrackId")?,
        unit_price: decimal(row, "UnitPrice")?,
        quantity: int(row, "Quantity")?,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn the_stored_timestamp_format_parses_without_shifting_the_clock() {
        // The exact form in the bundled database.
        let parsed = parse_timestamp("2007-01-02 00:00:00.000000 +00:00")
            .expect("the stored format should parse");

        assert_eq!(parsed.to_string(), "2007-01-02 00:00:00");
        assert_eq!(
            serde_json::to_value(parsed).expect("should serialize"),
            serde_json::json!("2007-01-02T00:00:00")
        );
    }

    #[test]
    fn a_non_utc_offset_keeps_the_written_wall_clock() {
        // The original reads the column into a DateTime and serializes what was
        // written, so a stored offset must not move the value.
        let parsed = parse_timestamp("2007-01-02 09:30:00.000000 -05:00")
            .expect("an offset timestamp should parse");

        assert_eq!(parsed.to_string(), "2007-01-02 09:30:00");
    }

    #[test]
    fn the_plainer_timestamp_formats_parse_too() {
        for value in [
            "2007-01-02 00:00:00",
            "2007-01-02T00:00:00",
            "2007-01-02 00:00:00.000",
        ] {
            assert!(parse_timestamp(value).is_some(), "{value} should parse");
        }
    }

    #[test]
    fn unparseable_timestamps_yield_nothing_rather_than_panicking() {
        assert_eq!(parse_timestamp("not a date"), None);
        assert_eq!(parse_timestamp(""), None);
    }
}
