# Phase 8 — Parity verification

*Learning Rust through the port, part 9 of 10.*

| | |
|---|---|
| **What this phase built** | A complete golden-diff harness: the C# original and this port run side by side with identical seeded logins and a database copy each, 98 cases compared on status, nine headers and parsed body, a CI workflow that runs it on every push, and four more wire fixes the sweep exposed. |
| **Commit** | `dc4a2ef` — `git show dc4a2ef --stat` |
| **Files to open** | `tools/parity-diff.py`, `tools/run-parity.sh`, `.github/workflows/parity.yml`, `docs/parity.md`, and the Rust touched by the findings: `crates/api/src/middleware.rs` (`json_charset`), `crates/shared-kernel/src/auth.rs` (`unauthorized`), `crates/shared-kernel/src/errors.rs` (`default_problem_type`), `crates/shared-kernel/src/json.rs` (`into_problem_with`) |
| **Rust ideas introduced** | Less new syntax, more method: `&'static str` tables and why `format!` cannot produce one, `is_some_and`, string comparison on header values, deciding *where* a fix belongs, and the difference between a test that asserts your implementation and one that asserts the truth |

Most of this phase is a Python script and a shell script. It earns a place in
a Rust learning path anyway, because it is where the port learned the limits
of its own tests, and because each finding landed as a small, precise Rust
change worth reading.

---

## 1. What the harness does

`tools/run-parity.sh` builds both services, seeds four logins through the
`Identity__InMemoryUsers__N__*` environment variables (the form Phase 1's
config loader had to learn to bind), gives each service its own copy of
`data/chinook.db`, starts them on different ports, and hands off to
`tools/parity-diff.py`. The script exits 0 with an explanation if the
original or the .NET SDK is absent, so it is safe to run anywhere.

`parity-diff.py` sends each case to both hosts and compares:

- the status code;
- nine headers (`content-type`, `location`, `www-authenticate`, and the six
  security headers);
- the body **parsed as JSON**, so member order — which System.Text.Json and
  serde disagree on — is not a difference.

Five members are scrubbed because they cannot match by construction:
`timestampUtc`, `traceId`, `version`, and the JWKS `kid`/`n`. Tokens are
compared by shape. And because every endpoint shares one 60-per-minute
rate-limit budget (Phase 4), a `Pacer` waits out the window after fifty
requests rather than letting a 429 masquerade as a difference.

Read the `KNOWN_DIVERGENCES` dictionary: four cases differ on purpose, each
with a paragraph saying why. That is the discipline this phase teaches —
**every intentional difference is written down next to the check that would
otherwise flag it.**

---

## 2. Four findings, four small Rust changes

Comparing headers and error paths for the first time found four differences.
Each fix is a good example of a targeted change.

### 2a. `Content-Type` was missing its charset

ASP.NET writes `application/json; charset=utf-8`; axum writes a bare
`application/json`. Problem documents correctly carry no charset on either
side. The fix is a middleware that rewrites only plain JSON
(`middleware.rs`):

```rust
pub async fn json_charset(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;

    let is_plain_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == "application/json");

    if is_plain_json {
        response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(JSON_WITH_CHARSET));
    }
    response
}
```

`is_some_and` collapses "is it `Some`, and does the inside satisfy this?"
into one call. `to_str().ok()` is needed because a header value is bytes and
may not be UTF-8. The exact-equality check (not `starts_with`) is what keeps
`application/problem+json` untouched.

### 2b. A rejected login carried a `WWW-Authenticate` challenge

The challenge comes from the authentication middleware in ASP.NET, so it
belongs on a 401 from a *protected endpoint*, not on one an endpoint decided
itself. The port had one 401 shape; now it has two (`auth.rs`):

```rust
/// A 401 an endpoint produced itself, with no `WWW-Authenticate` challenge.
pub fn unauthorized() -> Response {
    status_code_page(StatusCode::UNAUTHORIZED, current_trace_id()).into_response()
}
```

versus `AuthorizationFailure::Unauthenticated`'s `into_response`, which adds
the header. `handle_login` and `handle_refresh` call the former; the
`Authorized<P>` extractor produces the latter. Naming the two paths as two
functions is the Rust-flavored fix: the type of 401 is chosen at the call
site, and the doc comment on each says which to use.

### 2c. The `Invalid request` problem used the wrong `type` vocabulary

`Results.Problem(...)` without an explicit `type` takes the framework's
default (`tools.ietf.org`), not the `www.rfc-editor.org` one the custom
handler passes. Both vocabularies are in use in the same service. The fix
exposes the defaults table (`errors.rs`):

```rust
pub fn default_problem_type(status: StatusCode) -> Option<&'static str> {
    problem_defaults(status).0
}
```

and `endpoints.rs`'s `invalid_request` uses it. This is where the `&'static
str` table from Phase 1 earns its keep: because every type URI is a literal
in a `match`, the function can return a `'static` reference with no
allocation. Had the URIs been built with `format!`, the return type would
have had to be `Option<String>` and every caller would allocate.

### 2d. A missing `Content-Type` answered the wrong document

A request with no content type never reaches the C# exception handler; it is
a bodiless 415 the problem-details middleware fills in. So `MalformedRequest`
branches on status (`json.rs`):

```rust
pub fn into_problem_with(self, trace_id: impl Into<String>) -> ProblemDetails {
    if self.status == StatusCode::BAD_REQUEST {
        ProblemDetails::malformed_request(self.status, self.detail, trace_id)
    } else {
        crate::errors::status_code_page(self.status, trace_id)
    }
}
```

`impl Into<String>` as a parameter type accepts `&str`, `String`, or anything
else convertible — an idiom the whole `errors.rs` uses so callers can pass
literals in tests and owned strings in production.

