Ports upstream ADR-0012.

# 0012. Give the shared kernel a budget, and fence its dependencies

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 4; port step 7 (see `docs/upgrade-plan.md`, section 5)

## Context

Step 3 takes the domain out of the shared crates. Steps 2 and 4 add the
module contract, bulkheads, and event and outbox plumbing to `shared-kernel`.
A shared kernel grows quietly: each addition looks reasonable, and nobody
sees the total. Contracts crates have the same failure mode: a "helper" with
logic turns a contract into a shared library.

## Decision

- `shared-kernel` has a budget for its public types: every `pub struct`,
  `pub enum`, `pub trait` and `pub type` in its sources outside test
  modules. The fence test in `architecture-tests` (`fences.rs`,
  `KERNEL_PUBLIC_TYPE_BUDGET`) enforces it. The number was set when the
  upgrade landed: the count then, 74, plus two, the same headroom the
  original gave itself. Raising it means editing the test, which a reviewer
  sees in the diff. The original's 30 is not copied: a Rust crate exposes
  error enums, option structs and trait-object aliases as types where .NET
  would use exceptions, records and generics.
- `shared-kernel` depends on no other workspace crate: no module, no
  contracts crate, and not `validation-rules`, the FluentValidation
  counterpart modules use (`module_boundaries.rs`, checked against `cargo
  metadata`). Validation reaches the kernel only as the `RequestValidator<T>`
  trait.
- **The driver stays in one file.** The original confined Npgsql to
  `SharedKernel.Persistence`. Here the PostgreSQL driver's own names
  (`sqlx::postgres`, `PgPool`, `PgConnection`, `PgRow`, `Postgres`) appear in
  `shared_kernel/src/persistence.rs` and nowhere else in the kernel; that
  file exports the aliases `DbPool`, `DbConnection`, `DbTransaction` and
  `DbRow`, and the outbox, inbox and health checks use those. `fences.rs`
  scans the sources. Generic sqlx items (`sqlx::query`, `sqlx::Row`) are
  used in `events` too; the outbox and inbox SQL (`FOR UPDATE SKIP LOCKED`,
  `ON CONFLICT DO NOTHING`) is PostgreSQL dialect either way.
- Contracts crates (`identity-contracts`, `orders-contracts`,
  `catalog-contracts`, `administration-contracts`) contain constants, data
  types with derived impls, and the `IntegrationEvent` accessors an event
  needs. Any other function, and any `static`, fails `fences.rs`; so does a
  `pub(crate)` item, the counterpart of upstream's "no InternalsVisibleTo".
  They depend on nothing in the workspace but `shared-kernel`.
- Event subscriptions are registered under the module that owns the handler
  (`Services::add_integration_event_handler(module_key, handler)`). A
  dispatcher hands an event to every subscription for its type and runs each
  handler in the inbox of that subscription's own module; a module's key
  yields only that module's handlers, and no subscription is unkeyed
  (guardrail test in `architecture-tests`).

## Consistency

Not a boundary decision.

## Consequences

- Growth of the kernel is visible and deliberate.
- The headroom in `fences.rs` is all that fits before the next conversation.
- Changing the database engine means rewriting `persistence.rs` and the
  dialect-specific SQL in `events`; the type names in the rest of the kernel
  do not change.
- The count is of declared types; re-exports and functions are not counted,
  so the budget limits how many concepts the kernel defines rather than how
  many entry points it has.
- The original's amendments (deleting unused caching option classes,
  flattening `RateLimitPolicyRegistry.Names`, adding `ModuleJson`, marking
  assemblies `IsAotCompatible`) describe .NET type shuffles with no Rust
  counterpart. The AOT concern is met by construction: event decoding and
  JSON go through typed functions and `serde` derives, not reflection.

## How to reverse

Delete the budget and driver tests in `fences.rs` and the kernel rows in
`module_boundaries.rs`. To take the driver out of the kernel entirely, move
`persistence` and `events` into a separate crate depended on by the modules
and the host.
