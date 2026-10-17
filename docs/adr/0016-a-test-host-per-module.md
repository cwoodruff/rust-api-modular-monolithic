Ports upstream ADR-0016.

# 0016. Test each module on a host of its own

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 8; port step 7 (see `docs/upgrade-plan.md`, section 5)

## Context

Before step 7 every endpoint test in this port builds the whole
application router: all five modules, Identity's token service, and the
whole data set. A Catalog endpoint test therefore depends on Orders,
Administration and Identity starting cleanly, and pays for them. Nothing
shows that a module can run without the others, which is the property the
module contract (ADR-0021) and extraction (ADR-0017) rely on.

## Decision

Two new crates. `test-support` (a dev-dependency only) holds the PostgreSQL
fixture and `ModuleTestHost<M: Module>`; `module-tests` holds one suite per
module, each on its own host. `ModuleTestHost` builds an in-process axum
router with:

- what `HostComposition` gives every module (problem details, JSON options,
  security headers) and the one module's `register_services` and
  `map_endpoints`;
- a database cloned from a per-module template that holds only that module's
  schema, optionally with its share of the Chinook seed. The template is
  migrated once per test process on the shared `rmm-test-postgres`
  container (or the server at `TEST_POSTGRES_URL`); each host clones it with `CREATE DATABASE ...
  TEMPLATE ... STRATEGY FILE_COPY` under a lock, so clones never race the
  template. Resetting truncates only that module's schema. Reporting is the
  exception: its views read the other three schemas, so its host migrates
  all four (ADR-0014);
- a header-based authenticator and a `PolicyEvaluator` that recognizes the
  names `identity-contracts` defines, with Identity's tenant rule, in place
  of `module-identity`. Identity's own host uses its real authentication;
- a recording wrapper around the module's `EventPublisher` (`published()`),
  and `deliver`, which runs the module's event subscriptions through its
  inbox as the publisher's dispatcher would. The outbox loop and the
  integrity job are off.

Outbox delivery (retries, backoff, dead letters, idempotence) is tested once,
in `shared-kernel`'s own tests, with a `ManualClock` and test tables. Each
module's side of `InvoiceFinalized` is tested in its own host. `crates/api`'s
tests keep what needs the whole host: the finalized-invoice flow across three
modules, the authorization pipeline, rate limits across modules, health,
security headers, error handling, endpoint layer order, OpenAPI, seed
integrity, migration order, configuration, and Identity.

## Consistency

Not a boundary decision.

## Consequences

- A module that cannot start without another module's services fails its own
  tests, not only the architecture tests.
- Endpoint tests own their database, so they assert exact statuses and seeded
  values instead of "not 401" or "404 or 500".
- The cost of a module host against the full host is measured by
  `tools/timing.sh` and reported in the README.
- The test `PolicyEvaluator` in `test-support` mirrors Identity's rules. If
  Identity changes a rule, `test-support` must change with it; the full-host
  authorization tests in `crates/api` are what catch a drift.

## How to reverse

Move the endpoint tests back into `crates/api` on the full router, delete
`module-tests`, and fold the fixture in `test-support` back into the host's
tests.
