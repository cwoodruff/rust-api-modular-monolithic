# Phase 4 — The host serves HTTP

*Learning Rust through the port, part 5 of 10.*

| | |
|---|---|
| **What this phase built** | The axum host: the middleware pipeline in the original's order, the root endpoint, `/health` and `/data-health` for all five modules, Swagger gating, the rate limiter, and the application state. `cargo run -p api` answers on port 5043. |
| **Commit** | `0f6d49b` — `git show 0f6d49b --stat` (error shapes corrected in `c051cb7`; trace id reworked in `94a83e5`) |
| **Files to open** | `crates/api/src/{main,lib,app,composition,middleware,routes,openapi}.rs`, `crates/shared-kernel/src/{health,data}.rs`, `crates/shared-persistence/src/state.rs`, `appsettings*.json` |
| **Rust ideas introduced** | `#[tokio::main]`, `anyhow` and `.context()`, `tracing` setup, axum routers, extractors and handlers, `IntoResponse`, `tower` layers and their order, `from_fn` middleware, `Arc`-based shared state, `DashMap` entry API, `Mutex` poisoning, `Instant`, `Box<dyn Any>` downcasting, graceful shutdown, `oneshot` tests |

If Phase 1 was about types, this phase is about **composition**: how the
pieces get built once at startup and how a request flows through them. The
C# host does all of this with a DI container and a middleware pipeline. The
Rust host does it with a function that returns a `Router`.

---

## 1. The binary is a thin wrapper over a library

`lib.rs` explains the split: the binary is thin so the integration tests can
build the very same router the server runs. That is the `WebApplicationFactory`
seam, obtained from the other direction.

`main.rs` is about 60 lines and is worth reading whole. The pieces:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
```

`#[tokio::main]` turns an `async fn main` into a normal `main` that starts
the tokio runtime and blocks on the future. The `rt-multi-thread` feature
from the workspace manifest is what gives it a thread pool.

`anyhow::Result<()>` is `Result<(), anyhow::Error>`, a catch-all error type
for applications (as opposed to `thiserror`, which is for libraries that want
callers to match on variants). With it you can write:

```rust
let listener = tokio::net::TcpListener::bind(address)
    .await
    .with_context(|| format!("failed to bind {address}"))?;
```

`.with_context` wraps whatever error occurred with a message, and `?` sends it
up to `main`, which prints the chain and exits non-zero. That is what makes
the Docker container in Phase 9 fail with a readable reason.

**Logging** is `tracing` with a subscriber configured from `RUST_LOG`, falling
back to `info,tower_http=warn`. `tracing` is structured: every macro call
carries key-value fields, which is why later phases write
`tracing::warn!(username = %request.username, "failed login attempt")` rather
than formatting a string.

**Graceful shutdown** is one future:

```rust
axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
    .with_graceful_shutdown(shutdown_signal())
    .await
```

`shutdown_signal` awaits `tokio::signal::ctrl_c()`. The
`into_make_service_with_connect_info::<SocketAddr>()` call is the one that
makes the client address available to the rate limiter — without it,
`ConnectInfo` would be absent from every request.

---

## 2. Application state: what the DI container held

`crates/shared-persistence/src/state.rs`:

```rust
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub environment: Environment,
    pub cache: Arc<CacheFacade>,
    pub cache_keys: Arc<CacheKeyComposer>,
    pub database: Arc<dyn DatabaseProbe>,
    pub repositories: Repositories,
}
```

axum hands every handler a **clone** of the state, so the state must be
`Clone` and cheap to clone. Every field is behind an `Arc` (or is a small
enum), so a clone is a handful of reference-count increments.

`Arc<dyn DatabaseProbe>` and the ten `Arc<dyn XRepository>` fields in
`Repositories` are the trait objects Phase 2 proved were legal. The host is
the only place their concrete types are named — `composition.rs`:

```rust
Ok(AppState {
    cache: Arc::new(CacheFacade::new(cache_options)),
    cache_keys: Arc::new(CacheKeyComposer::new(&config)),
    database: Arc::new(SqliteDatabaseProbe::new(pool.clone())),
    repositories: Repositories {
        albums: Arc::new(SqliteAlbumRepository::new(pool.clone())),
        // ... nine more
        invoice_lines: Arc::new(SqliteInvoiceLineRepository::new(pool)),
    },
    config: Arc::new(config),
    environment,
})
```

