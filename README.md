# rust-api-modular-monolithic

A Rust port of [aspnetcore-min-api-modular-monolithic](https://github.com/cwoodruff/aspnetcore-min-api-modular-monolithic),
an ASP.NET Core 10 Minimal API modular monolith over the Chinook database. It
follows the original through its own modular-monolith upgrade (upstream
`3172d97`): PostgreSQL with a schema per module, contracts crates, an outbox
between modules, per-module bulkheads, a Reporting read model, and Catalog
runnable in a process of its own.

The aim is the original's architecture and wire behavior, one to one, in
idiomatic Rust. Where the two differ on purpose, the difference is written
down: [docs/upgrade-plan.md](docs/upgrade-plan.md) lists every upstream change
and how it landed, and [docs/adr/](docs/adr/README.md) holds the decisions.

## Solution layout

```
crates/
  shared-kernel/             SharedKernel: module contract, Services and Endpoints builders,
                             caching, traffic control, events (outbox, inbox, dispatcher),
                             concurrency (work queue, gate), diagnostics (meters, health checks),
                             validation, persistence, named-policy authorization
  identity-contracts/        Identity.Contracts: permission and policy names
  orders-contracts/          Orders.Contracts: InvoiceFinalized
  catalog-contracts/         Catalog.Contracts (empty, as upstream's is)
  administration-contracts/  Administration.Contracts (empty)
  validation-rules/          FluentValidation's rules and messages, as a library modules use
  module-catalog/            Catalog: artists, albums, tracks, playlists, track sales   schema catalog
  module-orders/             Orders: invoices, invoice lines, the outbox                schema orders
  module-admin/              Administration: customers, employees, genres, media types,
                             customer purchase summaries                                schema administration
  module-reporting/          Reporting: views over the other three, integrity findings  schema reporting
  module-identity/           Identity: tokens, keys, logins, policies                   no tables
  api/                       ModularMonolith.Api: HostComposition, ModuleComposition, DbSeeder,
                             /healthz, one OpenAPI document per module
  architecture-tests/        ModularMonolith.Architecture.Tests
  module-tests/              ModularMonolith.Module.Tests: each module on a host of its own
  test-support/              the PostgreSQL fixture and header-based test authentication
samples/
  catalog-host/              samples/Catalog.Host: Catalog alone in its own process
data/chinook-postgres-seed.sql   the original's seed, byte for byte
docker-compose.yml, docker/postgres-init/
docs/adr/                    ADR-0001 to ADR-0020 as upstream's, plus ADR-0021 for the port
```

## Module contract

```rust
pub trait Module: Send + Sync {
    fn name(&self) -> &'static str;
    fn register_services(&mut self, services: &mut Services, config: &AppConfig) -> Result<(), ModuleError>;
    fn map_endpoints(&self, endpoints: &mut Endpoints);
}
```

The original's `IModule`, all three members. `Services` stands where
`IServiceCollection` stands but is a typed composition object, not a
container: each module builds and keeps its own services, and registers with
the host only what the host runs or reports on (its database, cache,
rate-limit policy, meter, work queue, gate, hosted services and event
subscriptions; Identity also registers the authenticator and the policies).
No module can be handed another module's services (ADR-0021). The host's
module list is explicit, in the original's order: Administration, Identity,
Catalog, Orders, Reporting. Each module crate exports its composition type and
a few constants; everything else is private to it.

More in [docs/architecture.md](docs/architecture.md).

## Guardrails

The ADRs say what each module owns and how each boundary is crossed. These
keep the code honest about it:

- `module_boundaries.rs`: no module depends on another (all 20 pairs); a
  module depends only on the kernel, contracts crates and `validation-rules`;
  contracts depend only on the kernel; the kernel depends on nothing in the
  workspace.
- `guardrails.rs`, over the endpoints a real host maps: every policy an
  endpoint names is registered and is an `identity-contracts` constant; every
  module endpoint carries exactly its module's rate limit; no two endpoints
  share a route and method; event handlers are keyed by the module that owns
  them; every endpoint that takes no caller is named with the reason.
- `fences.rs`: contracts hold constants, data and event accessors only; the
  kernel's public types stay within a budget, and the PostgreSQL driver's
  names appear in `persistence.rs` alone (ADR-0012).
- `extraction_readiness.rs`: Catalog references only the kernel and contracts;
  `catalog-host` only Catalog, the kernel and their contracts.
- `public_surface.rs`: what each module exports.
- At startup in Development and Test, `ModuleComposition` refuses to run if an
  endpoint names an unregistered policy or two endpoints share a route.

## Endpoints

Every route the original serves, with the same status codes, policies and
bodies. Data routes need a token; health routes do not.

| Module | Routes |
|---|---|
| host | `GET /`, `GET /healthz`, Swagger at `/swagger` (Development and Demo) |
| Catalog `/api/catalog` | albums, artists, playlists and tracks by id and as collections; tracks by artist, playlist, album, genre and media type; **`GET tracks/{id}/sales`** |
| Orders `/api/orders` | invoices and invoice lines by id, collection and filter; **`POST invoices/{id}/finalize`**; **`GET outbox/dead-letters`**, **`POST outbox/dead-letters/{id}/retry`** |
| Administration `/api/admin` | customers, employees, genres, media types; employee direct reports and manager; **`GET customers/{id}/purchases`**; genre create, update, delete |
| Reporting `/api/reporting` | **`GET sales-by-genre`**, **`GET invoices/{id}/lines`**, **`POST integrity/run`**, **`GET integrity/findings`** |
| Identity `/api/identity` | `login`, `refresh`, `logout`, `userinfo`, `.well-known/jwks.json` |

Each module also serves `/health` and `/data-health`. The routes in bold are
new since the first port, as is `/healthz`. Two changed: Catalog was
`/api/music`, and `GET /api/music/tracks/invoice/{id}` is gone, because it
joined across modules. `employees/{id}/reports-to` now returns the employee's
manager, and 404 for someone who reports to no one (upstream 665e49c).

Responses lost the members that were joined in from other modules: a track's
`Genre`, `MediaType`, `GenreName`, `MediaTypeName` and `InvoiceLines`; an
invoice's `Customer`; an invoice line's `TrackName` and `Track`; a customer's
`Invoices`; a genre's and a media type's `Tracks`. An invoice gained `Status`.

Finalizing returns `202 Accepted` with `Location: /api/orders/invoices/{id}`
and `{"invoiceId":…,"status":"Finalized","salesCountersUpdate":"eventual"}`,
or `409` if the invoice is already final. Track sales and the customer's
purchase summary update once the outbox is dispatched, not before.

## Run it

The host needs PostgreSQL 17. Start the local one, then the API, from the
repository root:

```sh
docker compose up -d
ASPNETCORE_ENVIRONMENT=Development cargo run -p api
curl -s localhost:5043/api/catalog/data-health
```

```json
{"module":"Catalog","status":"Data-Healthy","timestampUtc":"2026-10-17T00:12:44.0919590Z",
 "environment":"Development","version":"1.0.0","service":"ModularMonolith.Api",
 "database":{"connected":true}}
```

The compose container is named `rust-modular-monolith-postgres` and listens
on 5432; its init script creates the read-only `reporting` login. It is named
differently from the original's `modular-monolith-postgres` so both checkouts
can keep their own volume, but both use port 5432, so run one at a time. In
Development and Demo the host migrates every module in order and loads
`data/chinook-postgres-seed.sql` on first start (`Database:MigrateAndSeedOnStartup`).
`docker compose down -v` throws the database away and the next start seeds
it again.

### Logins

The data routes need a token, and there are no seeded users: the original
ships an empty list and expects user secrets to fill it, and so does this.
Logins are read **at startup**.

The host reads user secrets in Development and Demo from the original's
`UserSecretsId`, so the original's own commands work, run from its checkout:

```sh
dotnet user-secrets --project src/ModularMonolith.Api set "Identity:InMemoryUsers:0:Username" "demo"
dotnet user-secrets --project src/ModularMonolith.Api set "Identity:InMemoryUsers:0:Password" '<a-password>'
dotnet user-secrets --project src/ModularMonolith.Api set "Identity:InMemoryUsers:0:UserId" "user-1"
dotnet user-secrets --project src/ModularMonolith.Api set "Identity:InMemoryUsers:0:Tenant" "tenant-1"
dotnet user-secrets --project src/ModularMonolith.Api set "Identity:InMemoryUsers:0:Permissions:0" "catalog.read"
```

`USER_SECRETS_PATH` points at a `secrets.json` elsewhere. Environment
variables work too, and override secrets:

```sh
export Identity__InMemoryUsers__0__Username=demo
export Identity__InMemoryUsers__0__Password=secret123
export Identity__InMemoryUsers__0__UserId=user-1
export Identity__InMemoryUsers__0__Tenant=tenant-1
export Identity__InMemoryUsers__0__Permissions__0=catalog.read
```

The startup log says what loaded, with the original's messages:

```
Effective Identity:InMemoryUsers:0 => Username='demo', UserId='user-1', Roles='(none)', Permissions='catalog.read', Tenant='tenant-1', PasswordLength=9.
Loaded 1 in-memory login user(s) for the Development environment.
```

An entry missing `Username`, `Password` or `UserId` is ignored with a warning
naming its index and the missing fields, and a password that looks like a
pasted shell command gets a warning of its own. Either way every login then
answers 401, the same 401 a wrong password gets.

```sh
TOKEN=$(curl -s -X POST localhost:5043/api/identity/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"demo","password":"secret123"}' | jq -r .access_token)

curl -s -H "Authorization: Bearer $TOKEN" localhost:5043/api/catalog/albums/1
```

### Catalog on its own

```sh
cargo run -p catalog-host
```

Serves `/api/catalog/*` on port 5143 against the compose database, accepting
tokens from the monolith on 5043 (it fetches their JWKS), and reads
`InvoiceFinalized` from Orders' outbox by cursor without taking rows from
Orders' dispatcher (ADR-0017, ADR-0020). It runs no migrations: start the
monolith once first.

## Build and test

```sh
cargo build --workspace
cargo test --workspace

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

The database suites start their own `postgres:17-alpine` container with
testcontainers, so Docker has to be running; the compose database does not.
`TEST_POSTGRES_URL=postgres://postgres:postgres@localhost:5432/postgres`
points them at a throwaway server instead. Each test host gets its own clone
of a migrated, seeded template database, so tests run in parallel.

- `architecture-tests`: the guardrails above, no database.
- `module-tests`: each module on a host of its own, with header-based
  authentication standing in for Identity (ADR-0016): endpoints, the Orders
  outbox, both event handlers through their inboxes, the dispatcher's retry
  schedule and dead-lettering on a manual clock, cache isolation, Reporting as
  its read-only login.
- `api` tests: the whole host, for what needs more than one module: a
  finalized invoice reaching Catalog and Administration, the authorization
  pipeline, rate limits across modules, health, security headers, errors,
  OpenAPI, seed integrity, Identity.
- `catalog-host` tests: the sample's health, remote token validation and
  outbox feed.

CI (`.github/workflows/ci.yml`) runs formatting, clippy, the build and every
suite on each push to `main` and each pull request.

### Test cost

Median wall clock of three runs per suite, debug build, container start
included, on an Apple M5 Pro (18 cores, 24 GB), macOS 27.2, Docker 29.8,
cargo 1.97.1:

| Suite | Tests | Median (s) | Per test (ms) |
|---|---:|---:|---:|
| `architecture-tests` | 26 | 5.86 | 225 |
| `module-tests` | 41 | 23.42 | 571 |
| `catalog-host` | 9 | 10.25 | 1139 |
| `api` | — | — | — |

These were taken while other builds shared the machine, so read them as an
upper bound; `api`'s suite was still being written and is not in the table.
`tools/timing.sh [runs]` builds once and times `architecture-tests`,
`module-tests` and `api` with nothing else compiling.

## Check it against the original

`tools/run-parity.sh` runs the original and this port side by side and
compares their answers case by case. See [docs/parity.md](docs/parity.md).

### One header the comparison cannot see

Every response from this port carries an `x-trace-id`. The original sends no
such header.

```sh
curl -s -D - -o /dev/null localhost:5043/api/catalog/health | grep -i '^x-trace-id'
```
```
x-trace-id: 00-04a86a70afbf2f376882fad2b057433e-b8fe0ae130e34d7f-00
```

One identifier per request, in three places: that header, the `traceId` of
any problem document the request produces, and the `tracing` span its log
records are written under. The original puts its `traceId` in problem
documents only, so a client reporting a slow or wrong `200` has nothing to
quote. The change is additive, but it is wire-visible, so it is written down
rather than left to a green comparison to imply.

## Configuration

`appsettings.json`, `appsettings.Development.json` and `appsettings.Demo.json`
are the original's files, so keys, defaults and environment variables carry
over unchanged. Connection strings are Npgsql's
`Host=…;Port=…;Database=…;Username=…;Password=…` form; `postgres://` URLs work
too. `ConnectionStrings:AppDatabase` is every module's; Reporting reads
through `ConnectionStrings:Reporting`, a login in the `reporting_reader` role,
and migrates through `AppDatabase`.

### Rate limiting

Each module has its own fixed-window policy on its route group: `catalog:api`,
`orders:api`, `admin:api`, `identity:api`, `reporting:api`.
`global:public-anon` covers `GET /` only, and `/healthz` has none. Limits come
from `RateLimiting:Policies:<name>` (`PermitLimit`, `WindowSeconds`,
`QueueLimit`), 60 per 60 seconds with no queue in `appsettings.json`. A
rejection is a bare 429, which the host turns into a problem document, with no
`Retry-After`. Partitions are by client address: the limiter runs before
authentication, as the original's does. A burst against Catalog answers 429
from Catalog only.

### Caching

Each module has its own in-memory cache, every entry counting one toward
`Caching:Modules:<Module>:SizeLimit` (Catalog 1000, Orders 1000,
Administration 500). Reads are cache-aside for twenty minutes. Unlike the
original, whose tag invalidation is a no-op, a write here really invalidates
the collections it affects. See [docs/caching.md](docs/caching.md).

### Work queues, gates, outbox

`Concurrency:Orders:QueueCapacity`, `Concurrency:Reporting:QueueCapacity` (64)
and `Concurrency:Reporting:MaxConcurrentExpensive` (4);
`Outbox:Enabled` (`true`) and `Outbox:PollIntervalSeconds` (1);
`Reporting:Integrity:Enabled` (`true`) and `Reporting:Integrity:RunAtUtc`
(`02:00:00`).

### Metrics and health

Each module has a meter named `ModularMonolith.<Module>`, every measurement
tagged `module`: `modmono.http.requests` (tagged `status_code`),
`modmono.cache.hits`, `modmono.cache.misses`, `modmono.outbox.published`,
`modmono.outbox.dispatched`, `modmono.outbox.retried`,
`modmono.outbox.dead_lettered`, `modmono.work_queue.depth`. No exporter is
wired, as upstream wires none; `Services::meters()` reads them in process.
`GET /healthz` reports each module database's check, tagged with the module
name, and answers 503 when one is unhealthy.

## How modules talk to each other

Modules never call each other or share a transaction (ADR-0008):

- **Publish with the write.** `EventPublisher::publish(&event, &mut tx)`
  inserts the outbox row on the caller's transaction, so the event commits
  with the business change or not at all.
- **Deliver at least once.** Orders' `OutboxDispatcher` polls its outbox,
  `FOR UPDATE SKIP LOCKED`, 50 at a time, and hands each event to every
  handler subscribed to it.
- **Apply once.** Each handler runs inside the inbox guard on its own module's
  pool; the `(EventId, HandlerName)` inbox row commits with its writes, so a
  redelivered event is skipped.
- **Retry, then ask a person.** After 1 s, 5 s, 30 s, 2 min and 10 min; the
  sixth failure dead-letters the row, which waits for
  `POST /api/orders/outbox/dead-letters/{id}/retry`.
- **Order only within one outbox**, in write order, and only until a row
  fails.

Today Orders publishes `InvoiceFinalized`; Catalog keeps `TrackSales`
(ADR-0009) and Administration `CustomerPurchaseSummary` (ADR-0010). Both
count only invoices finalized through the endpoint; the seeded 458 start as
`Draft`.

## Reporting

`GET /api/reporting/sales-by-genre` and `GET /api/reporting/invoices/{id}/lines`
read two views over the Catalog, Orders and Administration schemas, as the
read-only `reporting_reader` role (ADR-0014), at most four at a time.
`POST /api/reporting/integrity/run` and `GET /api/reporting/integrity/findings`
run and list the orphan checks for the four cross-module foreign keys the
upgrade removed (ADR-0015). The job also runs daily at
`Reporting:Integrity:RunAtUtc`. It never repairs data.

## Failure domain

All modules run in one process. Each gets its own share of what can be split
in-process (ADR-0013):

| A module can isolate in-process | How |
|---|---|
| Its request budget | Its own rate-limit policy on its route group: a burst against Catalog returns 429 from Catalog only |
| Its cache | Its own cache with a size limit: filling it never evicts another module's entries |
| Its background work | Its own bounded work queue: a backlog makes producers wait instead of growing memory |
| Its expensive operations | Its own gate's concurrency cap |
| Its visibility | Its own meter (`module` tag on every measurement) and its own `/healthz` entry |
| Its data and failures in event handling | Its own schema, pool, inbox and retries (ADR-0003, ADR-0008) |

| A module cannot isolate in-process | Why |
|---|---|
| Memory and the allocator | One heap; a module that allocates without bound takes the process with it |
| The async runtime | One tokio runtime; blocking or CPU-heavy code in one task starves the others' worker threads |
| Crashes | A panic in a hosted task is contained, but an abort, a stack overflow or running out of memory ends the process |
| The database server | One PostgreSQL server: connections, CPU and locks are shared |
| Deploys | One build, one release, one restart, one rollback |

When one of the second list is the problem, the answer is extraction, not
another in-process limit. [docs/extraction-playbook.md](docs/extraction-playbook.md)
walks through it for Catalog, and `samples/catalog-host` runs Catalog on its
own.

## Docker

```sh
docker compose up -d
docker build -t modular-monolith-api .
docker run -p 8080:8080 -e ASPNETCORE_ENVIRONMENT=Demo \
  -e ConnectionStrings__AppDatabase="Host=host.docker.internal;Port=5432;Database=chinook;Username=chinook;Password=chinook" \
  -e ConnectionStrings__Reporting="Host=host.docker.internal;Port=5432;Database=chinook;Username=reporting;Password=reporting" \
  modular-monolith-api
```

There is no database in the image; point the two connection strings at a
PostgreSQL 17 server. Demo migrates and seeds on startup like Development.
The image runs as a non-root user with a healthcheck.

The environment is **not** defaulted. Development and Demo unlock Swagger, the
in-memory login store and the development signing key, and an image that
quietly enables all three is the wrong default. Left unset the host runs as
Production: it does not migrate or seed, so its database must already be
migrated (one Development or Demo run against it does that), and it needs a
real key provider — an RSA private key you supply, as a mounted file or a
variable:

```sh
-e Jwt__KeyProvider=File -e Jwt__PemKeyPath=/run/secrets/jwt-signing-key.pem
-e Jwt__KeyProvider=Environment -e JWT_SIGNING_KEY_PEM="$(cat jwt-signing-key.pem)"
```

The `kid` the JWKS document publishes is the key's RFC 7638 thumbprint, the
same across restarts and replicas. `Jwt__KeyProvider=KeyVault` is recognized
and refused: the original's Key Vault provider is not implemented here.

## Documentation

| | |
|---|---|
| [upgrade-plan.md](docs/upgrade-plan.md) | Every upstream change since the first port, how it maps to Rust, and the commits |
| [adr/](docs/adr/README.md) | The decision records: module map, boundaries, outbox, bulkheads, Reporting, tests, extraction |
| [architecture.md](docs/architecture.md) | Crates, rules, the module contract, the pipeline, events |
| [authn-authz.md](docs/authn-authz.md) | Tokens, claims, named policies, the tenant rule, remote identity |
| [caching.md](docs/caching.md) | Per-module caches, keys, lifetimes, and the invalidation fix |
| [extraction-playbook.md](docs/extraction-playbook.md) | What taking Catalog out of the process would take, week by week |
| [parity.md](docs/parity.md) | How parity is verified, and where the two deliberately differ |
| [rust-translation-plan.md](docs/rust-translation-plan.md) | The first port's plan and its preserve-versus-fix decisions |
| [learning/](docs/learning/README.md) | Eleven Rust lessons, one per phase, each walking the code it produced |

## License

MIT — see [LICENSE](LICENSE).
