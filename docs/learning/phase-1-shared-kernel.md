# Phase 1 — The shared kernel

*Learning Rust through the port, part 2 of 10.*

| | |
|---|---|
| **What this phase built** | The cross-cutting machinery every later phase depends on: configuration loading, environment gating, build info, RFC 7807 problem documents, the cache facade with its key composer, rate-limit partition keys, and the `Module` trait. |
| **Commit** | `719db0f` — `git show 719db0f --stat` |
| **Files to open** | `crates/shared-kernel/src/{lib,module,environment,build_info,config,errors,redaction,traffic_control}.rs` and `caching/{mod,key,options,facade}.rs` |
| **Rust ideas introduced** | Enums with data, `Option` and `Result`, `?`, `thiserror`, traits and trait objects, generics and bounds, closures and futures, `Arc`, type erasure with `Any`, task-locals, custom `Debug`, `serde` attributes, `#[cfg(test)]` unit tests |

This is the phase where the Rust *type system* starts doing work the C# code
did with conventions and a DI container. Read the crate-level doc in
`lib.rs` first; its second paragraph is the thesis of the whole phase:

> Where the C# original used an interface purely so the DI container could
> hand out an implementation, this port often uses a concrete type instead —
> Rust does not need the indirection, and generic methods (`GetOrAddAsync<T>`)
> cannot live on a trait object at all. Where the interface carried real
> polymorphism, such as `Module`, it stays a trait.

Keep that distinction in mind as you read: **trait when there is real
polymorphism, concrete type otherwise.**

---

## 1. `Environment`: an enum that carries data

C# gates behavior on `IHostEnvironment.IsDevelopment()` and a string compare
for `Demo`. Rust models the closed set of interesting values *and* the open
set of everything else in one type (`environment.rs`):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Environment {
    Development,
    Demo,
    Production,
    /// Any other name the host was started with.
    Other(String),
}
```

Three things to notice.

**Variants can hold data.** `Other(String)` keeps the literal name, so an
unknown environment like `Staging` is preserved verbatim rather than collapsed
to a default. A C# enum cannot do this; you would need a class hierarchy or a
record with a discriminator.

**`#[derive(...)]` generates trait implementations.** `Debug` gives `{:?}`
formatting, `Clone` gives `.clone()`, and `PartialEq`/`Eq` give `==`. You ask
for each explicitly. A type without `Clone` cannot be copied, which is the
default and is what makes ownership tractable.

**`matches!` is the idiomatic membership test:**

```rust
pub fn exposes_operational_metadata(&self) -> bool {
    matches!(self, Self::Development | Self::Demo)
}
```

The `|` inside a pattern means "either variant." This one method gates
Swagger, health metadata, the in-memory user store, and the dev signing key
throughout the port.

**`Option` combinators replace null checks.** Reading the variable:

```rust
pub fn from_process() -> Self {
    std::env::var(ENVIRONMENT_VARIABLE)
        .ok()                                        // Result -> Option
        .filter(|name| !name.trim().is_empty())      // blank counts as unset
        .map_or(Self::Production, |name| Self::from_name(&name))
}
```

`std::env::var` returns `Result<String, VarError>`. `.ok()` throws away the
error kind and gives `Option<String>`; `.filter` turns a blank into `None`;
`.map_or(default, f)` applies `f` if `Some` and yields the default if `None`.
No `if` statements, and no chance of forgetting the unset case, because
`Option` is not a `String` and the compiler will not let you use it as one.

Finally, `impl fmt::Display for Environment` is how a type opts in to `{}`
formatting and `.to_string()`. `Debug` (`{:?}`) is for developers; `Display`
(`{}`) is for output. The port keeps them distinct everywhere.

---

## 2. Errors: `Result`, `?`, and `thiserror`

Rust has no exceptions. A fallible function returns `Result<T, E>`, and the
caller must do something with the `Err` branch. `config.rs` defines what can
go wrong loading a file:

