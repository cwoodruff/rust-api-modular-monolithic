//! The ten validators, ported rule for rule from
//! `SharedKernel.Persistence.Validation`.
//!
//! Rules are written in the original's declaration order, because that is the
//! order failures appear in the `errors` map of a 400 response.
//!
//! Remember the governing semantic from [`super::rules`]: only `not_null`
//! rejects a missing value. A field with a length or pattern rule but no
//! `not_null` is optional, and is checked only when it is present.

use rust_decimal::Decimal;

use super::rules::{Rules, Validate, ValidationFailure, phone_pattern, postal_code_pattern};
use crate::api_models::{
    AlbumApiModel, ArtistApiModel, CustomerApiModel, EmployeeApiModel, GenreApiModel,
    InvoiceApiModel, InvoiceLineApiModel, MediaTypeApiModel, PlaylistApiModel, TrackApiModel,
};

/// The ceiling both price rules compare against: `(decimal)9.99`.
fn max_unit_price() -> Decimal {
    Decimal::new(999, 2)
}

impl Validate for AlbumApiModel {
    fn validate(&self) -> Result<(), Vec<ValidationFailure>> {
        let mut rules = Rules::new();
        rules
            .not_null("Title", self.title.as_ref())
            .min_length("Title", self.title.as_deref(), 3)
            .max_length("Title", self.title.as_deref(), 160)
            .not_null("ArtistId", self.artist_id.as_ref());
        rules.finish()
    }
}

impl Validate for ArtistApiModel {
    fn validate(&self) -> Result<(), Vec<ValidationFailure>> {
        let mut rules = Rules::new();
        rules
            .not_null("Name", self.name.as_ref())
            .max_length("Name", self.name.as_deref(), 120);
        rules.finish()
    }
}

impl Validate for GenreApiModel {
    fn validate(&self) -> Result<(), Vec<ValidationFailure>> {
        let mut rules = Rules::new();
        rules
            .not_null("Name", self.name.as_ref())
            .max_length("Name", self.name.as_deref(), 120);
        rules.finish()
    }
}

impl Validate for MediaTypeApiModel {
    fn validate(&self) -> Result<(), Vec<ValidationFailure>> {
        let mut rules = Rules::new();
        rules
            .not_null("Name", self.name.as_ref())
            .max_length("Name", self.name.as_deref(), 120);
        rules.finish()
    }
}

impl Validate for PlaylistApiModel {
    fn validate(&self) -> Result<(), Vec<ValidationFailure>> {
        let mut rules = Rules::new();
        rules
            .not_null("Name", self.name.as_ref())
            .max_length("Name", self.name.as_deref(), 120);
        rules.finish()
    }
}

impl Validate for TrackApiModel {
    fn validate(&self) -> Result<(), Vec<ValidationFailure>> {
        let mut rules = Rules::new();
        rules
            .not_null("Name", self.name.as_ref())
            .max_length("Name", self.name.as_deref(), 200)
            .greater_than("Bytes", self.bytes, 0)
            .greater_than("Milliseconds", self.milliseconds, 0)
            .not_null("Composer", self.composer.as_ref())
            .max_length("Composer", self.composer.as_deref(), 220)
            .greater_than("UnitPrice", self.unit_price, Decimal::ZERO)
            .less_than_or_equal("UnitPrice", self.unit_price, max_unit_price())
            .not_null("AlbumId", self.album_id.as_ref())
            .not_null("GenreId", self.genre_id.as_ref())
            .not_null("MediaTypeId", self.media_type_id.as_ref());
        rules.finish()
    }
}

impl Validate for CustomerApiModel {
    fn validate(&self) -> Result<(), Vec<ValidationFailure>> {
        let mut rules = Rules::new();
        rules
            .not_null("FirstName", self.first_name.as_ref())
            .not_null("LastName", self.last_name.as_ref())
            .email("Email", self.email.as_deref())
            .matches("Phone", self.phone.as_deref(), phone_pattern())
            .matches("Fax", self.fax.as_deref(), phone_pattern())
            .max_length("FirstName", self.first_name.as_deref(), 40)
            .max_length("LastName", self.last_name.as_deref(), 20)
            .max_length("Company", self.company.as_deref(), 80)
            .max_length("Address", self.address.as_deref(), 70)
            .max_length("City", self.city.as_deref(), 40)
            .max_length("State", self.state.as_deref(), 40)
            .max_length("Country", self.country.as_deref(), 40)
            .matches(
                "PostalCode",
                self.postal_code.as_deref(),
                postal_code_pattern(),
            );
        rules.finish()
    }
}

