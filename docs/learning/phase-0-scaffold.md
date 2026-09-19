# Phase 0 — The Cargo workspace scaffold

*Learning Rust through the port, part 1 of 10.*

| | |
|---|---|
| **What this phase built** | A Cargo workspace with one crate per C# project, a pinned dependency policy, CI, the bundled `chinook.db`, and an architecture test that reads the real dependency graph. |
| **Commit** | `f24b05b` — run `git show f24b05b --stat` to see exactly what landed. |
| **Files to open** | `Cargo.toml`, `rust-toolchain.toml`, `crates/*/Cargo.toml`, `crates/architecture-tests/`, `.github/workflows/ci.yml`, `.gitignore` |
| **Rust ideas introduced** | Workspaces, crates, manifests, feature flags, lints, visibility, integration tests, `cargo metadata`, iterators and collections in a test |

Nothing in this phase serves a request. What it does is decide *where things
go* and *who may see whom*, and in Rust both of those decisions are made in
`Cargo.toml` rather than in a solution file plus an analyzer. That is the first
thing to learn: in the C# original, module isolation was a rule enforced after
the fact by ArchUnitNET. Here the same rule is expressed by which crates a
manifest lists, and the compiler will not build a violation.

---

## 1. A workspace is a solution, a crate is a project

The C# solution has ten projects. The Rust workspace has ten crates, one each,
in `crates/`. The root `Cargo.toml` names them by glob:

```toml
[workspace]
resolver = "3"
members = ["crates/*"]
```

A **crate** is Rust's unit of compilation and of visibility. It is the closest
thing to a .NET assembly: it has a name, a version, and a public surface, and
other crates can only reach what it marks `pub`. A **workspace** is a set of
crates that share one `Cargo.lock` and one `target/` build directory, which is
what makes `cargo build --workspace` build the whole solution in one go.

Two things the workspace root does that a `.sln` cannot:

### 1a. Shared package metadata (`Directory.Build.props`)

```toml
[workspace.package]
version = "1.0.0"
edition = "2024"
rust-version = "1.85"
license = "MIT"
```

Each crate opts in with `version.workspace = true`, and so on. Open
`crates/module-music/Cargo.toml` and you will see five such lines. This is why
every module's health endpoint reports the same `version` without any of the
reflection the C# `BuildInfoProvider` does: there is only one version, and the
compiler bakes it in (Phase 1 shows the `env!("CARGO_PKG_VERSION")` call).

The **edition** is worth a sentence. Rust editions (2015, 2018, 2021, 2024)
are opt-in language revisions, chosen per crate, and a 2024 crate can depend on
a 2015 crate without either noticing. This port uses 2024 throughout, which is
why later phases can write `if let Some(x) = a && x.is_empty()` (let chains)
and use `Future` without importing it.

### 1b. Central version policy (`Directory.Packages.props`)

```toml
[workspace.dependencies]
axum = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "signal"] }
sqlx = { version = "0.8", default-features = false, features = [
    "runtime-tokio", "sqlite", "chrono", "macros",
] }
rust_decimal = { version = "1", features = ["serde", "serde-float"] }
```

Every version in the whole port is pinned here, once. A crate that wants axum
writes `axum = { workspace = true }` and gets that version and those features.
The comments in the real file record two pairings that had to be resolved up
front (`rsa` 0.9 wants `rand` 0.8) — the Rust ecosystem is stricter than NuGet
about a single version of a crate per dependency graph, so mismatches surface
at Phase 0 rather than at runtime.

**Features** are the thing with no NuGet analogue. A crate can expose optional
compile-time switches, and `sqlx` above is a good example: with
`default-features = false` it compiles almost nothing, and the four features
listed add exactly the SQLite driver, the tokio runtime binding, chrono type
support, and the query macros. Features are additive and unify across the
graph, so if two crates ask for different features of `tokio`, everyone gets
the union.

---

## 2. One manifest per crate, and the manifest *is* the architecture rule

Compare a module crate with the host:

```toml
# crates/module-music/Cargo.toml
[dependencies]
shared-kernel = { workspace = true }
shared-persistence = { workspace = true }
axum = { workspace = true }
serde = { workspace = true }
tracing = { workspace = true }
```

```toml
# crates/api/Cargo.toml
[dependencies]
shared-kernel = { workspace = true }
shared-persistence = { workspace = true }
# The host is the only crate allowed to see a concrete database driver.
shared-data-sqlite = { workspace = true }
module-admin = { workspace = true }
module-identity = { workspace = true }
# ...
```

The Music module cannot call into the Orders module, and cannot open SQLite,
because it does not list either crate. There is no `using` it could add to get
around this — a Rust `use` path only resolves inside crates the manifest names.
That is what the plan means by "crate dependency edges *are* the architecture
rules."

The host is also the only crate with two build targets:

```toml
[lib]
name = "api"
path = "src/lib.rs"

[[bin]]
name = "modular-monolith-api"
path = "src/main.rs"
```