`Arc::new(SqliteAlbumRepository::new(...))` has type
`Arc<SqliteAlbumRepository>`, and Rust **coerces** it to
`Arc<dyn AlbumRepository>` at the field boundary. This is the whole DI
container: one struct literal, written once, with no reflection and no
runtime resolution. The last `pool` is moved rather than cloned because
nothing after it needs the handle.

**Why `AppState` lives in `shared-persistence`, not `shared-kernel`.** The
doc comment says it: the state names the repository traits, and the kernel
is the root of the graph. The kernel instead defines a small trait,
`HealthContext`, with just the three things the health handlers need, and
`state.rs` implements it:

```rust
impl HealthContext for AppState {
    fn environment(&self) -> &Environment { &self.environment }
    fn service_name(&self) -> String { self.config.service_name() }
    fn database_probe(&self) -> Arc<dyn DatabaseProbe> { Arc::clone(&self.database) }
}
```

This is the standard Rust answer to a dependency cycle: the lower crate
defines the interface it needs, the upper crate implements it.

---

## 3. Routers, handlers, and extractors

`crates/shared-kernel/src/health.rs` builds the two routes every module
mounts:

```rust
pub fn routes<S: HealthContext>(module: &'static str) -> Router<S> {
    Router::new()
        .route("/health", get(move |State(state): State<S>| async move {
            Json(HealthResponse::new(module, HEALTHY).gated(&state))
        }))
        .route("/data-health", get(move |State(state): State<S>| async move {
            let connected = state.database_probe().can_connect().await;
            let status = if connected { DATA_HEALTHY } else { DEGRADED };
            let mut response = HealthResponse::new(module, status).gated(&state);
            if state.environment().exposes_operational_metadata() {
                response = response.with_database(connected);
            }
            Json(response)
        }))
}
```

Reading this teaches most of axum:

- **`Router<S>`** is a router whose handlers may ask for state of type `S`.
  It stays generic until someone calls `.with_state(state)`, which the host
  does last in `app.rs`.
- **A handler is any async function (or closure) whose arguments are
  extractors and whose return type implements `IntoResponse`.** Here the one
  argument is `State<S>`, destructured in the parameter list as
  `State(state)`. Phase 6 adds `Path<i32>`, `Json<T>`, and the custom
  `Authorized<P>`.
- **`move |...| async move { ... }`**: the outer `move` copies `module`
  (a `&'static str`) into the closure; the inner `async move` moves the
  extracted state into the future. Handlers must be `'static`, so borrowing
  from the enclosing function is not an option.
- **`Json(value)`** is a response type that serializes with serde and sets
  the content type.

The `HealthResponse` builder methods (`new`, `with_metadata`, `with_database`,
`gated`) take `self` by value and return `Self` — the consuming-builder
pattern, which is cheap because nothing is cloned. `gated` is what implements
"metadata only in Development and Demo."

`timestamp_utc` is a nice small example of doing arithmetic instead of
reaching for a format flag: .NET's `"O"` writes seven fractional digits, chrono
offers three or nine, so the port computes `timestamp_subsec_nanos() / 100`
and formats with `{ticks:07}`.

---

## 4. Composition and the middleware order

`app.rs` is the port of `Program.cs`. Two functions matter.

**`registry`** returns the hard-coded module list as `Vec<Box<dyn Module<AppState>>>`,
in the original's order, and `mount_all` (Phase 1) nests each one under its
prefix.

**`build`** applies the layers:

```rust
let mut app = router
    .layer(middleware::from_fn_with_state(identity, module_identity::authenticate))
    .layer(middleware::from_fn_with_state(RateLimiter::default(), own::rate_limit))
    .layer(cors())
    .layer(middleware::from_fn(own::security_headers))
    .layer(middleware::from_fn(own::json_charset))
    .layer(middleware::from_fn(own::status_code_pages))
    .layer(CatchPanicLayer::custom(own::panic_to_problem))
    .layer(TraceLayer::new_for_http())
    .layer(middleware::from_fn(own::request_id));

if secure_transport {
    app = app.layer(middleware::from_fn(own::hsts));
}

app.with_state(state)
```

