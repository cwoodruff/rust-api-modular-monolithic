# Phase 2 — Persistence contracts

*Learning Rust through the port, part 3 of 10.*

| | |
|---|---|
| **What this phase built** | The twelve Chinook entities, the eleven API models, conversions in both directions, the ten validators with FluentValidation's semantics, the repository traits, and the database-file probe. No SQL yet. |
| **Commit** | `11f819a` — `git show 11f819a --stat` |
| **Files to open** | `crates/shared-persistence/src/{lib,entities,api_models,convert,repositories,database}.rs` and `validation/{mod,rules,validators}.rs` |
| **Rust ideas introduced** | Structs and derives, `Option<T>` for nullable columns, `serde` casing and null policy, `Decimal` and `DateTime<Utc>`, a trait generic over its target, struct update syntax, a builder with `&mut self` chaining, `LazyLock` statics, let chains, `async_trait` and supertraits, object-safety proof, `Path`/`PathBuf`, RAII cleanup with `Drop` |

This crate holds shapes and contracts and never a database driver. That split
is the same one the C# solution makes between `SharedKernel.Persistence` and
`SharedKernel.DataSQLite`, but in Rust it has a sharper consequence: a module
crate can depend on this crate and *cannot* see SQLite, because SQLite lives in
a crate the module's manifest does not name (Phase 0).

---

## 1. Entities: structs, derives, and `Option`

```rust
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Track {
    pub id: i32,
    pub name: Option<String>,
    pub album_id: Option<i32>,
    pub media_type_id: Option<i32>,
    pub genre_id: Option<i32>,
    pub composer: Option<String>,
    pub milliseconds: Option<i32>,
    pub bytes: Option<i32>,
    pub unit_price: Option<Decimal>,
}
```

**Every nullable column is `Option<T>`.** The module doc notes that every
string column is nullable in the original, including ones the database
declares `NOT NULL`, so they stay `Option<String>` here. There is no
nullable-reference-types switch to flip; a `String` field simply cannot hold
null, and an `Option<String>` field must be unwrapped before use.

**Field names are `snake_case`; JSON names are `PascalCase`.** Rust's naming
convention is enforced by a compiler warning, so the struct uses `album_id`
and the serde attribute `rename_all = "PascalCase"` produces `AlbumId` on the
wire. The C# original sets `PropertyNamingPolicy = null`, so PascalCase is
the observable contract.

**`Default` gives a zero value for every field** — `0`, `None`, an empty
`Vec`. That is what makes `..Track::default()` work in the next section, and
it is why `Artist::default()` serializes as `{"Id": 0, "Name": null}` in the
test `null_columns_are_emitted_rather_than_omitted`. Compare with Phase 1's
`ProblemDetails`, where nulls are *omitted*: the two payload families have
different null policies, and each struct declares its own.

**Why does `Track` derive `PartialEq` but not `Eq`?** `PartialEq` gives you
`==`. `Eq` is an empty marker trait on top of it that promises equality is
total, and it can only be derived when every field's type also implements it.
`f64` does not (`NaN != NaN`), which is the usual reason a struct stops at
`PartialEq`. Here `Decimal` and `DateTime<Utc>` both implement `Eq`, so the
money-bearing types *could* carry it; the port simply left it off them. The
practical effect is the same either way: you can compare two `Track`s with
`==`, which the tests do.

**Money and dates.** `unit_price: Option<Decimal>` uses `rust_decimal`, and
the workspace enables its `serde-float` feature so a `Decimal` serializes as a
bare JSON number (`0.99`, not `"0.99"`). The test
`every_money_value_in_the_database_survives_the_round_trip` pins all 37
distinct prices through `f64 → Decimal → JSON`, because both money columns are
stored as SQLite `real`. Dates are `chrono::DateTime<Utc>`, which serializes
with a trailing `Z` — the Phase 6 parity diff is what forced that choice over
`NaiveDateTime`.

---

## 2. API models: the same shapes with graph members

`api_models.rs` mirrors the C# `ApiModels`, including nested collections and
the denormalized name fields. The one manual `Default` in the file is
instructive:

