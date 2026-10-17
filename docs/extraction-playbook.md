# Extraction playbook: Catalog

Ports upstream's `docs/extraction-playbook.md` to this repository's crates.

Nothing here has been done. This is the plan for taking Catalog out of the
process if a reason ever appears (a separate team, a different scaling
profile, a different release cadence). The code side is checked on every
build; the data side is written down here because no test can check it.

What is already checked on every build:

- The extraction-readiness tests in `architecture-tests`: `module-catalog`
  depends only on `shared-kernel`, `catalog-contracts`, `orders-contracts` and
  `identity-contracts`, and the two contracts crates depend on nothing but
  `shared-kernel`.
- `samples/catalog-host` builds from `module-catalog` and `shared-kernel`,
  plus the two contracts crates Cargo makes it name itself (it has no
  transitive naming, where C# lets `Catalog.Host` reach them through
  `Catalog.Module`). It serves `/api/catalog/*`, validates tokens against
  Identity's published keys, and consumes `InvoiceFinalized` without Orders
  running ([ADR-0017](adr/0017-catalog-host-reads-orders-outbox-by-cursor.md)).
  Its tests run on a database holding only the `catalog` and `orders` schemas.
- The module-boundary tests: no other module depends on `module-catalog`.

## What moves, and what callers change

| Moves with Catalog | Notes |
|---|---|
| `module-catalog` | Unchanged; registered by `catalog-host` instead of `api::host_composition`. |
| `catalog-contracts` | Empty today. Becomes the place for anything Catalog publishes. |
| The `catalog` schema | `Artist`, `Album`, `Track`, `Playlist`, `PlaylistTrack`, `TrackSales`, `InboxMessage`, `_sqlx_migrations`. |

| Shared, and becomes a published contract | Notes |
|---|---|
| `orders-contracts` (`InvoiceFinalized`) | Today a path dependency; after extraction a versioned crate and a wire schema. Adding a field is safe; renaming or removing one needs a new event version consumed side by side. |
| `identity-contracts` | Policy names and the claims they read (`permissions`, `tenant`, role). The token format is now an API between processes. |
| `shared-kernel` | A crate both deployables depend on. Its public-surface budget (ADR-0012) matters more once it ships to two places. |

**Compile-time callers: none.** No crate depends on `module-catalog` except
the host and the sample; Orders refers to tracks by id only (ADR-0005).

**Runtime callers, which do change:**

- HTTP clients of `/api/catalog/*`. A gateway (or the monolith as a reverse
  proxy) routes the prefix to the new service; the URLs stay.
- Reporting's views join `catalog."Track"` and `catalog."TrackSales"`
  (ADR-0014). They stop working when the schema leaves the database.
- The integrity job's `invoice-line-track`, `track-genre` and
  `track-media-type` checks read `catalog."Track"` (ADR-0015).
- `api::db_seeder` checks `catalog."Track"` to decide whether to seed.

## Events that cross the wire

| Event | From | To | Today | After |
|---|---|---|---|---|
| `InvoiceFinalized` | Orders | Catalog (track sales, ADR-0009) | In-process dispatcher | Broker topic |
| `InvoiceFinalized` | Orders | Administration (ADR-0010) | In-process dispatcher | Unchanged |

Catalog publishes nothing today. Reporting will need it to (see week 5).

## What a network adds, and what the outbox already models

1. **At least once.** Already the rule: the outbox redelivers until every
   handler succeeds, and Catalog's inbox (`shared_kernel::events::inbox::run`,
   keyed by event id and handler name) makes a redelivery a no-op. A broker
   redelivers on the same terms, so the handler does not change. What changes
   is that the event id must survive the trip: it is the message id on the
   wire.
2. **Asynchronous failure.** Today a failing Catalog handler shows up on
   Orders' outbox row (`Attempts`, `LastError`, dead letter at the sixth
   failure) and in Orders' dead-letter endpoints. After extraction Orders'
   dispatcher only knows the broker accepted the message; the failure happens
   in Catalog's process, later. Catalog needs its own retry schedule, dead
   letter queue, alert and replay endpoint, and the runbook for "a sale did
   not count" moves from Orders' team to Catalog's.

