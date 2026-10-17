Ports upstream ADR-0005.

# 0005. Orders refers to Catalog's Track by id only

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 2; port step 3 (see `docs/upgrade-plan.md`, section 5)

## Context

`orders."InvoiceLine"."TrackId"` points at `catalog."Track"`. Before step 3,
invoice responses joined in each line's `TrackName`, track responses carried
`InvoiceLines`, and the music module served `GET
/api/music/tracks/invoice/{id}` by reading invoice lines through the shared
repositories. The original enforced the reference with the foreign key
`FK_InvoiceLine_Track_TrackId`.

## Decision

- No foreign key between the two tables. Orders' migration creates `TrackId`
  as an indexed column only.
- Invoice lines carry `TrackId` only; `TrackName` and `Track` are gone from
  responses, and Track responses lose `InvoiceLines`.
- Delete `GET /api/catalog/tracks/invoice/{id}` (formerly
  `/api/music/tracks/invoice/{id}`): it read another module's data. `GET
  /api/orders/invoice-lines/invoice/{id}` serves invoice lines.
- If names are needed, Orders gets a track read contract in
  `catalog-contracts`. Track sales travel as events, not joins (ADR-0009).

## Consistency

Application-validated id, no database foreign key, eventual. Today Orders'
invoice-line validator requires a non-null `TrackId`; nothing checks that the
track exists.

## Consequences

- An invoice line can name a track id that does not exist, and a track that
  has been sold can be deleted (no track delete endpoint exists today).
- A removed track leaves invoice lines with a dangling `TrackId`; the line's
  price and quantity are still correct because they are stored on the line.
  Reporting's orphan check reports such rows (ADR-0015).
- Clients that read `TrackName` from invoice lines, or called the
  track-by-invoice route, must change.

## How to reverse

Give one pool and migration set both tables (undoing ADR-0003 for Orders and
Catalog), clean up orphans, add a migration that creates the foreign key, and
restore the endpoint and the joined members.
