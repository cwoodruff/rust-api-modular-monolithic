Ports upstream ADR-0009.

# 0009. Catalog counts track sales from Orders' InvoiceFinalized events

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 3; port step 4 (see `docs/upgrade-plan.md`, section 5)

## Context

Catalog wants to show how often a track has sold. The sales are Orders'
data: invoice lines, which reference tracks by id only (ADR-0005). Catalog
may not read Orders' tables, and Orders may not write Catalog's.

## Decision

- Orders publishes `InvoiceFinalized` (invoice id, customer id, invoice date,
  total, and each line's track id, quantity and unit price) from
  `POST /api/orders/invoices/{id}/finalize` (ADR-0008). The event type lives
  in `orders-contracts`.
- Catalog keeps `catalog."TrackSales"` (`TrackId`, `TimesSold`,
  `LastSoldAt`). Its subscription adds each line's quantity to the track's
  `TimesSold` and keeps the latest invoice date in `LastSoldAt`, through
  `inbox_guard::run` on Catalog's pool.
- `GET /api/catalog/tracks/{id}/sales` returns it, or zeros for a track with
  no counted sales, and 404 for an unknown track. It is not cached.

## Consistency

Outbox, eventual. `TimesSold` lags finalization by at least one dispatcher
poll, and by much longer if Catalog's handler is failing (retries, then a
dead letter).

What a client or UI must not promise:

- That a just-finalized invoice is already counted. The finalize endpoint
  returns 202 with `salesCountersUpdate: "eventual"` for this reason (and 409
  when the invoice is already final).
- That `TimesSold` equals the number of invoice lines for the track. It
  counts only invoices finalized through the endpoint; the seed's 458
  invoices (2,662 lines) start as Draft and are not counted until each is
  finalized. There is no backfill.
- That a deleted or unknown track id in an event is rejected. The handler
  counts whatever track id the event carries.

## Consequences

- Catalog can show sales without reading Orders, and keeps working while
  Orders is down.
- A Catalog handler failure does not affect Administration's handling of
  the same event, and vice versa (inbox per handler).
- If invoices could be un-finalized or refunded, a compensating event would
  be needed; none exists today.

## How to reverse

Drop `TrackSales`, its migration and the subscription; a sales figure would
then need a read contract on Orders (a query, synchronous and coupled to
Orders' uptime) instead.