```rust
impl Default for InvoiceApiModel {
    fn default() -> Self {
        Self {
            id: 0,
            customer_id: None,
            invoice_date: epoch(),
            // ...
            total: Decimal::ZERO,
            customer: None,
            invoice_lines: Vec::new(),
        }
    }
}
```

`DateTime<Utc>` has no `Default`, so the struct cannot derive one; the port
writes it and chooses the Unix epoch. When a derive fails, the compiler error
names the field, and the fix is always this: implement the trait by hand.

**Struct update syntax** is how the tests and converters build a model from a
few fields:

```rust
let album = AlbumApiModel {
    id: 1,
    title: Some("For Those About To Rock We Salute You".to_owned()),
    artist_id: Some(1),
    ..AlbumApiModel::default()
};
```

`..expr` fills every field not named from `expr`. It is the closest Rust has
to an object initializer with defaults, and it requires the rest of the value
to come from somewhere — here, `Default`.

**`Some("...".to_owned())`.** A string literal is `&'static str`; a struct
field of type `Option<String>` needs an owned `String`. `.to_owned()` (or
`.to_string()`, or `String::from`) allocates the copy. This is the most common
small conversion you will write in Rust.

---

## 3. `Convert<T>`: a trait generic over its target

The C# `IConvertModel<TTarget>` is implemented in both directions per pair.
Rust expresses that as a trait with a type parameter, implemented twice:

```rust
pub trait Convert<T> {
    fn convert(&self) -> T;
}

impl Convert<AlbumApiModel> for Album { fn convert(&self) -> AlbumApiModel { ... } }
impl Convert<Album> for AlbumApiModel { fn convert(&self) -> Album { ... } }
```

A type may implement `Convert<X>` and `Convert<Y>` at the same time because
they are different traits as far as the compiler is concerned. The caller
picks which one by annotating the result:

```rust
let model: AlbumApiModel = album.convert();
```

Without the annotation the call is ambiguous and the compiler says so.

`convert_all` is the generic helper:

```rust
pub fn convert_all<S, T>(values: &[S]) -> Vec<T>
where
    S: Convert<T>,
{
    values.iter().map(Convert::convert).collect()
}
```

`Convert::convert` names the trait method as a plain function, which `map`
accepts. The `&[S]` slice parameter accepts a `&Vec<S>` without conversion.

The converter bodies preserve the original's oddities on purpose — read the
comments on `Customer`'s `support_rep_id: None` and on `or_empty`. The tests
`a_converted_customer_drops_its_support_rep` and
`only_some_entities_coalesce_nulls` pin each one, so a future "cleanup" cannot
silently change the wire.

---

## 4. Validation: a rule builder with FluentValidation's semantics

`validation/rules.rs` is the file that teaches the most about API design in
Rust. Start with its opening doc: **only `NotNull` fails on a null value**;
every other rule passes when the value is absent. The builder reproduces
that:

```rust
pub fn max_length(&mut self, property: &str, value: Option<&str>, max: usize) -> &mut Self {
    if let Some(text) = value {
        let length = text.chars().count();
        if length > max {
            self.fail(property, max_length_message(&split_pascal_case(property), max, length));
        }
    }
    self
}
```

Points to take from this:

- **`&mut self` returning `&mut Self`** is how you build a chainable API on a
  mutable value. Each call mutates the accumulator and hands the same borrow
  back. The validator then calls `.finish()` to consume it (`self` by value)
  and produce the `Result`.
- **`Option<&str>` as the parameter type.** A caller holding
  `Option<String>` passes `self.title.as_deref()`; one holding `Option<i32>`
  passes `self.artist_id.as_ref()` to `not_null<T>`. `as_deref` and `as_ref`
  both borrow the inside of an `Option` without moving it out.
- **`text.chars().count()`, not `text.len()`.** `len()` is bytes. The test
  `length_counts_characters_not_bytes` uses "Gonçalves" (9 characters, 10
  bytes) to make the point.
