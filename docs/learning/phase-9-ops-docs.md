# Phase 9 — A container that runs, and the docs to go with it

*Learning Rust through the port, part 10 of 10.*

| | |
|---|---|
| **What this phase built** | A working multi-stage Dockerfile with a non-root user and a healthcheck, a `.dockerignore`, the README rewritten around what someone actually does first, and four architecture documents with statuses corrected to what shipped. |
| **Commit** | `227800f` — `git show 227800f --stat` (image size corrected in `18cdf93`; PEM providers for Production in `a0cb0a3`) |
| **Files to open** | `Dockerfile`, `.dockerignore`, `README.md`, `Cargo.toml` (`[profile.release]`), `docs/{architecture,caching,authn-authz,parity}.md`, and any `//!` crate doc |
| **Rust ideas introduced** | Release profiles (`lto`, `codegen-units`, `strip`), how Cargo caches dependency builds and how to exploit that in Docker, `--locked`, binary naming, static versus dynamic linking on Linux, fail-fast startup with `anyhow`, rustdoc conventions (`//!`, `///`, intra-doc links, `# Errors` sections), `cargo doc` |

The last phase has the least Rust *code* and the most Rust *tooling*. It is
where you learn what a Rust build actually produces, how long it takes, how
to make it not take that long twice, and how the documentation you have been
reading in every other phase was written.

---

## 1. The release profile

`Cargo.toml`, at the bottom:

```toml
[profile.release]
lto = "thin"
codegen-units = 1
strip = "symbols"
```

Cargo has two built-in profiles: `dev` (what `cargo build` and `cargo test`
use — fast to compile, slow to run, with debug assertions) and `release`
(`cargo build --release` — the opposite). These three settings tune release:

- **`lto = "thin"`** — link-time optimization across crate boundaries. Rust
  compiles each crate separately, so without LTO a call from `api` into
  `shared-kernel` cannot be inlined. Thin LTO recovers most of that at
  moderate link-time cost; `"fat"` recovers more and costs more.
- **`codegen-units = 1`** — by default rustc splits each crate into parallel
  codegen units for compile speed, at the cost of cross-unit optimization.
  One unit is slower to build and produces better code.
- **`strip = "symbols"`** — remove symbol tables and debug info from the
  binary. This is most of the difference between a 30 MB and an 8 MB
  executable.

None of these affect `cargo test`. They exist so the Docker image below gets
the smallest, fastest binary without anyone remembering flags.

---

## 2. The Dockerfile, and the dependency-caching trick

The comment at the top of the `Dockerfile` explains why the original's could
not produce a working container. This one is a standard Rust multi-stage
build with one non-obvious move.

### 2a. Build stage: manifests first, then stubs, then sources

```dockerfile
FROM rust:1-slim-bookworm AS build
WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY crates/api/Cargo.toml crates/api/
# ... one COPY per crate manifest

RUN set -eux; \
    for crate in api architecture-tests module-admin ...; do \
        mkdir -p "crates/$crate/src"; \
        echo "" > "crates/$crate/src/lib.rs"; \
    done; \
    mkdir -p crates/api/src; \
    echo "fn main() {}" > crates/api/src/main.rs

RUN cargo build --release --locked -p api

COPY crates crates
RUN set -eux; \
    find target/release -maxdepth 2 -name '*modular_monolith_api*' -delete; \
    find target/release/.fingerprint -maxdepth 1 \
        \( -name 'api-*' -o -name 'module-*' -o -name 'shared-*' \) -exec rm -rf {} +; \
    cargo build --release --locked -p api
```

Why the dance: Docker caches each layer until its inputs change. A Rust
build's expensive part is compiling the ~200 dependency crates, and those
depend only on the manifests and the lockfile. So the first `cargo build`
runs against **empty stub sources**, which compiles every dependency and
nothing of ours, and that layer stays cached until `Cargo.lock` changes. The
second build, after copying the real sources, only compiles the workspace
crates.

The `find ... -delete` lines remove Cargo's **fingerprints** for the workspace
crates. Cargo decides what to rebuild by fingerprinting inputs, and the stub
sources were an input; without clearing them, Cargo might consider the empty
`lib.rs` up to date. Dependency fingerprints are left alone so they stay
cached.

`--locked` makes Cargo fail if `Cargo.lock` would need to change, which is
what you want in CI and in an image: the build uses exactly the versions the
repository committed.

### 2b. Runtime stage

```dockerfile
FROM debian:bookworm-slim AS runtime
RUN ... apt-get install -y --no-install-recommends ca-certificates curl; \
    useradd --system --create-home --uid 10001 api
COPY --from=build /src/target/release/modular-monolith-api /usr/local/bin/
COPY appsettings.json appsettings.Development.json ./
COPY data/chinook.db data/chinook.db
RUN mkdir -p /app/data/identity; chown -R api:api /app
USER api
EXPOSE 8080
ENV Port=8080
HEALTHCHECK ... CMD curl --fail --silent --output /dev/null http://localhost:8080/ || exit 1
ENTRYPOINT ["modular-monolith-api"]
```

- The binary's name, `modular-monolith-api`, comes from the `[[bin]]` table
  in `crates/api/Cargo.toml` (Phase 0).
- A Rust binary built on the default Linux target links **dynamically**
  against glibc, which is why the runtime image is `debian:bookworm-slim`
  and not `scratch`. (A `musl` target produces a fully static binary and
  would allow `scratch`; the port did not need it.) The README reports the
  result: roughly 170 MB as `docker images` sees it, of which the binary is
  about 8 MB.
- `data/identity` must be writable because the development key provider
  writes there on first run; the `chown` is what lets the non-root `api`
  user do that.