impl Validate for EmployeeApiModel {
    fn validate(&self) -> Result<(), Vec<ValidationFailure>> {
        let mut rules = Rules::new();
        rules
            .not_null("FirstName", self.first_name.as_ref())
            .not_null("LastName", self.last_name.as_ref())
            .email("Email", self.email.as_deref())
            .matches("Phone", self.phone.as_deref(), phone_pattern())
            .matches("Fax", self.fax.as_deref(), phone_pattern())
            // Note the tighter first-name limit than Customer's 40.
            .max_length("FirstName", self.first_name.as_deref(), 20)
            .max_length("LastName", self.last_name.as_deref(), 20)
            .max_length("Title", self.title.as_deref(), 30)
            .max_length("Address", self.address.as_deref(), 70)
            .max_length("City", self.city.as_deref(), 40)
            .max_length("State", self.state.as_deref(), 40)
            .max_length("Country", self.country.as_deref(), 40)
            .matches(
                "PostalCode",
                self.postal_code.as_deref(),
                postal_code_pattern(),
            );
        rules.finish()
    }
}

impl Validate for InvoiceApiModel {
    fn validate(&self) -> Result<(), Vec<ValidationFailure>> {
        let mut rules = Rules::new();
        rules
            .not_null("CustomerId", self.customer_id.as_ref())
            // InvoiceDate and Total are non-nullable in the original too, so
            // these two rules can never fail. They are kept because they are
            // declared, and dropping them would quietly change the rule set.
            .not_null("InvoiceDate", Some(&self.invoice_date))
            .not_null("Total", Some(&self.total))
            .greater_than("Total", Some(self.total), Decimal::ZERO)
            .not_null("BillingAddress", self.billing_address.as_ref())
            .not_null("BillingCity", self.billing_city.as_ref())
            .not_null("BillingCountry", self.billing_country.as_ref())
            .not_null("BillingState", self.billing_state.as_ref())
            .not_null("BillingPostalCode", self.billing_postal_code.as_ref())
            .max_length("BillingAddress", self.billing_address.as_deref(), 70)
            .max_length("BillingCity", self.billing_city.as_deref(), 40)
            .max_length("BillingCountry", self.billing_country.as_deref(), 40)
            .max_length("BillingState", self.billing_state.as_deref(), 40)
            .matches(
                "BillingPostalCode",
                self.billing_postal_code.as_deref(),
                postal_code_pattern(),
            );
        rules.finish()
    }
}

