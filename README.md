# rust-api-modular-monolithic

A Rust port of [aspnetcore-min-api-modular-monolithic](https://github.com/cwoodruff/aspnetcore-min-api-modular-monolithic)
— an ASP.NET Core 10 Minimal API modular monolith over the Chinook database —
built to expose the same 47 routes with the same wire behavior.

The full translation plan, including the endpoint inventory and the
preserve-versus-fix decisions that define what "equivalent" means here, is in
[`docs/rust-translation-plan.md`](docs/rust-translation-plan.md).

## Status

**Phase 6 of 9 — Music and Orders are live.** 37 of the 47 routes are
implemented, and **every one of them has been diffed byte-for-byte against the
running C# original**: `tools/parity-diff.py` reports 45/45 routes identical,
ignoring only members that cannot match by construction (timestamps, trace
identifiers, the build's version string, and each host's own signing key).

Under it: `shared-kernel` carries the cross-cutting machinery (configuration,
environment gating, RFC 7807 errors, the cache facade, rate-limit partitioning,
authorization guards, the module contract, health payloads);
`shared-persistence` carries the domain (entities, API models, validators,
repository traits, application state); `shared-data-sqlite` implements all ten
repositories in sqlx; and Identity issues RS256 tokens whose claims match the
original's exactly.

Next: Administration and its Genre write endpoints in Phase 7. See the phase
table in the plan.

```console
$ curl -s localhost:5043/api/music/data-health
{"module":"Music","status":"Data-Healthy","timestampUtc":"2026-09-11T18:14:33.6630810Z",
 "environment":"Development","version":"1.0.0","service":"ModularMonolith.Api",
 "database":{"connected":true}}
```

## Layout

Each crate maps to one project in the C# solution, and the dependency edges
between them are the architecture rules — `cargo` enforces what ArchUnitNET
enforced.

| Crate | Ports | Phase |
|---|---|---|
| `shared-kernel` | `SharedKernel` — module contract, config, errors, caching, traffic control | 1 |
| `shared-persistence` | `SharedKernel.Persistence` — entities, API models, validators, repository traits | 2 |
| `shared-data-sqlite` | `SharedKernel.DataSQLite` — sqlx repository implementations | 3 |
| `api` | `ModularMonolith.Api` — the host that composes every module | 4 |
| `module-identity` | `Identity.Module` — tokens, keys, stores, policies | 5 |
| `module-music` | `Music.Module` — albums, artists, playlists, tracks | 6 |
| `module-orders` | `Orders.Module` — invoices, invoice lines | 6 |
| `module-admin` | `Admin.Module` — customers, employees, genres, media types | 7 |
| `module-reporting` | `Reporting.Module` — health endpoints only, as in the original | 7 |
| `architecture-tests` | `ModularMonolith.Architecture.Tests` | 0 |

A module crate may depend only on `shared-kernel` and `shared-persistence` —
never on another module, and never on `shared-data-sqlite`. Only the host binds
a concrete database driver.

## Build and test

```sh
cargo build --workspace
cargo test --workspace

# Serves on http://localhost:5043. Run it from the repository root so the
# host finds appsettings.json and data/chinook.db.
ASPNETCORE_ENVIRONMENT=Development cargo run -p api
```

In `Development` or `Demo` the OpenAPI document is served at
`/swagger/v1/swagger.json`; outside them that path answers 404, as it does in
the original.

Lint and format the way CI does:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Data

`data/chinook.db` is bundled in the repository, as it is in the C# original —
there are no migrations and no seeding step. The host resolves it from the
`ConnectionStrings:AppDatabase` setting (or the `CONNECTIONSTRINGS__APPDATABASE`
environment variable), falling back to a walk up the directory tree looking for
`data/chinook.db`.

## Configuration

Configuration keys keep their original names so existing `appsettings.json`
files and environment variables carry over unchanged: `ServiceName`,
`ConnectionStrings:AppDatabase`, `Jwt:*`, `Identity:InMemoryUsers`, and
`ASPNETCORE_ENVIRONMENT` (where `Development` and `Demo` both unlock Swagger,
health metadata, and the development signing-key provider).

Like the original, this port ships **no seeded users** — supply them through
configuration or environment variables in Development or Demo. Unlike the
original, the development RSA signing key is generated on first run and is
gitignored rather than committed.

## License

MIT — see [LICENSE](LICENSE).
