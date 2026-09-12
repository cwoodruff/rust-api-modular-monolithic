# Caching

**Status: implemented, L1 only.** The second tier is not built, which matches
the original — it defaults to `L1` and ships no `Caching` section at all, so
the L1 path is the one that actually runs there too.

## The shape

Every read in every service is cache-aside: compose a key, read through the
cache, fall back to the repository on a miss. Nothing about the HTTP layer
caches — there is no output caching, no `ETag`, no `Cache-Control`.

```rust
let key = state.cache_keys.compose("music", "album", "v1", &format!("by-id:{id}"));

state.cache.try_get_or_add(
    &key,
    || async { state.repositories.albums.get_by_id(id).await },
    Some(CacheEntryOptions::for_service(["music:album", "music:album:by-id"])),
).await
```

## Keys

Nine colon-separated segments, in fixed order, with an empty segment for each
absent partition:

```
env : app : module : entity : version : tenant : locale : feature : discriminator
development:modularmonolith.api:music:album:v1::::by-id:1
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
the four-argument form. The caches are global, not per-tenant, despite
`Caching:Partitioning:TenantAware` existing in the options.

## Lifetimes

Twenty minutes, hard-coded in every service, with ±10% jitter so entries
written together do not expire together. `Caching:DefaultTTLSeconds` (300) and
the per-module values under `Caching:PerModule` are read into the options and
then never consulted — in the original as here.

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
collection. `try_get_or_add` carries the error out instead, so a repository
failure becomes a 500 the way an unhandled exception does in the original.
Nothing is cached when the factory fails, and `None` is never cached either: a
404 must not pin a negative result for twenty minutes.

## Configuration

The full `Caching` section binds, including the members nothing reads, so an
existing configuration file carries over unchanged. Defaults, which are what
actually run:

| Key | Default | Read? |
|---|---|---|
| `Caching:Enabled` | `true` | yes |
| `Caching:Tier` | `L1` | yes |
| `Caching:Provider` | `InMemory` | no |
| `Caching:DefaultTTLSeconds` | `300` | only when a caller passes no lifetime, which none do |
| `Caching:SWR:*` | off | no |
| `Caching:Stampede:SingleFlight` | `true` | no — single flight is unconditional |
| `Caching:Partitioning:*` | off | no |
| `Caching:PerModule:*` | unset | no |
| `Caching:Redis:*` | — | no |