```rust
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read configuration file {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("configuration file {path} is not valid JSON")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}
```

- `thiserror::Error` is a derive macro that implements the standard `Error`
  trait and `Display` from the `#[error("...")]` attributes. The alternative
  is writing both by hand, which `errors.rs` does for `ApiError` so you can see
  the expanded form.
- `#[source]` links the underlying cause, so `error.source()` walks the chain
  the way `InnerException` does.
- Variants with named fields (`Read { path, source }`) are struct-like
  variants. They are pattern-matched with the same braces.

The `?` operator is how a function propagates an error to its caller:

```rust
let value = serde_json::from_str(&text).map_err(|source| ConfigError::Parse {
    path: path.to_path_buf(),
    source,
})?;
```

Read it right to left: `from_str` yields `Result<Value, serde_json::Error>`;
`map_err` rewraps the error as a `ConfigError`; `?` returns early with that
error if it is `Err`, or unwraps the `Ok` value if not. `?` also applies an
implicit `From` conversion when the error types differ, which Phase 7 uses
heavily.

**A missing file is not an error.** Look at `read_json_layer`:

```rust
let text = match std::fs::read_to_string(path) {
    Ok(text) => text,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
    Err(source) => return Err(ConfigError::Read { path: path.to_path_buf(), source }),
};
```

The `if` after a pattern is a **match guard**. The function's return type is
`Result<Option<Value>, ConfigError>`: three outcomes — a value, no file, or a
broken file — each of which the caller must handle separately. That is the
whole point of nesting `Option` in `Result` rather than returning null.

---

## 3. Configuration: recursion over `serde_json::Value`, and why keys are lowercased

The C# host reads `appsettings.json`, then `appsettings.{Environment}.json`,
then environment variables, and its keys are case-insensitive. The port uses
the `figment` crate for layering and does the case-folding itself:

```rust
fn lowercase_keys(value: Value) -> Value {
    match value {
        Value::Object(entries) => Value::Object(
            entries.into_iter()
                .map(|(key, nested)| (key.to_lowercase(), lowercase_keys(nested)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(lowercase_keys).collect()),
        scalar => scalar,
    }
}
```

This is a recursive function over an enum, and it takes `value` **by value**
(no `&`), so it consumes its input and builds a new one; nothing is cloned.
The last arm, `scalar => scalar`, binds anything not matched above and returns
it unchanged.

The module doc explains why this matters: without it, `SERVICENAME` from the
environment and `ServiceName` from a file would land under different keys and
the environment would silently fail to override the file.

**The bug only running it found.** `environment_layer` splits `A__B__0__C` on
`__` and then rewrites any map whose keys are exactly `0..n` into an array,
because `Identity__InMemoryUsers__0__Username=demo` otherwise produces a map
keyed `"0"` that will not deserialize into a `Vec`. The test
`index_segments_become_arrays_so_lists_actually_bind` pins it. Read
`as_index_sequence` for a compact example of `Vec<(usize, &Value)>`,
`sort_by_key`, and `enumerate` used to check density.

**Generic binding.** `section_or_default` is the port of
`Configure<TOptions>`:

```rust
pub fn section_or_default<T>(&self, key: &str) -> T
where
    T: DeserializeOwned + Default,
{
    let normalized = normalize_key(key);
    if self.figment.find_value(&normalized).is_err() {
        return T::default();
    }
    match self.figment.extract_inner(&normalized) {
        Ok(section) => section,
        Err(error) => {
            tracing::warn!(section = key, %error, "configuration section could not be bound; using defaults");
            T::default()
        }
    }
}
```

`T` is a type parameter with two **bounds**: it must be deserializable and
must have a default. The `where` clause is just a readable place to put
bounds. `T::default()` calls the trait method on whatever type the caller
asked for, and the caller often does not name it — `let jwt: JwtAuthOptions =
config.section_or_default("Jwt");` lets inference fill `T` from the
annotation on the left.

