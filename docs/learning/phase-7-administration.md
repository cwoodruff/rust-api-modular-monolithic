# Phase 7 — Administration, and the only write surface

*Learning Rust through the port, part 8 of 10.*

| | |
|---|---|
| **What this phase built** | Twelve read endpoints (customers, employees, genres, media types), the Genre `POST`/`PUT`/`DELETE` trio, validation and cache invalidation in the service, the three-policy guard stack, and a JSON extractor that fails the way the original fails. |
| **Commit** | `8f71d46` — `git show 8f71d46 --stat` (the guard-coverage test arrived in `11035d8`) |
| **Files to open** | `crates/module-admin/src/{lib,endpoints,services}.rs`, `crates/shared-kernel/src/json.rs`, `crates/api/tests/administration.rs`, `crates/architecture-tests/tests/route_guards.rs` |
| **Rust ideas introduced** | Custom body extractors via `FromRequest`, the newtype pattern, layered error enums with `From` chains, enum variants as functions, tuple responses with headers, `serde(alias)`, `Result<bool, E>` for "did it happen," source-scanning tests that fail closed |

The reads in this module look exactly like Phase 6's. What is new is the
*write* path, and with it three things Rust makes you spell out: how a
malformed body is reported, how a validation failure travels up to a 400, and
how a "not found" on update differs from a database error.

---

## 1. Three policies on one handler

```rust
type Reader = Authorized<AdministrationRead>;
type Writer = Authorized<AdministrationWrite>;

async fn create_genre(
    _caller: Writer,
    State(state): State<AppState>,
    JsonBody(request): JsonBody<GenreRequest>,
) -> Result<(StatusCode, [(HeaderName, String); 1], Json<GenreApiModel>), ApiError> {
```

`AdministrationRead` stacks `Role("Admin")`, `Permission("administration.read")`
and `TenantScope`; `AdministrationWrite` swaps the middle one. The module's
doc says why it matters that the policy is in the signature: which set a
route carries is visible as `Reader` or `Writer` rather than in a call at the
top of the body. The kernel test
`the_administration_policies_stay_distinct_through_the_extractor` proves the
read permission cannot open a write route.

**The return type is a tuple** of status, a fixed-size array of one header
pair, and a JSON body. axum implements `IntoResponse` for tuples of that
shape, so the handler returns:

```rust
Ok((StatusCode::CREATED, [(header::LOCATION, location)], Json(created)))
```

and gets a 201 with a `Location` header. `[(HeaderName, String); 1]` is an
array type — one element, known at compile time. This is how you attach
headers without touching a `Response` by hand.

---

## 2. `JsonBody<T>`: a custom extractor for the whole request

axum's built-in `Json<T>` rejects a bad body with a plain-text 400. The C#
host answers with a problem document titled `Malformed request.`, so the port
wraps `Json` (`shared-kernel/src/json.rs`):

```rust
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonBody<T>(pub T);

impl<T, S> FromRequest<S> for JsonBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::Malformed(MalformedRequest::from(rejection))),
        }
    }
}
```

- **`FromRequest` versus `FromRequestParts`.** Phase 5's `Authorized<P>`
  needed only headers, so it implemented `FromRequestParts` and can appear
  anywhere in the argument list. A body extractor consumes the request, so it
  implements `FromRequest` and must be the *last* argument of a handler. axum
  enforces that at compile time.
- **The newtype pattern.** `pub struct JsonBody<T>(pub T);` is a tuple struct
  with one public field. It exists to give `T` a different set of trait
  impls — here, a different rejection — at zero runtime cost. Handlers
  destructure it in the parameter list: `JsonBody(request): JsonBody<GenreRequest>`.
- **Delegation.** The impl calls axum's own `Json::from_request` and only
  translates the failure. `JsonRejection` exposes `.status()` and
  `.body_text()`, which `MalformedRequest` captures. Its `into_problem_with`
  then branches: a 400 becomes `Malformed request.` with the parser's message
  as the detail, and anything else (a 415 for a missing `Content-Type`) takes
  the defaults-table document instead — because, as the comment records, the
  C# exception handler never sees those.