impl Validate for InvoiceLineApiModel {
    fn validate(&self) -> Result<(), Vec<ValidationFailure>> {
        let mut rules = Rules::new();
        rules
            .not_null("InvoiceId", self.invoice_id.as_ref())
            .not_null("TrackId", self.track_id.as_ref())
            .not_null("Quantity", self.quantity.as_ref())
            .greater_than("Quantity", self.quantity, 0)
            .not_null("UnitPrice", self.unit_price.as_ref())
            .greater_than("UnitPrice", self.unit_price, Decimal::ZERO)
            .less_than_or_equal("UnitPrice", self.unit_price, max_unit_price());
        rules.finish()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn failures<T: Validate>(model: &T) -> Vec<ValidationFailure> {
        model.validate().err().unwrap_or_default()
    }

    fn properties<T: Validate>(model: &T) -> Vec<String> {
        failures(model)
            .into_iter()
            .map(|failure| failure.property_name)
            .collect()
    }

    fn valid_album() -> AlbumApiModel {
        AlbumApiModel {
            title: Some("Let It Be".to_owned()),
            artist_id: Some(1),
            ..AlbumApiModel::default()
        }
    }

    fn valid_track() -> TrackApiModel {
        TrackApiModel {
            name: Some("Across the Universe".to_owned()),
            composer: Some("Lennon/McCartney".to_owned()),
            album_id: Some(1),
            genre_id: Some(1),
            media_type_id: Some(1),
            bytes: Some(1000),
            milliseconds: Some(1000),
            unit_price: Some(Decimal::new(99, 2)),
            ..TrackApiModel::default()
        }
    }

    fn valid_customer() -> CustomerApiModel {
        CustomerApiModel {
            first_name: Some("Luis".to_owned()),
            last_name: Some("Gonçalves".to_owned()),
            ..CustomerApiModel::default()
        }
    }

    fn valid_invoice() -> InvoiceApiModel {
        InvoiceApiModel {
            customer_id: Some(1),
            total: Decimal::new(999, 2),
            billing_address: Some("Av. Brigadeiro Faria Lima, 2170".to_owned()),
            billing_city: Some("São José dos Campos".to_owned()),
            billing_state: Some("SP".to_owned()),
            billing_country: Some("Brazil".to_owned()),
            billing_postal_code: Some("12345".to_owned()),
            ..InvoiceApiModel::default()
        }
    }

    fn valid_invoice_line() -> InvoiceLineApiModel {
        InvoiceLineApiModel {
            invoice_id: Some(1),
            track_id: Some(1),
            quantity: Some(1),
            unit_price: Some(Decimal::new(99, 2)),
            ..InvoiceLineApiModel::default()
        }
    }

    #[test]
    fn the_valid_fixtures_really_are_valid() {
        assert!(valid_album().validate().is_ok());
        assert!(valid_track().validate().is_ok());
        assert!(valid_customer().validate().is_ok());
        assert!(valid_invoice().validate().is_ok());
        assert!(valid_invoice_line().validate().is_ok());
    }

    #[test]
    fn album_requires_a_title_of_at_least_three_characters() {
        let mut album = valid_album();
        album.title = Some("ab".to_owned());

        assert_eq!(properties(&album), vec!["Title"]);

        album.title = Some("abc".to_owned());
        assert!(album.validate().is_ok());
    }

    #[test]
    fn album_title_is_capped_at_160_characters() {
        let mut album = valid_album();
        album.title = Some("a".repeat(160));
        assert!(album.validate().is_ok());

        album.title = Some("a".repeat(161));
        assert_eq!(properties(&album), vec!["Title"]);
    }

    #[test]
    fn an_empty_album_reports_every_broken_rule_at_once() {
        // Title is null, so NotNull fires; the length rules skip null, so they
        // do not. ArtistId is null, so it fires too.
        assert_eq!(
            properties(&AlbumApiModel::default()),
            vec!["Title", "ArtistId"]
        );
    }

    #[test]
    fn a_customer_email_is_optional_but_checked_when_present() {
        // The heart of the null semantics: no NotNull on Email.
        let mut customer = valid_customer();
        assert!(customer.email.is_none());
        assert!(customer.validate().is_ok(), "a missing email is fine");

        customer.email = Some("not-an-email".to_owned());
        assert_eq!(properties(&customer), vec!["Email"]);

        customer.email = Some("luisg@embraer.com.br".to_owned());
        assert!(customer.validate().is_ok());
    }

    #[test]
    fn a_customer_postal_code_is_optional_but_must_be_a_us_code() {
        let mut customer = valid_customer();

        customer.postal_code = Some("12345-6789".to_owned());
        assert!(customer.validate().is_ok());

        // The very format the seed data is full of.
        customer.postal_code = Some("K1A 0B1".to_owned());
        assert_eq!(properties(&customer), vec!["PostalCode"]);
    }

    #[test]
    fn customer_and_employee_disagree_about_first_name_length() {
        let long_name = "a".repeat(30);

        let customer = CustomerApiModel {
            first_name: Some(long_name.clone()),
            last_name: Some("Smith".to_owned()),
            ..CustomerApiModel::default()
        };
        let employee = EmployeeApiModel {
            first_name: Some(long_name),
            last_name: Some("Smith".to_owned()),
            ..EmployeeApiModel::default()
        };

        assert!(customer.validate().is_ok(), "Customer allows 40 characters");
        assert_eq!(
            properties(&employee),
            vec!["FirstName"],
            "Employee allows only 20"
        );
    }

    #[test]
    fn a_track_price_must_sit_between_zero_and_the_ceiling() {
        let mut track = valid_track();

        track.unit_price = Some(Decimal::new(999, 2));
        assert!(track.validate().is_ok(), "9.99 is the inclusive ceiling");

        track.unit_price = Some(Decimal::new(1000, 2));
        assert_eq!(properties(&track), vec!["UnitPrice"]);

        track.unit_price = Some(Decimal::ZERO);
        assert_eq!(properties(&track), vec!["UnitPrice"]);

        // No NotNull on UnitPrice, so absent is acceptable.
        track.unit_price = None;
        assert!(track.validate().is_ok());
    }

    #[test]
    fn the_price_message_names_the_ceiling_the_way_csharp_formats_it() {
        let mut track = valid_track();
        track.unit_price = Some(Decimal::new(1050, 2));

        assert_eq!(
            failures(&track)[0].message,
            "'Unit Price' must be less than or equal to '9.99'."
        );
    }

    #[test]
    fn track_sizes_must_be_positive_when_supplied() {
        let mut track = valid_track();

        track.bytes = Some(0);
        track.milliseconds = Some(-1);
        assert_eq!(properties(&track), vec!["Bytes", "Milliseconds"]);

        track.bytes = None;
        track.milliseconds = None;
        assert!(track.validate().is_ok(), "both rules skip a missing value");
    }

    #[test]
    fn an_empty_track_reports_only_the_required_fields() {
        assert_eq!(
            properties(&TrackApiModel::default()),
            vec!["Name", "Composer", "AlbumId", "GenreId", "MediaTypeId"]
        );
    }

    #[test]
    fn an_invoice_needs_a_positive_total_and_every_billing_field() {
        let mut invoice = valid_invoice();

        invoice.total = Decimal::ZERO;
        assert_eq!(properties(&invoice), vec!["Total"]);

        let empty = InvoiceApiModel::default();
        assert_eq!(
            properties(&empty),
            vec![
                "CustomerId",
                "Total",
                "BillingAddress",
                "BillingCity",
                "BillingCountry",
                "BillingState",
                "BillingPostalCode",
            ],
            "the NotNull rules on the non-nullable InvoiceDate and Total cannot fire"
        );
    }

    #[test]
    fn an_invoice_line_needs_every_field() {
        assert_eq!(
            properties(&InvoiceLineApiModel::default()),
            vec!["InvoiceId", "TrackId", "Quantity", "UnitPrice"]
        );
    }

    #[test]
    fn an_invoice_line_quantity_must_be_positive() {
        let mut line = valid_invoice_line();
        line.quantity = Some(0);

        assert_eq!(properties(&line), vec!["Quantity"]);
    }

    #[test]
    fn the_three_name_only_validators_share_one_rule_set() {
        for name in [Some("a".repeat(121)), None] {
            let genre = GenreApiModel {
                name: name.clone(),
                ..GenreApiModel::default()
            };
            let media_type = MediaTypeApiModel {
                name: name.clone(),
                ..MediaTypeApiModel::default()
            };
            let playlist = PlaylistApiModel {
                name: name.clone(),
                ..PlaylistApiModel::default()
            };

            assert_eq!(properties(&genre), vec!["Name"]);
            assert_eq!(properties(&media_type), vec!["Name"]);
            assert_eq!(properties(&playlist), vec!["Name"]);
        }

        let valid = GenreApiModel {
            name: Some("Rock".to_owned()),
            ..GenreApiModel::default()
        };
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn a_genre_name_of_exactly_120_characters_is_accepted() {
        // The boundary the Administration write endpoints exercise.
        let genre = GenreApiModel {
            name: Some("a".repeat(120)),
            ..GenreApiModel::default()
        };

        assert!(genre.validate().is_ok());
    }
}
