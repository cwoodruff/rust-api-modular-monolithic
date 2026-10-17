//! Reporting's wire shapes, ported from `Reporting.Module/Domain`. PascalCase,
//! nulls written (P1).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use shared_kernel::json::dotnet;
use sqlx::FromRow;
use uuid::Uuid;

/// One row of `reporting.sales_by_genre`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct SalesByGenreRow {
    #[sqlx(rename = "GenreId")]
    pub genre_id: i32,
    #[sqlx(rename = "GenreName")]
    pub genre_name: Option<String>,
    #[sqlx(rename = "TrackCount")]
    pub track_count: i64,
    #[sqlx(rename = "UnitsSold")]
    pub units_sold: i64,
}

/// One row of `reporting.invoice_lines_with_names`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct InvoiceLineWithNamesRow {
    #[sqlx(rename = "InvoiceLineId")]
    pub invoice_line_id: i32,
    #[sqlx(rename = "InvoiceId")]
    pub invoice_id: i32,
    #[sqlx(rename = "TrackId")]
    pub track_id: Option<i32>,
    #[sqlx(rename = "TrackName")]
    pub track_name: Option<String>,
    #[sqlx(rename = "CustomerId")]
    pub customer_id: Option<i32>,
    #[sqlx(rename = "CustomerName")]
    pub customer_name: Option<String>,
    #[sqlx(rename = "UnitPrice")]
    pub unit_price: Option<Decimal>,
    #[sqlx(rename = "Quantity")]
    pub quantity: Option<i32>,
}

/// A cross-module reference that points at nothing (ADR-0015).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct IntegrityFinding {
    #[sqlx(rename = "Id")]
    pub id: Uuid,
    #[sqlx(rename = "CheckName")]
    pub check_name: String,
    #[sqlx(rename = "SourceSchema")]
    pub source_schema: String,
    #[sqlx(rename = "SourceTable")]
    pub source_table: String,
    #[sqlx(rename = "SourceId")]
    pub source_id: i32,
    #[sqlx(rename = "MissingReference")]
    pub missing_reference: String,
    #[sqlx(rename = "DetectedAt")]
    #[serde(with = "dotnet::offset")]
    pub detected_at: DateTime<Utc>,
    #[sqlx(rename = "ResolvedAt")]
    #[serde(with = "dotnet::offset_option")]
    pub resolved_at: Option<DateTime<Utc>>,
}

/// What one integrity run found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct IntegrityRunResult {
    pub detected: i64,
    pub resolved: i64,
    pub open: i64,
    #[serde(with = "dotnet::offset")]
    pub ran_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_run_result_reads_as_upstreams_record() {
        let result = IntegrityRunResult {
            detected: 1,
            resolved: 0,
            open: 1,
            ran_at: DateTime::UNIX_EPOCH,
        };

        assert_eq!(
            serde_json::to_string(&result).unwrap(),
            r#"{"Detected":1,"Resolved":0,"Open":1,"RanAt":"1970-01-01T00:00:00+00:00"}"#
        );
    }
}