The tests at the bottom of `json.rs` build a one-route router and post three
bodies: well-formed, unparseable, and untyped. That is the pattern for testing
any extractor in isolation.

---

## 3. Body casing with `serde(alias)`

```rust
#[derive(Debug, Clone, Deserialize)]
struct GenreRequest {
    #[serde(alias = "name")]
    #[serde(rename = "Name")]
    name: Option<String>,
}
```

ASP.NET's binder matches JSON members case-insensitively, so `Name` and
`name` both bind in the original. `rename` sets the canonical name; `alias`
adds another accepted spelling on input. The field is `Option<String>` so a
body with no name at all deserializes and then *fails validation* with the
FluentValidation message, exactly as the original does — the doc comment
notes the C# `[Required]` attribute is decorative on Minimal APIs.

Two response shapes coexist and are two structs: `GenreApiModel` (PascalCase,
for `POST`) and the private `GenreEcho { id, name }` (lowercase, for `PUT`).
The comment explains the original's anonymous-object origin. Rust makes you
name both, which is fine; the names document the asymmetry.

---

## 4. Layered errors with `From`

The service defines its own failure type and two conversions
(`services.rs`):

```rust
pub(crate) enum WriteFailure {
    Validation(Vec<ValidationFailure>),
    Repository(RepositoryError),
}

impl From<RepositoryError> for WriteFailure { ... }

impl From<WriteFailure> for ApiError {
    fn from(failure: WriteFailure) -> Self {
        match failure {
            WriteFailure::Validation(failures) => Self::Validation(validation_errors_from(
                failures.into_iter().map(|f| (f.property_name, f.message)),
            )),
            WriteFailure::Repository(error) => Self::Repository(error),
        }
    }
}
```

Now watch `?` do the work in `create_genre`:

```rust
pub(crate) async fn create_genre(state: &AppState, name: Option<String>)
    -> Result<GenreApiModel, WriteFailure>
{
    let model = GenreApiModel { id: 0, name, tracks: Vec::new() };
    model.validate().map_err(WriteFailure::Validation)?;
    let created = state.repositories.genres.add(Genre { id: 0, name: model.name.clone() }).await?;
    invalidate_genres(state, None).await;
    Ok(created.convert())
}
```

- `model.validate()` returns `Result<(), Vec<ValidationFailure>>`;
  `.map_err(WriteFailure::Validation)` wraps the `Vec` using the **enum
  variant as a function** — a tuple variant *is* a constructor function, so
  it can be passed where a `FnOnce(Vec<_>) -> WriteFailure` is expected.
- `.add(...).await?` returns `RepositoryError` on failure; `?` applies
  `From<RepositoryError> for WriteFailure` automatically.
- The handler then writes `services::create_genre(&state, request.name).await?`
  and `?` applies `From<WriteFailure> for ApiError`.

Three error types, three layers, and no `try`/`catch`. Each `From` impl is a
single, testable place where one layer's vocabulary becomes the next's. The
test `a_validation_failure_becomes_the_originals_400` checks the whole chain
produces the right document.

---

## 5. `Result<bool, E>`: three outcomes on a write

```rust
pub(crate) async fn update_genre(state: &AppState, id: i32, name: Option<String>)
    -> Result<bool, WriteFailure>
{
    let model = GenreApiModel { id, name, tracks: Vec::new() };
    model.validate().map_err(WriteFailure::Validation)?;
    let updated = state.repositories.genres.update(Genre { id, name: model.name.clone() }).await?;
    if updated { invalidate_genres(state, Some(id)).await; }
    Ok(updated)
}
```

`Ok(true)` — it happened; `Ok(false)` — no such row; `Err(_)` — validation or
database. The handler maps them to 200, 404, and the error's own status:

```rust
if services::update_genre(&state, id, request.name).await? {
    Ok(Json(GenreEcho { id, name }))
} else {
    Err(ApiError::NotFound)
}
```

The `bool` comes from Phase 3's `rows_affected()` (fix F11), so it describes
what the statement did rather than what a prior check believed.