**`tracing` fields.** `%error` means "format with `Display`"; `?error` would
mean `Debug`. That distinction is the whole reason the next section exists.

---

## 4. Custom `Debug` to keep secrets out of logs

`redaction.rs` is short and worth reading in full. The idea: `#[derive(Debug)]`
prints every field, and a `tracing` call that writes `?config` calls `Debug`.
So the types that hold secrets write their own:

```rust
impl std::fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("environment", &self.environment)
            .field("values", &crate::redaction::REDACTED)
            .finish()
    }
}
```

`debug_struct` is a builder that produces the same `Name { field: value }`
shape the derive would, so the log line stays readable. The test
`the_configuration_does_not_print_the_values_it_holds` formats the config with
`{:?}` and asserts the password and the connection string are absent. You will
meet this pattern six more times in Phase 5.

---

## 5. `ProblemDetails`: a struct that becomes an HTTP response

`errors.rs` ports the C# exception handler's four document shapes. The struct
shows the `serde` attributes you need for wire-exact JSON:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProblemDetails {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_uri: Option<String>,
    pub title: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<BTreeMap<String, Vec<String>>>,
    #[serde(rename = "traceId")]
    pub trace_id: String,
}
```

- `rename = "type"` because `type` is a Rust keyword and cannot be a field
  name.
- `skip_serializing_if = "Option::is_none"` omits the member entirely. The
  test `absent_fields_are_omitted_rather_than_null` explains why: ASP.NET
  omits unset members, and an explicit `null` would be a wire difference.
  Contrast this with the API models in Phase 2, where nulls **are** written.
- `BTreeMap` rather than `HashMap` so the `errors` object is byte-stable
  across runs. Small choices like this are what make a parity diff readable.

Turning it into a response is one trait:

```rust
impl IntoResponse for ProblemDetails {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let mut response = (status, Json(self)).into_response();
        response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
        response
    }
}
```

`IntoResponse` is axum's contract for "anything a handler can return." Tuples
like `(StatusCode, Json<T>)` already implement it, so the method builds on
that and then overwrites the content type. Every handler in later phases
returns `Result<Json<T>, ApiError>`, and `ApiError` implements the same trait,
so the framework knows how to render both arms.

**The `'static` string table.** Look at `default_type_for`. It is a `match`
that returns `&'static str` because `HeaderValue::from_static` and the
defaults table both want strings that live forever, and `format!` produces an
owned `String` that does not. The comment in the source says exactly this.
When you see a slightly repetitive `match` over literals in Rust, this
lifetime requirement is usually why.

**`ApiError` and `From`.** The enum at the bottom of the file is the error
half of every handler's `Result`. Each `impl From<X> for ApiError` is what
lets `?` convert a `RepositoryError` or a `MalformedRequest` into an
`ApiError` without a `map_err`. The `From<Infallible>` impl with its empty
`match infallible {}` is a small idiom: `Infallible` has no variants, so the
match has no arms and the function can never actually run.

---

## 6. A request-scoped value with `tokio::task_local!`

C# reaches for `HttpContext.TraceIdentifier` or `AsyncLocal<T>`. Rust's
equivalent for async code is a task-local:

```rust
tokio::task_local! {
    static REQUEST_TRACE_ID: String;
}

pub async fn with_trace_id<F>(trace_id: String, work: F) -> F::Output
where
    F: std::future::Future,
{
    REQUEST_TRACE_ID.scope(trace_id, work).await
}

pub fn current_trace_id() -> String {
    REQUEST_TRACE_ID.try_with(Clone::clone).unwrap_or_else(|_| new_trace_id())
}
```

`scope` runs a future with the value set; anything that future awaits, at any
depth, can read it with `try_with`. `F::Output` is an **associated type** —
"whatever this future produces." Phase 4's `request_id` middleware wraps every
request in `with_trace_id`, which is why a 404 from a handler, a rejection
from an extractor, and the `x-trace-id` header all agree.

