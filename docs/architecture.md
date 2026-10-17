# Architecture

**Status: implemented.** This describes what is in the repository, not a plan.

A port of [aspnetcore-min-api-modular-monolithic](https://github.com/cwoodruff/aspnetcore-min-api-modular-monolithic)
as it stands after its own modular-monolith upgrade (upstream `3172d97`).
Where this document says "the original", it means that. How the port got
here from its first, SQLite-based shape is in [upgrade-plan.md](upgrade-plan.md);
the decisions are in [adr/](adr/README.md).

## The shape

One process, five modules, one PostgreSQL server with a schema per module.
Each module owns its tables, its migrations, its services and its routes, and
reads nothing outside its own schema. Requests flow **endpoint → service →
the module's own pool**; there is no repository layer.

```
                         ┌────────────────────────────────────────────┐
   request ────────────► │ api (host)                                 │
                         │  HSTS, panic → 500, status-code pages,     │
                         │  security headers, CORS, authentication    │
                         └──────┬───────┬────────┬────────┬───────┬───┘
                                │       │        │        │       │
   rate limit, metrics and      ▼       ▼        ▼        ▼       ▼
   authorization per group  ┌──────┐┌────────┐┌───────┐┌──────┐┌─────────┐
   and per endpoint ───────►│Admin ││Identity││Catalog││Orders││Reporting│
                            └──┬───┘└────────┘└───┬───┘└──┬───┘└────┬────┘
                               │                  │       │         │ reads all three
                               ▼                  ▼       ▼         ▼ through views
                      administration.*     catalog.*  orders.*  reporting.*
                      (+ InboxMessage)  (+ InboxMessage) (+ OutboxMessage)

   Orders ──InvoiceFinalized──► orders."OutboxMessage" ──dispatcher──┬─► Catalog: TrackSales
                                                                     └─► Administration: CustomerPurchaseSummary
```

Identity owns no tables. Its `/data-health` asks whether `AppDatabase`
answers at all.

## Crates, and why the boundaries hold

| Crate | Ports | Depends on (workspace) |
|---|---|---|
| `shared-kernel` | `SharedKernel` | nothing |
| `identity-contracts` | `Identity.Contracts` | nothing |
| `orders-contracts` | `Orders.Contracts` | `shared-kernel` |
| `catalog-contracts`, `administration-contracts` | the empty `*.Contracts` | nothing |
| `validation-rules` | FluentValidation, the library | nothing |
| `module-catalog` | `Catalog.Module` | kernel, `catalog-`, `orders-`, `identity-contracts` |
| `module-orders` | `Orders.Module` | kernel, `orders-`, `identity-contracts` |
| `module-admin` | `Admin.Module` | kernel, `administration-`, `orders-`, `identity-contracts`, `validation-rules` |
| `module-reporting` | `Reporting.Module` | kernel, `identity-contracts` |
| `module-identity` | `Identity.Module` | kernel, `identity-contracts` |
| `api` | `ModularMonolith.Api` | everything above |
| `catalog-host` (in `samples/`) | `samples/Catalog.Host` | `module-catalog`, kernel, `identity-` and `orders-contracts` |
| `architecture-tests`, `module-tests`, `test-support` | the three test projects | — |

The rules, each checked by `architecture-tests` against `cargo metadata`:

1. **A module never depends on another module.** Modules share contracts and
   events, never code. All 20 ordered pairs are asserted.
2. **A module depends only on the kernel, contracts crates and
   `validation-rules`.**
3. **Contracts depend on nothing but the kernel**, and hold only constants,
   data types and the accessors an integration event needs.
4. **The kernel depends on nothing in the workspace**, and names the
   PostgreSQL driver in one file, `persistence.rs`.

A forbidden edge is usually a compile error before any test runs, because
Rust visibility is structural: everything in a module crate except its
composition type and a few constants is `pub(crate)`. The tests give the
failure a readable message and catch a dependency added without thought.
`public_surface.rs` pins what each module chose to make `pub`.

## The module contract

The original's `IModule` has three members, and so does this:

```rust
pub trait Module: Send + Sync {
    fn name(&self) -> &'static str;
    fn register_services(&mut self, services: &mut Services, config: &AppConfig) -> Result<(), ModuleError>;
    fn map_endpoints(&self, endpoints: &mut Endpoints);
}
```

`Services` stands where `IServiceCollection` stands, but it is a typed
composition object rather than a container. A module builds its own services
from configuration and keeps them; what it registers with the host is only
what the host must run or report on: its database (for migration, health and
the dispatcher's inbox lookups), its cache, rate-limit policy, meter, work
queue and gate, its hosted services, its event subscriptions, and — for
Identity — the authenticator and the policy evaluator. Nothing in `Services`
hands one module another module's services, which is the Rust form of the
original's "no module service resolves to another module's type" (ADR-0021).

The host's module list is hand-written, in the original's order:
Administration, Identity, Catalog, Orders, Reporting
(`api::host_composition::get_modules`). No discovery.

## Endpoints and their metadata

A module maps its routes through `Endpoints::map_group(prefix, tag, state)`,
the counterpart of `MapGroup`, and chains conventions onto each endpoint:

```rust
group
    .map_get("/albums/{id:int}", handlers::get_album_by_id)
    .with_name("CatalogGetAlbumById")
    .require_authorization(&[permissions::CATALOG_READ, policies::TENANT_SCOPED])
    .produces(&[200, 401, 403, 404, 429]);
```

Every endpoint is also recorded as an `EndpointDescriptor`: method, route,
name, tag, policies, rate-limit policy, description, statuses. axum's router
is opaque, so the descriptors are what the host's startup check, the
per-module OpenAPI documents and the guardrail tests read.

Route constraints are kept. `{id:int}` and `{id:guid}` are stripped for axum
and enforced by a layer outside authorization, so `/albums/abc` is a 404, as
ASP.NET's routing makes it, rather than a 401 or a 400. A route written with
a trailing slash (`albums/`) answers with and without it.

Handlers are named functions over the module's state, so the unit tests in
each module crate call them with fake services.

## Authorization

An endpoint lists policy **names**, constants from `identity-contracts`. The
`PolicyEvaluator` Identity registers decides each one at request time; all
must pass. A caller with no credentials is challenged (401) before any policy
runs; one who fails a policy is forbidden (403); a name nobody registered is
the server's fault (500). In Development and Test the host refuses to start
if any endpoint names an unregistered policy or two endpoints share a route
and method (`api::module_composition`). See [authn-authz.md](authn-authz.md).

## The middleware pipeline

Outermost first, as the original's `Program.cs` reads:

1. HSTS — outside Development
2. a panic becomes the RFC 7807 500, where `UseExceptionHandler` sits
3. status-code pages: a bodiless 4xx or 5xx gets a problem document
4. `charset=utf-8` on JSON responses
5. the six security headers, on every response including error responses
6. CORS
7. authentication: the caller, if any, goes into the request
8. routing, then per module group: the module's rate limit
9. per endpoint: route constraints, authorization, the group's filters
   (request metrics), the endpoint's filters (validation), the handler

The original fixed its security headers to survive the exception handler in
`09a4c25`; this port always set them on the way out, so that divergence is
retired. The rate limiter still partitions before authentication, so module
policies partition by client address, as the original's do (P24).

## Events

Orders publishes `InvoiceFinalized` through an outbox in its own schema
(ADR-0008). `EventPublisher::publish` inserts the outbox row on the caller's
transaction, so the event commits with the status change or not at all.
`OutboxDispatcher`, a hosted service, polls `orders."OutboxMessage"` 50 rows
at a time with `FOR UPDATE SKIP LOCKED`, through Orders' bounded work queue,
and hands each event to every subscription for its type. Each handler runs in
`inbox::run` on its own module's pool: the `(EventId, HandlerName)` inbox row
and the handler's writes commit together, so a redelivery is skipped.
Failures retry after 1 s, 5 s, 30 s, 2 min and 10 min; the sixth
dead-letters the row.

The persisted strings are the original's (`Orders.Contracts.Events.InvoiceFinalized`,
`Catalog.Modules.Events.InvoiceFinalizedHandler`,
`Admin.Modules.Events.InvoiceFinalizedHandler`), so a Rust and a C# host
could read each other's outbox and inbox.

## Reporting

Reporting owns schema `reporting`: an `IntegrityFinding` table and two views
over the other three schemas, read as the `reporting` login in the read-only
`reporting_reader` role its migration grants (ADR-0014). Its two report
queries hold a `ModuleGate` slot each. `IntegrityCheckJob` runs the four
orphan checks for the removed foreign keys daily at
`Reporting:Integrity:RunAtUtc`, or on demand, through Reporting's work queue
(ADR-0015). It records findings and repairs nothing.

## Startup

`api::build` registers every module, then — in Development, Test and Demo,
when `Database:MigrateAndSeedOnStartup` is true — migrates Administration,
Catalog, Orders, Reporting in that order and loads `data/chinook-postgres-seed.sql`
if `catalog."Track"` is empty. It maps every module's endpoints, runs the
endpoint check in Development and Test, and builds the pipeline. The binary
then starts the hosted services: the outbox dispatcher, both work queues, the
integrity job.

## Failure domain

What a module can and cannot isolate in one process is the original's table,
ported in the [README](../README.md#failure-domain).

## Verification

`architecture-tests` holds the boundary, guardrail, fence, extraction and
public-surface suites. `module-tests` runs each module on a host of its own;
`api`'s tests run the whole host. All database suites run against PostgreSQL
in a container. `tools/run-parity.sh` compares this port with the running
original; see [parity.md](parity.md).
