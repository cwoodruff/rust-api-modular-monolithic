//! Administration's wire shapes, ported from `Admin.Module/Models`.
//!
//! PascalCase, nulls written, collections empty rather than absent (P1).
//! Upstream dropped `Customer.Invoices` and `Genre.Tracks`/`MediaType.Tracks`
//! when the cross-module navigations went (ADR-0004, ADR-0006, ADR-0007).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use shared_kernel::json::dotnet;

/// A customer.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct CustomerApiModel {
    pub id: i32,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub company: Option<String>,
    pub address: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub postal_code: Option<String>,
    pub phone: Option<String>,
    pub fax: Option<String>,
    pub email: Option<String>,
    pub support_rep_id: Option<i32>,
    pub support_rep_name: Option<String>,
    pub support_rep: Option<EmployeeApiModel>,
}

/// An employee. `Customers` and `InverseReportsToNavigation` are always empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct EmployeeApiModel {
    pub id: i32,
    pub last_name: Option<String>,
    pub first_name: Option<String>,
    pub title: Option<String>,
    pub reports_to: Option<i32>,
    #[serde(with = "dotnet::utc_option")]
    pub birth_date: Option<DateTime<Utc>>,
    #[serde(with = "dotnet::utc_option")]
    pub hire_date: Option<DateTime<Utc>>,
    pub address: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub postal_code: Option<String>,
    pub phone: Option<String>,
    pub fax: Option<String>,
    pub email: Option<String>,
    pub customers: Vec<CustomerApiModel>,
    pub inverse_reports_to_navigation: Vec<EmployeeApiModel>,
    /// The manager's "First Last", on the by-id route only.
    pub reports_to_navigation: Option<String>,
}

/// A genre.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct GenreApiModel {
    pub id: i32,
    pub name: Option<String>,
}

/// A media type.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct MediaTypeApiModel {
    pub id: i32,
    pub name: Option<String>,
}

/// A customer's purchases, summed from `InvoiceFinalized` (ADR-0010).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct CustomerPurchasesApiModel {
    pub customer_id: i32,
    pub total_spent: Decimal,
    pub invoice_count: i32,
    #[serde(with = "dotnet::utc_option")]
    pub last_purchase_at: Option<DateTime<Utc>>,
}

/// The lowercase echo `PUT /genres/{id}` answers with (P11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GenreUpdated {
    pub id: i32,
    pub name: Option<String>,
}

/// `POST /genres`'s body. Binds `Name` or `name`, as upstream's web defaults do.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CreateGenreRequest {
    #[serde(rename = "Name", alias = "name", default)]
    pub name: Option<String>,
}

/// `PUT /genres/{id}`'s body.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct UpdateGenreRequest {
    #[serde(rename = "Name", alias = "name", default)]
    pub name: Option<String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_genre_carries_only_its_id_and_name() {
        let genre = GenreApiModel {
            id: 1,
            name: Some("Rock".to_owned()),
        };

        assert_eq!(
            serde_json::to_string(&genre).unwrap(),
            r#"{"Id":1,"Name":"Rock"}"#
        );
    }

    #[test]
    fn the_put_echo_is_lowercase() {
        let echo = GenreUpdated {
            id: 1,
            name: Some("Rock".to_owned()),
        };

        assert_eq!(
            serde_json::to_string(&echo).unwrap(),
            r#"{"id":1,"name":"Rock"}"#
        );
    }

    #[test]
    fn the_request_binds_either_casing() {
        let upper: CreateGenreRequest = serde_json::from_str(r#"{"Name":"Jazz"}"#).unwrap();
        let lower: CreateGenreRequest = serde_json::from_str(r#"{"name":"Jazz"}"#).unwrap();
        let missing: CreateGenreRequest = serde_json::from_str("{}").unwrap();

        assert_eq!(upper.name.as_deref(), Some("Jazz"));
        assert_eq!(lower.name.as_deref(), Some("Jazz"));
        assert_eq!(missing.name, None);
    }

    #[test]
    fn a_customer_no_longer_lists_invoices() {
        let json = serde_json::to_value(CustomerApiModel::default()).unwrap();

        assert!(json.get("Invoices").is_none());
        assert!(json.get("SupportRep").is_some());
    }

    #[test]
    fn employee_dates_render_as_dotnet_writes_them() {
        let employee = EmployeeApiModel {
            birth_date: Some(
                DateTime::parse_from_rfc3339("1962-02-18T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            ..EmployeeApiModel::default()
        };

        let json = serde_json::to_value(employee).unwrap();
        assert_eq!(json["BirthDate"], "1962-02-18T00:00:00Z");
        assert_eq!(json["HireDate"], serde_json::Value::Null);
        assert_eq!(json["Customers"], serde_json::json!([]));
    }
}
