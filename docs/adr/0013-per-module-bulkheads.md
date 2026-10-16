Ports upstream ADR-0013.

# 0013. Per-module bulkheads inside one process

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 5; port steps 2 and 3 (see `docs/upgrade-plan.md`, section 5)

## Context

All five modules run in one process. Before the upgrade they also share
every in-process resource: one rate-limit bucket per caller for every
endpoint, caches with no per-module bound, unbounded background work, and no
way to tell from metrics which module is busy, failing or slow. A burst
against one module can spend the others' rate limit, evict their cache and
compete for their runtime threads, and nobody can see which module did it.

## Decision

Each module gets its own slice of the resources that can be split in-process,
and every measurement says which module it belongs to. The module's name
(`Module::name()`) is the key throughout. Each module creates its slices in
`register_services` through `Services` (ADR-0021).

| Bulkhead | Mechanism | Configuration |
|---|---|---|
| Rate limit | One fixed-window policy per module (`catalog:api`, `orders:api`, `admin:api`, `identity:api`, `reporting:api`), partitioned as before by the caller's address, applied once as a `layer` on the module's route group. `global:public-anon` covers only `GET /`. | `RateLimiting:Policies:<name>:PermitLimit` / `WindowSeconds` / `QueueLimit` (default 60 per 60 s, no queue) |
| Cache | A `moka` cache per module with `max_capacity` (each entry weighs 1) behind its own cache facade, registered under the module's key. The optional L2 stays shared. | `Caching:Modules:<Module>:SizeLimit` (Catalog 1000, Orders 1000, Administration 500) |
| Background work | `ModuleWorkQueue`: a bounded `tokio::sync::mpsc` channel with one consumer task. `run` waits for capacity when the channel is full and returns the work's result to the caller. The outbox dispatcher runs each batch through its module's queue, so a backlog makes the producer wait instead of growing memory, and a module's background work runs one item at a time. Work sees the queue's own shutdown token. | `Concurrency:<Module>:QueueCapacity` (default 64) |
| Expensive operations | `ModuleGate`: a `tokio::sync::Semaphore` capping concurrent expensive work for a module (Reporting uses it, ADR-0014). | `Concurrency:<Module>:MaxConcurrentExpensive` (default 4) |
| Visibility | `ModuleMeter` for `ModularMonolith.<Module>`: named counters, each tagged `module`, readable in-process. Requests by status code, cache hits and misses, outbox published, dispatched, retried and dead-lettered, work-queue depth. | — |
| Health | A `HealthCheck` per module pool, tagged with the module name; the host reports them at `/healthz` with the original's JSON, 503 when unhealthy. | — |

Every key above has its value written in `appsettings.json`, and a test fails
if one is missing. No metrics crate is added: `ModuleMeter` is a few named
atomic counters. The optional Prometheus exporter is not added, as upstream;
an exporter can read the meters later.

One difference from the original is accepted: .NET's `MemoryCache` refuses a
new entry when full, while `moka` evicts an older one. Both bound the module,
which is what this record asks of the cache, and the isolation test asserts
only that one module never evicts another's entries.

## What this does not isolate

One process is still one failure domain for everything below. These need
separate processes (extraction, ADR-0017), not more in-process code:

- **Memory.** There is no garbage collector to pause other modules, but the
  heap is shared. A module that allocates heavily can exhaust the process's
  memory, and allocation failure aborts every module. The cache bounds only
  the caches.
- **The runtime's threads.** All modules share one multi-threaded `tokio`
  runtime and its blocking pool. A task that blocks or burns CPU without
  yielding holds a worker thread from every module; the work queue and gate
  limit only the work routed through them.
- **Crashes.** A panic in a request handler is caught by the panic layer and
  becomes a 500, and a panic in a spawned task ends only that task. A stack
  overflow, an abort or an out-of-memory condition takes down every module.
- **The database.** All modules use one PostgreSQL server and connection
  string. Pools are per module, but the server's connections, CPU and locks
  are shared.
- **Deploys.** Every release ships and restarts all modules together; a bad
  change in one is a rollback for all.
- **Requests rejected before the counter.** The request counter is a layer
  inside the module's route group. Requests the rate limiter or the
  authorization check reject before it are not counted per module.

## Consistency

Not a boundary decision.

## Consequences

- A burst against one module returns 429 from that module only.
- Filling one module's cache cannot evict another module's entries.
- Metrics can split traffic, cache efficiency and outbox flow by module.
- Configuration grows by a few keys per module; all have defaults.
- The outbox published counter counts at publish time, before the publishing
  transaction commits, so a rolled-back publish is still counted. The
  counter's own description says so.

## How to reverse

Apply one rate-limit policy on every endpoint again, register one shared
cache, and drop the queue, gate and meter registrations from each module's
`register_services`. Nothing outside the module registrations depends on
them.
