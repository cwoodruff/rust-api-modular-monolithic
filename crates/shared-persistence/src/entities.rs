//! The Chinook tables, ported from `SharedKernel.Persistence.Entities`.
//!
//! These are row shapes: the scalar columns of each table and nothing else.
//! The C# entities also carry EF Core navigation properties (`virtual Artist?
//! Artist`, `ICollection<Track> Tracks`), which exist so the ORM can populate
//! object graphs by tracking. There is no tracking here — repositories project
//! straight into the [`api_models`](crate::api_models), which is where the
//! graph shapes live.
//!
//! That has one wire consequence worth naming. `GET /api/orders/invoice-lines/
//! {id}` is the single endpoint that serializes an *entity* rather than an API
//! model, because the C# repository's `GetById` returns one — so its response
//! includes `Invoice` and `Track` members, always null, from navigation
//! properties that were never loaded. The Orders module reproduces that exact
//! shape when it lands in Phase 6.
//!
//! Every string column is nullable in the original, including ones the database
//! declares `NOT NULL`, so they stay `Option<String>` here.

use chrono::NaiveDateTime;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// A musical artist.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Artist {
    /// Primary key.
    pub id: i32,
    /// The artist's name.
    pub name: Option<String>,
}

/// An album, belonging to an artist.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Album {
    /// Primary key.
    pub id: i32,
    /// The album's title.
    pub title: Option<String>,
    /// The artist who recorded it.
    pub artist_id: Option<i32>,
}

/// A musical genre.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Genre {
    /// Primary key.
    pub id: i32,
    /// The genre's name.
    pub name: Option<String>,
}

/// A media format, such as MPEG audio.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaType {
    /// Primary key.
    pub id: i32,
    /// The format's name.
    pub name: Option<String>,
}

/// A track on an album.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Track {
    /// Primary key.
    pub id: i32,
    /// The track's title.
    pub name: Option<String>,
    /// The album it appears on.
    pub album_id: Option<i32>,
    /// Its media format.
    pub media_type_id: Option<i32>,
    /// Its genre.
    pub genre_id: Option<i32>,
    /// Who wrote it.
    pub composer: Option<String>,
    /// Length in milliseconds.
    pub milliseconds: Option<i32>,
    /// Size in bytes.
    pub bytes: Option<i32>,
    /// Price of a single track.
    pub unit_price: Option<Decimal>,
}

/// A named collection of tracks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Playlist {
    /// Primary key.
    pub id: i32,
    /// The playlist's name.
    pub name: Option<String>,
}

/// The join between playlists and tracks.
///
/// The only table without a surrogate key: its primary key is the pair of
/// foreign keys, so unlike every other entity it has no `Id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistTrack {
    /// The playlist side of the pair.
    pub playlist_id: i32,
    /// The track side of the pair.
    pub track_id: i32,
}

/// A customer of the store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Customer {
    /// Primary key.
    pub id: i32,
    /// Given name.
    pub first_name: Option<String>,
    /// Family name.
    pub last_name: Option<String>,
    /// Employer.
    pub company: Option<String>,
    /// Street address.
    pub address: Option<String>,
    /// City.
    pub city: Option<String>,
    /// State or province.
    pub state: Option<String>,
    /// Country.
    pub country: Option<String>,
    /// Postal code.
    pub postal_code: Option<String>,
    /// Telephone number.
    pub phone: Option<String>,
    /// Fax number.
    pub fax: Option<String>,
    /// Email address.
    pub email: Option<String>,
    /// The employee who supports this customer.
    pub support_rep_id: Option<i32>,
}

/// An employee of the store.
///
/// Employees form a management hierarchy through [`Employee::reports_to`],
/// which points at another employee's `Id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Employee {
    /// Primary key.
    pub id: i32,
    /// Family name.
    pub last_name: Option<String>,
    /// Given name.
    pub first_name: Option<String>,
    /// Job title.
    pub title: Option<String>,
    /// The manager this employee reports to.
    pub reports_to: Option<i32>,
    /// Date of birth.
    pub birth_date: Option<NaiveDateTime>,
    /// Date hired.
    pub hire_date: Option<NaiveDateTime>,
    /// Street address.
    pub address: Option<String>,
    /// City.
    pub city: Option<String>,
    /// State or province.
    pub state: Option<String>,
    /// Country.
    pub country: Option<String>,
    /// Postal code.
    pub postal_code: Option<String>,
    /// Telephone number.
    pub phone: Option<String>,
    /// Fax number.
    pub fax: Option<String>,
    /// Email address.
    pub email: Option<String>,
}

