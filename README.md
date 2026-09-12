# rust-api-modular-monolithic

A Rust port of [aspnetcore-min-api-modular-monolithic](https://github.com/cwoodruff/aspnetcore-min-api-modular-monolithic)
— an ASP.NET Core 10 Minimal API modular monolith over the Chinook database —
built to serve the same 47 routes with the same wire behavior.

**Complete and verified.** Every route is implemented and checked against the
running original across 98 cases — every route, the authorization matrix,
routing failures, the identity flows and the write surface — comparing status
codes, nine headers and parsed bodies. Zero unexplained differences. Four cases
differ on purpose; [docs/parity.md](docs/parity.md) says which and why.

## Quick start

```sh
# Serves on http://localhost:5043. Run from the repository root so the host
# finds appsettings.json and data/chinook.db.
ASPNETCORE_ENVIRONMENT=Development cargo run -p api

curl -s localhost:5043/api/music/data-health
```

```json
{"module":"Music","status":"Data-Healthy","timestampUtc":"2026-09-12T00:04:16.7824499Z",
 "environment":"Development","version":"1.0.0","service":"ModularMonolith.Api",
 "database":{"connected":true}}
```

The data routes need a token, and there are no seeded users — the original
ships an empty list and expects secrets to fill it, and so does this. To get
in, supply a login. Logins are read **at startup**, so stop the server you just
started and hand the variables to the new process:

```sh
export ASPNETCORE_ENVIRONMENT=Development
export Identity__InMemoryUsers__0__Username=demo
export Identity__InMemoryUsers__0__Password=secret123
export Identity__InMemoryUsers__0__UserId=user-1
export Identity__InMemoryUsers__0__Tenant=tenant-1
export Identity__InMemoryUsers__0__Permissions__0=music.read
cargo run -p api
```

It logs what it loaded, which is worth a look before reaching for curl:

```
INFO module_identity::stores: effective in-memory login username=demo user_id=user-1 …
INFO module_identity::stores: loaded in-memory logins count=1
```

If instead it says `WARN no usable in-memory logins are configured`, the
variables did not reach the process — a different terminal, an IDE run
configuration that does not inherit your shell, or an entry missing
`Username`, `Password` or `UserId`, any one of which drops that login rather
than failing startup. Every login attempt then answers 401, the same 401 a
wrong password gets. `Roles`, `DisplayName` and `Email` are optional.

```sh
TOKEN=$(curl -s -X POST localhost:5043/api/identity/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"demo","password":"secret123"}' | jq -r .access_token)

curl -s -H "Authorization: Bearer $TOKEN" localhost:5043/api/music/albums/1
```

## Build and test

```sh
cargo build --workspace
cargo test --workspace          # 332 tests

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Check it against the original

```sh
git clone https://github.com/cwoodruff/aspnetcore-min-api-modular-monolithic
tools/run-parity.sh aspnetcore-min-api-modular-monolithic
```

Builds both, starts them side by side with the same seeded logins and a
database copy each, sweeps every case, and stops them. Needs the .NET SDK;
without it the script says so and exits cleanly. See
[docs/parity.md](docs/parity.md).

## Docker

```sh
docker build -t modular-monolith-api .
docker run -p 8080:8080 -e ASPNETCORE_ENVIRONMENT=Demo modular-monolith-api
```

Roughly 170 MB as `docker images` reports it — an 8 MB binary on
`debian:bookworm-slim` — running as a non-root user, with a healthcheck.

The environment is **not** defaulted. Development and Demo unlock Swagger, the
in-memory login store and the development signing key, and an image that
quietly enables all three is the wrong default. Left unset the host runs as
Production and exits at startup naming what to configure — which is the
original's behavior too, minus the mystery. A production container needs a real
key provider:

```sh
-e Jwt__KeyProvider=KeyVault -e Jwt__KeyVaultVaultUri=... -e Jwt__KeyVaultKeyName=...
```

## Layout

Each crate maps to one project in the C# solution, and the dependency edges
between them are the architecture rules — `cargo` enforces what ArchUnitNET
enforced.

| Crate | Ports |
|---|---|
| `shared-kernel` | `SharedKernel` — module contract, config, errors, caching, auth guards, health |
| `shared-persistence` | `SharedKernel.Persistence` — entities, API models, validators, repository traits, state |
| `shared-data-sqlite` | `SharedKernel.DataSQLite` — sqlx repository implementations |
| `api` | `ModularMonolith.Api` — the host that composes every module |
| `module-identity` | `Identity.Module` — tokens, keys, stores |
| `module-music` | `Music.Module` — albums, artists, playlists, tracks |
| `module-orders` | `Orders.Module` — invoices, invoice lines |
| `module-admin` | `Admin.Module` — customers, employees, genres, media types |
| `module-reporting` | `Reporting.Module` — health endpoints only, as in the original |
| `architecture-tests` | `ModularMonolith.Architecture.Tests` |

A module crate may depend only on `shared-kernel` and `shared-persistence` —
never on another module, never on `shared-data-sqlite`. Only the host binds a
concrete database driver.

## Documentation

| | |
|---|---|
| [rust-translation-plan.md](docs/rust-translation-plan.md) | The plan, the endpoint inventory, and every preserve-versus-fix decision |
| [architecture.md](docs/architecture.md) | Crate layout, layering rules, middleware order |
| [authn-authz.md](docs/authn-authz.md) | Tokens, claims, policies, the tenant guard |
| [caching.md](docs/caching.md) | Keys, lifetimes, and the invalidation fix |
| [parity.md](docs/parity.md) | How parity is verified, and where the two deliberately differ |

## Data

`data/chinook.db` is bundled, as it is in the original — no migrations, no
seeding step. The host resolves it from `ConnectionStrings:AppDatabase` (or
`CONNECTIONSTRINGS__APPDATABASE`), falling back to a walk up the directory tree.

Note the original ships *two* copies of this file — its host reads the clean
one under its content root, and this repository bundles that copy byte for
byte. See [docs/parity.md](docs/parity.md).

## Configuration

Keys keep their original names, so existing `appsettings.json` files and
environment variables carry over unchanged: `ServiceName`,
`ConnectionStrings:AppDatabase`, `Jwt:*`, `Identity:InMemoryUsers`, and
`ASPNETCORE_ENVIRONMENT` — where `Development` and `Demo` both unlock Swagger,
health metadata, and the development signing-key provider.

Environment variables use the `__` separator and index arrays numerically, the
way ASP.NET does: `Identity__InMemoryUsers__0__Username`.

Unlike the original, the development RSA signing key is generated on first run
and gitignored rather than committed.

## License

MIT — see [LICENSE](LICENSE).