- `ENV Port=8080` is read by `main.rs` through the config layer — the same
  `Port` key the README's quick start uses.

### 2c. The environment is deliberately not defaulted

The long comment in the Dockerfile is a design decision, and Rust makes it
cheap to enforce: with `ASPNETCORE_ENVIRONMENT` unset the host runs as
Production, `build_keys` (Phase 5) returns `KeyError::Unavailable` with a
message naming `Jwt:PemKeyPath`, `main` propagates it through
`anyhow::Context`, and the container exits non-zero with the whole chain
printed. That is *fail fast with a reason*, and it falls out of `Result`
plumbing without any startup-validation framework.

### 2d. `.dockerignore`

Without it, the build context would include `target/` (gigabytes after a few
builds) and `.git/`. It also excludes `data/identity/` so a locally generated
dev key can never be copied into an image.

---

## 3. Documentation is part of the code

You have been reading rustdoc throughout this series without necessarily
noticing. Every file in the port opens with a crate- or module-level doc
comment and every public item has one. The conventions:

```rust
//! Module-level documentation. Goes at the top of a file, describes the
//! module as a whole, and is where the port explains *why*.

/// Item-level documentation. Goes directly above a `fn`, `struct`, `enum`,
/// trait, or field, and describes *what*.
pub fn thing() {}
```

Rustdoc is Markdown. Two conventions the port follows that Clippy checks for
under its pedantic lints:

- **`# Errors`** — a section on any function returning `Result`, saying when
  it fails. See `AppConfig::load` in Phase 1.
- **`# Panics`** — a section on any function that can panic. See
  `mount_all` in Phase 1.

**Intra-doc links** resolve to the item: `` [`Module`](module::Module) `` and
`` [`Self::get_or_add`] `` become hyperlinks in the generated HTML and are
checked by `cargo doc` (a broken link is a warning). The port uses them
heavily to point from one design note to another.

Build and read the whole thing:

```sh
cargo doc --workspace --no-deps --open
```

`--no-deps` skips the ~200 dependencies. The result is a browsable site with
every crate's docs, cross-linked, and it is the single best way to get an
overview of the port: start at `shared_kernel`, read the crate doc, and
follow the links.

The four documents in `docs/` are the human-facing layer above that:
`architecture.md` (crates, layering rules, middleware order),
`caching.md` (keys, lifetimes, the invalidation fix), `authn-authz.md`
(claims, policies, the tenant guard), and `parity.md` (what is compared,
what cannot be, every accepted divergence). Each was rewritten in this phase
to describe what shipped rather than what was planned.

---

## 4. What "done" looked like

The plan's closing section and the README's opening paragraph agree: every
route implemented, every test passing, 98 parity cases with zero unexplained
differences, and a container verified serving real data. The commit message
for this phase lists what was checked by hand rather than assumed: the image
builds, refuses to start under Production with a message naming what to
configure, and under Demo serves the root endpoint, data-health, a login and
347 albums.

Notice the shape of that list. Each item is a thing someone could run. That
is the standard the whole port holds itself to, and it is a good one to carry
into your own Rust work: a phase is finished when its acceptance check has
been executed, not when the code compiles.

---

## Try it

1. **Time a cold and a warm build.** `cargo clean && time cargo build --release -p api`,
   then touch `crates/module-music/src/lib.rs` and time it again. The
   difference is what the Dockerfile's stub trick preserves.
2. **Measure the binary.** `ls -lh target/release/modular-monolith-api`,
   then comment out `strip = "symbols"`, rebuild, and measure again.
3. **Build and run the image.** `docker build -t mm-api .` then
   `docker run -p 8080:8080 mm-api` (watch it fail fast with a reason) and
   `docker run -p 8080:8080 -e ASPNETCORE_ENVIRONMENT=Demo mm-api` (watch
   it serve). `docker exec` in and confirm `whoami` is `api`.
4. **Break the cache on purpose.** Add a dependency to
   `crates/module-music/Cargo.toml` and rebuild the image; note which layers
   rebuild. Then change only a `.rs` file and rebuild again.
5. **Write a doc comment with a broken link** — `` /// See [`NoSuchThing`] ``
   on any function — and run `cargo doc -p shared-kernel` to see the
   warning. Then fix the link and open the page.
6. **Read the docs site.** `cargo doc --workspace --no-deps --open` and
   spend twenty minutes starting from `shared_kernel::auth::Authorized`.

## Vocabulary for this phase

| C# / .NET | Rust | Notes |
|---|---|---|
| `Debug` / `Release` configuration | `dev` / `release` profile | `--release` flag |
| `PublishTrimmed` / `PublishReadyToRun` | `lto`, `codegen-units`, `strip` | In `[profile.release]` |
| `dotnet restore` layer in Docker | Stub-source build for dependency caching | Plus fingerprint cleanup |
| `--locked-mode` restore | `cargo build --locked` | Fails if the lockfile would change |
| Self-contained publish | musl target (not used here) | Default target links glibc dynamically |
| `AssemblyName` | `[[bin]] name` | |
| XML doc comments (`///<summary>`) | `///` and `//!` rustdoc, Markdown | `cargo doc` |
| `<see cref="..."/>` | `` [`path::Item`] `` intra-doc link | Checked by `cargo doc` |
| DocFX site | `cargo doc --workspace --no-deps --open` | Built in |
| Startup validation / `IStartupFilter` | `Result` from `main` via `anyhow` | Chain printed on exit |

**Back to the start:** [Phase 0 — the scaffold](phase-0-scaffold.md), or the
[index](README.md) for the whole series.
