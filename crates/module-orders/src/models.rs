//! Orders' wire shapes, ported from `Orders.Module/Models`.
//!
//! PascalCase, nulls written, collections empty rather than absent (P1). The
//! members upstream removed when Orders stopped joining into Administration's
//! and Catalog's tables are gone here too: an invoice no longer embeds its
//! `Customer`, and a line no longer carries `TrackName` or `Track`
//! (ADR-0004, ADR-0005). An invoice gains `Status`.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use shared_kernel::json::dotnet;
use uuid::Uuid;

/// An invoice, with its lines on the by-id route.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct InvoiceApiModel {
    pub id: i32,
    pub customer_id: Option<i32>,
    #[serde(with = "dotnet::utc")]
    pub invoice_date: DateTime<Utc>,
    pub billing_address: Option<String>,
    pub billing_city: Option<String>,
    pub billing_state: Option<String>,
    pub billing_country: Option<String>,
    pub billing_postal_code: Option<String>,
    pub total: Decimal,
    /// `Draft` or `Finalized`.
    pub status: String,
    pub invoice_lines: Vec<InvoiceLineApiModel>,
}

impl Default for InvoiceApiModel {
    fn default() -> Self {
        Self {
            id: 0,
            customer_id: None,
            invoice_date: DateTime::UNIX_EPOCH,
            billing_address: None,
            billing_city: None,
            billing_state: None,
            billing_country: None,
            billing_postal_code: None,
            total: Decimal::ZERO,
            status: InvoiceStatus::Draft.as_str().to_owned(),
            invoice_lines: Vec::new(),
        }
    }
}

/// One line of an invoice. `Invoice` is always null.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct InvoiceLineApiModel {
    pub id: i32,
    pub invoice_id: Option<i32>,
    pub track_id: Option<i32>,
    pub unit_price: Option<Decimal>,
    pub quantity: Option<i32>,
    pub invoice: Option<InvoiceApiModel>,
}

/// Port of `InvoiceStatus`, stored as its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InvoiceStatus {
    Draft,
    Finalized,
}

impl InvoiceStatus {
    /// The stored and serialized name.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "Draft",
            Self::Finalized => "Finalized",
        }
    }
}

/// The finalize endpoint's 202 body. Explicitly camelCase upstream, unlike
/// every other Orders model (P20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InvoiceFinalizeAccepted {
    pub invoice_id: i32,
    pub status: String,
    pub sales_counters_update: String,
}

/// A dead-lettered outbox message, as `GET /outbox/dead-letters` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct DeadLetter {
    pub id: Uuid,
    pub event_type: String,
    #[serde(with = "dotnet::offset")]
    pub occurred_at: DateTime<Utc>,
    pub attempts: i32,
    pub last_error: Option<String>,
    #[serde(with = "dotnet::offset")]
    pub dead_lettered_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn an_invoice_carries_exactly_upstreams_members_in_order() {
        // Compared parsed: a decimal serializes as a JSON number, and `0` and
        // `0.0` are the same number to every client.
        let json = serde_json::to_value(InvoiceApiModel::default()).unwrap();
        let expected: serde_json::Value = serde_json::from_str(
            r#"{"Id":0,"CustomerId":null,"InvoiceDate":"1970-01-01T00:00:00Z","BillingAddress":null,"BillingCity":null,"BillingState":null,"BillingCountry":null,"BillingPostalCode":null,"Total":0.0,"Status":"Draft","InvoiceLines":[]}"#,
        )
        .unwrap();

        assert_eq!(json, expected);
        // Member order, read off the text since parsed maps sort their keys.
        let text = serde_json::to_string(&InvoiceApiModel::default()).unwrap();
        let order = [
            "\"Id\"",
            "\"CustomerId\"",
            "\"InvoiceDate\"",
            "\"BillingAddress\"",
            "\"BillingCity\"",
            "\"BillingState\"",
            "\"BillingCountry\"",
            "\"BillingPostalCode\"",
            "\"Total\"",
            "\"Status\"",
            "\"InvoiceLines\"",
        ];
        let positions: Vec<usize> = order.iter().map(|key| text.find(key).unwrap()).collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    }

    #[test]
    fn a_line_carries_a_null_invoice_and_nothing_from_catalog() {
        let json = serde_json::to_string(&InvoiceLineApiModel::default()).unwrap();

        assert_eq!(
            json,
            r#"{"Id":0,"InvoiceId":null,"TrackId":null,"UnitPrice":null,"Quantity":null,"Invoice":null}"#
        );
    }

    #[test]
    fn the_finalize_body_is_camel_case() {
        let body = InvoiceFinalizeAccepted {
            invoice_id: 1,
            status: "Finalized".to_owned(),
            sales_counters_update: "eventual".to_owned(),
        };

        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"invoiceId":1,"status":"Finalized","salesCountersUpdate":"eventual"}"#
        );
    }

    #[test]
    fn a_dead_letter_writes_its_times_as_offsets() {
        let letter = DeadLetter {
            id: Uuid::nil(),
            event_type: "Orders.Contracts.Events.InvoiceFinalized".to_owned(),
            occurred_at: DateTime::UNIX_EPOCH,
            attempts: 6,
            last_error: None,
            dead_lettered_at: DateTime::UNIX_EPOCH,
        };

        let json = serde_json::to_value(&letter).unwrap();
        assert_eq!(json["OccurredAt"], "1970-01-01T00:00:00+00:00");
        assert_eq!(json["DeadLetteredAt"], "1970-01-01T00:00:00+00:00");
    }
}