---

## 3. The accepted divergences

Three validation failures and one malformed-JSON case are reported as
`known`. The reasoning is in the plan and in `docs/parity.md`, and it is
instructive on both sides:

- **Security headers on error responses.** The original *loses* them,
  because its middleware sets them before calling the pipeline and
  `UseExceptionHandler` clears the response. The port sets them on the way
  out (Phase 4's `security_headers` runs after `next.run`), so they survive.
  Not reproduced: no client depends on a header being absent, and losing
  `nosniff` and the CSP on an error is a real weakness. The plan flags it as
  worth reporting upstream.
- **The malformed-JSON `detail`.** .NET's message names a C# type and
  parameter. Matching it would mean hardcoding another framework's internals
  into a Rust handler. `type`, `title` and `status` match.

The principle: preserve everything wire-visible *unless* matching it would
mean copying a bug into the port or copying an implementation detail with no
contract behind it. Each such case gets a written justification at the point
of comparison.

---

## 4. What the oracle taught about tests

The plan's outcome section puts it in one sentence: *fourteen real bugs in
this port were found by diffing against the running service, and not one of
them was caught by the test suite.* The table below collects the ones the
verification rounds recorded, with the Rust construct each became, so you can
see the pattern rather than the count.

| Found by running the original | Where it landed |
|---|---|
| JWT claim names are full XML Schema URIs, not `name`/`email`/`role` | `#[serde(rename = "http://...")]` on `AccessTokenClaims` |
| `aud` is emitted twice, as an array | `aud: Vec<String>` with the value pushed twice |
| One role is a string, several are an array | `#[serde(untagged)] enum ClaimValues` |
| `expires_at_utc` is a `DateTimeOffset` with trimmed fraction and `+00:00` | `format_offset` in the identity endpoints |
| Bodiless 4xx/5xx are `application/problem+json`, not text | `status_code_page` + the `status_code_pages` layer |
| Two `type` vocabularies coexist; 429 has none | `problem_defaults` table returning `Option<&'static str>` |
| `traceId` is a W3C `traceparent` | `new_trace_id` + `is_traceparent` |
| Env-var arrays did not bind | `numeric_maps_to_arrays` in the config loader |
| `traceId` was mostly zeroes | A test that asserts variance, not just length |
| Dates lacked their `Z` | `DateTime<Utc>` instead of `NaiveDateTime` |
| `ArtistName` null on one route | A repository method returning API models |
| Invoice-line entity lacked two null members | `#[serde(flatten)]` + `Option<()>` |
| The wrong `chinook.db` was bundled | `seed_integrity.rs` |
| Charset, login challenge, `Invalid request` type, 415 document | The four changes in §2 |

Every one of these had a passing test before the diff. The tests were
correct about the code and wrong about the world. Two things follow for
anyone writing Rust tests:

1. **Serde attributes are contracts, and they need an external witness.** A
   round-trip test (`serialize` then `deserialize`) passes for any consistent
   pair of attributes. Only a captured document catches the wrong `rename`.
2. **Type-level fixes are cheap once you know the truth.** Each row above is
   a few lines. The expensive part was discovering the discrepancy, and the
   harness makes that discovery repeatable — which is why it runs in CI
   (`.github/workflows/parity.yml`) and not just once.

---

## 5. The sweep as a probe of the API itself

One finding was not a bug in the port: a 98-case sweep trips the shared
60-per-minute budget. The harness paces around it, but the observation
stands for any client doing a broad sweep, including a test suite or a
crawler. When a verification tool has to work around a property of the
system, that property is worth writing down where users will see it — which
`docs/parity.md` and the README both do.

---

## Try it

1. **Run it.** Clone the original beside this repository and run
   `tools/run-parity.sh ../aspnetcore-min-api-modular-monolithic`. It needs
   the .NET 10 SDK; without it the script says so and exits 0.
2. **Add a case.** In `parity-diff.py`, add
   `Case("tracks by composer", "/api/music/tracks/composer/Angus%20Young", role="admin")`
   if you built the Phase 6 exercise, and see it reported as a difference
   (the original has no such route — a 404 on one side, a 200 on the other).
3. **Reintroduce a fixed bug.** Comment out the `.layer(middleware::from_fn(own::json_charset))`
   line in `app.rs`, run the sweep, and count how many cases fail on
   `content-type`.
4. **Promote a known divergence.** Delete one entry from
   `KNOWN_DIVERGENCES`, run the sweep, and read how the failure is reported —
   then put it back.
5. **Write the missing witness.** Pick any `#[serde(rename = ...)]` in the
   port and write a test that asserts against a JSON *literal* copied from a
   real response, not against a round trip.
6. **Read the two 401s.** Grep for `unauthorized()` and
   `AuthorizationFailure::Unauthenticated` and confirm each call site is the
   one the doc comment says it should be.

## Vocabulary for this phase

| C# / .NET | Rust | Notes |
|---|---|---|
| `string?.Equals(x) == true` | `option.is_some_and(\|v\| v == x)` | |
| `string` parameter accepting anything stringy | `impl Into<String>` | Callers pass `&str` or `String` |
| Const string table returning `string` | `match` returning `&'static str` | Literals only; no `format!` |
| `Results.Unauthorized()` (no challenge) | `auth::unauthorized()` | Two 401 shapes, two functions |
| Golden-file test | Golden-*service* diff (`tools/parity-diff.py`) | The oracle is the running original |
| `[Fact]` asserting a round trip | A test asserting a captured literal | Round trips cannot see a wrong `rename` |

**Next:** [Phase 9 — ops and docs](phase-9-ops-docs.md): a container that
runs, release profiles, and the documentation as a deliverable.
