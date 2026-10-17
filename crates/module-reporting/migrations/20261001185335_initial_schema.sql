-- Port of Reporting's InitialSchema migration (ADR-0014, ADR-0015).
--
-- Reporting owns one table, its integrity findings, and two views that read
-- the other three modules' schemas. The views are versioned: a change adds a
-- V2 in a new migration, never edits V1. The migration runs as the owner; the
-- module reads as a login in reporting_reader, which may select from the
-- three schemas and write only its own findings.

CREATE TABLE reporting."IntegrityFinding" (
    "Id" uuid NOT NULL,
    "CheckName" varchar(100) NOT NULL,
    "SourceSchema" varchar(63) NOT NULL,
    "SourceTable" varchar(63) NOT NULL,
    "SourceId" integer NOT NULL,
    "MissingReference" varchar(200) NOT NULL,
    "DetectedAt" timestamp with time zone NOT NULL,
    "ResolvedAt" timestamp with time zone NULL,
    CONSTRAINT "PK_IntegrityFinding" PRIMARY KEY ("Id")
);

CREATE UNIQUE INDEX "IX_IntegrityFinding_Open" ON reporting."IntegrityFinding" ("CheckName", "SourceId") WHERE "ResolvedAt" IS NULL;

-- ReportingSql.CreateSalesByGenreV1
CREATE VIEW reporting.sales_by_genre AS
SELECT g."Id" AS "GenreId",
       g."Name" AS "GenreName",
       COUNT(DISTINCT t."Id") AS "TrackCount",
       COALESCE(SUM(ts."TimesSold"), 0)::bigint AS "UnitsSold"
FROM administration."Genre" g
LEFT JOIN catalog."Track" t ON t."GenreId" = g."Id"
LEFT JOIN catalog."TrackSales" ts ON ts."TrackId" = t."Id"
GROUP BY g."Id", g."Name";

-- ReportingSql.CreateInvoiceLinesWithNamesV1
CREATE VIEW reporting.invoice_lines_with_names AS
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
LEFT JOIN administration."Customer" c ON c."Id" = i."CustomerId";

-- ReportingSql.GrantReaderV1
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'reporting_reader') THEN
        CREATE ROLE reporting_reader NOLOGIN;
    END IF;
END
$$;

GRANT USAGE ON SCHEMA catalog, orders, administration TO reporting_reader;
GRANT SELECT ON ALL TABLES IN SCHEMA catalog, orders, administration TO reporting_reader;
ALTER DEFAULT PRIVILEGES IN SCHEMA catalog, orders, administration GRANT SELECT ON TABLES TO reporting_reader;

GRANT USAGE ON SCHEMA reporting TO reporting_reader;
GRANT SELECT ON reporting.sales_by_genre, reporting.invoice_lines_with_names TO reporting_reader;
GRANT SELECT, INSERT, UPDATE ON reporting."IntegrityFinding" TO reporting_reader;
