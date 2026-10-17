# Parity

How this port is checked against the original, what "equivalent" was taken to
mean, and where the two deliberately differ.

## Running it

```sh
git clone https://github.com/cwoodruff/aspnetcore-min-api-modular-monolithic
tools/run-parity.sh aspnetcore-min-api-modular-monolithic
```

That starts one throwaway `postgres:17-alpine` container, with
`docker/postgres-init` mounted so the read-only `reporting` login exists, and
creates two databases in it, `original` and `port`. It builds both services and
starts them in Development on separate ports, each pointed at its own database
through `ConnectionStrings__AppDatabase` and `ConnectionStrings__Reporting`.
Each migrates and seeds its own database on startup from the same
`data/chinook-postgres-seed.sql`, so the two start identical. The write cases
insert rows, and identical starting points keep the generated keys lined up.
Both get the same four logins through `Identity__InMemoryUsers__*`. The script
sweeps every case, then stops both services and removes the container.

Without the original checked out, the .NET SDK or Docker, it explains what it
needs and exits 0: a machine that cannot run the check is not a failing
machine. `ORIGINAL_PORT` and `PORT_PORT` move the two services off 5043 and
5055. `PARITY_SHOW_LOGS=1` prints both services' logs after a failed run.

`.github/workflows/parity.yml` runs the same script on every push.

## What is compared

134 cases:

- **Every route**: the five modules' health and data-health, root, `/healthz`, JWKS, every read.
- **Absent ids and empty collections**, including track sales, customer purchases and an invoice's report lines.
- **The authorization matrix**: anonymous callers, a missing permission, a missing role, a missing tenant claim, a matching and a mismatched tenant header.
- **Routing failures**: an unknown route, a wrong method, a route constraint (`/albums/abc`, a dead-letter id that is not a GUID), and the deleted `tracks/invoice/{id}` route.
- **The identity flows**, including blank and missing members, which the validation filter refuses.
- **The event flow**:
  - track sales and purchases read before an invoice is finalized;
  - the finalize itself (202, `Location`, body);
  - finalizing again (409), a missing invoice, and a caller without `orders.write`;
  - after both outbox dispatchers have had three seconds to deliver: the invoice, the two read models, the dead letters, and a retry of an unknown one.
- **Reporting**: sales by genre, an invoice's lines with names, an integrity run, open findings, and refusals for callers without the role or permission.
- **The whole Genre write surface.**
- **Swagger**: UI and all six documents, by status only, since each framework generates its own document.

For each case the harness compares:

- the **status code**;
- nine **headers**: content type, location, the auth challenge, and all six security headers;
- the **body**, parsed as JSON, so member order is not treated as a difference.

Member order does differ in places. JSON objects are unordered, and no client
can depend on it.

One case, `/api/catalog/tracks/playlist/1`, compares its rows sorted by `Id`.
Upstream joins `PlaylistTrack` to `Track` with no `ORDER BY`, and the order the
original answers in follows the planner's choice of join, which turns on table
statistics: CI saw `PlaylistTrack` order where a local run saw `Track` order.
No fixed order could match both, so the case checks the same rows in any order.

## Result

**133 of 134 identical, one known divergence, no unexplained differences.**

The first run against the PostgreSQL original found three real bugs in the
port. None of them showed up in the test suites, because each suite asserted
what the port did rather than what the original does:

1. **Collections came back in the wrong order.** Upstream's list queries have
   no `ORDER BY`, so PostgreSQL answers them in heap order. The seeded tracks
   are not stored in `Id` order: element 1525 of `/api/catalog/tracks` is
   track 1679 in the original, but was 1526 here. List queries now order by
   `ctid`, the physical order, in every module. By-id projections keep the
   explicit order upstream gives them.
2. **A total with nothing added read `0.0`.** .NET writes `0m` as `0`, and the
   workspace's decimal serialization writes every decimal as a float.
   `shared_kernel::json::dotnet::decimal` writes a scale-zero decimal as an
   integer. `GET /api/admin/customers/{id}/purchases` uses it.
