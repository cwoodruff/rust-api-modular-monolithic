Ports upstream ADR-0018.

# 0018. Module map and entity ownership after the upgrade

- Status: Accepted
- Date: 2026-10-16
- Phase: after upstream phase 9; port step 8 (see `docs/upgrade-plan.md`, section 5)
- Supersedes: [0001](0001-module-map-and-ownership.md)

## Context

ADR-0001 records who owns which table before any code moved, and calls itself
the one table to check ownership against. Steps 3 to 5 give every module its
own pool, schema and migrations and add tables (ADR-0008 to ADR-0010,
ADR-0014), each recorded in its own ADR. ADR-0001's table does not list them,
and its context (`shared-persistence`, `shared-data-sqlite`, one `AppState`)
describes code that no longer exists.

## Decision

The ownership table, as the code stands. Only the owner writes a table; the
owner's migrations create it, in the owner's schema, and only the owner's
pool queries it (`architecture-tests`, `module_boundaries.rs`).

| Module | Crate | Schema | Route group | Owns |
|---|---|---|---|---|
| Catalog | `module-catalog` | `catalog` | `/api/catalog` | Artist, Album, Track, Playlist, PlaylistTrack; TrackSales (read model fed by `InvoiceFinalized`, ADR-0009); InboxMessage (ADR-0008) |
| Orders | `module-orders` | `orders` | `/api/orders` | Invoice (with Status), InvoiceLine; OutboxMessage (ADR-0008) |
| Administration | `module-admin` | `administration` | `/api/admin` | Customer, Employee, Genre, MediaType; CustomerPurchaseSummary (read model fed by `InvoiceFinalized`, ADR-0010); InboxMessage (ADR-0008) |
| Reporting | `module-reporting` | `reporting` | `/api/reporting` | IntegrityFinding (ADR-0015); views `sales_by_genre` and `invoice_lines_with_names`, which read the other three schemas as the `reporting_reader` role (ADR-0014) |
| Identity | `module-identity` | none | `/api/identity` | No persisted entities (users, refresh tokens and key material are held in memory or in configuration) |

Each schema also holds its module's `_sqlx_migrations` history table
(ADR-0003). `OutboxMessage` and `InboxMessage` have their row types and SQL
in `shared_kernel::events`; each module creates the tables in its own schema
with its own migrations, and the rows belong to that module.

The contracts crates (`identity-contracts`, `orders-contracts`,
`catalog-contracts`, `administration-contracts`) own no tables.
`samples/catalog-host` owns the unmigrated `catalog_host` schema (ADR-0017).

Cross-module references are ids with no foreign key: `Invoice.CustomerId`
(ADR-0004), `InvoiceLine.TrackId` (ADR-0005), `Track.GenreId` (ADR-0006),
`Track.MediaTypeId` (ADR-0007).

## Consistency

Not a boundary decision. Each reference and each read model keeps the
consistency choice its own ADR records.

## Consequences

- Reviewers again have one current table to check ownership against.
- A new table needs a row here, or a superseding ADR, in the same PR.

## How to reverse

Edit this table in a superseding ADR, as ADR-0001 says.
