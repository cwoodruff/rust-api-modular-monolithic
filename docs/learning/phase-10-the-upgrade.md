# Phase 10 — Following the original's upgrade

*Learning Rust through the port, part 11 of 11.*

| | |
|---|---|
| **What this phase built** | The original's ten-phase modular-monolith upgrade, in Rust: PostgreSQL with a schema per module, contracts crates, a module contract with `register_services` back in it, named-policy authorization, per-module bulkheads, an outbox and inbox, a Reporting read model, Catalog in a process of its own, and three test suites on a real database. |
| **Commits** | `ec939b2` (plan and ADRs), `9763240` (the switch), `165dd4b` (test fixture), `7c4446c` (architecture suites), `39d8578` (`catalog-host`), `933c4f2` (module test host) |
| **Files to open** | `crates/shared-kernel/src/{module,endpoints,auth,persistence,concurrency,hosting}.rs`, `crates/shared-kernel/src/events/`, `crates/module-catalog/src/`, `crates/test-support/src/postgres.rs`, `crates/api/src/host_composition.rs` |
| **Rust ideas introduced** | A typed composition object in place of a DI container, builders that borrow mutably and finish on `Drop`, metadata recorded beside an opaque router, runtime policy names versus compile-time types, `&mut PgConnection` through a transaction, higher-ranked closures (`for<'c>`), `Any` and `downcast_ref` for typed erasure, bounded `mpsc` with `reserve`, `oneshot` replies, `catch_unwind` on futures, `OnceLock` statics that outlive tokio runtimes, `sqlx::migrate!` per crate |

The earlier phases ported a codebase that kept its domain in shared projects.
The original then rebuilt itself around module ownership, and this phase
follows it. Very little of it is new Rust syntax. Most of it is *where* to put
things so the compiler enforces what C# enforced with reflection tests. Read
[`docs/upgrade-plan.md`](../upgrade-plan.md) section 3 first: it is the
mapping table this document walks through.

---

## 1. `Services`: composition without a container

ASP.NET's `IServiceCollection` lets any module ask for any registered type.
That is why the original needs `GuardrailTests` to check, after the fact, that
no module's service resolves to another module's type.

Rust has no container, and this port does not build one. A module builds its
own services and keeps them. What it hands the host is a short, typed list
(`shared-kernel/src/module.rs`):

```rust
pub trait Module: Send + Sync {
    fn name(&self) -> &'static str;
    fn register_services(&mut self, services: &mut Services, config: &AppConfig) -> Result<(), ModuleError>;
    fn map_endpoints(&self, endpoints: &mut Endpoints);
}
```

Two things to notice.

**`&mut self` in `register_services`.** The module stores what it built in its
own fields, an `Option<Arc<CatalogState>>` in `CatalogModule`, and
`map_endpoints` reads it back. The two-phase shape of the original (register
everything, then map everything) survives because the host calls all the
`register_services` before any `map_endpoints`.

**`Services` has methods, not a type map.** `add_module_cache(name, size)`
returns an `Arc<CacheFacade>` and also records it, so the configuration test
can check every module's cache has a size in `appsettings.json`. There is no
`get::<T>()`. A module cannot ask `Services` for another module's state,
because there is no method that would hand it over. The guardrail the original
writes as a test is, here, the absence of an API.

## 2. A builder that finishes when it is dropped

`Endpoints::map_group` returns a `RouteGroup<'_, S>`, which borrows the
`Endpoints` mutably for as long as it lives (`endpoints.rs`):

```rust
pub fn map_group<S>(&mut self, prefix: &'static str, tag: &'static str, state: S) -> RouteGroup<'_, S>
```

The group collects pending endpoints and does its work in `Drop`: wrap each
method router in its layers, apply the group's rate limit, nest the result
under the prefix, push a descriptor per endpoint. `Drop` runs when the group
goes out of scope, which is where the C# builder would also have finished
applying conventions.

The lifetime `'_` is what makes this safe. While a group is alive, nothing else
can touch `Endpoints`, so two groups cannot interleave half-finished routes.
Try mapping two groups in one scope without a block between them and read the
borrow checker's message.

Each `map_get` returns `&mut EndpointConventions`, so calls chain the way
`.WithName(...).RequireAuthorization(...)` chain in C#:

```rust
group
    .map_get("/albums/{id:int}", handlers::get_album_by_id)
    .with_name("CatalogGetAlbumById")
    .require_authorization(READ)
    .produces(BY_ID);
```

## 3. Metadata beside an opaque router

axum's `Router` cannot be inspected: you cannot ask it which routes it has or
what layers sit on them. The original's `EndpointDataSource` can, and the
startup check, the OpenAPI generator and five guardrail tests depend on that.

So the builder writes the metadata down as it goes: an `EndpointDescriptor`
per endpoint, with method, route, name, tag, policies, rate-limit policy and
statuses. `api::build` returns them with the router. The router serves; the
descriptors describe. Because both come from the same builder call, they
cannot disagree.

