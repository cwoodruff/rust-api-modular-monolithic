//! Reporting's SQL, ported from `ReportingSql`.
//!
//! The view definitions are kept here as well as in the migration so the
//! rename-detection test can read what the views select without parsing a
//! migration file. They must stay byte-for-byte the migration's: a change to
//! a view is a new `V2` constant and a new migration, never an edit of `V1`.

/// `reporting.sales_by_genre`, version 1.
#[allow(dead_code)]
pub(crate) const CREATE_SALES_BY_GENRE_V1: &str = r#"CREATE VIEW reporting.sales_by_genre AS
SELECT g."Id" AS "GenreId",
       g."Name" AS "GenreName",
       COUNT(DISTINCT t."Id") AS "TrackCount",
       COALESCE(SUM(ts."TimesSold"), 0)::bigint AS "UnitsSold"
FROM administration."Genre" g
LEFT JOIN catalog."Track" t ON t."GenreId" = g."Id"
LEFT JOIN catalog."TrackSales" ts ON ts."TrackId" = t."Id"
GROUP BY g."Id", g."Name";"#;

/// `reporting.invoice_lines_with_names`, version 1.
#[allow(dead_code)]
pub(crate) const CREATE_INVOICE_LINES_WITH_NAMES_V1: &str = r#"CREATE VIEW reporting.invoice_lines_with_names AS
SELECT il."Id" AS "InvoiceLineId",
       il."InvoiceId" AS "InvoiceId",
       il."TrackId" AS "TrackId",
       t."Name" AS "TrackName",
       i."CustomerId" AS "CustomerId",
       c."FirstName" || ' ' || c."LastName" AS "CustomerName",
       il."UnitPrice" AS "UnitPrice",
       il."Quantity" AS "Quantity"
FROM orders."InvoiceLine" il
JOIN orders."Invoice" i ON i."Id" = il."InvoiceId"
LEFT JOIN catalog."Track" t ON t."Id" = il."TrackId"
LEFT JOIN administration."Customer" c ON c."Id" = i."CustomerId";"#;

/// One orphan check: a cross-module reference whose foreign key was removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OrphanCheck {
    pub name: &'static str,
    pub source_schema: &'static str,
    pub source_table: &'static str,
    pub reference_column: &'static str,
    pub target_schema: &'static str,
    pub target_table: &'static str,
}

/// One check per foreign key phase 2 removed, in upstream's order (ADR-0015).
pub(crate) const ORPHAN_CHECKS: [OrphanCheck; 4] = [
    OrphanCheck {
        name: "invoice-line-track",
        source_schema: "orders",
        source_table: "InvoiceLine",
        reference_column: "TrackId",
        target_schema: "catalog",
        target_table: "Track",
    },
    OrphanCheck {
        name: "invoice-customer",
        source_schema: "orders",
        source_table: "Invoice",
        reference_column: "CustomerId",
        target_schema: "administration",
        target_table: "Customer",
    },
    OrphanCheck {
        name: "track-genre",
        source_schema: "catalog",
        source_table: "Track",
        reference_column: "GenreId",
        target_schema: "administration",
        target_table: "Genre",
    },
    OrphanCheck {
        name: "track-media-type",
        source_schema: "catalog",
        source_table: "Track",
        reference_column: "MediaTypeId",
        target_schema: "administration",
        target_table: "MediaType",
    },
];

/// Every `(schema, table, column)` Reporting reads from another module.
///
/// Port of `ReportingSql.References` plus `OrphanCheckReferences`: the list a
/// test checks against the owners' migrated tables, so a rename in Catalog,
/// Orders or Administration fails in Reporting's tests rather than at 2 a.m.
#[allow(dead_code)]
pub(crate) const REFERENCES: &[(&str, &str, &str)] = &[
    // sales_by_genre
    ("administration", "Genre", "Id"),
    ("administration", "Genre", "Name"),
    ("catalog", "Track", "Id"),
    ("catalog", "Track", "GenreId"),
    ("catalog", "TrackSales", "TrackId"),
    ("catalog", "TrackSales", "TimesSold"),
    // invoice_lines_with_names
    ("orders", "InvoiceLine", "Id"),
    ("orders", "InvoiceLine", "InvoiceId"),
    ("orders", "InvoiceLine", "TrackId"),
    ("orders", "InvoiceLine", "UnitPrice"),
    ("orders", "InvoiceLine", "Quantity"),
    ("orders", "Invoice", "Id"),
    ("orders", "Invoice", "CustomerId"),
    ("catalog", "Track", "Name"),
    ("administration", "Customer", "Id"),
    ("administration", "Customer", "FirstName"),
    ("administration", "Customer", "LastName"),
    // orphan checks
    ("orders", "Invoice", "Id"),
    ("catalog", "Track", "MediaTypeId"),
    ("administration", "MediaType", "Id"),
];