A crate can be a **library**, a **binary**, or both. The port makes `api` both
so that integration tests can build the exact router the binary serves, which
is the seam the C# tests get from `WebApplicationFactory<Program>`. Phase 4
shows how thin `main.rs` is as a result.

---

## 3. Visibility replaces `PublicSurfaceTests`

The C# suite has a test asserting each module assembly exports exactly one
public type. Rust does that at compile time with two keywords:

- `pub` — visible to other crates.
- `pub(crate)` — visible anywhere inside this crate, and nowhere outside it.

A module crate in this port exposes `pub struct MusicModule`, two `pub const`s
for its name and prefix, and nothing else; `mod endpoints;` and `mod services;`
are declared without `pub`, so they are private modules. You will see this
pattern in Phase 6. The architecture-tests crate's own doc comment says it
plainly: *"The third C# class, `PublicSurfaceTests`, has no runtime equivalent
here: Rust visibility enforces it at compile time."*

---

## 4. Lints are a project-wide contract

```toml
[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
unwrap_used = "warn"
expect_used = "warn"
```

Every crate opts in with `[lints] workspace = true`. Three things to know:

- `unsafe_code = "forbid"` means no crate in this workspace can write an
  `unsafe` block, and unlike `deny`, `forbid` cannot be overridden lower down.
  For a web service that is almost always the right setting.
- **Clippy** is Rust's linter (`cargo clippy`), roughly Roslyn analyzers. The
  two lints here warn on `.unwrap()` and `.expect()`, which are the calls that
  turn a `None` or an `Err` into a panic. The plan's comment: *"Keeps the port
  honest about the error paths the C# original handled explicitly."*
- Tests are allowed to unwrap. Every test module in the port opens with
  `#![allow(clippy::unwrap_used, clippy::expect_used)]` — the inner-attribute
  form (`#![...]`) applies to the enclosing module.

CI runs clippy with `-D warnings`, which promotes every warning to an error, so
an `unwrap()` in production code fails the build.

---

## 5. Toolchain pinning

```toml
# rust-toolchain.toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

This is `global.json`. `rustup` reads it and makes sure the stable channel plus
the formatter and linter are installed before `cargo` runs. Note it pins the
*channel*, not a version — the workspace declares `rust-version = "1.85"` as
its minimum, but the code already uses let chains, which need 1.88 or later.
That is a small drift worth knowing about if you ever install an older stable.

---

## 6. The architecture test, line by line

`crates/architecture-tests/tests/module_boundaries.rs` is the best first Rust
file to read in the repository, because it is ordinary Rust doing something
concrete and it uses a dozen idioms you will meet everywhere else.

### 6a. Integration tests live in `tests/`

A file under a crate's `tests/` directory is compiled as its own binary that
links against the crate's public API. The crate itself is empty (`src/lib.rs`
is only a doc comment); all the behavior is in the test file. Functions marked
`#[test]` are what `cargo test` runs.

### 6b. Constants and arrays

```rust
const MODULES: [&str; 5] = [
    "module-admin",
    "module-identity",
    "module-music",
    "module-orders",
    "module-reporting",
];
```

`[&str; 5]` is a fixed-size array of five string slices. `&str` is a borrowed
view of UTF-8 text, and a string literal has the type `&'static str` — it lives
for the whole program. You will see `&'static str` on every module's `name()`
in Phase 1.

### 6c. Running a process and parsing JSON

```rust
let output = Command::new(env!("CARGO"))
    .args(["metadata", "--no-deps", "--format-version", "1",
           "--manifest-path", concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")])
    .output()
    .expect("`cargo metadata` should be runnable from the test harness");

let metadata: serde_json::Value =
    serde_json::from_slice(&output.stdout).expect("`cargo metadata` should emit valid JSON");
```

- `env!("CARGO")` is a *compile-time* macro: Cargo sets that variable while
  building, and the macro pastes its value in as a literal. `concat!` glues
  literals together. Neither exists at runtime.
- `.expect("...")` unwraps a `Result` or `Option`, panicking with that message
  if it is `Err`/`None`. In a test that is exactly what you want.
- `serde_json::Value` is the dynamic JSON type (like `JsonNode`). Indexing it
  with `metadata["packages"]` never panics; a missing key yields `Value::Null`.

### 6d. Building a graph with iterators

```rust
let dependencies = package["dependencies"]
    .as_array()
    .expect("every package should have a dependencies array")
    .iter()
    .filter(|dependency| dependency["kind"].is_null())
    .filter(|dependency| dependency["path"].is_string())
    .filter_map(|dependency| dependency["name"].as_str())
    .map(str::to_owned)
    .collect();

graph.insert(name, dependencies);
```

This is LINQ, with two Rust-specific details. `filter_map` combines `Where`
and `Select` for a function that returns `Option`: the `None`s drop out.
`str::to_owned` converts a borrowed `&str` into an owned `String` — the
borrowed strings point into `metadata`, which will be dropped, so the graph
needs its own copies. And `.collect()` is generic over the target container;
here the compiler infers `BTreeSet<String>` from the type of `graph`'s values.