**Layers wrap from the inside out.** The first `.layer()` call is the
innermost, closest to the handlers; the last is outermost and runs first on
the way in. So this list reads in *reverse* of `Program.cs`, and the comment
at the top of `middleware.rs` lists the original order so you can check.
`request_id` is outermost on purpose: it establishes the trace identifier
(Phase 1's task-local) before anything else runs, so the panic handler and
the status-code pages, which both write a `traceId`, see it.

`tower` is the middleware abstraction axum is built on; `tower-http` supplies
ready-made layers (`CatchPanicLayer`, `TraceLayer`, `CorsLayer`). Anything
custom is written as an `async fn` and wrapped with `from_fn`.

---

## 5. Writing middleware with `from_fn`

The signature every custom layer in `middleware.rs` shares:

```rust
pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    for (name, value) in SECURITY_HEADERS {
        headers.insert(name, HeaderValue::from_static(value));
    }
    response
}
```

`next.run(request).await` calls the rest of the stack; code before it is the
inbound side, code after it is the outbound side. The `SECURITY_HEADERS`
constant is an array of `(HeaderName, &str)` tuples, and `from_static`
requires a `&'static str` — which is why they are literals in a `const`.

**Middleware with state.** `rate_limit` receives the limiter as a `State`
extractor because it was registered with `from_fn_with_state`:

```rust
pub async fn rate_limit(
    axum::extract::State(limiter): axum::extract::State<RateLimiter>,
    request: Request,
    next: Next,
) -> Response {
    let address = request.extensions().get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(socket)| socket.ip());
    let partition = traffic_control::partition_key(None, address);
    if limiter.try_acquire(&partition) { next.run(request).await }
    else { StatusCode::TOO_MANY_REQUESTS.into_response() }
}
```

`request.extensions()` is a typed bag keyed by type — the equivalent of
`HttpContext.Items`, but you ask for a `ConnectInfo<SocketAddr>` and get an
`Option` of exactly that. Phase 5's authentication layer *inserts* the
principal into the same bag for the guards to read.

Returning a bare `StatusCode` works because `StatusCode: IntoResponse`. The
outer `status_code_pages` layer then notices an empty body on a 429 and fills
in the problem document, exactly as `UseStatusCodePages()` does.

**Reading and rebuilding a response.** `status_code_pages` shows how:

```rust
let (parts, body) = response.into_parts();
let bytes = axum::body::to_bytes(body, usize::MAX).await ...;
if !bytes.is_empty() { return (parts, Body::from(bytes)).into_response(); }
```

`into_parts` splits head and body; `(parts, body).into_response()` puts them
back. The rest of the function merges the inner layers' headers with the
problem document's, so a `WWW-Authenticate` set by a guard survives.

**Spans.** `request_id` opens a `tracing` span with the trace id, method and
path, and wraps the inner future with `.instrument(span)`. Everything logged
while that future runs is nested under the span, which is what puts
`request{trace_id=...}` in front of every log line in the README's example.

---

## 6. The rate limiter: a small concurrent data structure

```rust
pub struct RateLimiter {
    windows: Arc<DashMap<String, Window>>,
    permit_limit: u32,
    window: Duration,
    last_sweep: Arc<std::sync::Mutex<Option<Instant>>>,
}
```

- The struct is `Clone` because `from_fn_with_state` clones it per request;
  the `Arc`s make every clone share one map.
- **`DashMap`'s entry API**: `self.windows.entry(key).or_insert(Window {...})`
  returns a guard that holds the shard lock while you mutate the value, so
  the check-and-increment in `try_acquire` is atomic per partition.
- **`Instant`** is a monotonic clock for measuring elapsed time; it is never
  affected by wall-clock changes. `now.duration_since(entry.started)` is the
  age of the window.
- **`Mutex` poisoning.** `self.last_sweep.lock()` returns a `Result`, and it
  is `Err` if a thread panicked while holding the lock. The port handles that
  with `let Ok(mut last) = ... else { return; }` and a comment: a failure to
  tidy up must not turn into a failed request. C# locks cannot be poisoned;
  Rust's can, and the type makes you decide what to do.
- **`retain`** on the map is the sweep (fix F12), keeping only windows still
  open. It runs at most once per window and only when a request is already
  being counted, so an idle process does no work.

The unit tests at the bottom of the file are a good template for testing time
without mocking a clock: use a millisecond window and `std::thread::sleep`.

---

## 7. Panics become 500s

```rust
pub fn panic_to_problem(panic: Box<dyn std::any::Any + Send + 'static>) -> Response {
    let detail = panic
        .downcast_ref::<&str>().map(|message| (*message).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned());
    tracing::error!(%detail, "unhandled panic");
    ProblemDetails::internal_server_error(trace_id()).into_response()
}
```

A panic payload is a `Box<dyn Any>`; a `panic!("literal")` carries a `&str`
and a `panic!("{x}")` carries a `String`, so both are tried. The message is
logged and *not* returned, matching the C# handler's decision to leak nothing
on a 500. `CatchPanicLayer::custom` is the tower-http layer that catches the
unwind and calls this.

---

## 8. Swagger gating and the OpenAPI document

`routes::swagger` returns an empty `Router` outside Development and Demo, so
the paths simply do not exist there and fall through to a 404 — the same
outcome as the original's `if (ShouldExposeOperationalMetadata) app.UseSwagger()`.
`openapi.rs` builds the document with `serde_json::json!`, a macro that lets
you write JSON literally with Rust expressions spliced in; the paths are
assembled from `app::modules()` so the document describes what is mounted.
Fix F3 is here too: only the Bearer scheme is declared.

---

## 9. Testing the router without a socket

`crates/api/tests/host.rs` builds the app with `api::build` and drives it
with `tower::ServiceExt::oneshot`:

```rust
let response = app.clone().oneshot(request).await.expect("the router should answer");
let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("...");
```

No port is bound; the request goes straight into the service. `app.clone()`
is needed because `oneshot` consumes the service. The `TestResponse` helper
struct with `json()`, `text()` and `header()` is the same shape the identity
and module tests reuse in later phases.

The commit message lists what the 28 tests here pin: payload shapes,
Development-versus-Production gating, the six security headers on every
response, CORS preflight, status-code pages, Swagger gating and rate limiting.

---

## Try it

1. **Reorder the pipeline and see what breaks.** Move the `request_id` layer
   to be the first `.layer()` call (innermost) and run
   `cargo test -p api`. The trace-id tests explain why it must be outermost.
2. **Add a header.** Append `("x-served-by", "rust")` to `SECURITY_HEADERS`
   (you will need a `HeaderName::from_static`), run the host, and curl
   `-D -` to see it. Then run the tests and find which one counts headers.
3. **Write a middleware.** Add a `from_fn` layer that logs the elapsed time
   of each request with `Instant::now()` before `next.run` and
   `tracing::info!(elapsed_ms = ...)` after.
4. **Poison a mutex on purpose.** In a scratch test, lock a
   `std::sync::Mutex<i32>` inside `std::thread::spawn(|| { let _g = m.lock().unwrap(); panic!() })`,
   join the thread, and inspect what `m.lock()` returns on the main thread.
5. **Serve on a different port.** `Port=5099 ASPNETCORE_ENVIRONMENT=Development cargo run -p api`
   — then find where `Port` is read in `main.rs` and how the config layer
   lowercases it.
6. **Trigger the panic handler.** Temporarily add a route whose handler calls
   `panic!("boom")`, hit it, and confirm the response is the 500 document with
   no detail while the log line has the message.

## Vocabulary for this phase

| C# / .NET | Rust | Notes |
|---|---|---|
| `Program.Main` | `#[tokio::main] async fn main() -> anyhow::Result<()>` | |
| `IServiceCollection` / DI | `AppState` struct built in `composition.rs` | Explicit, once |
| `IServiceProvider.GetRequiredService<T>` | `state.repositories.albums` | A field |
| `WebApplication` / `IApplicationBuilder` | `axum::Router` | |
| `MapGroup("/api/music")` | `Router::nest("/api/music", router)` | |
| `MapGet("/x", handler)` | `.route("/x", get(handler))` | |
| Minimal API parameter binding | Extractors (`State`, `Path`, `Json`, custom) | Order-independent |
| `IResult` / `TypedResults` | `impl IntoResponse` | Tuples of status/headers/body work |
| `app.Use(...)` middleware | `.layer(middleware::from_fn(f))` | Applied inside-out |
| `RequestDelegate next` | `Next` + `next.run(request).await` | |
| `HttpContext.Items` | `request.extensions()` | Keyed by type |
| `HttpContext.Connection.RemoteIpAddress` | `ConnectInfo<SocketAddr>` extension | Needs `with_connect_info` |
| `UseExceptionHandler` | `CatchPanicLayer::custom(...)` | Panics only; errors are values |
| `ILogger` | `tracing` macros + `tracing_subscriber` | Structured fields |
| Log scope | `tracing::info_span!` + `.instrument()` | |
| `Stopwatch` | `std::time::Instant` | Monotonic |
| `lock (obj)` | `Mutex<T>::lock()` → `Result<Guard, Poisoned>` | Guard unlocks on drop |
| `WebApplicationFactory` | `api::build(state, identity)` + `oneshot` | No socket |

**Next:** [Phase 5 — the Identity module](phase-5-identity.md): tokens, keys,
stores, and the `Authorized<P>` extractor.
