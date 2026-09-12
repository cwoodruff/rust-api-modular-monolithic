# Parity

How this port is checked against the original, what "equivalent" was taken to
mean, and where the two deliberately differ.

## Running it

```sh
git clone https://github.com/cwoodruff/aspnetcore-min-api-modular-monolithic
tools/run-parity.sh aspnetcore-min-api-modular-monolithic
```

That builds both, starts them on separate ports, gives each **its own copy** of
`data/chinook.db` (the write cases insert rows, and identical copies keep the
generated keys lined up), seeds both with the same four logins, sweeps every
case, and stops them. Without the original checked out or the .NET SDK
installed it explains what it needs and exits 0 — a machine that cannot run the
check is not a failing machine.

`.github/workflows/parity.yml` runs the same script on every push.

## What is compared

98 cases: every route, the authorization matrix, routing failures, the identity
flows, and the whole write surface. For each one:

- the **status code**
- nine **headers** — content type, location, the auth challenge, and all six
  security headers
- the **body**, parsed as JSON, so member order is not treated as a difference

Member order genuinely differs: System.Text.Json writes base-class members
last, so `Id` lands at the end of an API model, while serde writes fields in
declaration order. JSON objects are unordered, and no client can depend on it.

## What is not compared

Five members cannot match by construction, and are scrubbed rather than excused
case by case:

| Member | Why |
|---|---|
| `timestampUtc` | a wall clock reading |
| `traceId` | a per-request correlation id |
| `version` | the .NET informational version embeds the build's commit hash |
| `kid`, `n` | JWKS key material; each host generates its own signing key |

`access_token`, `refresh_token` and `expires_at_utc` are scrubbed for the same
reason — their values are per-request. Their shapes are pinned by the
integration tests instead.

## Pacing

Every endpoint in the application shares one rate-limit budget — 60 requests
per 60 seconds per client — so a sweep this size trips it. The runner paces
itself around the window rather than letting a 429 masquerade as a difference.

That is worth knowing independently of the port: **any** client doing a broad
sweep of this API gets throttled, including a test suite, a crawler or an
aggressive monitoring probe.

## Accepted divergences

Four cases differ on purpose. The harness reports them as `known`, and the run
still passes.

### The original loses its security headers on error responses

Three validation-failure cases. The original answers them with **no security
headers at all** — no `nosniff`, no CSP, none of the six.

The cause: its security-headers middleware sets them *before* calling the rest
of the pipeline, and `UseExceptionHandler` clears the response before writing
the problem document, which wipes them. This port sets them on the way out, so
an error response keeps them.

Not reproduced. Losing `nosniff` and the CSP on an error response is a real
weakness, and no client depends on a header being *absent*. **This looks like a
genuine bug in the original worth reporting upstream** — setting the headers in
an `OnStarting` callback rather than before `next()` would fix it there.

### A malformed-JSON `detail` names framework internals

The `type`, `title` and `status` match. The `detail` is the parser's own
diagnostic, and .NET's reads:

```
Failed to read parameter "CreateGenreRequest request" from the request body as JSON.
```

Matching that would mean hardcoding a C# type and parameter name into a Rust
handler. The message is a human-readable diagnostic, not a contract.

## Deliberate fixes

Behaviors this port changes on purpose, each one either invisible on the wire
or visible only as a *more correct* answer. The full list with rationale is in
[the translation plan](rust-translation-plan.md); the ones with observable
effects:

| | Change |
|---|---|
| F1 | **Tag-based cache invalidation works.** The C# `RemoveByTagAsync` is a no-op, so a write leaves every collection entry stale for twenty minutes. Confirmed live: after the same create, the original served 28 genres and this port served 29. A read here can be fresher, never staler. |
| F3 | The OpenAPI document omits the `X-API-Key` security scheme the original references without ever defining. |
| F6 | The development signing key is generated on first run and gitignored, rather than committed. The C# repository has a real RSA private key in git. |
| F7 | Single-flight is exact rather than best-effort. Only ever reduces duplicate work. |
| F8 | The in-process HTTPS redirect is left to the edge. It has no forwarded-headers configuration, so it redirect-loops behind a TLS terminator — and it is why the original's container cannot serve traffic. HSTS itself is reproduced. |
| F9 | The data-health probe logs why it failed instead of discarding the reason. Same answer on the wire. |
| — | The Dockerfile produces a container that runs. The original's builds `net10.0` with the .NET 9 SDK, has no `.dockerignore`, and would exit at startup even if it built. |

## Quirks preserved

The other direction matters as much. These all look like bugs, are all
wire-visible, and are all reproduced:

- `GET /api/admin/customers/{id}` returns nine columns as `null` — company,
  address, city, state, country, postal code, phone, fax, email — because the
  C# projection has a `// ... other fields ...` comment where they belong. The
  *collection* route returns them populated. The same customer has two shapes.
- `GET /api/admin/employees/{id}/reports-to` returns the employee, not their
  manager: the service passes the route id straight to a plain `FindAsync`.
- `GET /api/orders/invoice-lines/{id}` serves an *entity*, so its response
  carries `Invoice` and `Track` members that are always null. It is the only
  endpoint in the application that does this.
- A genre's `Tracks` list is always empty, even for a genre with 1,297 tracks.
- `PUT /api/admin/genres/{id}` answers lowercase `{"id":…,"name":…}` while
  `POST` answers a PascalCase model.
- JWKS lives under `/api/identity/.well-known/jwks.json`, not at the root.
- The tenant guard succeeds when no `X-Tenant-Id` header is sent.
- Collection endpoints declare a 404 they can never return, and return every
  row — 3,503 tracks — because nothing pages.
- Four rate-limit policy names and four permissions are declared and reach
  nothing.

## Data

Both services must read the same data, and the C# repository ships **two**
copies of `data/chinook.db`:

| Copy | Genres | Genre 1 | State |
|---|---|---|---|
| `src/ModularMonolith.Api/data/chinook.db` | 25 | `Rock` | stock Chinook, clean |
| `data/chinook.db` (repository root) | 34 | `Updated_<guid>` | carries eight rows left by a previous test run |

The host reads the first — `TryFindDb(builder.Environment.ContentRootPath)` —
and never opens the second. Every table but `Genre` is identical between them.

This repository bundles the copy the host actually reads, byte for byte.
Bundling the other one was a real bug here, and finding it took the parity diff
from 22/26 to 45/45.