3. **The port could not start at all for anyone who had used
   `dotnet user-secrets`.** `secrets.json` is written with a UTF-8 byte-order
   mark, which .NET's JSON provider skips and serde_json refused. The config
   loader now strips it.

## What is not compared

Several members cannot match by construction. They are scrubbed everywhere
rather than excused case by case:

| Member | Why |
|---|---|
| `timestampUtc` | a wall-clock reading |
| `traceId` | a per-request correlation id |
| `version` | the .NET informational version embeds the build's commit hash |
| `kid`, `n` | JWKS key material; each host generates its own signing key |
| `RanAt`, `DetectedAt` | when an integrity run happened |
| `totalDurationMs` | how long `/healthz` took |

`access_token`, `refresh_token` and `expires_at_utc` are scrubbed for the same
reason: their values differ per request. The integration tests pin their
shapes instead.

## Pacing

Each module group has its own rate-limit budget of 60 requests per 60 seconds
per client, and the root endpoint has another (ADR-0013). A sweep this size
trips them. The runner counts requests per policy and waits out the window
before any one budget runs dry, rather than letting a 429 look like a
difference. `/healthz`, Swagger and unmatched routes are not limited.

## Accepted divergences

One case differs on purpose. The harness reports it as `known`, and the run
still passes.

### A malformed-JSON `detail` names framework internals

The `type`, `title` and `status` match. The `detail` is the parser's own
diagnostic, and .NET's reads:

```
Failed to read parameter "CreateGenreRequest request" from the request body as JSON.
```

Matching it would mean hardcoding a C# type and parameter name into a Rust
handler. The message is a human-readable diagnostic, not a contract.

### No longer a divergence: security headers on error responses

The earlier port of the original answered validation failures with no security
headers at all, and this harness listed three cases as known for it. Upstream
fixed this in 09a4c25 by setting the headers in `Response.OnStarting`. Those
cases now match.

## Deliberate fixes

These are behaviors the port changes on purpose. Each is either invisible on
the wire or visible only as a more correct answer. The full list with
rationale is in [the translation plan](rust-translation-plan.md) and
[the upgrade plan](upgrade-plan.md); the ones with observable effects:

| | Change |
|---|---|
| F1 | **Tag-based cache invalidation works.** Upstream's `RemoveByTagAsync` is still a no-op, so a write leaves every collection entry stale for twenty minutes. A read here can be fresher, never staler. The sweep reads every collection before it writes, so it does not observe the difference. |
| F3 | The OpenAPI documents omit the `X-API-Key` security requirement upstream references without defining. |
| F6 | The development signing key is generated on first run and gitignored rather than committed. |
| F8 | The in-process HTTPS redirect is left to the edge; HSTS is reproduced. |
| F14 | Every response carries an `x-trace-id` header. It is additive, and the harness does not compare it. |
| F19 | A full module cache evicts an old entry where .NET's `MemoryCache` refuses the new one. Either way the module stays within its share, and no response differs. |

## Quirks preserved

These look like bugs, are wire-visible, and are reproduced:

- `GET /api/admin/customers/{id}` returns nine columns as `null`, and the collection routes return `SupportRepId: null`.
- `GET /api/orders/invoice-lines/{id}` carries an `Invoice` member that is always null.
- `PUT /api/admin/genres/{id}` answers lowercase `{"id":…,"name":…}`, while `POST` answers a PascalCase model.
- The finalize body is camelCase, while every other model is PascalCase.
- `GET /api/reporting/invoices/{id}/lines` answers 404 for an invoice with no lines, not only for a missing one.
- JWKS lives under `/api/identity/.well-known/jwks.json`, not at the root.
- The tenant policy succeeds when no `X-Tenant-Id` header is sent.
- Collections return every row, 3,503 tracks included, because nothing pages.
- Rate limits partition by address: the limiter runs before authentication.