`BTreeMap` and `BTreeSet` are ordered by key. The test uses them rather than
`HashMap`/`HashSet` so that failure messages and iteration order are stable.

### 6e. Borrowing across a function boundary

```rust
fn dependencies_of<'a>(
    graph: &'a BTreeMap<String, BTreeSet<String>>,
    crate_name: &str,
) -> &'a BTreeSet<String> {
    graph.get(crate_name)
        .unwrap_or_else(|| panic!("`{crate_name}` should be a workspace member"))
}
```

The `'a` is a **lifetime**: it says the returned reference borrows from
`graph`, not from `crate_name`. The compiler needs that to know the result is
valid exactly as long as the graph is. Most functions never need to spell one
out, but a function returning a reference derived from one of two reference
parameters does. Note also `{crate_name}` inside the format string — since
Rust 2021, identifiers can be captured inline.

### 6f. The rule itself

```rust
#[test]
fn modules_never_depend_on_each_other() {
    let graph = workspace_dependency_graph();
    for source in MODULES {
        let dependencies = dependencies_of(&graph, source);
        for target in MODULES {
            if source == target { continue; }
            assert!(
                !dependencies.contains(target),
                "`{source}` depends on `{target}`; modules must communicate through \
                 the shared kernel, never directly"
            );
        }
    }
}
```

`assert!` takes a condition and an optional format message. The message is only
evaluated on failure. The backslash at the end of a string line continues it
without inserting a newline or the leading whitespace.

---

## 7. CI, briefly

`.github/workflows/ci.yml` runs four commands, and they are the four you
should run locally before every commit:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace --all-targets
```

`--all-targets` includes tests, benches and examples in the build and lint
passes, so a warning in a test file also fails CI.

---

## 8. Two decisions recorded in `.gitignore`

```
# Development JWT signing key, generated on first run.
# The C# original committed this RSA private key; this port does not.
data/identity/

# Scratch databases integration tests copy out of data/chinook.db
**/chinook-*.db
```

The first is fidelity decision F6 in the plan. The second previews the test
strategy from Phase 3 onward: every test that writes gets a private copy of
the database, named so that it can never be committed by mistake.

---

## Try it

1. **Break the architecture and watch the compiler and the test both object.**
   Add `module-orders = { workspace = true }` under `[dependencies]` in
   `crates/module-music/Cargo.toml`, then run
   `cargo test -p architecture-tests`. Read the failure message; it names the
   pair. Revert.
2. **See the graph the test sees.** Run
   `cargo metadata --no-deps --format-version 1 | jq '.packages[] | {name, deps: [.dependencies[] | select(.path != null) | .name]}'`.
   Then run `cargo tree -p api --depth 1` for the human-readable version.
3. **Find out what a feature costs.** Run `cargo tree -p sqlx -e features`
   and compare against the feature list in the root manifest.
4. **Trip a lint.** Put `let x: Option<i32> = None; x.unwrap();` inside any
   non-test function in `crates/shared-kernel/src/build_info.rs`, run
   `cargo clippy -p shared-kernel -- -D warnings`, read the message, revert.
5. **Read one manifest with `cargo` docs open.** `cargo doc --workspace
   --no-deps --open` builds and opens the API documentation for every crate.
   The crate-level docs (`//!` comments) are the port's own guide to itself.

## Vocabulary for this phase

| C# / .NET | Rust | Notes |
|---|---|---|
| Solution (`.sln`) | Workspace (`[workspace]`) | Shares one lockfile and build dir |
| Project (`.csproj`) | Crate (`Cargo.toml`) | Unit of compilation *and* visibility |
| Assembly | Crate output (`.rlib` / binary) | |
| `Directory.Build.props` | `[workspace.package]` | Inherited with `key.workspace = true` |
| `Directory.Packages.props` | `[workspace.dependencies]` | |
| `global.json` | `rust-toolchain.toml` | |
| NuGet package | Crate from crates.io | Versions unify per graph |
| — | Cargo features | Compile-time optional functionality |
| `TreatWarningsAsErrors` | `RUSTFLAGS=-D warnings` / `clippy -- -D warnings` | |
| Roslyn analyzers | Clippy | `[workspace.lints.clippy]` |
| `public` / `internal` | `pub` / `pub(crate)` | `pub(crate)` is per crate, not per assembly-with-InternalsVisibleTo |
| ArchUnitNET | Manifest edges + `cargo metadata` test | The compiler enforces most of it |
| `[Fact]` | `#[test]` | |
| Test project | `tests/*.rs` or `#[cfg(test)] mod tests` | Integration vs unit |

**Next:** [Phase 1 — the shared kernel](phase-1-shared-kernel.md), where the
first real Rust types land: enums, traits, errors, and the cache.