- **Let chains.** Several rules read:

  ```rust
  if let Some(text) = value
      && !pattern.is_match(text)
  {
      self.fail(...);
  }
  ```

  Combining a pattern binding with a boolean in one `if` is a 2024-edition
  feature (stable since Rust 1.88). The pre-2024 spelling is a nested `if`.
- **Generic comparisons.** `greater_than<T>` takes `T: PartialOrd + Display`,
  so it works for `i32` and `Decimal` alike, and can render the threshold into
  the message.

**The `Validate` trait** has one method returning `Result<(), Vec<ValidationFailure>>`:
`Ok(())` means valid, `Err(failures)` carries every broken rule, because
FluentValidation's default cascade is `Continue`. `validators.rs` implements
it for each API model in the original's declaration order — order matters,
because it is the order failures appear in the 400's `errors` map.

**`LazyLock` for compiled regexes:**

```rust
pub fn phone_pattern() -> &'static Regex {
    static PATTERN: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| compiled(r"\(?\d{3}\)?[-\.]? *\d{3}[-\.]? *[-\.]?\d{4}"));
    &PATTERN
}
```

A `static` inside a function is still a program-wide static; `LazyLock`
compiles the pattern on first use and every later call gets the same
`&'static Regex`. This is `static readonly Regex` with lazy initialization,
without a lock in the hot path. The pattern is deliberately unanchored because
the original's is, and the test says so.

`compiled` wraps `Regex::new` and uses `unreachable!` on failure: the patterns
are literals known good at authoring time, so a failure would be a bug in the
source, not a runtime condition.

**Messages** are reproduced from FluentValidation's English templates.
`split_pascal_case` ports the display-name splitter, and the tests show the
subtlety: the `errors` key is `FirstName` while the message quotes
`'First Name'`.

---

## 5. Repository traits: `async_trait`, supertraits, and object safety

```rust
#[async_trait]
pub trait Repository<T>: Send + Sync {
    async fn entity_exists(&self, id: i32) -> RepositoryResult<bool>;
    async fn get_all(&self) -> RepositoryResult<Vec<T>>;
    async fn add(&self, entity: T) -> RepositoryResult<T>;
    async fn update(&self, entity: T) -> RepositoryResult<bool>;
    async fn delete(&self, id: i32) -> RepositoryResult<bool>;
}

#[async_trait]
pub trait AlbumRepository: Repository<Album> {
    async fn get_by_artist_id(&self, id: i32) -> RepositoryResult<Vec<AlbumApiModel>>;
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<AlbumApiModel>>;
}
```

- **`#[async_trait]`** rewrites each `async fn` into one returning a boxed
  future, which is what makes the trait usable as `dyn AlbumRepository`.
  Native `async fn` in traits exists in modern Rust but is not object-safe
  without extra work; the crate is the pragmatic choice when you need trait
  objects, which the host does.
- **`AlbumRepository: Repository<Album>`** is a **supertrait**: anything that
  implements `AlbumRepository` must also implement `Repository<Album>`. This
  is interface inheritance.
- **`RepositoryResult<T>`** is a type alias for
  `Result<T, RepositoryError>`, and the error type is re-exported from
  `shared-kernel` so the kernel can turn one into a 500 without depending on
  this crate. Read the comment at the `pub use` to see the dependency
  direction being managed by hand.

The trait doc reproduces the C# `IRepository<T>` and explains two quirks
carried over: `GetById` is redeclared per entity with different return types
(model versus entity), and `GetAll` is unbounded.

**A compile-time proof.** The test module holds a struct with no tests:

```rust
#[allow(dead_code)]
struct ObjectSafetyProof {
    albums: std::sync::Arc<dyn AlbumRepository>,
    // ... one field per trait
}
```

If any trait stopped being object-safe, this struct would not compile. It
also previews Phase 4's `Repositories` struct exactly.

---

## 6. Finding the database: `Path`, `PathBuf`, and lexical normalization

`database.rs` ports the connection-string probe. Two types do the work:
`Path` is a borrowed path (like `&str`), and `PathBuf` is an owned one (like
`String`). Functions take `&Path` and return `PathBuf`.

