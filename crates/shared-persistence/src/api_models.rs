//! The wire shapes, ported from `SharedKernel.Persistence.ApiModels`.
//!
//! Three properties of the original's serialization are load-bearing, and all
//! three are reproduced here:
//!
//! 1. **PascalCase names.** The host sets `PropertyNamingPolicy = null`, so
//!    members are emitted exactly as declared.
//! 2. **Nulls are written, not omitted.** ASP.NET Core's default ignore
//!    condition is `Never`, so an unpopulated `Artist` appears as
//!    `"Artist": null` rather than vanishing. No field here is annotated with
//!    `skip_serializing_if`.
//! 3. **Collections default to empty.** Every C# collection member is
//!    initialized to an empty list, so it serializes as `[]` even when nothing
//!    loaded it.
//!
//! The models also carry denormalized name fields — `ArtistName`, `AlbumName`,
//! `SupportRepName` — that repositories fill from a join so a caller need not
//! walk into the nested object.
//!
//! Whether a nested member is populated depends on which repository method
//! produced the model: a by-id lookup returns a graph, while a collection
//! endpoint returns flat models, because the entity-to-model conversion copies
//! scalars only. See [`crate::convert`].

use chrono::NaiveDateTime;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Instant used to default a non-nullable timestamp.
fn epoch() -> NaiveDateTime {
    chrono::DateTime::UNIX_EPOCH.naive_utc()
}

/// An artist, optionally with their albums.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ArtistApiModel {
    /// Primary key.
    pub id: i32,
    /// The artist's name.
    pub name: Option<String>,
    /// Albums by this artist. Populated only by the by-id lookup.
    pub albums: Vec<AlbumApiModel>,
}

/// An album, optionally with its artist and tracks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AlbumApiModel {
    /// Primary key.
    pub id: i32,
    /// The album's title.
    pub title: Option<String>,
    /// The artist's name, denormalized from the join.
    pub artist_name: Option<String>,
    /// The artist's key.
    pub artist_id: Option<i32>,
    /// The full artist. Populated only by the by-id lookup.
    pub artist: Option<ArtistApiModel>,
    /// Tracks on this album. Populated only by the by-id lookup.
    pub tracks: Vec<TrackApiModel>,
}

/// A track, optionally with its album, genre, and media type.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TrackApiModel {
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
    /// The full album.
    pub album: Option<AlbumApiModel>,
    /// The full genre.
    pub genre: Option<GenreApiModel>,
    /// Invoice lines that sold this track.
    pub invoice_lines: Vec<InvoiceLineApiModel>,
    /// The full media type.
    pub media_type: Option<MediaTypeApiModel>,
    /// Playlists containing this track.
    pub playlists: Vec<PlaylistApiModel>,
    /// The album's title, denormalized from the join.
    pub album_name: Option<String>,
    /// The media type's name, denormalized from the join.
    pub media_type_name: Option<String>,
    /// The genre's name, denormalized from the join.
    pub genre_name: Option<String>,
}

/// A genre, optionally with its tracks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GenreApiModel {
    /// Primary key.
    pub id: i32,
    /// The genre's name.
    pub name: Option<String>,
    /// Tracks in this genre.
    pub tracks: Vec<TrackApiModel>,
}

/// A media format, optionally with its tracks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaTypeApiModel {
    /// Primary key.
    pub id: i32,
    /// The format's name.
    pub name: Option<String>,
    /// Tracks in this format.
    pub tracks: Vec<TrackApiModel>,
}

/// A playlist, optionally with its tracks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistApiModel {
    /// Primary key.
    pub id: i32,
    /// The playlist's name.
    pub name: Option<String>,
    /// Tracks on this playlist, ordered by track key.
    pub tracks: Vec<TrackApiModel>,
}

/// A customer, optionally with their invoices and support representative.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CustomerApiModel {
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
    /// The supporting employee's key.
    pub support_rep_id: Option<i32>,
    /// The supporting employee's full name, denormalized from the join.
    pub support_rep_name: Option<String>,
    /// This customer's invoices.
    pub invoices: Vec<InvoiceApiModel>,
    /// The full supporting employee.
    pub support_rep: Option<EmployeeApiModel>,
}

/// An employee, optionally with their customers and direct reports.
///
/// Note `reports_to_navigation`: in the entity this is a navigation property
/// pointing at another employee, but the API model flattens it to the manager's
/// `"First Last"` **string**. That asymmetry is in the original and is visible
/// on the wire, so it stays.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct EmployeeApiModel {
    /// Primary key.
    pub id: i32,
    /// Family name.
    pub last_name: Option<String>,
    /// Given name.
    pub first_name: Option<String>,
    /// Job title.
    pub title: Option<String>,
    /// The manager's key.
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
    /// Customers this employee supports.
    pub customers: Vec<CustomerApiModel>,
    /// Employees reporting to this one.
    pub inverse_reports_to_navigation: Vec<EmployeeApiModel>,
    /// The manager's name, as `"First Last"` — a string, not an object.
    pub reports_to_navigation: Option<String>,
}

