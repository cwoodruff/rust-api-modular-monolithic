Ports upstream ADR-0019.

# 0019. Dependencies added during the upgrade that no step listed

- Status: Accepted
- Date: 2026-10-16
- Phase: after upstream phase 9; port step 8 (see `docs/upgrade-plan.md`, section 5)

## Context

The original's record lists NuGet packages and a local tool that its upgrade
added without any phase naming them. None of them exist in this port: there
is no EF Core, no `WebApplicationFactory`, no `JwtBearer` and no `dotnet-ef`.
This record is the Rust counterpart, not a translation of the upstream
table. It answers the same question for this repository: which crates came
in during the upgrade without a step of `docs/upgrade-plan.md` naming them.

The plan's section 3 names mechanisms (a pool per module, a bounded channel,
a test container, a JWKS fetch), not crates or features. So the baseline is
the workspace's `[workspace.dependencies]` at the start of the upgrade
(commit `e9c9764`), and every crate or feature added to it since is listed
here. Versions are not repeated; the workspace `Cargo.toml` holds them.

## Decision

These stay, for the reasons given:

| Dependency | Where | Status against the baseline | Why |
|---|---|---|---|
| `sqlx` features `postgres` and `migrate` (and `uuid`, to bind event ids), replacing `sqlite` | every module crate, `shared-kernel` (`persistence` and `events` only, ADR-0012) | Existing crate, features changed | PostgreSQL (ADR-0002) and per-crate `sqlx::migrate!` with a per-schema `_sqlx_migrations` table (ADR-0003). |
| `tokio` features `sync` and `time` | `shared-kernel`, modules | Existing crate, features added | `mpsc` and `Semaphore` for `ModuleWorkQueue` and `ModuleGate` (ADR-0013); intervals and sleeps for the dispatcher and the integrity job (ADR-0008, ADR-0015). |
| `tokio-util` | `shared-kernel`, `samples/catalog-host` | New | `CancellationToken` for hosted-task shutdown, including the queue's own shutdown token that work receives (ADR-0013). Same maintainers as `tokio`. |
| `uuid` with `serde` | `shared-kernel`, `orders-contracts` | Existing crate (Identity used it), feature added and used in more crates | Outbox and inbox event ids, and the id inside `InvoiceFinalized` (ADR-0008). |
| `reqwest` (rustls, JSON) | `samples/catalog-host` | New | Fetches Identity's JWKS over HTTP for `jsonwebtoken::jwk::JwkSet` (ADR-0020). Only the sample links it. |

## Consistency

Not a boundary decision.

## Consequences

- Every crate and feature in the workspace is either at baseline, required
  by a step of the plan, or listed here. A later addition needs a row here
  or its own record.
- `reqwest` is confined to the sample, which the monolith's binary does not
  depend on; a test in `architecture-tests` keeps `crates/api` and the
  modules free of it, and of any container runner.
- The plan named `testcontainers`, and it was used at first. It was removed:
  a container per test process, parked in a `static` that never drops, was
  never stopped, and runs left dozens behind. `test-support` now starts one
  named container through the Docker CLI and every process shares it, which
  needs no crate at all.
- `tokio-util` must move in step with `tokio`; a mismatch shows up as a
  compile error, not at runtime.

## How to reverse

Remove the dependency and the feature that needs it: for `reqwest`, the
sample's remote identity (or the sample); for `tokio-util`, a hand-written shutdown
signal; for the `sqlx` and `tokio` features, the PostgreSQL and bulkhead
decisions themselves.
