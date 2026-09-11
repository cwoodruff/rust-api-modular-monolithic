//! Conversions between entities and API models, ported from `IConvertModel<T>`.
//!
//! The C# interface is implemented in both directions — an entity converts to
//! its API model and back — so [`Convert`] is generic over the target and
//! implemented twice per pair.
//!
//! Two things about these conversions are easy to mistake for bugs:
//!
//! - **Entity to model copies scalars only.** Nested members stay empty, which
//!   is exactly why every collection endpoint returns flat models while the
//!   by-id endpoints return graphs: the by-id repositories build their models
//!   directly from a join rather than going through here.
//! - **Model to entity coalesces some nulls to empty strings, inconsistently.**
//!   `Employee` coalesces all fourteen of its strings, `Artist` and `Album`
//!   coalesce their one, and nothing else coalesces at all. That is what the
//!   original does, and a write path would persist the difference.

use crate::api_models::{
    AlbumApiModel, ArtistApiModel, CustomerApiModel, EmployeeApiModel, GenreApiModel,
    InvoiceApiModel, InvoiceLineApiModel, MediaTypeApiModel, PlaylistApiModel, TrackApiModel,
};
use crate::entities::{
    Album, Artist, Customer, Employee, Genre, Invoice, InvoiceLine, MediaType, Playlist, Track,
};

/// Port of `IConvertModel<TTarget>`.
pub trait Convert<T> {
    /// Produces the target representation of this value.
    fn convert(&self) -> T;
}

/// Port of `ConvertExtensions.ConvertAll<TTarget>`.
pub fn convert_all<S, T>(values: &[S]) -> Vec<T>
where
    S: Convert<T>,
{
    values.iter().map(Convert::convert).collect()
}