Route constraints show the same split. `{id:int}` means nothing to axum, so
`parse_route` rewrites it to `{id}` for the router and keeps the constraint
for a layer that answers 404 when the value is not an integer.

## 4. Named policies instead of policy types

Phase 5 made authorization a type: a handler took `Authorized<MusicRead>`, and
the extractor ran `MusicRead::REQUIREMENTS` before the body. A misspelled
policy could not compile.

The upgrade gave that up on purpose (ADR-0021). The original's guardrails
include "every policy an endpoint references is registered", and with types
that test would test nothing. More practically, three different hosts decide
the same policies differently: the monolith from its own tokens,
`catalog-host` from tokens it validates over HTTP, and the module test host
from request headers. A policy written as a type has one meaning; a policy
written as a name can be decided by whichever `PolicyEvaluator` the host
registered:

```rust
pub trait PolicyEvaluator: Send + Sync {
    fn is_registered(&self, policy: &str) -> bool;
    fn evaluate(&self, policy: &str, principal: &Principal) -> PolicyDecision;
}
```

The cost is the one the original pays: a wrong name is now possible. It is
caught three times before a request arrives (the names are constants in
`identity-contracts`, the guardrail test checks them, and the host refuses to
start in Development), and a fourth time at request time, as a 500.

The handler side kept one extractor, `Caller`, for handlers that need the
user. It does not check policies; the layer already did. It only refuses to
invent a caller that is not there.

## 5. The outbox: one transaction, passed by `&mut`

The rule that makes an outbox work is that the event row and the business
change commit together. In sqlx that means the publisher takes the caller's
connection, not a pool (`events/publisher.rs`):

```rust
pub async fn publish<E: IntegrationEvent>(&self, event: &E, connection: &mut DbConnection) -> Result<(), sqlx::Error>
```

The caller has a `Transaction`, which dereferences to a connection, so Orders'
finalize writes `publisher.publish(&event, &mut transaction)` and then
`transaction.commit()`. If the commit never happens, `Transaction`'s `Drop`
rolls back, and the outbox row goes with it. Nothing in the type lets the
publisher commit on its own.

The inbox runs the other direction: it owns the transaction and lends it to the
handler (`events/inbox.rs`):

```rust
pub async fn run<F>(pool: &DbPool, schema: &str, event_id: Uuid, handler_name: &str, now: DateTime<Utc>, handle: F)
    -> Result<bool, HandlerError>
where
    F: for<'c> FnOnce(&'c mut DbConnection) -> BoxFuture<'c, Result<(), HandlerError>>,
```

`for<'c>` is a higher-ranked bound. It says the closure must accept a
connection borrowed for *any* lifetime `'c`, and return a future that lives no
longer than that borrow. The guard picks the lifetime: it borrows the
transaction for exactly the handler's run, then commits. Without the `for<'c>`
you would have to name a lifetime the caller cannot know.

## 6. Typed erasure: `Any` and `downcast_ref`

The dispatcher handles rows whose type is a string, `EventType`, and hands
them to handlers that want a concrete `InvoiceFinalized`. C# solves this with
`Type` and reflection. Here each event type contributes a decoder, a plain
`fn(&str) -> Result<Arc<dyn Any + Send + Sync>, String>`, and each
subscription downcasts back:

```rust
let event = event.downcast_ref::<E>().ok_or_else(|| -> HandlerError {
    format!("event is not a {}", E::EVENT_TYPE).into()
})?;
```

`Subscription::new::<E, H>` is generic, but the `Subscription` it returns is
not: the generic code is captured in a closure stored as
`Arc<dyn for<'a> Fn(...)>`. That is how a `Vec<Subscription>` can hold handlers
for different event types. It is also what upstream moved to for AOT: typed
delegates instead of reflection.

## 7. A bounded queue that returns results

`ModuleWorkQueue` is a `tokio::sync::mpsc::channel(capacity)` with one
consumer. A full queue makes producers wait, which is the backpressure the
original gets from `BoundedChannelFullMode.Wait`.

Two details are worth reading in `concurrency.rs`.

**`reserve` before counting.** The queue reports how many items are waiting.
Counting after `send` races with the consumer, which can take and uncount the
item first; counting before `send` counts a producer that is still blocked.
`reserve()` waits for room and returns a permit, the count goes up, then
`permit.send(item)` cannot fail and the consumer can only see an item that is
already counted.

**A `oneshot` for the reply.** `run` wraps the work so its result goes back
through a `oneshot::Sender`, and the caller awaits the receiver. The work
itself is wrapped in `AssertUnwindSafe(work(token)).catch_unwind()`, from the
`futures` crate, so a panicking item reaches its caller as
`WorkError::Panicked` and the consumer moves on to the next one.

## 8. Migrations per crate, history per schema

Each module crate embeds its own migrations at compile time:

```rust
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
```

The path is relative to the crate's manifest, so the SQL travels with the
module and the binary carries every module's migrations without reading files
at runtime.