```rust
pub fn find_database_upward(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(directory) = current {
        let candidate = directory.join(RELATIVE_DATABASE_PATH);
        if has_usable_database(&candidate) {
            return Some(candidate);
        }
        current = directory.parent();
    }
    None
}
```

`while let` loops while the pattern matches; `parent()` returns `None` at the
filesystem root, which ends the loop. `has_usable_database` uses
`metadata().is_ok_and(|file| file.is_file() && file.len() > 0)` — a zero-byte
placeholder does not count, and the test says why.

`normalize` resolves `.` and `..` **lexically**, by iterating
`path.components()` and matching on `Component::CurDir` / `ParentDir`. The
comment explains why it is not `std::fs::canonicalize`: that requires the path
to exist, and the fallback path by definition does not.

`parse_data_source` shows string handling: `split(';')`, `split_once('=')`,
`replace(' ', "")`, and `matches!(key.as_str(), "datasource" | "filename")`.

---

## 7. RAII cleanup in tests

```rust
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
```

`Drop` runs when a value goes out of scope, on every path including a panic
mid-test. It is the deterministic `IDisposable` — no `using` block, no
`finally`. `let _ = ...` discards a `Result` you have decided not to check
(and silences the must-use warning). Phase 3 uses the same pattern for
per-test database copies.

---

## Try it

1. **Change a null policy and watch a test catch it.** Add
   `#[serde(skip_serializing_if = "Option::is_none")]` to `Artist.name`,
   run `cargo test -p shared-persistence null_columns`, read the diff.
2. **Implement a converter for a new pair.** Define `struct ArtistSummary
   { id: i32, display: String }` in a scratch test and `impl Convert<ArtistSummary>
   for Artist`. Then call `convert_all(&artists)` and see inference pick it.
3. **Add a rule.** Give `GenreApiModel` a `min_length("Name", ..., 2)` rule
   and make the Phase 7 write tests tell you what changed
   (`cargo test -p api genre`).
4. **See what `Eq` demands.** Add `Eq` to `Track`'s derive list and confirm
   it compiles. Then change `unit_price` to `Option<f64>` and read the
   compiler's explanation of which field now prevents it. Revert both.
5. **Make a trait non-object-safe.** Add
   `fn map<U>(&self, f: impl Fn(T) -> U) -> Vec<U>;` to `Repository<T>` and
   watch `ObjectSafetyProof` fail to compile.
6. **Run the probe from a strange directory.** In a test, call
   `resolve_database_path(None, Path::new("/"))` and see the fallback path it
   constructs without touching the disk.

## Vocabulary for this phase

| C# / .NET | Rust | Notes |
|---|---|---|
| `class` / `record` with properties | `struct` with `pub` fields | No getters unless you write them |
| `T?` (nullable) | `Option<T>` | |
| `string` | `String` (owned) / `&str` (borrowed) | Literals are `&'static str` |
| `decimal` | `rust_decimal::Decimal` | `serde-float` feature for numeric JSON |
| `DateTime` (Utc) | `chrono::DateTime<Utc>` | Serializes with `Z` |
| Object initializer | Struct literal + `..Default::default()` | |
| `IConvertModel<T>` | `trait Convert<T>` | Implemented per target type |
| `IValidator<T>` / `AbstractValidator<T>` | `trait Validate` + `Rules` builder | Rules live with the model |
| `RuleFor(x => x.P).NotNull()` | `rules.not_null("P", self.p.as_ref())` | |
| `static readonly Regex` | `static LazyLock<Regex>` in a fn | |
| Interface inheritance | Supertrait (`trait A: B`) | |
| `Task<T>` in an interface | `#[async_trait]` + `async fn` | Boxed future; object-safe |
| `IDisposable` / `using` | `impl Drop` | Runs on scope exit, even on panic |
| `System.IO.Path` (string ops) | `std::path::{Path, PathBuf}` | Typed, not strings |
| `Path.GetFullPath` | `std::fs::canonicalize` (touches disk) | The port normalizes lexically instead |

**Next:** [Phase 3 — sqlx repositories](phase-3-sqlx-repositories.md): the
traits get implementations against the real Chinook file.