`Clone::clone` passed as a function is a common shorthand for `|s| s.clone()`.

---

## 7. The `Module` trait, and why it is generic

```rust
pub trait Module<S>: Send + Sync
where
    S: Clone + Send + Sync + 'static,
{
    fn name(&self) -> &'static str;
    fn prefix(&self) -> &'static str;
    fn router(&self) -> Router<S>;
}

pub fn mount_all<S>(mut router: Router<S>, modules: &[Box<dyn Module<S>>]) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    for module in modules {
        router = router.nest(module.prefix(), module.router());
    }
    router
}
```

This is the port of `IModule`, minus `RegisterServices` (there is no container
to populate). Read the doc comment for why it is `Module<S>` rather than
`Module` over a concrete state type: the shared kernel is the root of the
dependency graph and cannot name `AppState`, which lives two crates up. So the
trait is generic over "whatever state the host has," and the host fills in
`S = AppState`.

The bounds are the ones axum requires of any shared state. `'static` here
means "contains no borrowed references," not "lives forever."

`Box<dyn Module<S>>` is a **trait object**: a heap-allocated value of any type
implementing the trait, dispatched at runtime through a vtable. This is the
one place in the kernel where polymorphism is real (five modules, one list),
and so it is a trait. The `&[...]` parameter is a **slice** — a borrowed view
of a contiguous sequence, which accepts a `Vec`, an array, or any part of one.

---

## 8. The cache facade: the hardest file in the phase

`caching/facade.rs` is where the C# `ICacheFacade` interface with generic
methods becomes a concrete struct, and it deserves a slow read.

### 8a. Why not a trait?

A Rust trait with a generic method — `fn get_or_add<T>(...)` — is not
**object safe**: you cannot make a `dyn CacheFacade` from it, because a
vtable would need one entry per `T`. The C# interface exists only so DI can
inject it. There is one implementation. So it is a struct.

### 8b. Type erasure with `Any`

One cache holds every module's shapes, the way `IMemoryCache` stores boxed
objects:

```rust
struct CachedEntry {
    value: Arc<dyn Any + Send + Sync>,
    absolute: Duration,
    sliding: Option<Duration>,
    tags: Arc<[String]>,
}
```

`dyn Any` is a trait object you can later **downcast**:
`entry.value.downcast_ref::<T>().cloned()` yields `Option<T>` — `None` if the
stored type is not `T`. `Arc` is an atomically reference-counted pointer;
cloning it bumps a counter rather than copying the value, which is what makes
handing the same cached `Vec<TrackApiModel>` to many requests cheap.
`Arc<[String]>` is a shared, immutable slice — cheaper than `Arc<Vec<String>>`
when nothing will ever push to it.

### 8c. Closures and futures as parameters

```rust
pub async fn try_get_or_add<T, E, F, Fut>(
    &self,
    key: &CacheKey,
    factory: F,
    options: Option<CacheEntryOptions>,
) -> Result<Option<T>, E>
where
    T: Clone + Send + Sync + 'static,
    E: Clone + Send + Sync + 'static,
    F: FnOnce() -> Fut + Send,
    Fut: Future<Output = Result<Option<T>, E>> + Send,
```

Four type parameters. `F` is the factory, a closure called at most once
(`FnOnce`) that returns a future `Fut`, whose output is the repository's
result. Callers in Phase 6 write `|| async { state.repositories.albums.get_by_id(id).await }`
and the compiler infers all four. The `Send` bounds are what let the future
move between tokio worker threads.

### 8d. Single flight through `moka`

```rust
let outcome = self.entries
    .try_get_with(cache_key, async move {
        match factory().await {
            Ok(Some(value)) => { register_tags(&index, &index_key, &effective.tags); Ok(CachedEntry { ... }) }
            Ok(None) => Err(NotStored::Missing),
            Err(error) => Err(NotStored::Failed(error)),
        }
    })
    .await;

match outcome {
    Ok(entry) => Ok(entry.value.downcast_ref::<T>().cloned()),
    Err(shared) => match shared.as_ref() {
        NotStored::Missing => Ok(None),
        NotStored::Failed(error) => Err(error.clone()),
    },
}
```

