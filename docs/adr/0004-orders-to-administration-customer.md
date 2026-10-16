Ports upstream ADR-0004.

# 0004. Orders refers to Administration's Customer by id only

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 2; port step 3 (see `docs/upgrade-plan.md`, section 5)

## Context

`orders."Invoice"."CustomerId"` points at `administration."Customer"`. Before
step 3, `GET /api/orders/invoices/{id}` embedded the customer (with the
support rep's name) through a joined query in `shared-data-sqlite`, and `GET
/api/admin/customers/{id}` embedded the customer's invoices the same way. The
original enforced the reference with the foreign key
`FK_Invoice_Customer_CustomerId`.

## Decision

- No foreign key between the two tables. Orders' migration creates
  `CustomerId` as an indexed column only.
- Invoice responses carry `CustomerId` only; the `Customer` member is gone.
  Customer responses no longer list invoices; `GET
  /api/orders/invoices/customer/{id}` already serves that.
- If an invoice response needs customer details again, Orders asks
  Administration through a read contract in `administration-contracts`, not
  a join.

## Consistency

Application-validated id, no database foreign key, eventual. Today
"validated" means Orders' invoice validator requires a non-null
`CustomerId`; nothing checks that the customer exists. A read contract can
add that check at write time.

## Consequences

- The database accepts an invoice for a customer id that does not exist, and
  accepts deleting a customer that has invoices (there is no delete endpoint
  for customers today).
- If a customer row disappears, its invoices keep a dangling `CustomerId`.
  Nothing fails at read time; the invoice simply refers to nobody.
  Reporting's orphan check reports such rows (ADR-0015).
- The invoice endpoint loses the embedded customer and support-rep name.

## How to reverse

Give one pool and one migration set both tables, which means undoing
ADR-0003 for these two modules, remove any orphans, and add a migration that
creates the foreign key. Restore the joined query and the `Customer` member
in the invoice response.