sqlx keeps its history in a table called `_sqlx_migrations` in whatever schema
`search_path` names first. `ModuleDatabase::migrate` opens one connection,
creates the module's schema, sets `search_path` to it, and runs the migrator.
Each schema therefore gets its own history table, which is what EF's
`MigrationsHistoryTable("__EFMigrationsHistory", schema)` does upstream. The
pool's other connections never have `search_path` changed, so queries name
their schema: `catalog."Track"`.

## 9. One shared container, and `OnceLock`

Every `#[tokio::test]` runs on a runtime of its own, created for that test and
dropped after it. Anything a test shares with another test therefore cannot
belong to a runtime. `test-support/src/postgres.rs` follows three rules:

- **The server is a `OnceLock<PostgresServer>`**, a plain struct of host, port
  and credentials. `get_or_init` finds the container the first time any test
  asks, starting it if it is not running, and every later test gets the same
  address.
- **The container outlives the process, on purpose.** The first version used
  the `testcontainers` crate and parked its handle in a `static`. A `static`
  is never dropped, so the handle's `Drop` never stopped the container, and
  nothing else did: one run left a hundred behind. Now every test process
  shares one container, `rmm-test-postgres`, started with the Docker CLI if
  it is not already up. Each process stamps the databases it creates with
  its start time, and the first template request drops any older than six
  hours. A resource that cannot be cleaned up at exit is made safe to reuse
  instead.
- **No pools are shared.** A sqlx pool spawns tasks on the runtime that made
  it, so a pool created in one test is unusable in the next. Fixture
  operations open one connection, use it, and close it.

The lock that serializes `CREATE DATABASE … TEMPLATE` is a
`tokio::sync::Mutex` in a `static`. A tokio mutex does no I/O and holds no
runtime, so a static one works across tests the way a `std::sync::Mutex`
would, while still being safe to hold across an `.await`.

Template cloning is what makes this fast. The first host migrates and seeds
a template through the real `api::build`; every later host gets a
`CREATE DATABASE … TEMPLATE … STRATEGY FILE_COPY` copy in tens of
milliseconds, and no test ever sees another's writes.

---

## Try it

1. **Reach across a module.** Add `module-orders = { workspace = true }` to
   `crates/module-catalog/Cargo.toml` and run
   `cargo test -p architecture-tests --test module_boundaries`. Read which
   rule fails, then take it out.
2. **Misspell a policy.** Change `permissions::CATALOG_READ` to
   `"catalog.raed"` in one Catalog route and run `cargo run -p api` in
   Development. Read the `Endpoint validation failed:` message. Then run the
   guardrail tests.
3. **Interleave two groups.** In any module's `endpoints.rs`, call
   `endpoints.map_group(...)` twice without a block around the first and read
   the borrow checker's message about the `RouteGroup` lifetime.
4. **Watch the outbox.** Start the host, finalize invoice 1 with an admin
   token, and run
   `docker exec rust-modular-monolith-postgres psql -U chinook -d chinook -c 'select "EventType", "ProcessedAt" from orders."OutboxMessage"'`
   before and after the dispatcher's next poll.
5. **Break a handler.** Make Catalog's `InvoiceFinalizedHandler` return an
   error, run `cargo test -p module-tests --test kernel_outbox_dispatcher`,
   and compare with `orders."OutboxMessage"."LastError"` after a real
   finalize.
6. **Look at the history tables.** After one Development run,
   `\dt *._sqlx_migrations` in `psql`. Four schemas, four tables.

## Vocabulary for this phase

| C# / .NET | Rust | Notes |
|---|---|---|
| `IServiceCollection` | `Services` | Typed methods, no type map |
| `IModule.RegisterServices` | `Module::register_services(&mut self, …)` | The module keeps what it builds |
| `MapGroup(...)` | `Endpoints::map_group` → `RouteGroup` | Finishes in `Drop` |
| `EndpointDataSource` | `Vec<EndpointDescriptor>` | Recorded by the builder |
| `RequireAuthorization(name)` | `.require_authorization(&[name])` | Decided by a `PolicyEvaluator` |
| `DbContext.SaveChanges` with an outbox row | `Transaction` + `publish(&e, &mut tx)` + `commit()` | Rollback on `Drop` |
| `Func<CancellationToken, Task>` | `for<'c> FnOnce(&'c mut C) -> BoxFuture<'c, …>` | Higher-ranked closure |
| `Type`, reflection | `dyn Any` + `downcast_ref` | Typed erasure |
| `Channel.CreateBounded` | `tokio::sync::mpsc::channel(n)` | `reserve` then `send` |
| `TaskCompletionSource<T>` | `tokio::sync::oneshot` | |
| `BackgroundService` | `HostedService::execute` + `CancellationToken` | `tokio_util` |
| `Database.Migrate()` per context | `sqlx::migrate!` per crate, `search_path` per schema | `_sqlx_migrations` per schema |
| `ICollectionFixture` / static fixture | `OnceLock` static | Must not hold runtime-bound values |
| Testcontainers | one shared container via the Docker CLI | Reused across processes; stale databases dropped by stamp |

**Back to the start:** [Phase 0 — the scaffold](phase-0-scaffold.md), or the
[index](README.md) for the whole series.
