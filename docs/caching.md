# Caching

**Status: implemented, L1 only, one cache per module.** The second tier is not
built. The original defaults to `L1` too, and its `appsettings.json` says
`"Tier": "L1"`, so the L1 path is the one that runs there as well.

## One cache per module

Since the original's bulkhead phase (ADR-0013) each module has a cache of its
own. A module asks for it in `register_services`:

```rust
let cache = services.add_module_cache(NAME, DEFAULT_CACHE_SIZE);
```

That builds a `CacheFacade` holding at most
`Caching:Modules:<Module>:SizeLimit` entries — every entry counts one —
falling back to the module's default: Catalog 1000, Orders 1000,
Administration 500, the same numbers `appsettings.json` sets. Filling one
module's cache never evicts another's. Reporting and Identity have no cache.

One difference in how a full cache behaves (F19). .NET's `MemoryCache`
refuses a new entry once its size limit is reached; moka, underneath this
facade, evicts an old one. Either way the module stays within its share,
which is what the bulkhead is for, and `module-tests/tests/kernel_cache.rs`
asserts only that.

Each cache counts on its module's meter: `modmono.cache.hits` and
`modmono.cache.misses`, tagged `module`. A miss is counted once per caller
before single flight, as the original counts it, so callers that wait on a
coalesced load are misses too.

## The shape

Every read in every service is cache-aside: compose a key, read through the
cache, fall back to the database on a miss. Nothing about the HTTP layer
caches — there is no output caching, no `ETag`, no `Cache-Control`.

```rust
let key = self.keys.compose("catalog", "album", "v1", &format!("by-id:{id}"));

self.cache
    .try_get_or_add(&key, load, Some(CacheEntryOptions::for_service(["catalog:album", "catalog:album:by-id"])))
    .await
```

`load` queries the module's own pool; there is no repository between them.

## Keys

Nine colon-separated segments, in fixed order, with an empty segment for each
absent partition:

```
env : app : module : entity : version : tenant : locale : feature : discriminator
development:modularmonolith.api:catalog:album:v1::::by-id:1
```

Two details of the original are preserved because they change what a key *is*:

- **The discriminator is not lowercased.** Every other segment is.
- **The `env` and `app` segments come from configuration with their own
  fallbacks** — `ASPNETCORE_ENVIRONMENT` then `DOTNET_ENVIRONMENT` then
  `"prod"`, and `ServiceName` then `"mmapi"`. Those defaults are independent of
  the host's resolved environment and of the default service name, so an
  unconfigured host composes keys under `prod:mmapi:` while reporting
  `Development` and `ModularMonolith.Api` everywhere else.

Tenant, locale and feature exist and are never populated: every service calls
the four-argument form. The caches are per module, not per tenant.

## Lifetimes

Twenty minutes, hard-coded in every service, with ±10% jitter so entries
written together do not expire together. `Caching:DefaultTTLSeconds` (300)
applies only when a caller passes no lifetime, which none do — in the original
as here. Track sales and customer purchases are not cached at all: their
counters move with every event.

## Invalidation, and the one real divergence

This is the interesting part.

The C# `CompositeCacheFacade.RemoveByTagAsync` is a **no-op**:

```csharp
public Task RemoveByTagAsync(string tag, CancellationToken ct = default)
{
    // Minimal implementation: tagging not supported natively without an index.
    return Task.CompletedTask;
}
```

Every service write calls it, and the documentation describes tag-based
eviction as a working mechanism. It is not. The consequence is that after a
write, only the single `by-id` entry is genuinely dropped — the `all` entry and
every `by-*` collection keep serving stale data for the rest of their twenty
minutes.

**This port keeps a real tag index**, so a write invalidates what it claims to.
Recorded as F1 in [the translation plan](rust-translation-plan.md), and
confirmed against the running original: with both services warmed and then
given the same create, the original answered 28 genres and this port answered
29.

A read here can therefore be *fresher* than the original's. It can never be
staler, and no client can depend on being served data that a write has already
superseded.

One smaller divergence in the same file. The C# single-flight removes each
per-key semaphore from its dictionary inside the same `finally` that releases
it, so concurrent callers can end up waiting on different semaphore instances
and more than one runs the factory. Here the coalescing is the cache's own and
is exact. That only ever reduces duplicate work (F7).

## Failures

`get_or_add` takes a factory returning `Option<T>`, which cannot distinguish
"not found" from "the database is down" — a failure would read as an empty
collection. `try_get_or_add` carries the error out instead, so a database
failure becomes a 500 the way an unhandled exception does in the original.
Nothing is cached when the factory fails, and `None` is never cached either: a
404 must not pin a negative result for twenty minutes.

The failure reaches **every** caller that coalesced onto that factory call, not
just the one that ran it. Both "the factory failed" and "the factory found
nothing" travel as the cache's own error, because both mean the same thing to
it — do not store this — and `try_get_with` hands its error to everyone waiting
on the key. An earlier version parked the error in a `Mutex` beside the call,
where only the caller that ran the factory looked: the rest were handed the
cache's `None`, which the services turn into a 404 or an empty array. A database
outage read as *missing data* for every concurrent request but one.

## Configuration

The original slimmed `CacheOptions` in its bulkhead phase, and so did this
port: the members nothing read (`SWR`, `Stampede`, `Partitioning`,
`PerModule`, `Redis`) are gone.

| Key | Default | Read? |
|---|---|---|
| `Caching:Enabled` | `true` | yes |
| `Caching:Tier` | `L1` | yes; `L1L2` is recognized and has no second tier to use |
| `Caching:Provider` | `InMemory` | no |
| `Caching:DefaultTTLSeconds` | `300` | only when a caller passes no lifetime, which none do |
| `Caching:Modules:Catalog:SizeLimit` | `1000` | yes |
| `Caching:Modules:Orders:SizeLimit` | `1000` | yes |
| `Caching:Modules:Administration:SizeLimit` | `500` | yes |