impl OrphanCheck {
    /// `orders."InvoiceLine"`.
    fn source(&self) -> String {
        format!(r#"{}."{}""#, self.source_schema, self.source_table)
    }

    /// `catalog."Track"`.
    fn target(&self) -> String {
        format!(r#"{}."{}""#, self.target_schema, self.target_table)
    }

    /// `s."TrackId" IS NOT NULL AND NOT EXISTS (… t."Id" = s."TrackId")`.
    fn orphan(&self) -> String {
        let reference = format!(r#"s."{}""#, self.reference_column);
        format!(
            "{reference} IS NOT NULL AND NOT EXISTS (SELECT 1 FROM {} t WHERE t.\"Id\" = {reference})",
            self.target()
        )
    }

    /// Records every orphan not already open. Parameters: `$1` name, `$2`
    /// source schema, `$3` source table, `$4` the missing-reference prefix,
    /// `$5` now.
    pub(crate) fn detect_sql(&self) -> String {
        format!(
            r#"INSERT INTO reporting."IntegrityFinding" ("Id", "CheckName", "SourceSchema", "SourceTable", "SourceId", "MissingReference", "DetectedAt") SELECT gen_random_uuid(), $1, $2, $3, s."Id", $4 || s."{reference}"::text, $5 FROM {source} s WHERE {orphan} ON CONFLICT ("CheckName", "SourceId") WHERE "ResolvedAt" IS NULL DO NOTHING"#,
            reference = self.reference_column,
            source = self.source(),
            orphan = self.orphan(),
        )
    }

    /// Resolves open findings whose orphan is gone. Parameters: `$1` name, `$2` now.
    pub(crate) fn resolve_sql(&self) -> String {
        format!(
            r#"UPDATE reporting."IntegrityFinding" f SET "ResolvedAt" = $2 WHERE f."ResolvedAt" IS NULL AND f."CheckName" = $1 AND NOT EXISTS (SELECT 1 FROM {source} s WHERE s."Id" = f."SourceId" AND {orphan})"#,
            source = self.source(),
            orphan = self.orphan(),
        )
    }

    /// `catalog.Track ` — with the trailing space, so a finding reads
    /// `catalog.Track 42`.
    pub(crate) fn missing_reference_prefix(&self) -> String {
        format!("{}.{} ", self.target_schema, self.target_table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_detect_statement_is_upstreams() {
        assert_eq!(
            ORPHAN_CHECKS[0].detect_sql(),
            r#"INSERT INTO reporting."IntegrityFinding" ("Id", "CheckName", "SourceSchema", "SourceTable", "SourceId", "MissingReference", "DetectedAt") SELECT gen_random_uuid(), $1, $2, $3, s."Id", $4 || s."TrackId"::text, $5 FROM orders."InvoiceLine" s WHERE s."TrackId" IS NOT NULL AND NOT EXISTS (SELECT 1 FROM catalog."Track" t WHERE t."Id" = s."TrackId") ON CONFLICT ("CheckName", "SourceId") WHERE "ResolvedAt" IS NULL DO NOTHING"#
        );
        assert_eq!(
            ORPHAN_CHECKS[0].missing_reference_prefix(),
            "catalog.Track "
        );
    }

    #[test]
    fn the_views_are_the_migrations() {
        let migration = include_str!("../migrations/20261001185335_initial_schema.sql");

        assert!(migration.contains(CREATE_SALES_BY_GENRE_V1));
        assert!(migration.contains(CREATE_INVOICE_LINES_WITH_NAMES_V1));
    }

    #[test]
    fn every_orphan_check_reference_is_listed() {
        for check in ORPHAN_CHECKS {
            for (schema, table, column) in [
                (check.source_schema, check.source_table, "Id"),
                (
                    check.source_schema,
                    check.source_table,
                    check.reference_column,
                ),
                (check.target_schema, check.target_table, "Id"),
            ] {
                assert!(
                    REFERENCES.contains(&(schema, table, column)),
                    "{schema}.{table}.{column} is missing from REFERENCES"
                );
            }
        }
    }
}
