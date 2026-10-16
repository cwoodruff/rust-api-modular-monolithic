Ports upstream ADR-0008.

# 0008. Hand-rolled integration events through a per-module outbox

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 3; port steps 2 and 4 (see `docs/upgrade-plan.md`, section 5)

## Context

Until step 4 nothing crosses a module boundary at write time; any
cross-module effect would need a shared transaction, which step 3 removed on
purpose (ADR-0003). Orders now needs Catalog and Administration to react when
an invoice is finalized, without a shared transaction and without either
module calling the other.

## Decision

Integration events, an outbox and an inbox, written by hand in
`shared_kernel::events`, with no messaging crate.

- **Publishing.** `EventPublisher::publish(&event, &mut tx)` inserts an
  outbox row on the caller's `sqlx::Transaction<'_, Postgres>`, opened on the
  publishing module's own pool. The row commits with the business change or
  not at all. There is no in-memory publish path, so an event never exists
  for a change that rolled back.
- **Event contracts.** Event types live in the publisher's contracts crate
  (`orders-contracts`, `InvoiceFinalized`). Consumers depend on that crate and
  never on the publisher's module crate (`architecture-tests`).
- **Storage.** Each publishing module has an `"OutboxMessage"` table and each
  consuming module an `"InboxMessage"` table, in its own schema, created by
  its own migrations with the original's column names. The persisted event
  type is the original's string, `Orders.Contracts.Events.InvoiceFinalized`.
- **Dispatch.** One `OutboxDispatcher` per publishing module, a hosted task
  registered through `Services`, polls its outbox: due, undelivered, not
  dead-lettered rows, oldest first, 50 at a time, `FOR UPDATE SKIP LOCKED`
  so a second instance skips rows already taken. It decodes each row with
  the typed decode function registered for its event type (no reflection)
  and hands the event to every `Subscription` registered under its module
  key (ADR-0012).
- **Delivery guarantee: at least once.** A row is marked processed only
  when every handler has succeeded. A crash between a handler committing and
  the row being marked delivers the event again.
- **Inbox per handler.** `inbox_guard::run` runs each handler on its own
  module's pool: insert `(EventId, HandlerName)` into that module's inbox
  first (`ON CONFLICT DO NOTHING`); if the row already existed, skip;
  otherwise run the handler on the same transaction and commit its changes
  and the inbox row together. So a handler takes effect once per event,
  however often the event arrives, and a handler that succeeded is skipped
  when a sibling's failure causes a retry. Handler names are the original's
  (`Catalog.Modules.Events.InvoiceFinalizedHandler`,
  `Admin.Modules.Events.InvoiceFinalizedHandler`), so a Rust and a C# host
  sharing one database recognize each other's inbox rows.
- **Retries.** A failed delivery is retried after 1 s, 5 s, 30 s, 2 min and
  10 min, measured on the `Clock` trait so tests can advance a
  `ManualClock`. The sixth failed delivery dead-letters the row: the
  dispatcher leaves it alone, and `GET /api/orders/outbox/dead-letters`
  (admin) lists it. `POST /api/orders/outbox/dead-letters/{id}/retry` resets
  its attempts so the next poll delivers it again. Nothing deletes or replays
  dead letters automatically; a person looks at them. `LastError` is
  truncated as the original truncates it.
- **Ordering, stated narrowly.** Rows are delivered in insertion order
  (`OccurredAt`, then `Id`) within one dispatcher, and only until a row
  fails: a row waiting for a retry is delivered after rows written later.
  There is no ordering across modules or across dispatchers, and handlers
  must not depend on any. There is no sequence number or lock to enforce
  more; the handlers here are commutative counters, which do not need it.

Why no messaging crate: the original declined MediatR and MassTransit so the
transaction boundary, the delivery guarantee, the retry schedule and the
dead-letter policy are code a reader can follow in one folder. The same holds
here. An in-process channel or an actor crate would not give a transactional
outbox, and a broker client would add infrastructure the original does not
have. A few hundred lines in `shared_kernel::events` are cheaper to own than
someone else's semantics.

## Consistency

Outbox, eventual. The publisher's change and its event commit together;
consumers catch up after the next poll (one second by default). ADR-0009 and
ADR-0010 record what each consumer's readers may and may not assume.

## Consequences

- Cross-module effects survive a consumer being down or failing; they wait
  in the outbox and are retried.
- Every handler must be safe to run more than once in principle; the inbox
  makes it run once in practice, but only for changes it makes on its own
  module's transaction through the guard.
- Handlers must not commit the transaction they are given, open another, or
  touch another module's pool.
- Someone has to watch the dead-letter list. The dispatcher's counters
  (published, dispatched, retried, dead-lettered) are recorded on the module's
  `ModuleMeter` (ADR-0013).
- Outbox and inbox rows are never deleted yet; a cleanup job is future work.

## How to reverse

Replace `EventPublisher` with a direct call into the consumers inside the
publisher's transaction (which brings back a shared transaction across
schemas, and needs one pool for both modules) or with a message bus.
Handlers, contracts and inbox tables can stay as they are behind either.
