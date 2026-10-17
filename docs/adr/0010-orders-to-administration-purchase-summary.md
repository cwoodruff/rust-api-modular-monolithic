Ports upstream ADR-0010.

# 0010. Administration summarizes customer purchases from Orders' InvoiceFinalized events

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 3; port step 4 (see `docs/upgrade-plan.md`, section 5)

## Context

Administration manages customers and wants each customer's purchase totals.
Invoices are Orders' data and reference customers by id only (ADR-0004).

## Decision

- Administration keeps `administration."CustomerPurchaseSummary"`
  (`CustomerId`, `TotalSpent`, `InvoiceCount`, `LastPurchaseAt`). Its
  `InvoiceFinalized` subscription adds the invoice total, increments the
  count and keeps the latest invoice date, through `inbox_guard::run` on
  Administration's pool. Events without a customer id are ignored.
- `GET /api/admin/customers/{id}/purchases` (`role.admin`,
  `administration.read`, `tenant.scoped`) returns it, or zeros for a
  customer with no counted purchases, and 404 for an unknown customer. It is
  not cached.

## Consistency

Outbox, eventual, as in ADR-0009.

What a client or UI must not promise:

- That a just-finalized invoice is already in the summary.
- That `TotalSpent` or `InvoiceCount` cover the customer's whole history.
  Only invoices finalized through the endpoint are counted; seeded invoices
  are not, and there is no backfill.
- That the summary matches `GET /api/orders/invoices/customer/{id}` at any
  given moment. Orders is the source of truth for invoices; the summary is a
  projection that catches up.

## Consequences

- Administration can show purchase totals without reading Orders.
- A failure in this handler is retried and eventually dead-lettered without
  holding back Catalog's track sales.
- Totals are not recalculated if an invoice's lines change after
  finalization; nothing allows that today.

## How to reverse

Drop the summary table, its migration and the subscription; purchase totals
would then come from a read contract on Orders.