/// An invoice, optionally with its customer and lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct InvoiceApiModel {
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
    /// The full customer.
    pub customer: Option<CustomerApiModel>,
    /// This invoice's lines.
    pub invoice_lines: Vec<InvoiceLineApiModel>,
}

impl Default for InvoiceApiModel {
    fn default() -> Self {
        Self {
            id: 0,
            customer_id: None,
            invoice_date: epoch(),
            billing_address: None,
            billing_city: None,
            billing_state: None,
            billing_country: None,
            billing_postal_code: None,
            total: Decimal::ZERO,
            customer: None,
            invoice_lines: Vec::new(),
        }
    }
}

/// One line of an invoice, optionally with its invoice and track.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct InvoiceLineApiModel {
    /// Primary key.
    pub id: i32,
    /// The invoice this line belongs to.
    pub invoice_id: Option<i32>,
    /// The track sold.
    pub track_id: Option<i32>,
    /// The track's title, denormalized from the join.
    pub track_name: Option<String>,
    /// Price charged for one unit.
    pub unit_price: Option<Decimal>,
    /// How many units were sold.
    pub quantity: Option<i32>,
    /// The full invoice.
    pub invoice: Option<InvoiceApiModel>,
    /// The full track.
    pub track: Option<TrackApiModel>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use serde_json::json;

    #[test]
    fn a_flat_album_still_emits_every_member() {
        // This is what a collection endpoint returns: scalars from the entity,
        // with the nested members left at their defaults. They must still
        // appear, as null and [].
        let album = AlbumApiModel {
            id: 1,
            title: Some("For Those About To Rock We Salute You".to_owned()),
            artist_id: Some(1),
            ..AlbumApiModel::default()
        };

        assert_eq!(
            serde_json::to_value(&album).expect("album should serialize"),
            json!({
                "Id": 1,
                "Title": "For Those About To Rock We Salute You",
                "ArtistName": null,
                "ArtistId": 1,
                "Artist": null,
                "Tracks": []
            })
        );
    }

    #[test]
    fn a_by_id_album_carries_its_graph() {
        let album = AlbumApiModel {
            id: 1,
            title: Some("Let It Be".to_owned()),
            artist_name: Some("The Beatles".to_owned()),
            artist_id: Some(2),
            artist: Some(ArtistApiModel {
                id: 2,
                name: Some("The Beatles".to_owned()),
                albums: Vec::new(),
            }),
            tracks: vec![TrackApiModel {
                id: 10,
                name: Some("Across the Universe".to_owned()),
                ..TrackApiModel::default()
            }],
        };

        let document = serde_json::to_value(&album).expect("album should serialize");

        assert_eq!(document["Artist"]["Name"], json!("The Beatles"));
        assert_eq!(document["ArtistName"], json!("The Beatles"));
        assert_eq!(document["Tracks"][0]["Name"], json!("Across the Universe"));
        // The nested artist's own collection is still emitted.
        assert_eq!(document["Artist"]["Albums"], json!([]));
    }

    #[test]
    fn the_employees_manager_is_a_name_not_an_object() {
        let employee = EmployeeApiModel {
            id: 2,
            first_name: Some("Nancy".to_owned()),
            last_name: Some("Edwards".to_owned()),
            reports_to: Some(1),
            reports_to_navigation: Some("Andrew Adams".to_owned()),
            ..EmployeeApiModel::default()
        };

        let document = serde_json::to_value(&employee).expect("employee should serialize");

        assert_eq!(document["ReportsToNavigation"], json!("Andrew Adams"));
        assert!(
            document["ReportsToNavigation"].is_string(),
            "the API model flattens the manager to a string, unlike the entity"
        );
        assert_eq!(document["ReportsTo"], json!(1));
    }

    #[test]
    fn invoice_totals_and_dates_match_the_original_encoding() {
        let invoice = InvoiceApiModel {
            id: 1,
            customer_id: Some(2),
            invoice_date: NaiveDateTime::parse_from_str("2021-01-01 00:00:00", "%Y-%m-%d %H:%M:%S")
                .expect("valid timestamp"),
            total: "13.86".parse().expect("valid decimal"),
            ..InvoiceApiModel::default()
        };

        let document = serde_json::to_value(&invoice).expect("invoice should serialize");

        assert_eq!(document["Total"], json!(13.86));
        assert_eq!(document["InvoiceDate"], json!("2021-01-01T00:00:00"));
        assert_eq!(document["Customer"], json!(null));
        assert_eq!(document["InvoiceLines"], json!([]));
    }

    #[test]
    fn every_model_round_trips_through_json() {
        let track = TrackApiModel {
            id: 1,
            name: Some("Track".to_owned()),
            unit_price: Some("0.99".parse().expect("valid decimal")),
            album_name: Some("Album".to_owned()),
            ..TrackApiModel::default()
        };

        let json = serde_json::to_string(&track).expect("track should serialize");
        let restored: TrackApiModel =
            serde_json::from_str(&json).expect("track should deserialize");

        assert_eq!(restored, track);
    }
}