**Cache invalidation** runs only when the write happened. `invalidate_genres`
calls `remove_by_tag` (the Phase 1 fix F1) and then removes the specific
`by-id` key, reproducing both calls the C# service makes. The commit message
records the end-to-end confirmation: after the same create, the original's
collection still answered 28 genres and this port answered 29.

---

## 6. Quirks preserved on purpose

Three read endpoints do something a reviewer would want to "fix," and each
has a comment saying not to:

- `/employees/{id}/reports-to` returns the employee, not their manager
  (`services::reports_to` doc).
- A genre's `Tracks` list is always empty, because the service converts an
  entity (`genre_by_id`).
- `customer_by_id` omits nine columns the C# projection never selects
  (Phase 3's P17).

The Rust lesson is about *where* to preserve a quirk: in the repository when
it is a projection, in the service when it is a conversion, and in the
endpoint when it is a response shape — with a test at that layer.

---

## 7. A test that reads the source

`crates/architecture-tests/tests/route_guards.rs` closes the one hole
`Authorized<P>` leaves: a handler that never asks for a caller compiles and
serves data to anyone. The test parses each module's `routes()` function out
of its source text, finds every handler, and checks its parameter list names
a guard — or is listed in `ANONYMOUS_ROUTES` with a reason.

The design notes at the top are worth reading as a model for any
"lint-shaped" test:

- It is a syntactic check, and says so.
- It **fails closed**: a module whose source cannot be found, a route whose
  path is not a literal, or an allowlist entry naming a route that no longer
  exists each fail the test rather than passing quietly.
- It has a `MINIMUM_ROUTES_FOUND` floor so a parser that silently matched
  nothing cannot report every module clean.

If Phase 6's first exercise had you add an unguarded route, this is the test
that caught it.

---

## Try it

1. **Add a write.** Implement `PUT /media-types/{id}` in the same shape as
   `update_genre`: a `MediaTypeRequest`, a service function returning
   `Result<bool, WriteFailure>`, validation through `MediaTypeApiModel`, and
   tag invalidation. Then write the 200/400/404 tests.
2. **Watch `?` refuse a conversion.** Delete `impl From<RepositoryError> for
   WriteFailure` and read the error on the `.await?` line. It names the
   missing `From`.
3. **Move the body extractor.** Put `JsonBody(request)` *before* `State(state)`
   in `create_genre`'s parameters and read axum's compile error about the
   last argument.
4. **See both malformed documents.** With the host running and an admin
   token, `curl -X POST .../api/admin/genres -H 'Content-Type: application/json' -d '{'`
   and then the same without the header. Compare `title` and `type`.
5. **Bypass validation and see the layer that catches it.** Comment out the
   `validate()` line in `create_genre` and run
   `cargo test -p api genre`. The failures show that nothing downstream
   re-checks — validation lives in the service on purpose.
6. **Break the guard test.** Remove `_caller: Reader` from `all_genres`,
   run `cargo test -p architecture-tests route_guards`, and read the message
   that names the route and the handler.

## Vocabulary for this phase

| C# / .NET | Rust | Notes |
|---|---|---|
| Custom `IBindableFromHttpContext` / model binder | `impl FromRequest<S> for MyType` | Must be the last handler argument |
| Header-only binder | `impl FromRequestParts<S>` | Any position |
| Wrapper type for different behavior | Newtype: `struct JsonBody<T>(pub T);` | Zero-cost |
| Case-insensitive JSON binding | `#[serde(alias = "name")]` | Input only |
| `Results.Created(location, body)` | `(StatusCode::CREATED, [(LOCATION, url)], Json(body))` | Tuple `IntoResponse` |
| `throw new ValidationException(...)` | `Err(WriteFailure::Validation(failures))` | Converted by `From` at each layer |
| Exception filter / handler mapping | `impl From<Inner> for Outer` + `?` | |
| `Func<T, E>` from a constructor | Enum variant as a function: `.map_err(WriteFailure::Validation)` | |
| `bool` return meaning "found" | `Result<bool, E>` | Three outcomes, all typed |
| Source-analysis test (Roslyn) | Test that reads `.rs` files and fails closed | `route_guards.rs` |

**Next:** [Phase 8 — parity verification](phase-8-parity.md): the harness,
the last four differences, and what the oracle taught about testing.
