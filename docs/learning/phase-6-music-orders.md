# Phase 6 — Music and Orders

*Learning Rust through the port, part 7 of 10.*

| | |
|---|---|
| **What this phase built** | Twenty-two read endpoints across two modules, cache-aside services behind them, the `music.read`/`orders.read` + `tenant.scoped` guards, the full authorization matrix in tests, and the first run of the parity harness against the C# original. |
| **Commit** | `fdd81e1` — `git show fdd81e1 --stat` |
| **Files to open** | `crates/module-music/src/{lib,endpoints,services}.rs`, `crates/module-orders/src/{lib,endpoints,services}.rs`, `crates/module-reporting/src/lib.rs`, `crates/api/tests/modules.rs`, `crates/shared-kernel/src/caching/facade.rs` (`try_get_or_add`), `crates/shared-kernel/src/data.rs` (`found`) |
| **Rust ideas introduced** | Module-crate layout and public surface, type aliases, generic helpers bounded by axum's `Handler` trait, services as free functions, async closures captured by reference, higher-order async functions, `?` inside `async` blocks, `#[serde(flatten)]`, `Option<()>`, the `_name` idiom |

Everything hard was built in Phases 1 to 5. This phase is about how thin a
module becomes once the kernel and the persistence layer exist — and about
the moment the port stopped trusting its own tests and started diffing
against the running original.

---

## 1. A module crate, top to bottom

`crates/module-music/src/lib.rs` is 41 lines:

```rust
mod endpoints;
mod services;

pub const NAME: &str = "Music";
pub const PREFIX: &str = "/api/music";

#[derive(Debug, Clone, Copy, Default)]
pub struct MusicModule;

impl Module<AppState> for MusicModule {
    fn name(&self) -> &'static str { NAME }
    fn prefix(&self) -> &'static str { PREFIX }
    fn router(&self) -> Router<AppState> {
        health::routes(NAME).merge(endpoints::routes())
    }
}
```

`mod endpoints;` and `mod services;` without `pub` make both modules private
to the crate. The doc comment says this is how the port reproduces the C#
`PublicSurfaceTests`: the crate exports its module type, two constants, and
nothing else. `Router::merge` combines two routers at the same level (as
opposed to `nest`, which adds a prefix).

`MusicModule` is a unit struct deriving `Copy`, so it can be passed around
freely; `Box::new(module_music::MusicModule)` in the host's registry is the
only place it is constructed.

`module-reporting` is the degenerate case: no `endpoints.rs`, no
`services.rs`, just `health::routes(NAME)`. Its doc explains that this is
finished, not unfinished — the original has no reporting endpoints either.

---

## 2. Endpoints: type aliases and a generic route helper

```rust
type Answer<T> = Result<Json<T>, ApiError>;
type Caller = Authorized<MusicRead>;

async fn album_by_id(
    _caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Answer<AlbumApiModel> {
    found(services::album_by_id(&state, id).await?)
}
```

- **Type aliases** are pure renames; `Answer<T>` is exactly
  `Result<Json<T>, ApiError>` everywhere. They keep fifteen signatures
  readable.
- **`_caller`**: a parameter named with a leading underscore is one you
  require but do not read. The guard runs in the extractor (Phase 5), so the
  handler needs the argument to exist and needs nothing from it. The
  underscore silences the unused-variable warning while keeping the type in
  the signature — which is the whole point.
- **`Path(id): Path<i32>`** destructures the route parameter. The `{id}`
  syntax in the route string is axum 0.8's.
- **`found(...)`** from `shared_kernel::data` maps `Option<T>` to
  `Result<Json<T>, ApiError>`, turning `None` into `ApiError::NotFound`. It is
  the port of `album is not null ? Ok(album) : NotFound()`.
- The `?` after `.await` converts a `RepositoryError` into an `ApiError` via
  the `From` impl from Phase 1, so a database failure becomes a 500 without
  any code in the handler.

**Trailing slashes.** The original registers collection routes as `albums/`
and ASP.NET treats the slash as insignificant; axum does not. The helper:

