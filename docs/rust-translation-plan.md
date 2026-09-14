# Rust Translation Plan — ASP.NET Core Minimal API Modular Monolith

**Source:** [cwoodruff/aspnetcore-min-api-modular-monolithic](https://github.com/cwoodruff/aspnetcore-min-api-modular-monolithic) (ASP.NET Core 10 Minimal APIs, Chinook SQLite)
**Target:** this repository — an equivalent Rust solution with the same functions, features, and wire behavior.

---

## 1. What we are porting

The source is a modular monolith over the Chinook music-store database with **five modules** behind one host:

| Module | Prefix | Surface |
|---|---|---|
| Music | `/api/music` | 15 read endpoints (albums, artists, playlists, tracks + by-FK lookups) + health/data-health |
| Orders | `/api/orders` | 7 read endpoints (invoices, invoice-lines) + health/data-health |
| Administration | `/api/admin` | 12 read endpoints (customers, employees, genres, media-types) + **Genre POST/PUT/DELETE** (the only writes in the app) + health/data-health |
| Identity | `/api/identity` | login, refresh, logout, userinfo, JWKS + health/data-health |
| Reporting | `/api/reporting` | health/data-health only (placeholder module) |

Plus a root `GET /` metadata endpoint. **47 routes total.**

Cross-cutting features to reproduce:

- **Architecture pattern:** `IModule` contract (`Name`, `RegisterServices`, `MapEndpoints`); host composes a hard-coded module list. Endpoint → Service → Repository → DB layering; endpoints never touch repositories.
- **AuthN:** JWT Bearer, **RS256** (RSA-2048), 15-min access tokens, 7-day opaque refresh tokens with **rotation** and permission re-read on refresh, JWKS published at `/api/identity/.well-known/jwks.json`, dev signing key persisted to a JSON file, Key Vault provider as the production option. In-memory user store, enabled only in Development/Demo (plaintext passwords from config), `DisabledUserStore` otherwise.
- **AuthZ:** policy registry — 8 permission policies (`music.read`, `music.write`, `orders.read`, `orders.write`, `admin.users.manage`, `administration.read`, `administration.write`, `report.view`), `role.admin`, and `tenant.scoped` (tenant claim vs `X-Tenant-Id` header). No global fallback — endpoints are anonymous unless marked. Admin endpoints stack role + permission + tenant.
- **Caching:** service-layer cache-aside via an `ICacheFacade` (L1 in-memory now, optional L2 later), composed keys `env:app:module:entity:version:tenant:locale:feature:discriminator`, 20-minute TTLs with ±10% jitter, per-key single-flight, tag-based invalidation API.
- **Rate limiting:** one policy `global:public-anon` — fixed window, 60 req/60 s, queue 0, 429 rejection — applied to **every** endpoint; partition key from `client_id`/`tenant`/`sub` claim falling back to client IP.
- **Validation:** FluentValidation-style validators per API model (exact lengths/regexes documented below), run **inside services**, surfaced as RFC 7807 `ValidationProblem` 400s.
- **Errors:** global handler → RFC 7807 ProblemDetails with a `traceId` extension; distinct handling for validation, malformed request/JSON, and unhandled 500s.
- **Security headers** on every response; **CORS** for localhost:3000/4200/5173; HSTS + HTTPS redirect outside Development; Swagger UI only in Development/Demo.
- **Health:** per-module `GET /health` (always 200) and `GET /data-health` (DB connectivity check; `Data-Healthy`/`Degraded`, still HTTP 200); extra metadata (env, version, service) only in Development/Demo.
- **Data:** bundled `chinook.db` SQLite file, no migrations, no seeding; connection string `AppDatabase` with a fallback directory-walk to find `data/chinook.db`.
- **Tests:** ~193 tests — HTTP integration tests, mocked service unit tests, and architecture tests enforcing module isolation, shared-kernel direction, and a frozen public surface per module.

---

## 2. Target workspace layout

Cargo workspace mirroring the .NET solution one-to-one. Crate dependency edges *are* the architecture rules — the compiler enforces what ArchUnitNET enforced.

```
rust-api-modular-monolithic/
├── Cargo.toml                     # [workspace]
├── data/chinook.db                # copied from source repo (bundled, like the original)
├── crates/
│   ├── shared-kernel/             # ← SharedKernel: Module trait, cache facade, key composer,
│   │                              #    build info, rate-limit partition keys, policy name constants
│   ├── shared-persistence/        # ← SharedKernel.Persistence: entities, API models, converters,
│   │                              #    validators, repository traits, pool/config helpers
│   ├── shared-data-sqlite/        # ← SharedKernel.DataSQLite: sqlx repository implementations
│   ├── module-admin/              # ← Admin.Module
│   ├── module-identity/           # ← Identity.Module (token service, key mgmt, stores, authz)
│   ├── module-music/              # ← Music.Module
│   ├── module-orders/             # ← Orders.Module
│   ├── module-reporting/          # ← Reporting.Module
│   └── api/                       # ← ModularMonolith.Api host binary
├── docs/                          # this plan + ported architecture docs
└── Dockerfile
```

Allowed dependency edges (enforced by `Cargo.toml`, checked in CI with `cargo deny`/a small `cargo metadata` test):

- `module-*` → `shared-kernel`, `shared-persistence` (never another `module-*`, never `shared-data-sqlite`)
- `shared-data-sqlite` → `shared-persistence`, `shared-kernel`
- `api` → everything (composition root; the only crate that sees `shared-data-sqlite` concretes)
- `shared-*` crates never depend on any module (shared kernel is a leaf)

The .NET `PublicSurfaceTests` (each module exports exactly its module type) translates to Rust visibility: each module crate exposes only `pub fn router(...)` / `pub struct <X>Module` and keeps endpoints, services, and internals `pub(crate)`.

---

## 3. Technology mapping

| ASP.NET Core concept | Rust choice | Notes |
|---|---|---|
| Minimal API host + routing | **axum** (tokio) | `Router::nest("/api/music", …)` ≈ `MapGroup` |
| Middleware pipeline | **tower** layers | Same ordering as `Program.cs` (see §5) |
| DI container | Explicit composition in `api` + `AppState` (`Arc<dyn Trait>`) | Repositories/services as trait objects; no DI framework needed |
| EF Core + SQLite | **sqlx** (sqlite, async, compile-time checked queries) | Source uses EF LINQ only — queries must be re-derived as SQL (§6). Pool ≈ `AddDbContextPool` |
| System.Text.Json | **serde / serde_json** | `#[serde(rename_all = "PascalCase")]` on API models; token/health payloads keep their literal lowercase/snake names |
| FluentValidation | Hand-rolled validator functions (or `garde` with custom rules) | FluentValidation semantics: rules like max-length/regex/email **pass on null** unless NotNull is chained — easiest to guarantee by hand |
| JWT Bearer + `TokenService` | **jsonwebtoken** (RS256) + **rsa** crate for keygen | Reuse the dev key JSON format `{Kid, PrivateKeyPkcs8Base64}` so an existing dev key file loads unchanged |
| Authorization policies | Per-route tower layers / extractor guards | `require_permission("music.read")`, `require_role("Admin")`, `tenant_scoped()` combinators |
| `IMemoryCache` + `ICacheFacade` | **moka** (async) behind our own `CacheFacade` trait | `get_with` gives real single-flight; implement key composer + jitter ourselves |
| `RateLimiter` fixed window | Small custom tower layer (`DashMap<partition, window>`) | 60/60s fixed window with claim→IP partition fallback is simpler to match exactly than adapting `governor` |
| ProblemDetails | Custom `ApiError` type implementing `IntoResponse` | RFC 7807 bodies + `traceId` extension |
| Swagger / Swashbuckle | **utoipa** + utoipa-swagger-ui | Mounted only in Development/Demo |
| `appsettings.json` + env + user-secrets | **figment** (or `config`) | Keep the same key names (`ConnectionStrings:AppDatabase` → `CONNECTIONSTRINGS__APPDATABASE`, `Jwt:*`, `Identity:InMemoryUsers`, `ServiceName`); env var override with `__` separator for drop-in compatibility |
| `ILogger` / LoggerMessage | **tracing** + tracing-subscriber | Keep the security-relevant warn events (failed login, logout mismatch, tenant mismatch) |
| CORS / security headers / HSTS | **tower-http** (`CorsLayer`, `SetResponseHeaderLayer`) | Same six headers, same hard-coded origin list |
| `WebApplicationFactory` tests | **axum-test** (or `tower::ServiceExt::oneshot`) | Per-test app with a copied scratch `chinook.db`, like `TestAuthHelpers` |
| NSubstitute service tests | **mockall** on repository traits | Same assertions: cache-key args, validation failures, invalidation calls |
| ArchUnitNET tests | Cargo graph + a `cargo metadata` assertion test | Plus Rust visibility for public-surface freezing |
| Dockerfile | Multi-stage `rust:` build → `debian:bookworm-slim` | Fix what the source got wrong: .dockerignore, non-root user, working env defaults, port 8080 |

---

## 4. Fidelity decisions — preserve vs. fix

The source has documented quirks. Recommended stance: **preserve everything wire-visible; fix internal bugs.** Each item below is explicitly decided so parity testing knows what "equal" means.

### Preserve (wire compatibility)

| # | Quirk | Decision |
|---|---|---|
| P1 | Mixed JSON casing — PascalCase API models vs lowercase health/root payloads vs snake_case token envelope (`access_token`, `expires_at_utc`) | Replicate field-for-field |
| P2 | Collection routes registered with trailing slash (`/api/admin/customers/`); ASP.NET matches both forms | Register canonical routes + `NormalizePathLayer` (trim trailing slash) so both forms work, as in the original |
| P3 | JWKS at `/api/identity/.well-known/jwks.json` (not root `/.well-known/`) | Replicate |
| P4 | No pagination anywhere — every collection returns the full result set (~3.5k tracks) | Replicate for v1 parity; pagination is a flagged v2 enhancement |
| P5 | Write surface is Genre-only; other services' create/update methods exist but are unrouted | Port the routed surface only; add service-layer write methods where tests need them |
| P6 | `tenant.scoped` succeeds when no `X-Tenant-Id` header is sent (isolation only enforced when the caller volunteers the header); missing tenant *claim* always fails | Replicate exactly (documented behavior + tests depend on it); note in README as a known design choice |
| P7 | Rate limiter runs **before** authentication, so partitioning effectively falls back to client IP | Replicate the pipeline order; still implement the full claim→IP partition-key logic |
| P8 | Health/data-health always HTTP 200 (even `Degraded`); metadata only in Dev/Demo | Replicate |
| P9 | 400/401/403/404/429/500 status semantics per endpoint, ProblemDetails shapes, `traceId` extension | Replicate |
| P10 | Only `global:public-anon` (60 req/60 s fixed window) exists; other four policy names are declared-but-dead | Replicate the one live policy; carry the dead names as constants for parity with the docs |
| P11 | PUT genre returns lowercase `{ id, name }` while POST returns a PascalCase `GenreApiModel` | Replicate (it's the observable contract) |
| P12 | No `Retry-After` on 429; `X-XSS-Protection: 0`; exact CSP string | Replicate |
| P13 | Dev-only in-memory users from config (`Identity:InMemoryUsers`), plaintext compare, disabled outside Dev/Demo; **no seeded users** — supplied via env/secrets | Replicate, including the startup diagnostics log events |
| P14 | Env gating on the literal `ASPNETCORE_ENVIRONMENT` value with `Demo` as a first-class environment | Replicate via an `Environment` enum read from the same variable |
| P15 | The host calls `UseStatusCodePages()`, so any 4xx/5xx with an empty body — a bare `Results.NotFound()`, an auth challenge — comes back as `text/plain` reading `Status Code: 404; Not Found` rather than empty | Replicate; `errors::status_code_page_body` produces the exact string (found in Phase 1, not in the original inventory) |
| P16 | The cache key composer takes its environment segment from the *configuration value* `ASPNETCORE_ENVIRONMENT`, not the host's resolved environment, so an unconfigured host composes keys under `prod` while reporting `Development` everywhere else. Its app-name fallback is `mmapi`, not the `ServiceName` default | Replicate both fallbacks exactly |
| P17 | `CustomerRepository.GetById` projects `Id`, `FirstName`, `LastName`, then a literal `// ... other fields ...` comment, resuming at `SupportRepId`. The nine columns that comment stands in for — company, address, city, state, country, postal code, phone, fax, email — are never selected, so `GET /api/admin/customers/{id}` returns them all as `null` while the *collection* endpoint returns them populated. The same customer looks different depending on which endpoint served it | Replicate; a test pins the nulls and the collection/by-id divergence (found in Phase 3). One line to fix here if the original ever fixes it |
| P18 | `InvoiceRepository.GetById`'s nested customer is deliberately partial too: key, names, company, email, phone, and the support rep's key and name, leaving address, city, state, country, postal code, and fax null | Replicate |

### Fix (internal bugs — not wire-visible, or wire-visible only as *more correct* behavior)

| # | Bug in source | Fix in Rust |
|---|---|---|
| F1 | `RemoveByTagAsync` is a **no-op** — list caches stay stale up to 20 min after writes despite docs claiming tag invalidation works | Implement a real tag→keys index in the cache facade so genre create/update/delete actually invalidates `all`/`by-*` entries. (Reads become *fresher*, never staler — safe divergence.) |
| F2 | `BaseRepository.Dispose()` disposes a pooled DbContext | N/A — Rust ownership; sqlx pool handles lifecycle |
| F3 | Swagger references an undefined `X-API-Key` security scheme | Emit only the Bearer scheme in the OpenAPI doc |
| F4 | Dockerfile uses .NET 9 images for net10.0 and can't start in Production (Dev key provider throws) | Ship a working Dockerfile with sane env defaults documented |
| F5 | `Produces(404)` declared on collection endpoints that always return 200 | Document 404 only on by-id routes in the OpenAPI spec |
| F6 | Committed dev RSA private key | Generate on first run; keep the file **gitignored**; loader stays format-compatible |
| F7 | Single-flight is best-effort: `CompositeCacheFacade` removes each per-key semaphore from its dictionary in the same `finally` that releases it, so concurrent callers can wait on different semaphore instances and more than one runs the factory | Coalescing is handled by the cache itself and is strict. Only ever reduces duplicate work (found in Phase 1) |
| F8 | `UseHttpsRedirection()` 307s plain HTTP to HTTPS in-process, with no forwarded-headers configuration — so behind a TLS-terminating proxy it redirect-loops, and it is the reason the original's container cannot serve traffic | HSTS is reproduced; the redirect is left to the edge, which the original's own deployment guide recommends. The probe honors `X-Forwarded-Proto` (Phase 4) |
| F9 | The `/data-health` handler catches every exception and discards it, so a degraded host reports `connected: false` with nothing to diagnose from | Same answer on the wire, but the failure is logged (Phase 4) |
| F10 | `TokenService.RefreshAsync` validates the refresh token, does its work, and revokes at the end — so every concurrent request arriving in that window passes, and each is issued its own fresh pair. Since every pair carries a new refresh token, a captured one can be replayed for as long as the race keeps being won | The token is spent first, as one `DashMap::remove` that both checks and removes, so exactly one caller is told it was valid. A sixteen-thread test got sixteen pairs before and gets one after |
| F11 | Every `update` and `delete` asks whether the row exists and then writes. Two statements, no transaction, so the answer is stale by the time the write runs: a row deleted in between yields a reported success that changed nothing, and the client gets a 204 or a 200 for a row someone else removed | The statement reports `rows_affected()`. One round trip instead of two, describing what happened rather than what was true a moment earlier |
| F12 | `InMemoryRefreshTokenStore` checks expiry on read and never removes anything, so the map grows for the life of the process — holding credentials. The rate limiter's partition map is keyed by caller address and likewise never shrinks, so a public host accumulates one entry per address it has ever seen | Both sweep: expired tokens at most once a minute, closed rate-limit windows at most once per window, each off the write path so an idle process does no work |
| F13 | `#[derive(Debug)]` on the credential-bearing types prints what they hold — the plain-text password on a user record and a login body, the refresh token on three more, the PKCS#8 signing key, and the whole configuration including connection string and passwords. `InMemoryRefreshTokenStore` is keyed by `"{userId}:{token}"`, so printing it prints every live credential | Each writes its own `Debug` and substitutes `<redacted>`, keeping the half that makes the record useful |
| F14 | The original's `traceId` appears in problem documents and nowhere else. The value itself is sound — `context.TraceIdentifier` is the ambient activity id, stable across a request and correlating with ASP.NET's own log scope — but it never reaches a successful response, so a client reporting a slow or wrong `200` has nothing to quote | An `x-trace-id` header on every response, carrying the same identifier the body reports. **This port had also regressed the value itself**: every response-building path called `new_trace_id()` as it built its document, so two documents in one request disagreed and neither matched a log record. One identifier is now established per request — ambient for every document, a field on the span wrapping the request, and the header. An inbound `traceparent` continues the trace when it really is one |
| F15 | Neither `KeyVault` nor `Dev` can sign in Production — the first is unimplemented, the second refuses there — so a Production host has no usable key provider and cannot start | `File` and `Environment` providers read an RSA key supplied as a PEM. The `kid` is the RFC 7638 thumbprint, so it is stable across restarts and replicas |
| F16 | A timestamp written through sqlx's `DateTime` encoder (RFC 3339) could not be read back by the row decoder, which knew only EF Core's form. An employee's dates came back `None` and an invoice's came back as the Unix epoch — a wrong answer shaped like a real one. Nine of the ten repositories' writes had no test at all | Writes render the stored form themselves; the reader accepts both. One write test per entity, each reading the row back |

### Verified against the running C# service (done in Phase 5)

The .NET 10 SDK turned out to be available, so rather than wait for Phase 8 these were captured from the real service — a seeded `Identity:InMemoryUsers` set, a live login, and curl against every shape. Several answers contradicted what the code alone suggested.

| # | What was verified | Result |
|---|---|---|
| V1 | **JWT claim names** | Not the short names. `DefaultInboundClaimTypeMap` is cleared but the **outbound** map is not, so name, email, and role are emitted as the full XML Schema URIs: `http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name`, `.../claims/emailaddress`, and `http://schemas.microsoft.com/ws/2008/06/identity/claims/role`. `sub`, `jti`, `iat`, `nbf`, `exp`, `iss`, `tenant`, and `permissions` are plain |
| V2 | **`aud` is duplicated** | `"aud": ["modular-api", "modular-api"]` — an array, not a string. `JwtSecurityToken`'s constructor adds the audience *and* the claim list already carries one |
| V3 | **Multi-valued claims collapse** | One role serializes as a string, two or more as an array. Same for `permissions`. A reader must accept both forms |
| V4 | **`expires_at_utc`** | `2026-09-11T23:09:46.900751+00:00` — a `DateTimeOffset`, so a `+00:00` offset rather than `Z`, and fractional digits with trailing zeros trimmed (0–7), unlike the health endpoints' fixed seven |
| V5 | **Bodiless 4xx/5xx are ProblemDetails JSON, not text** | `AddProblemDetails()` is registered, so `UseStatusCodePages()` emits `application/problem+json`: `{"type":"https://tools.ietf.org/html/rfc9110#section-15.5.5","title":"Not Found","status":404,"traceId":"…"}`. **This corrects P15**, which assumed the middleware's plain-text default |
| V6 | **Two different `type` vocabularies** | The default table uses `https://tools.ietf.org/html/rfc9110#…`; the custom exception handler uses `https://www.rfc-editor.org/rfc/rfc9110#…`. Both appear in the same service |
| V7 | **429 carries no `type`** | `{"title":"Too Many Requests","status":429,"traceId":"…"}` — ASP.NET's defaults table has no entry for 429, so `type` is omitted and `title` falls back to the reason phrase. Still no `Retry-After`, confirming P12 |
| V8 | **`traceId` is a W3C traceparent** | `00-{32 hex}-{16 hex}-00` in *both* shapes, not the `0HN…` connection id |
| V9 | **FluentValidation messages** | Confirmed exactly as implemented in Phase 2: `The length of 'Name' must be 120 characters or fewer. You entered 121 characters.` |
| V10 | **API model member order** | `{"Name":…,"Tracks":[],"Id":1}` — System.Text.Json writes derived members *before* base ones, so `Id` lands last. Semantically irrelevant, and the parity harness compares parsed documents |
| V11 | **Genre write surface** | `POST` → 201 with `Location: /api/admin/genres/{id}` and a PascalCase model; `PUT` → 200 with lowercase `{"id":…,"name":…}` (confirming P11); `PUT` on a missing id → 404; `DELETE` → 204 |
| V12 | **Auth flows** | Refresh rotates — reusing a spent refresh token answers 401. Logout with a mismatched `userId` answers 403; with the right one, 204. A 401 from the JWT challenge carries `WWW-Authenticate: Bearer` |

**V13 — the port was then checked against that capture.** Phase 5's Rust host was run with the same seeded login and its token diffed against the C# one: every claim key matches, including the three URI-named ones; every stable claim value matches; the lifetime is 900 seconds on both; the envelope keys and the `expires_at_utc` format match; and both refresh tokens are 88 characters. The only differences are the deliberately volatile claims (`jti`, `iat`, `nbf`, `exp`).

Two bugs surfaced only by running it, neither caught by the test suite:

- **Environment-variable arrays did not bind.** `Identity__InMemoryUsers__0__Username` splits into a map keyed `"0"`, which will not deserialize into a `Vec`, so the host started with no logins and rejected every password. ASP.NET's binder rewrites an index-keyed map as an array; the config loader now does too. This is the form the original's own documentation uses for seeding logins, so it would have bitten any real deployment.
- **`traceId` was degenerate.** Both halves were derived from a counter, so the span was identical on every response and the trace was mostly zeroes. The test only checked segment lengths and passed happily; it now checks that the values actually vary.

**V14 — Music and Orders were diffed route by route (Phase 6).** Both services were run side by side against the same database with the same seeded logins, and every route compared. **45 of 45 are byte-identical**, ignoring four members that cannot match by construction: `timestampUtc` (a clock reading), `traceId` (per request), `version` (the .NET informational version embeds the build's commit hash), and the JWKS `kid`/`n` (each host generates its own key). The harness is checked in as `tools/parity-diff.py`, which is the Phase 8 deliverable arriving early.

Four real bugs surfaced from that diff, none of which the test suite would have caught, because every one of them was a case of my tests asserting what I had implemented rather than what the original does:

1. **Dates were missing their `Z`.** EF reads the `+00:00` column as `Kind=Utc`, so `System.Text.Json` writes `2007-01-02T00:00:00Z`. The port emitted no zone marker. Entities and API models now carry `DateTime<Utc>` rather than `NaiveDateTime`.
2. **`ArtistName` was null on `/albums/artist/{id}`.** The C# query `Include`s the artist and `Album.Convert()` reads `Artist?.Name`, so that route populates the member while the plain collection route leaves it null — the same model with two shapes depending on the endpoint. I had read the `Include` as dead code.
3. **The invoice-line entity response was missing its two navigation members.** `Invoice` and `Track`, both always null, appear because the C# serializes an entity there.
4. **The wrong database file was bundled**, as above.

**V15 — Administration and the write surface were diffed (Phase 7).** The harness now covers **64 routes, all identical**. The Genre write surface was diffed separately, since writes need a database each rather than a shared one: **9 of 10 cases identical** — create, create with a lowercase member name, both validation failures, the forbidden and unauthorized paths, update-missing, update-invalid and delete-missing all match, including the `Location` header and the lowercase `PUT` echo.

The tenth is the `detail` of a malformed-JSON problem, and it is an accepted divergence: the `type`, `title` and `status` match, but the detail is the parser's own diagnostic and the .NET one names its own internals — `Failed to read parameter "CreateGenreRequest request" from the request body as JSON.` Matching it would mean hardcoding a C# type and parameter name into a Rust handler, which is copying another framework's implementation detail for no benefit. The message is a human-readable diagnostic, not a contract.

**F1 was also confirmed end to end.** With both services warmed and then given the same create, the original's collection endpoint still answered 28 genres while this port answered 29 — the stale read its no-op tag invalidation leaves behind, and the reason the fix exists.

**V16 — the harness was completed and run clean (Phase 8).** It now covers **98 cases**: every route, the authorization matrix, routing failures, the identity flows, and the whole write surface. Status codes, nine headers, and parsed bodies are all compared. **Zero unexplained differences.** `tools/run-parity.sh` starts both services, gives each its own database copy, runs the sweep and stops them; `.github/workflows/parity.yml` runs it on every push.

Comparing headers and error paths for the first time found four more real differences, every one of them in a place the test suite had no opinion about:

1. **`Content-Type` was missing its charset.** ASP.NET writes `application/json; charset=utf-8`; axum writes a bare `application/json`. Problem documents correctly carry no charset on either side. A middleware now adds it to plain JSON only.
2. **A rejected login carried a `WWW-Authenticate` challenge.** The header comes from the authentication middleware, so it belongs on a 401 from a *protected endpoint* — not on one an endpoint decided itself, like a bad password. `auth::unauthorized()` is the version without it.
3. **The `Invalid request` problem used the wrong `type` vocabulary.** `Results.Problem(...)` called without a `type` takes the framework's default (`tools.ietf.org`), not the `www.rfc-editor.org` one the custom exception handler passes explicitly. Both vocabularies really are in use, and which one you get depends on how the problem was raised.
4. **A missing `Content-Type` answered the wrong document.** It never reaches the C# exception handler at all — it is a bodiless 415 the problem-details middleware fills in — so it reads `Unsupported Media Type`, not `Malformed request.`

### Accepted divergences

Four cases differ on purpose, and the harness reports them as `known` rather than as failures:

| Case | Why |
|---|---|
| Three validation failures | **The original loses its security headers on exception-handled responses.** Its security-headers middleware sets them *before* calling the rest of the pipeline, and `UseExceptionHandler` clears the response before writing the problem document, wiping them. This port sets them on the way out, so an error response keeps them. Not reproduced: losing `nosniff` and the CSP on an error response is a real weakness, and no client depends on a header being *absent*. **Worth reporting upstream.** |
| Malformed JSON | The `detail` is the parser's own diagnostic, and .NET's names its internals — `Failed to read parameter "CreateGenreRequest request" from the request body as JSON.` Matching it would mean hardcoding a C# type and parameter name into a Rust handler. The `type`, `title` and `status` do match. |

### A note on the sweep itself

Every endpoint shares one rate-limit budget of 60 requests per 60 seconds, so a 98-case sweep trips it — the harness paces itself around the window rather than letting a 429 masquerade as a difference. That is worth knowing independently of the port: any client doing a broad sweep of this API, including a test suite or a crawler, gets throttled.

---

## 5. Host composition (crate `api`)

Reproduce `Program.cs` semantics:

1. Load config (figment: `appsettings.json` → `appsettings.{Environment}.json` → env vars).
2. Resolve `ConnectionStrings:AppDatabase`; if unset/missing file, walk up from the executable/content root looking for `data/chinook.db` (same probe as the original).
3. Build sqlx `SqlitePool` (max connections ~128 to mirror pool size).
4. Construct repositories (sqlx impls) → services (with cache facade, key composer, validators) → `AppState`.
5. Build each module's `Router` and nest under its prefix; hard-coded module list, same as `GetModules()`.
6. Layer stack, outermost → innermost, matching middleware order:
   - HSTS + HTTPS-redirect semantics (non-Development only; in practice delegated to the edge, as the source's docs recommend)
   - global error handler → ProblemDetails (+ panic catcher → 500)
   - security headers layer (6 headers, unconditional)
   - CORS layer (`Default` policy, hard-coded localhost origins)
   - rate-limit layer (fixed window 60/60 s, partition-key logic)
   - auth layer (JWT validation → request extensions: subject, roles, permissions, tenant); route guards run per-endpoint
   - Swagger UI mount (Dev/Demo only)
   - `NormalizePathLayer` (trailing-slash tolerance)
7. Root `GET /` endpoint with Dev/Demo-gated metadata (version from `CARGO_PKG_VERSION`/build info, `ServiceName` from config).
8. Serve on `HTTP :5043` by default (`ASPNETCORE_URLS`-equivalent config override; Docker uses 8080).

Module contract in `shared-kernel`:

```rust
pub trait Module {
    fn name(&self) -> &'static str;
    fn prefix(&self) -> &'static str;            // "/api/music"
    fn router(&self, state: AppState) -> Router; // ≈ MapEndpoints
}
```

(Registration of services happens in the host via plain constructors — Rust doesn't need the `RegisterServices` half of the .NET contract.)

---

## 6. Data layer (crates `shared-persistence`, `shared-data-sqlite`)

- **Entities:** 12 structs matching Chinook tables (`Album`, `Artist`, `Customer`, `Employee`, `Genre`, `Invoice`, `InvoiceLine`, `MediaType`, `Playlist`, `PlaylistTrack` composite-key join, `Track`). Nullable columns → `Option<T>`; money columns → `rust_decimal::Decimal`; dates → `chrono::NaiveDateTime`.
- **API models:** mirror the C# ApiModels including denormalized fields (`ArtistName`, `AlbumName`, `SupportRepName`, Employee's `ReportsToNavigation` as a `"First Last"` string) and nested-collection shapes, with the same include/flatten asymmetries (by-id returns graphs, collections return flat models).
- **Repository traits** in `shared-persistence`, one per entity, matching the C# interfaces: `entity_exists`, `get_all`, `get_by_id`, `add`, `update`, `delete`, plus the per-entity by-FK queries (`tracks by artist/album/genre/mediatype/playlist/invoice`, `invoices by customer`, `customers by support-rep`, `employees direct-reports/reports-to`, etc.).
- **sqlx implementations** in `shared-data-sqlite`. The source has **no raw SQL to lift** (it's all EF LINQ) — SQL is re-derived per query. Notable translations:
  - `Track::get_by_artist_id` → join through `Album`
  - `Track::get_by_invoice_id` → `WHERE EXISTS (invoice_line …)`
  - `Playlist::get_by_id` → two lean queries (header, then ordered tracks) as the source deliberately does
  - Album/Artist by-id graph loads → separate queries per collection (the moral equivalent of `AsSplitQuery`)
  - All reads non-tracking by nature; writes return the row (`RETURNING` — supported by SQLite ≥3.35 via sqlx)
- **Validators:** exact FluentValidation rules, including the subtle semantics (optional fields validated only when present; the **unanchored** US phone regex; anchored US ZIP regex `^[0-9]{5}(?:-[0-9]{4})?$`; Album title 3–160; Track name ≤200, composer ≤220, unit price >0 and ≤9.99; Customer first ≤40/last ≤20; Employee first/last ≤20; etc.). Validation errors carry field names matching the C# `PropertyName`s so the 400 body's `errors` map is identical.

---

## 7. Identity module

- **Key management:** `KeyMaterial` trait — `signing_key()`, `validation_keys()`, `current_kid()`, `jwks()`. Dev provider: RSA-2048 generated on first run, persisted as `{Kid, PrivateKeyPkcs8Base64}` (compatible with the .NET dev key file), lock-file guarded, path from `Jwt:DevelopmentKeyPath`; **refuses to run outside Dev/Demo**, same as the source. Production slot: a `KeyVault`-style provider stubbed behind the same trait (Azure Key Vault via REST, or documented as out-of-scope for v1 with the trait ready).
- **Token service:** RS256 access tokens (15 min; `sub`, `jti`, `iat`, `nbf`, `exp`, `iss` default `https://auth.local`, `aud` default `modular-api`, name/email, `tenant`, repeated role and `permissions` claims — exact names pinned in Phase 8); opaque 64-byte refresh tokens, in-memory store keyed `userId:token`, rotation on refresh, permission/tenant re-read from the user store, revoke-on-missing-user.
- **Validation:** issuer/audience/lifetime/signature, 30 s clock skew, `typ` ∈ {`at+jwt`, `JWT`}, reject tokens without `sub`, tolerate a doubled `Bearer Bearer ` prefix.
- **Endpoints:** login (400 blank fields / 401 bad creds), refresh, logout (403 on sub mismatch, 204), userinfo, JWKS — same request/response shapes.
- **Policies:** constants + guard combinators for the 8 permissions, `role.admin`, `tenant.scoped` (with the default-allow-when-no-header behavior, P6).

---

## 8. Phased delivery

Each phase compiles, passes its tests, and is a meaningful commit. Order chosen so auth lands before the protected modules that need it.

| Phase | Deliverable | Acceptance |
|---|---|---|
| **0. Scaffold** ✅ | Workspace, crate skeletons, CI (fmt, clippy -D warnings, test), `chinook.db` copied in, `.gitignore` for the dev key | `cargo build` green; boundary test passes |
| **1. Shared kernel** ✅ | Config loader (appsettings + env layering), `Environment` gating, build info, ProblemDetails/`ApiError`, cache facade (moka L1, key composer, jitter, single-flight, **real tag index** per F1), rate-limit partition keys + policy name constants, `Module` trait | Unit tests for key composition (`env:app:module:entity:v1::::disc`), jitter bounds, tag invalidation, partition fallback order |
| **2. Persistence** ✅ | Entities, API models (serde-exact), validators, repository traits, pool + connection-string probe | Validator unit tests mirroring the C# rules; probe finds `data/chinook.db` |
| **3. sqlx repositories** ✅ | All 10 repository impls against the real Chinook file | Repository tests against a scratch copy of chinook.db (counts, known rows: e.g. 347 albums, 3503 tracks); write tests for Genre CRUD |
| **4. Host skeleton** ✅ | axum app, middleware stack, root endpoint, health + data-health for all five modules, Swagger (Dev/Demo) | Integration tests: root/health payload shapes, Dev vs Prod metadata gating, security headers on every response, CORS preflight |
| **5. Identity module** ✅ | Keys, token service, stores, 5 auth endpoints, policy guards | Port the 19 identity integration tests + 3 authorization-pipeline tests (token claims, 200 admin, 403 non-admin) |
| **6. Music + Orders modules** | 15 + 7 read endpoints, services with cache-aside, guards (`music.read`/`orders.read` + `tenant.scoped`) | Port album/artist/track/playlist/invoice/invoice-line endpoint tests; 401/403 matrix |
| **7. Administration + Reporting** | Customers/employees/genres/media-types reads, Genre POST/PUT/DELETE with validation + cache invalidation, stacked admin guards; Reporting health | Port admin endpoint tests incl. the 14 genre-write tests and error-scenario tests (bad JSON → 400 ProblemDetails, validation `errors` map) |
| **8. Parity verification** | Golden-diff harness: run the .NET app and the Rust app side by side with identical seeded users; script hits all 47 routes (plus error cases) and diffs status, headers-of-interest, and JSON bodies | Zero unexplained diffs; every intentional divergence traces to an F-item in §4. Pin JWT claim names here |
| **9. Ops + docs** ✅ | Working Dockerfile (multi-stage, non-root, port 8080), README, and the architecture docs ported with statuses corrected | Done. A 37 MB non-root image with a healthcheck, verified serving real data; every README command run as written |

Cache/rate-limit behavior tests (port of `CachingBehaviorTests`, `RateLimitingTests` — burst >60 → 429) land alongside phases 6–7.

**Suggested v2 backlog (explicitly out of parity scope):** pagination, real L2/Redis tier, tenant-partitioned cache keys, the four unused rate-limit policies, `Retry-After`, remaining write endpoints for the other entities, OpenTelemetry.

---

## 9. Testing strategy (target ≈ the source's 193 tests)

1. **Integration (HTTP)** — axum-test against a full app instance with a per-test scratch copy of `chinook.db` and injected in-memory users (mirroring `TestAuthHelpers`: `demo`/music.read, `usermo`/music+orders, `report`/report.view, `admin`/all — tenant-1). Covers every endpoint's happy path, auth matrix (401 no token, 403 wrong permission/tenant mismatch), error scenarios, health gating, rate limiting, caching behavior.
2. **Service unit tests** — mockall repository doubles; assert cache-key composition arguments, validation short-circuits, invalidation calls on writes.
3. **Architecture tests** — a test that reads `cargo metadata` and asserts the §2 dependency edges (module isolation, shared-kernel-as-leaf); public surface enforced at compile time by visibility.
4. **Golden-diff parity suite** (Phase 8) — the strongest equivalence evidence; kept as a script in `tools/` so it can be re-run whenever either side changes.

---

## 9a. Outcome

All nine phases are complete. The port serves all 47 routes, 332 tests pass,
and `tools/run-parity.sh` compares 98 cases against the running original with
zero unexplained differences.

Worth carrying forward: **fourteen real bugs in this port were found by diffing
against the running service, and not one of them was caught by the test
suite.** In every case the tests asserted what had been implemented rather than
what the original does. Reading the C# source was necessary and not sufficient
— it produced the wrong JWT claim names, the wrong error-response media type,
a missing `charset`, dates without their `Z`, a dead `Include` mistaken for
dead code, and the wrong one of two bundled database files. The oracle was
worth more than the source.

## 10. Risks & watch items

- **Serde casing drift** is the most likely source of silent incompatibility — the mixed PascalCase/lowercase/snake_case contract (P1) needs per-payload `rename` discipline and is exactly what the golden-diff suite exists to catch.
- ~~**Decimal/date formatting:**~~ **Settled in Phase 2.** Money uses `rust_decimal` with the `serde-float` feature, so a `Decimal` serializes as a bare JSON number the way `System.Text.Json` writes C#'s `decimal`; dates use `chrono::NaiveDateTime`, whose default format matches the offset-free ISO-8601 the original emits. Both money columns are stored as SQLite `real`, so values arrive as `f64` — a test pins all 37 distinct `Invoice.Total` and `Track.UnitPrice` values in the bundled database through `f64` → `Decimal` → JSON to catch any `10.899999999999999` tail.
- **JWT claim-name uncertainty** (outbound claim-type mapping) — resolved empirically, not by reading code (§4).
- **SQLite concurrency:** EF pooled 128 connections; sqlx SQLite writes serialize on a single writer. Fine for this read-heavy API. Phase 3 kept the pool at 128 but deliberately did **not** switch on WAL: journal mode lives in the file header, so enabling it would rewrite the committed `chinook.db` and dirty the working tree just by running the app.
- ~~**The bundled database is not pristine Chinook.**~~ **Resolved in Phase 6.** The C# repository ships *two* copies of the file, and the host reads the one under its content root — `src/ModularMonolith.Api/data/chinook.db`, where Genre 1 is `Rock` and the table holds 25 rows — stock Chinook. Phase 0 had bundled the repository-root copy instead, the one the application never opens, whose Genre table carries `Updated_<guid>` and 34 rows. Every other table is identical between them. The authoritative copy is now what this repository bundles, which is what took the parity diff from 22/26 to 45/45. Original finding, for the record:
- **(historical) The repository-root database is not pristine Chinook.** Found in Phase 3: the committed `data/chinook.db` carries leftovers from a previous run of the C# test suite. Genre 1's name is `Updated_<guid>`, and genres 26–33 are rows those tests created (`CacheTest_`, `TestGenre_`, `Concurrent_`, `CharsetTest_`), so the table holds 34 rows where stock Chinook holds 25. Genre is the only entity with write endpoints, which is why it is the only table affected. Tests assert genre *behavior* rather than genre *contents*. Worth deciding before Phase 8: a golden diff needs both sides reading byte-identical data, so either keep this file as-is on both, or reset both to stock together — never one alone.
- **The unanchored phone regex and US-only ZIP regex** will reject some legitimate Chinook seed values if ever validated on read — the source only validates on write, so keep validation write-only.
- **Scope control:** the source's docs describe more than the code does (Redis tier, SWR, tiered rate limits, EF migrations). We port the *code's* behavior; the docs' roadmap becomes our v2 backlog.