/// `null` becomes `""`, matching the original's `?? string.Empty`.
fn or_empty(value: &Option<String>) -> Option<String> {
    Some(value.clone().unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Entity to API model
// ---------------------------------------------------------------------------

impl Convert<ArtistApiModel> for Artist {
    fn convert(&self) -> ArtistApiModel {
        ArtistApiModel {
            id: self.id,
            name: self.name.clone(),
            albums: Vec::new(),
        }
    }
}

impl Convert<AlbumApiModel> for Album {
    fn convert(&self) -> AlbumApiModel {
        AlbumApiModel {
            id: self.id,
            title: self.title.clone(),
            // The original reads `Artist?.Name` from a navigation property that
            // a flat query never loaded, so this is always null here.
            artist_name: None,
            artist_id: self.artist_id,
            artist: None,
            tracks: Vec::new(),
        }
    }
}

impl Convert<TrackApiModel> for Track {
    fn convert(&self) -> TrackApiModel {
        TrackApiModel {
            id: self.id,
            name: self.name.clone(),
            album_id: self.album_id,
            media_type_id: self.media_type_id,
            genre_id: self.genre_id,
            composer: self.composer.clone(),
            milliseconds: self.milliseconds,
            bytes: self.bytes,
            unit_price: self.unit_price,
            ..TrackApiModel::default()
        }
    }
}

impl Convert<GenreApiModel> for Genre {
    fn convert(&self) -> GenreApiModel {
        GenreApiModel {
            id: self.id,
            name: self.name.clone(),
            tracks: Vec::new(),
        }
    }
}

impl Convert<MediaTypeApiModel> for MediaType {
    fn convert(&self) -> MediaTypeApiModel {
        MediaTypeApiModel {
            id: self.id,
            name: self.name.clone(),
            tracks: Vec::new(),
        }
    }
}

impl Convert<PlaylistApiModel> for Playlist {
    fn convert(&self) -> PlaylistApiModel {
        PlaylistApiModel {
            id: self.id,
            name: self.name.clone(),
            tracks: Vec::new(),
        }
    }
}

impl Convert<CustomerApiModel> for Customer {
    fn convert(&self) -> CustomerApiModel {
        CustomerApiModel {
            id: self.id,
            first_name: self.first_name.clone(),
            last_name: self.last_name.clone(),
            company: self.company.clone(),
            address: self.address.clone(),
            city: self.city.clone(),
            state: self.state.clone(),
            country: self.country.clone(),
            postal_code: self.postal_code.clone(),
            phone: self.phone.clone(),
            fax: self.fax.clone(),
            email: self.email.clone(),
            // The original's Customer.Convert() omits SupportRepId, so a
            // customer from a collection endpoint reports no support rep even
            // when the row has one.
            support_rep_id: None,
            support_rep_name: None,
            invoices: Vec::new(),
            support_rep: None,
        }
    }
}

impl Convert<EmployeeApiModel> for Employee {
    fn convert(&self) -> EmployeeApiModel {
        EmployeeApiModel {
            id: self.id,
            last_name: self.last_name.clone(),
            first_name: self.first_name.clone(),
            title: self.title.clone(),
            reports_to: self.reports_to,
            birth_date: self.birth_date,
            hire_date: self.hire_date,
            address: self.address.clone(),
            city: self.city.clone(),
            state: self.state.clone(),
            country: self.country.clone(),
            postal_code: self.postal_code.clone(),
            phone: self.phone.clone(),
            fax: self.fax.clone(),
            email: self.email.clone(),
            customers: Vec::new(),
            inverse_reports_to_navigation: Vec::new(),
            reports_to_navigation: None,
        }
    }
}

impl Convert<InvoiceApiModel> for Invoice {
    fn convert(&self) -> InvoiceApiModel {
        InvoiceApiModel {
            id: self.id,
            customer_id: self.customer_id,
            invoice_date: self.invoice_date,
            billing_address: self.billing_address.clone(),
            billing_city: self.billing_city.clone(),
            billing_state: self.billing_state.clone(),
            billing_country: self.billing_country.clone(),
            billing_postal_code: self.billing_postal_code.clone(),
            total: self.total,
            customer: None,
            invoice_lines: Vec::new(),
        }
    }
}

impl Convert<InvoiceLineApiModel> for InvoiceLine {
    fn convert(&self) -> InvoiceLineApiModel {
        InvoiceLineApiModel {
            id: self.id,
            invoice_id: self.invoice_id,
            track_id: self.track_id,
            track_name: None,
            unit_price: self.unit_price,
            quantity: self.quantity,
            invoice: None,
            track: None,
        }
    }
}

// ---------------------------------------------------------------------------
// API model to entity
// ---------------------------------------------------------------------------

impl Convert<Artist> for ArtistApiModel {
    fn convert(&self) -> Artist {
        Artist {
            id: self.id,
            name: or_empty(&self.name),
        }
    }
}

impl Convert<Album> for AlbumApiModel {
    fn convert(&self) -> Album {
        Album {
            id: self.id,
            title: or_empty(&self.title),
            artist_id: self.artist_id,
        }
    }
}

impl Convert<Track> for TrackApiModel {
    fn convert(&self) -> Track {
        Track {
            id: self.id,
            name: self.name.clone(),
            album_id: self.album_id,
            media_type_id: self.media_type_id,
            genre_id: self.genre_id,
            composer: self.composer.clone(),
            milliseconds: self.milliseconds,
            bytes: self.bytes,
            unit_price: self.unit_price,
        }
    }
}

impl Convert<Genre> for GenreApiModel {
    fn convert(&self) -> Genre {
        Genre {
            id: self.id,
            name: self.name.clone(),
        }
    }
}

impl Convert<MediaType> for MediaTypeApiModel {
    fn convert(&self) -> MediaType {
        MediaType {
            id: self.id,
            name: self.name.clone(),
        }
    }
}

impl Convert<Playlist> for PlaylistApiModel {
    fn convert(&self) -> Playlist {
        Playlist {
            id: self.id,
            name: self.name.clone(),
        }
    }
}

impl Convert<Customer> for CustomerApiModel {
    fn convert(&self) -> Customer {
        Customer {
            id: self.id,
            first_name: self.first_name.clone(),
            last_name: self.last_name.clone(),
            company: self.company.clone(),
            address: self.address.clone(),
            city: self.city.clone(),
            state: self.state.clone(),
            country: self.country.clone(),
            postal_code: self.postal_code.clone(),
            phone: self.phone.clone(),
            fax: self.fax.clone(),
            email: self.email.clone(),
            // Unlike the entity-to-model direction, this one does carry it.
            support_rep_id: self.support_rep_id,
        }
    }
}

impl Convert<Employee> for EmployeeApiModel {
    fn convert(&self) -> Employee {
        Employee {
            id: self.id,
            last_name: or_empty(&self.last_name),
            first_name: or_empty(&self.first_name),
            title: or_empty(&self.title),
            reports_to: self.reports_to,
            birth_date: self.birth_date,
            hire_date: self.hire_date,
            address: or_empty(&self.address),
            city: or_empty(&self.city),
            state: or_empty(&self.state),
            country: or_empty(&self.country),
            postal_code: or_empty(&self.postal_code),
            phone: or_empty(&self.phone),
            fax: or_empty(&self.fax),
            email: or_empty(&self.email),
        }
    }
}

impl Convert<Invoice> for InvoiceApiModel {
    fn convert(&self) -> Invoice {
        Invoice {
            id: self.id,
            customer_id: self.customer_id,
            invoice_date: self.invoice_date,
            billing_address: self.billing_address.clone(),
            billing_city: self.billing_city.clone(),
            billing_state: self.billing_state.clone(),
            billing_country: self.billing_country.clone(),
            billing_postal_code: self.billing_postal_code.clone(),
            total: self.total,
        }
    }
}

impl Convert<InvoiceLine> for InvoiceLineApiModel {
    fn convert(&self) -> InvoiceLine {
        InvoiceLine {
            id: self.id,
            invoice_id: self.invoice_id,
            track_id: self.track_id,
            unit_price: self.unit_price,
            quantity: self.quantity,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn entity_to_model_leaves_the_graph_empty() {
        let album = Album {
            id: 1,
            title: Some("Let It Be".to_owned()),
            artist_id: Some(2),
        };

        let model: AlbumApiModel = album.convert();

        assert_eq!(model.id, 1);
        assert_eq!(model.title.as_deref(), Some("Let It Be"));
        assert_eq!(model.artist_id, Some(2));
        assert_eq!(
            model.artist_name, None,
            "a flat conversion has no artist to read a name from"
        );
        assert!(model.artist.is_none());
        assert!(model.tracks.is_empty());
    }

    #[test]
    fn a_converted_customer_drops_its_support_rep() {
        // Faithful to the original: Customer.Convert() lists every scalar
        // except SupportRepId.
        let customer = Customer {
            id: 1,
            first_name: Some("Luis".to_owned()),
            last_name: Some("Gonçalves".to_owned()),
            support_rep_id: Some(3),
            ..Customer::default()
        };

        let model: CustomerApiModel = customer.convert();

        assert_eq!(model.first_name.as_deref(), Some("Luis"));
        assert_eq!(model.support_rep_id, None);
    }

    #[test]
    fn a_converted_customer_model_keeps_its_support_rep() {
        // ...while the reverse direction does carry it.
        let model = CustomerApiModel {
            id: 1,
            support_rep_id: Some(3),
            ..CustomerApiModel::default()
        };

        let customer: Customer = model.convert();

        assert_eq!(customer.support_rep_id, Some(3));
    }

    #[test]
    fn employee_model_to_entity_coalesces_every_null_string() {
        let employee: Employee = EmployeeApiModel {
            id: 1,
            first_name: Some("Andrew".to_owned()),
            ..EmployeeApiModel::default()
        }
        .convert();

        assert_eq!(employee.first_name.as_deref(), Some("Andrew"));
        for (field, value) in [
            ("last_name", &employee.last_name),
            ("title", &employee.title),
            ("address", &employee.address),
            ("city", &employee.city),
            ("email", &employee.email),
        ] {
            assert_eq!(
                value.as_deref(),
                Some(""),
                "{field} should coalesce to an empty string, not stay null"
            );
        }
    }

    #[test]
    fn only_some_entities_coalesce_nulls() {
        // Artist and Album coalesce their one string; Genre does not.
        let artist: Artist = ArtistApiModel::default().convert();
        let album: Album = AlbumApiModel::default().convert();
        let genre: Genre = GenreApiModel::default().convert();

        assert_eq!(artist.name.as_deref(), Some(""));
        assert_eq!(album.title.as_deref(), Some(""));
        assert_eq!(genre.name, None, "Genre.Convert() has no coalesce");
    }

    #[test]
    fn scalars_survive_a_round_trip_through_both_directions() {
        let original = Track {
            id: 1,
            name: Some("Track".to_owned()),
            album_id: Some(2),
            media_type_id: Some(3),
            genre_id: Some(4),
            composer: Some("Composer".to_owned()),
            milliseconds: Some(1000),
            bytes: Some(2000),
            unit_price: Some("0.99".parse().expect("valid decimal")),
        };

        let model: TrackApiModel = original.convert();
        let restored: Track = model.convert();

        assert_eq!(restored, original);
    }

    #[test]
    fn convert_all_maps_a_whole_collection() {
        let genres = vec![
            Genre {
                id: 1,
                name: Some("Rock".to_owned()),
            },
            Genre {
                id: 2,
                name: Some("Jazz".to_owned()),
            },
        ];

        let models: Vec<GenreApiModel> = convert_all(&genres);

        assert_eq!(models.len(), 2);
        assert_eq!(models[1].name.as_deref(), Some("Jazz"));
    }
}
