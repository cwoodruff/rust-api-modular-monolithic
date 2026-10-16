# Upgrade plan: bring the port level with the original's modular-monolith upgrade

The original, [aspnetcore-min-api-modular-monolithic](https://github.com/cwoodruff/aspnetcore-min-api-modular-monolithic),
was ported at commit `344fec3` (2026-09-11). Since then it has taken about ninety
commits: its own ten-phase upgrade plan (`docs/upgrade-plan.md` upstream, phases
0–9), merged as PRs #67 to #89 and ending at `3172d97`. This document maps
every one of those changes onto this repository and states how each lands
in Rust.

The stance from [rust-translation-plan.md](rust-translation-plan.md) stays the
same: **preserve everything wire-visible, fix internal bugs.** Where the original
changed its wire behavior, the port changes with it. Where the original fixed a
bug this port had already fixed (security headers on error responses), the
known divergence disappears.

---

## 1. What the original changed

| # | Upstream change | Upstream phase | Lands here in |
|---|---|---|---|
| U1 | `Music` module renamed `Catalog`: `/api/music` → `/api/catalog`, `music.read` → `catalog.read`, cache module `music` → `catalog` | pre-0 | step 3 |
| U2 | Security headers survive the exception handler (#66) | 0 | already true here; divergence retired in step 8 |
| U3 | ADR scaffold, ADR-0001 … ADR-0020 | 0–9 | step 1 |
| U4 | Startup endpoint validation: duplicate routes, unknown policies (Development and Test) | 0 | step 2 |
| U5 | CI builds and tests on push and PR | 0 | step 8 (CI gains a Postgres service) |
| U6 | SQLite → PostgreSQL 17, one schema per owning module, docker-compose, Chinook seed SQL, real migrations | 1 | step 3 |
| U7 | One `DbContext` per module, migrations per module, history table per schema; four cross-module FKs and navigations removed | 2 | step 3 |
| U8 | `SharedKernel.Persistence` and `SharedKernel.DataSQLite` deleted: entities, models, validators and data access move into each module; the repository layer is dissolved | 2 | step 3 |
| U9 | `*.Contracts` projects per module (Catalog and Administration empty; Orders holds `InvoiceFinalized`; Identity holds permission and policy names) | 2, 4 | step 2 |
| U10 | Wire shapes lose the joined cross-module members: Track `GenreName`/`MediaTypeName`/`Genre`/`MediaType`/`InvoiceLines`; Invoice `Customer`; InvoiceLine `TrackName`/`Track`; Customer `Invoices`; Genre and MediaType `Tracks`. Invoice gains `Status` | 2, 3 | step 3 |
| U11 | `GET /api/music/tracks/invoice/{id}` deleted | 2 | step 3 |
| U12 | Hand-rolled outbox and inbox: `IIntegrationEvent`, `IEventPublisher`, `OutboxDispatcher`, `InboxGuard`, retry 1 s/5 s/30 s/2 min/10 min, dead letter after the sixth failure | 3 | steps 2, 4 |
| U13 | `POST /api/orders/invoices/{id}/finalize` (202, `salesCountersUpdate: "eventual"`, 409 when already final) | 3 | step 4 |
| U14 | Consumers: Catalog `TrackSales` + `GET /api/catalog/tracks/{id}/sales`; Administration `CustomerPurchaseSummary` + `GET /api/admin/customers/{id}/purchases` | 3 | step 4 |
| U15 | Dead letters: `GET /api/orders/outbox/dead-letters`, `POST …/{id}/retry` | 3 | step 4 |
| U16 | Policy names are compiled constants in `Identity.Contracts`; no string literal reaches `RequireAuthorization` | 4 | step 2 |
| U17 | Fences: Contracts hold only interfaces/enums/records/constants; SharedKernel public-type budget (30); handlers keyed by module | 4 | step 7 |
| U18 | Bulkheads: one rate-limit policy per module group (`catalog:api` …), per-module caches with size limits, `ModuleWorkQueue`, `ModuleGate`, `ModuleMeter`, `/healthz` | 5 | step 2 (kernel), step 3 (wiring) |
| U19 | Reporting owns a read model: schema `reporting`, two views over the other three schemas, a read-only `reporting_reader` role, four endpoints, a nightly `IntegrityCheckJob` with orphan findings | 6 | step 5 |
| U20 | Handlers are named static functions with typed results; `ValidationFilter<T>` on write endpoints; one OpenAPI document per module plus `v1`; filter-order test; AOT-readiness for kernel and contracts | 7 | steps 2, 3 |
| U21 | Identity validates login, refresh and logout bodies through the filter (`'<field>' is required.`); logout's blank fields become 400 | — | step 3 |
| U22 | `reports-to` returns the employee's **manager**, 404 at the top of the chain (665e49c) | — | step 3 |
| U23 | A test host per module (`ModuleTestHost<TModule>`), template-database cloning, timing scripts | 8 | step 7 |
| U24 | `samples/Catalog.Host`: Catalog alone in its own process, validating Identity's tokens over JWKS, reading Orders' outbox by cursor | 9 | step 6 |
| U25 | Extraction playbook; extraction-readiness tests | 9 | steps 6, 7 |
| U26 | Demo loads user secrets and migrates and seeds; startup warning for a password that looks like a pasted command (8716cab) | — | step 3 |
| U27 | Every per-module limit has its value in `appsettings.json`, and a test fails if one is missing | — | steps 3, 7 |

## 2. Target shape

```
crates/
  shared-kernel/            SharedKernel: Module contract, Services/Endpoints builders,
                            caching (per module), traffic control (per-module policies),
                            events (outbox, inbox, dispatcher), concurrency (work queue, gate),
                            diagnostics (meter, health checks), validation (filter),
                            persistence (pool + per-schema migrations), auth framework
  identity-contracts/       Identity.Contracts: Permissions, Policies
  catalog-contracts/        Catalog.Contracts (empty, as upstream)
  orders-contracts/         Orders.Contracts: InvoiceFinalized
  administration-contracts/ Administration.Contracts (empty, as upstream)
  module-catalog/           Catalog.Module   (was module-music)  schema catalog
  module-orders/            Orders.Module                         schema orders
  module-admin/             Admin.Module                          schema administration
  module-reporting/         Reporting.Module                      schema reporting
  module-identity/          Identity.Module                       no tables
  api/                      ModularMonolith.Api: HostComposition, ModuleComposition, DbSeeder
  test-support/             the Postgres fixture and ModuleTestHost (dev-dependency only)
  module-tests/             ModularMonolith.Module.Tests: one module per host
  architecture-tests/       ModularMonolith.Architecture.Tests
samples/
  catalog-host/             samples/Catalog.Host
data/chinook-postgres-seed.sql   the original's seed, byte for byte (its SHA-256 is pinned)
docker-compose.yml, docker/postgres-init/
docs/adr/                   0000 template, 0001–0020 as upstream, adapted to Rust, plus port-only records
```

`shared-persistence` and `shared-data-sqlite` are deleted, as their C# counterparts were.
`data/chinook.db` is deleted.

## 3. How the C# mechanisms map onto Rust

| Upstream | Here | Note |
|---|---|---|
| `IModule { Name; RegisterServices; MapEndpoints }` | `trait Module { name(); register_services(&mut self, &mut Services, &AppConfig); map_endpoints(&self, &mut Endpoints) }` | The trait gets its third member back. `Services` is a typed composition object, not a DI container: hosted services, event subscriptions, keyed module databases, health checks, caches, work queues, gates, meters, rate-limit policies, the authenticator. A module keeps its own state; there is no shared `AppState` holding every repository any more, which is the Rust form of "no module can inject another's repository" |
| `RequireAuthorization(Permissions.X).RequireAuthorization(Policies.Y)` | `.require_authorization(&[permissions::CATALOG_READ, policies::TENANT_SCOPED])` on the endpoint builder | Policy **names** are evaluated at request time by the `PolicyEvaluator` that Identity registers, so an unknown name is a real failure the startup check can find. Replaces the `Authorized<P>` extractor's compile-time list; the route-guard test now reads endpoint descriptors instead of parsing source |
| Endpoint metadata (`EndpointDataSource`) | `EndpointDescriptor { method, path, name, tag, policies, rate_limit, description, responses }` recorded by the builder | Drives `ModuleComposition::validate_endpoints`, the per-module OpenAPI documents, and the guardrail tests |
| `ValidationFilter<T>` + `IRequestValidator<T>` | `Validated<T>` extractor + `RequestValidator<T>` trait | Same 400 body. Extractors run before the handler body, which is the filter's position |
| Group filter vs endpoint filter order | group `layer` vs endpoint `route_layer` | Tested the same way |
| `DbContext` per module | `PgPool` per module + `sqlx::migrate!` per crate | Migrations run on a connection whose `search_path` is the module's schema, so each schema has its own `_sqlx_migrations` table — the counterpart of `<schema>.__EFMigrationsHistory` |
| EF Core is the repository | services query their own pool with `sqlx::query_as` | No repository traits. Services stay behind traits so handlers can be tested with fakes |
| `IEventPublisher.PublishAsync(e, db)` | `EventPublisher::publish(&e, &mut tx)` | Inserts the outbox row on the caller's transaction |
| `OutboxDispatcher<TContext>` (BackgroundService) | `OutboxDispatcher` hosted task | Same SQL, batch size, retry schedule, dead-letter rule, `LastError` truncation, log messages |
| `InboxGuard.RunAsync` | `inbox_guard::run` | Same `INSERT … ON CONFLICT DO NOTHING`, the handler writes through the same transaction |
| Keyed handlers (`AddIntegrationEventHandler<T,H>(moduleKey)`) | `Subscription { event_type, module_key, handler_name, handle }` | Event decoding is a typed function per event, not reflection — the AOT-friendly path upstream took too |
| `ModuleWorkQueue` (bounded `Channel`) | bounded `tokio::sync::mpsc` + one consumer task | Same backpressure; `run` returns the work's result to the caller |
| `ModuleGate` (`SemaphoreSlim`) | `tokio::sync::Semaphore` | |
| `ModuleMeter` (`System.Diagnostics.Metrics`) | `ModuleMeter`: named counters tagged `module`, readable in-process | No exporter wired, as upstream |
| `IHealthCheck` per context, `/healthz` | `HealthCheck` trait per module database, `/healthz` | Same JSON, 503 when unhealthy |
| `TimeProvider` | `Clock` trait (`SystemClock`, `ManualClock`) | The retry-schedule test advances a manual clock, as upstream's does |
| Testcontainers + template cloning | `testcontainers` + `CREATE DATABASE … TEMPLATE … STRATEGY FILE_COPY` under a lock | `TEST_POSTGRES_URL` points the suites at an existing server instead |
| User secrets (Development and Demo) | `~/.microsoft/usersecrets/<UserSecretsId>/secrets.json`, same id as the original | So the original's `dotnet user-secrets` commands seed this port too |
| `Catalog.Host` JwtBearer with a JWKS retriever | `reqwest` fetch + `jsonwebtoken::jwk::JwkSet`, cached | |

Strings that are **persisted** keep the original's values exactly, so a Rust host
and a C# host could share one database: the outbox `EventType`
(`Orders.Contracts.Events.InvoiceFinalized`) and the inbox `HandlerName`s
(`Catalog.Modules.Events.InvoiceFinalizedHandler`,
`Admin.Modules.Events.InvoiceFinalizedHandler`).

## 4. Decisions on the original's new quirks

| # | Quirk | Decision |
|---|---|---|
| P19 | Customer by-id still returns nine null members (P17); lists still return `SupportRepId: null` | Preserve |
| P20 | Finalize's 202 body is camelCase while every other DTO is PascalCase | Preserve |
| P21 | `Catalog.Host` counts an event type it does not subscribe to as delivered and moves the cursor past it | Preserve; it is the documented ADR-0017 behavior |
| P22 | The outbox `Published` counter increments before the transaction commits | Preserve; the instrument's own description says so |
| P23 | `GET /api/reporting/invoices/{id}/lines` answers 404 for an invoice with no lines, not only a missing one | Preserve |
| P24 | Rate-limit partitions are computed before authentication, so module policies partition by address | Preserve (continues P7) |
| F17 | `RemoveByTagAsync` is still a no-op upstream | Keep this port's real tag invalidation (F1) |
| F18 | Swagger's undefined `X-API-Key` requirement survives upstream | Keep emitting only `Bearer` (F3) |
| F19 | .NET's `MemoryCache` refuses a new entry when full; moka evicts | Accept: both bound the module, which is what ADR-0013 asks of it. The isolation test asserts only that one module never evicts another's entries |
| F20 | `ModuleWorkQueue` upstream passes the queue's own stopping token to the work | Same here: work sees the queue's shutdown token |

## 5. Steps

Each step is one commit on `upgrade/modular-monolith-parity`, with
`cargo fmt`, `cargo clippy -D warnings` and the tests green at each.

1. **Plan and decision records.** This file; `docs/adr/` with the template and
   ADR-0001 to ADR-0020, adapted to the crate names.
2. **Kernel and contracts.** Everything in §3 that lives in `shared-kernel`,
   added beside the old code; the four contracts crates.
3. **The switch.** PostgreSQL, the five modules on the new `Module` trait with
   their own schemas, migrations, models, validators, services, handlers and
   endpoints; Music becomes Catalog; the host becomes `HostComposition` +
   `ModuleComposition` + `DbSeeder` + `/healthz` + per-module OpenAPI; the old
   persistence crates and the SQLite file are deleted; docker-compose and the
   seed land. Existing tests move to the new host.
4. **Events.** Outbox dispatcher wired for Orders, finalize, the two consumers,
   dead letters.
5. **Reporting.** Views, the read-only role, the integrity job, four endpoints.
6. **Catalog.Host.** The sample binary, remote identity, the outbox feed, the
   extraction playbook.
7. **Tests.** `test-support`, `module-tests`, the full-host suites
   (cross-module flows, bulkheads, configuration, errors, health, identity,
   OpenAPI, rate limits, security headers, seed integrity, migration order,
   filter order, Demo), the architecture suites (guardrails, fences,
   extraction readiness, boundaries) and `tools/timing.sh`.
8. **Operations and docs.** CI with Postgres, the parity harness pointed at a
   PostgreSQL original, Dockerfile, README, `docs/`, a learning-series entry.

## 6. Out of scope, as upstream

No module is extracted; no real identity provider; no Redis, no broker; the
removed foreign keys are detected, not prevented.
