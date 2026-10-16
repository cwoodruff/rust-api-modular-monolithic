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

- `shared-kernel` has a budget for its public surface: the number of `pub`
  items reachable from the crate root (types, traits, functions, constants
  and re-exports, counted by the test from the crate's sources). The fence
  test in `architecture-tests` (`fences.rs`) enforces it, and the number is
  set in that test: the number in `fences.rs`. It is set from the count once
  steps 2 to 4 have landed, with the same small headroom the original gave
  itself. Raising it means editing the test, which a reviewer sees in the
  diff. The original's number (30 public types) is not copied, because a
  Rust crate's public items do not correspond one to one with .NET public
  types.
- `shared-kernel` depends on no module crate, no contracts crate and no
  validation library (`fences.rs`, checked against `cargo metadata`).
  Validation is the `RequestValidator<T>` trait; modules implement it
  themselves.
- **sqlx exception.** The original confined Npgsql to one namespace,
  `SharedKernel.Persistence`. Here `sqlx` with the `postgres` feature is
  allowed in `shared_kernel::persistence` (pool construction, per-schema
  migrations, the data health probe) and in `shared_kernel::events`, because
  `EventPublisher::publish` and `inbox_guard::run` take the caller's
  `sqlx::Transaction<'_, Postgres>`. The original's publisher takes a
  `DbContext`, an EF type rather than an Npgsql one, so it did not need the
  second module. No other kernel module names a `sqlx` item; `fences.rs`
  checks the sources. The outbox and inbox SQL (`FOR UPDATE SKIP LOCKED`,
  `ON CONFLICT DO NOTHING`) is PostgreSQL dialect either way.
- Contracts crates (`identity-contracts`, `orders-contracts`,
  `catalog-contracts`, `administration-contracts`) contain only traits,
  enums, plain structs with derived impls, and `pub const` items. They have
  no hand-written `impl` blocks with methods and depend on nothing but
  `serde` and the value crates their fields use (`fences.rs`).
- Anything a module registers with `Services` that other modules also
  register (today the event `Subscription`) carries the module's key, its
  schema name. A dispatcher runs only the subscriptions under its own key,
  nothing looks them up without a key, and one module's key yields only that
  module's handlers (guardrail test in `architecture-tests`).

## Consistency

Not a boundary decision.

## Consequences

- Growth of the kernel is visible and deliberate.
- The headroom in `fences.rs` is all that fits before the next conversation.
- The sqlx exception covers two kernel modules, not one, and has to be
  defended in review if it grows beyond them. Changing the database engine
  touches both.
- The count is of source-level `pub` items, so a `pub use` re-export counts
  as an item. That is deliberate: a re-export is surface a module can depend
  on.
- The original's amendments (deleting unused caching option classes,
  flattening `RateLimitPolicyRegistry.Names`, adding `ModuleJson`, marking
  assemblies `IsAotCompatible`) describe .NET type shuffles with no Rust
  counterpart. The AOT concern is met by construction: event decoding and
  JSON go through typed functions and `serde` derives, not reflection.

## How to reverse

Delete the budget and dependency tests in `fences.rs`. To remove the sqlx
exception instead, move `shared_kernel::persistence` and the transactional
parts of `shared_kernel::events` into a separate crate depended on by the
modules and the host.