```rust
fn collection<H, T>(router: Router<AppState>, path: &str, handler: H) -> Router<AppState>
where
    H: axum::handler::Handler<T, AppState> + Clone,
    T: 'static,
{
    router
        .route(path, get(handler.clone()))
        .route(&format!("{path}/"), get(handler))
}
```

`Handler<T, S>` is the trait axum implements for every function whose
arguments are extractors; `T` is a type-level encoding of the argument list
that you never name yourself. The bound lets `collection` accept any handler
and register it twice. The doc explains why this is preferred over a global
path-normalizing layer: a global rewrite would also affect routes the original
leaves alone.

---

## 3. Services as functions, and cache-aside in Rust

The C# services are classes so the container can inject a repository, a
cache, a key composer and a validator. Here all of those live in `AppState`,
so the services are free functions over `&AppState`:

```rust
pub(crate) async fn album_by_id(state: &AppState, id: i32) -> RepositoryResult<Option<AlbumApiModel>> {
    let key = state.cache_keys.compose(MODULE, "album", VERSION, &format!("by-id:{id}"));

    state
        .cache
        .try_get_or_add(
            &key,
            || async { state.repositories.albums.get_by_id(id).await },
            Some(entry_options(ALBUM_TAGS)),
        )
        .await
}
```

Study the closure. `|| async { ... }` is a closure returning a future. It
captures `state` and `id` **by reference** — no `move` — which is allowed
because the future is awaited before `album_by_id` returns, so the borrow
cannot outlive its source. The compiler checks that, and if you tried to
spawn the future onto another task it would refuse. This is the borrow
checker doing lifetime analysis on async code, and most of the time you
never notice.

Collection reads convert inside the factory:

```rust
|| async {
    let entities = state.repositories.albums.get_all().await?;
    Ok(Some(convert_all(&entities)))
}
```

`?` works inside an `async` block exactly as inside a function: the block's
return type is `Result<Option<Vec<AlbumApiModel>>, RepositoryError>` and `?`
returns early with the error. `Ok(Some(...))` is the shape `try_get_or_add`
wants — `None` means "not found, do not cache."

**Why `try_get_or_add` exists.** The commit message says it: the Phase 1
`get_or_add` took a factory returning `Option<T>`, so a database outage would
have read as an empty collection rather than a 500. Adding a fallible variant
is the reason the Phase 1 facade has two entry points and why
`RepositoryError` is `Clone` (every coalesced caller gets the error).

---

## 4. A higher-order async helper

The seven track collections share one shape, so `services.rs` factors it:

```rust
async fn tracks_cached<F, Fut>(state: &AppState, discriminator: &str, fetch: F)
    -> RepositoryResult<Vec<TrackApiModel>>
where
    F: FnOnce() -> Fut + Send,
    Fut: Future<Output = RepositoryResult<Vec<shared_persistence::entities::Track>>> + Send,
{
    let key = state.cache_keys.compose(MODULE, "track", VERSION, discriminator);
    let tracks = state.cache
        .try_get_or_add(&key, || async {
            let entities = fetch().await?;
            Ok(Some(convert_all(&entities)))
        }, Some(entry_options(TRACK_TAGS)))
        .await?;
    Ok(tracks.unwrap_or_default())
}

pub(crate) async fn tracks_by_album(state: &AppState, id: i32) -> RepositoryResult<Vec<TrackApiModel>> {
    tracks_cached(state, &format!("by-album:{id}"), || async {
        state.repositories.tracks.get_by_album_id(id).await
    }).await
}
```

Two generic parameters — the closure type and the future type — because in
Rust every closure and every `async` block has its own anonymous type. The
`Future` trait is in scope without an import because the crate is edition
2024, whose prelude includes it. `unwrap_or_default()` turns
`Option<Vec<_>>` into a `Vec`, empty if `None`.

The Orders module has the identical pattern as `lines_cached`.

---

## 5. One endpoint that serves an entity

`GET /api/orders/invoice-lines/{id}` returns an *entity* in the original,
navigation members and all. The port's entities have no navigation fields, so
the endpoint adds them back locally:

```rust
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct InvoiceLineEntityResponse {
    #[serde(flatten)]
    line: shared_persistence::entities::InvoiceLine,
    invoice: Option<()>,
    track: Option<()>,
}
```