`moka`'s `try_get_with` guarantees that concurrent callers for one key run the
initializer once and all receive the same outcome — which is exactly the
single-flight the C# semaphore tried and failed to provide (fix F7). The trick
here is routing both "found nothing" and "the factory failed" through the
**error** channel, because both mean "do not store this." The doc comment
titled *Why the outcome travels as the cache's error* records the bug that
motivated it: an earlier version parked the error in a `Mutex` beside the
call, so only the caller that ran the factory saw it and the others were told
the lookup found nothing.

`async move { ... }` creates a future that takes ownership of what it uses
(`index`, `index_key`, `effective`, `factory`). Without `move` the future
would borrow them, and a borrow cannot outlive the function that holds them.

### 8e. The tag index and the eviction listener

```rust
let entries = Cache::builder()
    .expire_after(EntryExpiry)
    .eviction_listener(move |key: Arc<String>, entry: CachedEntry, _cause: RemovalCause| {
        forget_tags(&eviction_tags, key.as_str(), &entry.tags);
    })
    .build();
```

A `DashMap<String, HashSet<String>>` maps each tag to the keys carrying it,
and the listener removes a key from the index when the cache drops it. This is
fix F1: the C# `RemoveByTagAsync` was a no-op. The test
`removing_a_tag_invalidates_every_entry_carrying_it` is the acceptance test,
and its comment tells you that reverting to a no-op fails it with 3 stale hits
instead of 6 fresh ones.

`EntryExpiry` implements moka's `Expiry` trait so that each entry carries its
own lifetime, as the original sets expiry per entry rather than per cache.

### 8f. Jitter without a panic

```rust
fn apply_jitter(ttl: Duration, jitter_percent: f64) -> Duration {
    if ttl.is_zero() || jitter_percent <= 0.0 { return ttl; }
    let roll: f64 = rand::random();
    let jitter = (roll - 0.5) * 2.0 * jitter_percent;
    let adjusted = ttl.mul_f64((1.0 + jitter).max(0.0));
    if adjusted.is_zero() { ttl } else { adjusted }
}
```

`Duration` cannot be negative, and `mul_f64` with a negative factor panics.
C# would produce a negative `TimeSpan` and fall back. The `.max(0.0)` clamp is
the port's answer, and `an_absurd_jitter_percent_cannot_panic` runs it a
hundred times to prove it. Small type differences like this are a recurring
theme: Rust's types rule out some states, and you must decide what to do at
the boundary instead.

---

## 9. Keys, options, and the rest

- `caching/key.rs`: a plain struct with an `impl Display` that renders the
  nine-segment key. `Option<String>` segments render as empty through
  `self.tenant.as_deref().unwrap_or_default()` — `as_deref` turns
  `Option<String>` into `Option<&str>`, and `unwrap_or_default` yields `""`.
  `CachePartitions<'a>` holds `Option<&'a str>` — borrowed, because it is only
  ever built for one call.
- `caching/options.rs`: `#[serde(default)]` on a struct means every missing
  member takes its `Default` value, and the manual `impl Default for
  CacheOptions` sets the original's defaults (enabled, `L1`, 300 s). Members
  the original declares but never reads are kept and labelled as such.
- `traffic_control.rs`: `partition_key` iterates a fixed array of
  `(claim, prefix)` tuples and returns early on the first hit. `IpAddr` is an
  enum (`V4`/`V6`), and `normalize_address` matches on it. `RequestClaims` is
  a deliberately tiny trait so the kernel can ask "do you have claim X" without
  knowing what a principal is.
