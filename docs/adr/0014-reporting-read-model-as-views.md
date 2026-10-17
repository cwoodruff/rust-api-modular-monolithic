Ports upstream ADR-0014.

# 0014. Reporting reads across modules through views, as a read-only role

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 6; port step 5 (see `docs/upgrade-plan.md`, section 5)

## Context

Reporting exists to answer questions that span modules: sales per genre
(Catalog's tracks and sales, Administration's genres), invoice lines with
track and customer names (Orders, Catalog, Administration). Every other
cross-module need in this repository goes through events (ADR-0008) or ids
(ADR-0004 to ADR-0007). Doing that for reporting would mean Reporting keeping
copies of most of the other modules' data in sync by events: a lot of code
for a module that only reads.

## Decision

- Reporting owns schema `reporting`, with its own migrations in
  `module-reporting` (`sqlx::migrate!`) and its own `_sqlx_migrations`
  history table, migrated last (after administration, catalog and orders).
- Its read model is two PostgreSQL views in that schema, created by raw SQL
  in its migrations and read with `sqlx::query_as` into row structs:
  - `reporting.sales_by_genre`: genre, track count, units sold
    (`administration."Genre"`, `catalog."Track"`, `catalog."TrackSales"`);
  - `reporting.invoice_lines_with_names`: invoice lines with track and
    customer names (`orders."InvoiceLine"`, `orders."Invoice"`,
    `catalog."Track"`, `administration."Customer"`).
- **This is coupling, on purpose.** Reporting depends on other modules'
  table and column names: it reads their stores directly instead of each
  module exporting reporting data. It is acceptable here because:
  - it is read-only, enforced by the database, not by convention:
    Reporting's request-time pool connects as the `reporting_reader` role,
    which has `SELECT` on the three schemas and write access only to
    `reporting."IntegrityFinding"`. The migration creates the role and the
    grants; logins in it are provided per environment
    (`ConnectionStrings:Reporting`; the compose init script creates
    `reporting` locally);
  - it is one-directional: no module depends on `module-reporting`, and
    Reporting has no contracts crate;
  - a rename breaks Reporting's tests first:
    `module-tests/tests/reporting.rs` reads the view SQL out of Reporting's
    migration (each `FROM`/`JOIN schema."Table" alias` and `alias."Column"`,
    as upstream's `ViewColumnReader` does) and checks every column against
    `information_schema` on a database migrated by the owning modules'
    current migrations. The original checks against each module's EF model;
    the migrated schema is the nearest Rust equivalent. `module-reporting`
    also keeps a `REFERENCES` list of every column the views and orphan
    checks read, with a unit test that the orphan checks' columns are in it;
  - PostgreSQL also refuses to drop or retype a column a view uses, so a
    breaking change in another module fails its migration loudly rather
    than corrupting reports.
- Migrations run as the schema owner (`ConnectionStrings:AppDatabase`),
  never as the reader. The view SQL is versioned by name
  (`CREATE_SALES_BY_GENRE_V1` and `CREATE_INVOICE_LINES_WITH_NAMES_V1` in
  `module-reporting/src/sql.rs`, which a unit test checks appear verbatim in
  the migration); changing a view means adding a V2 and a new migration that
  replaces it, never editing V1.
- Report queries scan other modules' tables, so each runs inside Reporting's
  `ModuleGate` (default 4 at a time, ADR-0013) and its rate-limit policy.

## Consistency

Read only. Views read the owners' tables at query time, so they are as
current as the data they read: `invoice_lines_with_names` is immediately
consistent; `sales_by_genre` is as eventually consistent as `TrackSales`
(ADR-0009).

## Consequences

- Reporting answers cross-module questions without any module exporting
  data for it, and without copies to keep in sync.
- Other modules cannot freely rename or drop the columns listed in
  `REFERENCES`; the rename-detection test says which.
- Database-level coupling makes extracting Catalog, Orders or Administration
  to their own database harder: Reporting would then need replicated data or
  events. The extraction playbook starts from this record.
- Creating the role needs `CREATEROLE` for whoever runs migrations.
- `GET /api/reporting/invoices/{id}/lines` answers 404 for an invoice with
  no lines, not only for a missing one, as the original does.

## How to reverse

Replace the views with tables Reporting fills from integration events
(ADR-0008), drop the role's `SELECT` grants on the other schemas, and delete
the `REFERENCES` table and its test.