`#[serde(flatten)]` splices the inner struct's fields into this object rather
than nesting them. `Option<()>` is a field that can only ever serialize as
`null` — `()` is the unit type, and `None` is the only value the port
constructs. This is a private struct in `endpoints.rs`, which is exactly the
right scope for a shape one route needs.

---

## 6. What the oracle found

The commit message for this phase is the one to read in full. The harness
(`tools/parity-diff.py`, arriving two phases early) compared 45 routes and
found four real bugs, none of which the test suite would have caught because
*the tests asserted what had been implemented rather than what the original
does*:

| Bug | Rust change |
|---|---|
| Dates were missing their `Z` | `NaiveDateTime` → `DateTime<Utc>` in entities and models (Phase 2's files); chrono's serde impl writes the zone |
| `ArtistName` was null on `/albums/artist/{id}` | `AlbumRepository::get_by_artist_id` now returns `Vec<AlbumApiModel>` with the join's name, the one place the trait departs from the C# signature |
| Invoice-line entity response lacked `Invoice`/`Track` | `InvoiceLineEntityResponse` with `flatten` and two `Option<()>` |
| The wrong database file was bundled | `data/chinook.db` replaced; `seed_integrity.rs` now guards it |

The type-system lesson in the second bug is worth stating: the C# repository
returned `List<Album>` but had `Include`d the artist, and the conversion
happened to read the navigation. Rust entities have no navigation, so the
extra data has to appear in the *return type*. When a port changes a
signature, it is often because the original was carrying information in a
place the type did not declare.

---

## 7. The tests

`crates/api/tests/modules.rs` seeds four logins mirroring the C# fixtures and
walks every route through the authorization matrix: no token (401 with a
challenge), wrong permission (403), tenant mismatch (403), and success. It
reuses the `TestResponse` helper shape from Phase 4. The service-level unit
tests pin the cache key strings so a renamed discriminator cannot silently
split one entry into two.

---

## Try it

1. **Add a route.** Register `/tracks/composer/{name}` in
   `module-music/src/endpoints.rs` backed by the repository method from the
   Phase 3 exercise. Leave out the `_caller: Caller` parameter first and
   observe that it compiles — then run `cargo test -p architecture-tests
   route_guards` and read why it fails.
2. **Move the borrow.** In `album_by_id`, change `|| async { ... }` to
   `move || async move { ... }` and see whether it still compiles (it does)
   and what would break if `state` were `AppState` instead of `&AppState`.
3. **Make the cache lie.** Change `entry_options` to use a 1-second TTL,
   run the host, request `/api/music/albums` twice with a `sleep 2` between,
   and watch the debug log for the second factory call.
4. **See `flatten` on the wire.** Curl `/api/orders/invoice-lines/1` and
   `/api/orders/invoice-lines` with a token and compare the member sets.
5. **Trace the `?`.** In `all_albums`, replace `.await?` with `.await` and
   read the type error; it tells you the exact `Result` nesting at that
   point.
6. **Count the cache keys.** Run the host with `RUST_LOG=debug`, hit a few
   routes, and grep the log for `music:album` — then compare with the
   discriminators pinned in `services.rs`'s tests.

## Vocabulary for this phase

| C# / .NET | Rust | Notes |
|---|---|---|
| Module project with `internal` classes | Crate with private `mod`s | Public surface is `pub` items only |
| `using AlbumResult = ...` | `type Answer<T> = Result<Json<T>, ApiError>;` | Generic aliases allowed |
| Service class with injected deps | Free `async fn` over `&AppState` | |
| `Func<Task<T>>` factory | `\|\| async { ... }` | Borrows unless `move` |
| Generic method taking a delegate | `fn f<F, Fut>(...) where F: FnOnce() -> Fut, Fut: Future` | Two params per closure |
| `?? new List<T>()` | `.unwrap_or_default()` | |
| `[JsonExtensionData]`-style inlining | `#[serde(flatten)]` | |
| Always-null property | `Option<()>` | Only `None` is constructed |
| Unused required parameter | `_name: Type` | Warning-free |
| `Include(...)` side effect | Visible in the return type | Types carry what the data has |

**Next:** [Phase 7 — Administration](phase-7-administration.md): the write
surface, a custom JSON extractor, and layered error types.
