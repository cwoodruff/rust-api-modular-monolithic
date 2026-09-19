# Learning Rust through the port

Ten documents, one per delivery phase of the ASP.NET Core → Rust port, each
written as a Rust lesson anchored in the code that phase produced. They assume
you know C# and ASP.NET Core well and Rust not at all, and they teach Rust in
the order this codebase needed it.

Every document has the same shape:

- a header table naming the phase's commit (so `git show <hash>` shows you the
  exact change) and the files to open;
- numbered sections that walk real code from the repository, explaining the
  Rust idea each excerpt uses and the C# construct it replaces;
- a **Try it** list of hands-on exercises — most of them "break this and read
  the compiler's or the test's message";
- a **Vocabulary** table mapping .NET terms to Rust ones for that phase.

Read them in order the first time. The phases build on each other, and so do
the ideas: `Result` and `?` in Phase 1 are assumed by Phase 3, traits and
trait objects from Phase 1 are assumed by Phase 5, and so on.

| Phase | Document | What you learn |
|---|---|---|
| 0 | [The Cargo workspace scaffold](phase-0-scaffold.md) | Workspaces, crates, manifests as architecture rules, lints, a first integration test |
| 1 | [The shared kernel](phase-1-shared-kernel.md) | Enums with data, `Option`/`Result`/`?`, `thiserror`, traits vs concrete types, generics, closures and futures, `Arc`, type erasure, task-locals, custom `Debug` |
| 2 | [Persistence contracts](phase-2-persistence.md) | Structs and derives, serde casing and nulls, `Decimal`/`DateTime<Utc>`, a trait generic over its target, a builder, `LazyLock`, let chains, `async_trait`, `Path`/`PathBuf`, `Drop` |
| 3 | [sqlx repositories](phase-3-sqlx-repositories.md) | Implementing foreign traits, runtime SQL with sqlx, row decoding, collecting `Result`s, `transpose`, let-else, chrono parsing, per-test database copies |
| 4 | [The host serves HTTP](phase-4-host.md) | `#[tokio::main]`, `anyhow`, `tracing`, axum routers/handlers/extractors, tower layer order, `from_fn` middleware, shared state, `DashMap`, `Mutex` poisoning, panic handling |
| 5 | [The Identity module](phase-5-identity.md) | Crypto crates, composition over inheritance, `serde(untagged)`, custom extractors, `PhantomData`, associated consts, unit-struct policies, `Deref`, atomics, race tests |
| 6 | [Music and Orders](phase-6-music-orders.md) | Module crate layout, type aliases, generic route helpers, services as functions, async closures, higher-order async, `serde(flatten)`, what the parity diff found |
| 7 | [Administration](phase-7-administration.md) | `FromRequest` body extractors, newtypes, layered error enums with `From`, tuple responses with headers, `serde(alias)`, `Result<bool, E>`, a source-scanning test |
| 8 | [Parity verification](phase-8-parity.md) | The harness, four precise wire fixes, `&'static str` tables, and why tests that assert your implementation cannot find these bugs |
| 9 | [Ops and docs](phase-9-ops-docs.md) | Release profiles, Docker dependency caching, `--locked`, linking, fail-fast startup, rustdoc conventions, `cargo doc` |

## How to use these alongside the code

- Keep `docs/rust-translation-plan.md` open. Each learning document refers
  to its fidelity decisions by their labels there (`P17`, `F1`, `V13`, and
  so on).
- Run `cargo doc --workspace --no-deps --open` once. The crate-level `//!`
  comments are the port's own explanation of itself, and the learning docs
  point into them constantly.
- Do the **Try it** exercises. Several are designed so that the compiler
  error or the test failure *is* the lesson; the text tells you what to look
  for in the message.
- Nothing here requires the C# original to be checked out except the Phase 8
  exercise that runs the parity harness.

## Prerequisites

- A stable Rust toolchain, 1.88 or later (the repository's
  `rust-toolchain.toml` selects the stable channel and `rustup` installs it on
  first `cargo` invocation; `Cargo.toml` declares 1.88 as the minimum because
  the code uses 2024-edition let chains).
- `jq`, `curl`, and `sqlite3` for a few exercises.
- Docker for Phase 9's exercises; the .NET 10 SDK only for running the
  parity harness in Phase 8.