Also true already, and easy to forget: there is no ordering across modules,
and within one consumer only until a message fails (ADR-0008).

## The plan, in weeks

One team, part time on this, Catalog's API frozen for new features during
weeks 3 to 6.

### Week 1: the transport

- Choose the broker. Orders' dispatcher gets one more subscription for
  `InvoiceFinalized` that publishes to a topic; the in-process Catalog handler
  stays registered. Orders' row is done when Administration's handler and the
  publish have succeeded.
- `catalog-host` swaps `OrdersOutboxFeed` for a broker consumer that calls
  the same inbox guard with the same handler name
  (`Catalog.Modules.Events.InvoiceFinalizedHandler`).
- Publish `orders-contracts` and `shared-kernel` as crates.

Rollback: remove the publishing subscription; nothing consumed it yet.

### Weeks 2 to 3: dual run, one database

- Deploy `catalog-host` next to the monolith, both pointing at the same
  database. Both handle `InvoiceFinalized`; Catalog's inbox applies each once.
- Route a share of `GET /api/catalog/*` to the new service, then all of it.
  Writes stay on the monolith until the end of week 3, because each process
  has its own cache (20-minute TTLs in Catalog's services): a write on one
  side is not evicted on the other. Either route all of Catalog's traffic to
  one side at a time, or cut the TTLs for these two weeks.
- Watch: error rates per side, the consumer's lag, and Catalog's inbox count
  against Orders' outbox count for `InvoiceFinalized`.

Rollback: route the prefix back to the monolith. Both sides wrote the same
schema with the same code, so there is nothing to reconcile.

### Week 4: move the data

- Create Catalog's own database. Start PostgreSQL logical replication of the
  `catalog` tables from the shared database into it, and let it catch up.
- Freeze Catalog writes (write endpoints return 503) for minutes, wait for
  replication to drain, compare row counts and sequences, point
  `catalog-host`'s connection string at the new database, lift the freeze.
- Replication rather than application dual-writes: dual writes need either a
  distributed transaction or a reconciliation job, and this is the one moment
  the two copies must be identical. The freeze is the honest price.

Rollback, before writes resume on the new database: point the connection
string back. After: replicate the new database back into the shared one and
repeat the freeze.

### Week 5: Reporting and the integrity checks

The cross-schema joins are the data seam this monolith left open on purpose
(ADR-0014). Pick one:

- **Replicate back, read only.** Logical replication of `Track` and
  `TrackSales` from Catalog's database into a `catalog_replica` schema the
  reporting role can read; repoint the two views and the three integrity
  checks. Least code, couples Reporting to Catalog's table shape.
- **Events.** Catalog publishes `TrackChanged` and `TrackSoldCountChanged`
  (in `catalog-contracts`, through a Catalog outbox); Reporting keeps its own
  tables. More code, and the contract is explicit.

Either way the seeder's check moves to Catalog's own seeding.

### Week 6: remove Catalog from the monolith

- Remove `module_catalog::CatalogModule` from `api::host_composition::get_modules`,
  its in-process `InvoiceFinalized` subscription with it, and the
  `module-catalog` dependency from `crates/api`.
- Drop the `catalog` schema from the shared database after one more backup.
- `module-tests`' Catalog suites move with the code; the full-host
  finalized-invoice flow is split into Orders-publishes and
  Catalog-consumes contract tests.

### Weeks 7 to 8: what the monolith gave for free

- Identity's signing key rotation now has a second consumer caching the key
  set; agree on overlap time and cache lifetime (the sample refreshes every
  24 hours, and on an unknown key at most every 5 minutes).
- The per-module rate limits, metrics and health checks (ADR-0013) become the
  service's own, plus the network's: timeouts and retries on the gateway, and
  the consumer lag as a health signal.
- The policy copies in `catalog-host` (`RemoteIdentity` and `RemotePolicies`,
  [ADR-0020](adr/0020-catalog-host-validates-identitys-tokens-over-http.md))
  must track Identity's rules; add a contract test against a token Identity
  issues.

About eight weeks in all. The code work is small; most of it is weeks 3 and
4, where two copies of something must agree.