/// A customer invoice.
///
/// `invoice_date` and `total` are the only non-nullable columns on any entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Invoice {
    /// Primary key.
    pub id: i32,
    /// The customer billed.
    pub customer_id: Option<i32>,
    /// When the invoice was raised.
    pub invoice_date: NaiveDateTime,
    /// Billing street address.
    pub billing_address: Option<String>,
    /// Billing city.
    pub billing_city: Option<String>,
    /// Billing state or province.
    pub billing_state: Option<String>,
    /// Billing country.
    pub billing_country: Option<String>,
    /// Billing postal code.
    pub billing_postal_code: Option<String>,
    /// Invoice total.
    pub total: Decimal,
}

/// A single line of an invoice.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct InvoiceLine {
    /// Primary key.
    pub id: i32,
    /// The invoice this line belongs to.
    pub invoice_id: Option<i32>,
    /// The track sold.
    pub track_id: Option<i32>,
    /// Price charged for one unit.
    pub unit_price: Option<Decimal>,
    /// How many units were sold.
    pub quantity: Option<i32>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use rust_decimal::prelude::FromPrimitive;
    use serde_json::json;

    #[test]
    fn columns_serialize_under_their_original_names() {
        let track = Track {
            id: 1,
            name: Some("For Those About To Rock".to_owned()),
            album_id: Some(1),
            media_type_id: Some(1),
            genre_id: Some(1),
            composer: Some("Angus Young".to_owned()),
            milliseconds: Some(343_719),
            bytes: Some(11_170_334),
            unit_price: Decimal::from_f64(0.99),
        };

        assert_eq!(
            serde_json::to_value(&track).expect("track should serialize"),
            json!({
                "Id": 1,
                "Name": "For Those About To Rock",
                "AlbumId": 1,
                "MediaTypeId": 1,
                "GenreId": 1,
                "Composer": "Angus Young",
                "Milliseconds": 343_719,
                "Bytes": 11_170_334,
                "UnitPrice": 0.99
            })
        );
    }

    #[test]
    fn money_serializes_as_a_number_not_a_string() {
        // System.Text.Json writes C#'s decimal as a bare JSON number, so a
        // quoted "0.99" here would be a visible difference.
        let value =
            serde_json::to_value(Decimal::from_f64(0.99)).expect("decimal should serialize");

        assert!(value.is_number(), "expected a JSON number, got {value}");
        assert_eq!(value, json!(0.99));
    }

    /// Every distinct `Invoice.Total` and `Track.UnitPrice` in the bundled
    /// database.
    ///
    /// Both columns are stored as SQLite `real`, so a value reaches us as an
    /// `f64` and has to survive `f64` to `Decimal` to JSON without picking up
    /// a `10.899999999999999` tail. Pinning the real value set is cheap, and
    /// this is the path the whole money contract rests on.
    const MONEY_VALUES: [f64; 37] = [
        0.99, 1.98, 1.99, 2.97, 2.98, 3.96, 3.97, 4.95, 4.96, 5.94, 5.95, 6.93, 6.94, 6.95, 7.92,
        7.93, 7.94, 8.91, 8.92, 8.93, 9.9, 9.91, 9.92, 10.89, 10.9, 10.91, 10.93, 11.89, 11.9,
        11.91, 12.88, 12.89, 13.86, 13.87, 13.88, 13.89, 14.86,
    ];

    #[test]
    fn every_money_value_in_the_database_survives_the_round_trip() {
        for value in MONEY_VALUES {
            let stored = Decimal::from_f64(value).expect("the column value should convert");

            let rendered = serde_json::to_string(&stored).expect("decimal should serialize");

            assert_eq!(
                rendered,
                value.to_string(),
                "{value} did not round-trip cleanly"
            );
        }
    }

    #[test]
    fn null_columns_are_emitted_rather_than_omitted() {
        // ASP.NET Core writes null members by default, so an absent key would
        // change the document's shape.
        let document = serde_json::to_value(Artist::default()).expect("artist should serialize");

        assert_eq!(document, json!({ "Id": 0, "Name": null }));
    }

    #[test]
    fn dates_use_the_same_iso_form_as_system_text_json() {
        let employee = Employee {
            id: 1,
            hire_date: Some(
                NaiveDateTime::parse_from_str("2002-08-14 00:00:00", "%Y-%m-%d %H:%M:%S")
                    .expect("valid timestamp"),
            ),
            ..Employee::default()
        };

        let document = serde_json::to_value(&employee).expect("employee should serialize");

        assert_eq!(document["HireDate"], json!("2002-08-14T00:00:00"));
    }

    #[test]
    fn the_playlist_join_has_no_surrogate_key() {
        let document = serde_json::to_value(PlaylistTrack {
            playlist_id: 1,
            track_id: 2,
        })
        .expect("join row should serialize");

        assert_eq!(document, json!({ "PlaylistId": 1, "TrackId": 2 }));
    }
}
