# Architecture

**Status: implemented.** This describes what is in the repository, not a plan.

A port of [aspnetcore-min-api-modular-monolithic](https://github.com/cwoodruff/aspnetcore-min-api-modular-monolithic).
Where this document says "the original", it means that.

## The shape

One host, five modules, one SQLite database. Requests flow
**endpoint → service → repository → database**, and an endpoint never touches
a repository directly — the same layering the original enforces by convention.

```
                      ┌──────────────────────────────┐
   request ─────────► │ api (host)                   │
                      │  HSTS, errors, status pages, │
                      │  security headers, CORS,     │
                      │  rate limit, authentication  │
                      └──────────────┬───────────────┘
                                     │  AppState
        ┌──────────────┬─────────────┼─────────────┬──────────────┐
        ▼              ▼             ▼             ▼              ▼
   module-admin  module-identity module-music module-orders module-reporting
   /api/admin    /api/identity   /api/music   /api/orders   /api/reporting
        └──────────────┴─────────────┼─────────────┴──────────────┘
                                     ▼
                         shared-persistence (traits, shapes)
                                     ▼
                         shared-data-sqlite (sqlx)   ◄── only the host binds this
                                     ▼
                              data/chinook.db
```

## Crates, and why the boundaries hold

| Crate | Ports | Depends on |
|---|---|---|
| `shared-kernel` | `SharedKernel` | nothing in the workspace |
| `shared-persistence` | `SharedKernel.Persistence` | `shared-kernel` |
| `shared-data-sqlite` | `SharedKernel.DataSQLite` | `shared-persistence`, `shared-kernel` |
| `module-*` | the five module projects | `shared-kernel`, `shared-persistence` |
| `api` | `ModularMonolith.Api` | everything |
| `architecture-tests` | `ModularMonolith.Architecture.Tests` | — |

Three rules, and the reason each exists:

1. **A module never depends on another module.** Modules talk through the
   shared kernel or not at all. The original enforced this with ArchUnitNET
   assertions over assembly references.
2. **The shared crates never depend on a module.** They are the leaves.
3. **Only the host binds a concrete database driver.** Modules see repository
   *traits*; `sqlx` appears in exactly one crate plus the composition root.

`architecture-tests` reads the real graph from `cargo metadata` and asserts all
three. The rules are also structural — a forbidden edge is a compile error long
before the test runs — so the test exists to give a *readable* failure and to
catch a dependency added without thought.

The fourth C# rule, `PublicSurfaceTests` (a module assembly exports only its
module type), has no runtime equivalent here: Rust visibility enforces it at
compile time. Everything but each module's entry point is `pub(crate)`.

## Where things live, and one place they moved

Two pieces sit somewhere other than their C# counterpart, both deliberately:

**Authorization guards are in `shared-kernel`, not the Identity module.** In the
original, `Permissions` and `PolicyRegistry` live inside `Identity.Module` and
every other module refers to the policies by raw string, because a module may
not reference another module. The C# source says what it would rather do:

> Note: In a later iteration, move these to SharedKernel to share constants
> across modules.

That is the move. `ClaimsPrincipal` is a framework type and this is the
framework crate, so Identity *produces* an `AuthenticatedUser` and every other
module *consumes* one, with neither depending on the other.

**Health endpoints are defined once.** The original carries ten near-identical
files — a health and a data-health handler per module, differing only in a
name. One route builder in `shared-kernel` serves all five. The responses are
identical; only the duplication is gone.

## What the DI container became

The original resolves ten repositories, four caches worth of infrastructure and
fourteen services through `IServiceCollection`. There is no container here.
`AppState` holds the same things behind `Arc`s, the host builds it once, and
axum hands out clones.

`IModule` lost one of its three members in the process. `RegisterServices`
exists to populate a container; with no container there is nothing for it to
do, so the trait keeps identity and routes:

```rust
pub trait Module<S> {
    fn name(&self) -> &'static str;
    fn prefix(&self) -> &'static str;
    fn router(&self) -> Router<S>;
}
```

Module discovery is a hard-coded list in both code bases. The original returns
a literal array from `GetModules()`; `api::app::registry()` returns a literal
`Vec`. Adding a module means editing one function, on purpose.

## The middleware pipeline

Layer order is the original's, read top to bottom. axum applies layers
outermost-first, so `app.rs` reads in reverse of this list:

1. HSTS — non-Development only
2. panic → RFC 7807, where `UseExceptionHandler` sits
3. status code pages
4. `charset=utf-8` on JSON responses
5. the six security headers
6. CORS
7. rate limiting
8. authentication
9. routes

One consequence is worth knowing rather than discovering: **the rate limiter
runs before authentication**, so no request has claims when its partition key
is computed, and every caller is bucketed by address. That is the original's
ordering, kept.

## Verification

`tools/run-parity.sh` runs this port and the original side by side and compares
98 cases — every route, the authorization matrix, routing failures, the identity
flows and the write surface — across status codes, nine headers and parsed
bodies. See [parity.md](parity.md).