- `build_info.rs`: `env!("CARGO_PKG_VERSION")` — the version from `Cargo.toml`,
  pasted in at compile time. That single line replaces the C# reflection over
  `AssemblyInformationalVersion`.

---

## 10. How the tests are organized

Every file in this phase ends with:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    // ...
}
```

`#[cfg(test)]` compiles the module only under `cargo test`. `use super::*`
imports everything from the enclosing module, including private items — unit
tests in Rust can see private functions, which is why `apply_jitter` and
`lowercase_keys` are tested directly. Async tests use `#[tokio::test]`, and the
single-flight tests use `tokio::join!` to run six futures concurrently on one
task and an `AtomicUsize` to count factory calls without a lock.

---

## Try it

1. **Watch F1 matter.** In `facade.rs`, make `remove_by_tag` return
   immediately, run `cargo test -p shared-kernel removing_a_tag`, and read the
   assertion message. Revert.
2. **Make the compiler reject a trait object.** Add
   `fn peek<T: Clone>(&self) -> Option<T>;` to the `Module` trait, then try to
   compile. The error explains object safety better than any prose.
3. **Trace a `?`.** In `config.rs`, change `read_json_layer` to return
   `Result<Option<Value>, std::io::Error>` and see every call site that stops
   compiling. Then undo it.
4. **Add an environment.** Give `Environment` a `Staging` variant and follow
   the compiler's non-exhaustive-match errors through `name()` and the tests.
5. **Print a redacted config.** In a scratch test, build an `AppConfig` with
   `from_figment` and `println!("{config:?}")`. Then temporarily replace the
   manual `Debug` with `#[derive(Debug)]` and print again.
6. **Read the failing-caller test.** `every_coalesced_caller_is_told_the_factory_failed`
   is a good model for writing a concurrency test in Rust: six futures, one
   sleep, one atomic counter, and an assertion per caller.

## Vocabulary for this phase

| C# / .NET | Rust | Notes |
|---|---|---|
| `enum` + class hierarchy | `enum` with data-carrying variants | Exhaustively matched |
| `null` | `Option<T>` | `None` is a value, not an absence |
| Exception | `Result<T, E>` + `?` | Errors are values |
| `InnerException` | `#[source]` / `Error::source()` | |
| `ToString()` / `override ToString` | `impl Display` | `{}` |
| Debugger display | `impl Debug` | `{:?}`; derive or hand-write |
| Interface (for DI) | Usually a concrete type | Trait only for real polymorphism |
| Interface (polymorphic) | `trait` + `Box<dyn Trait>` | Trait objects need object safety |
| Generic method on interface | Generic method on a struct | Cannot live on a `dyn` |
| `object` / boxing | `Arc<dyn Any + Send + Sync>` + `downcast_ref` | |
| Shared reference / GC | `Arc<T>` | Atomic refcount; `Clone` is cheap |
| `Func<Task<T>>` | `F: FnOnce() -> Fut, Fut: Future<Output = T>` | |
| `AsyncLocal<T>` | `tokio::task_local!` | Scoped with `.scope(value, future)` |
| `IConfiguration` | `figment::Figment` behind `AppConfig` | Keys lowercased on load |
| `[JsonPropertyName]` | `#[serde(rename = "...")]` | |
| `JsonIgnoreCondition.WhenWritingNull` | `#[serde(skip_serializing_if = "Option::is_none")]` | Per field |
| `IMemoryCache` | `moka::future::Cache` | `try_get_with` is single-flight |
| `SemaphoreSlim` per key | (not needed) | The cache coalesces |
| `ConcurrentDictionary` | `dashmap::DashMap` | Sharded lock-free-ish map |
| `Interlocked.Increment` | `AtomicUsize::fetch_add` | |
| `[Fact]` in a test project | `#[test]` inside `#[cfg(test)] mod tests` | Sees private items |

**Next:** [Phase 2 — persistence contracts](phase-2-persistence.md): entities,
API models, validators, and the repository traits.
