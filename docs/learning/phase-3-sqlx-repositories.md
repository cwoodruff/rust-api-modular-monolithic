# Phase 3 — sqlx repository implementations

*Learning Rust through the port, part 4 of 10.*

| | |
|---|---|
| **What this phase built** | All ten repositories against the real Chinook file, a connection pool, the data-health probe, and integration tests that run on a private copy of the database. |
| **Commit** | `bd2f7eb` — `git show bd2f7eb --stat` (later hardened by `d2fac69` and `c6a2fab`) |
| **Files to open** | `crates/shared-data-sqlite/src/{lib,pool,probe,rows}.rs`, `repositories/{mod,common,music,orders,administration}.rs`, `tests/{repositories,seed_integrity}.rs` |
| **Rust ideas introduced** | Implementing a foreign trait for a local type, `sqlx` runtime queries and row decoding, raw string literals, `map_err` and `?` together, collecting `Result`s, `transpose`, let-else, numeric casts, chrono parsing, private modules, per-test fixtures with `Drop` |

The crate-level doc in `lib.rs` sets the scene: the C# repositories contain no
SQL despite the project name, so every statement here was re-derived from
LINQ. What you get to learn from that is how Rust talks to a database when
there is no ORM in the way.

---

## 1. Implementing the Phase 2 traits

Each repository is a small struct holding a pool, and then two `impl` blocks:

```rust
#[derive(Debug, Clone)]
pub struct SqliteGenreRepository {
    pool: SqlitePool,
}

impl SqliteGenreRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self { Self { pool } }
}

#[async_trait]
impl Repository<Genre> for SqliteGenreRepository { /* five CRUD methods */ }

#[async_trait]
impl GenreRepository for SqliteGenreRepository { /* get_by_id */ }
```

Two points about the shape:

- **`SqlitePool` is `Clone` and cheap to clone.** It is an `Arc` around the
  real pool, so `pool.clone()` in the host (Phase 4) hands each of the ten
  repositories a handle to one shared pool. This is the general Rust pattern
  for "shared service": wrap it in `Arc`, clone the handle.
- **The trait comes from another crate; the struct is local.** Rust's orphan
  rule says you may implement a trait for a type if *either* the trait or the
  type is defined in your crate. Here the type is, so the impl is allowed. It
  is the same rule that lets `shared-persistence` implement `HealthContext`
  for `AppState` in Phase 4.

`#[must_use]` on `new` makes the compiler warn if a caller discards the
result. The port puts it on every pure constructor and getter.

---

## 2. A query, start to finish

```rust
async fn get_all(&self) -> RepositoryResult<Vec<Genre>> {
    let rows = sqlx::query(r#"SELECT "Id", "Name" FROM "Genre" ORDER BY "Id""#)
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

    rows.iter()
        .map(rows::genre)
        .collect::<Result<Vec<_>, _>>()
        .map_err(common::database)
}
```

Walk it line by line.

**`r#"..."#` is a raw string literal.** Nothing inside is escaped, so the SQL
can quote identifiers with `"` freely. The `#` count can be raised
(`r##"..."##`) if the content itself contains `"#`.

**`sqlx::query(...)` builds a statement at runtime.** sqlx also has
`query!` macros that check SQL against a live database at compile time; the
crate doc explains why the port does not use them (nothing else in the
workspace needs a database to build). `.fetch_all(&self.pool)` runs it and
returns `Vec<SqliteRow>`.

**`.map_err(common::database)?`** converts sqlx's error into the shared
`RepositoryError` and returns early if it is `Err`. `common::database` is a
plain function used as the mapper, which reads better than a closure.

**Collecting a `Vec` of `Result`s into a `Result` of a `Vec`.**
`rows.iter().map(rows::genre)` yields `Result<Genre, sqlx::Error>` per row.
`.collect::<Result<Vec<_>, _>>()` stops at the first `Err` and returns it, or
returns `Ok(Vec<Genre>)`. The turbofish `::<...>` names the target type; the
underscores let inference fill the rest. This one idiom replaces a loop with
a `try`/`catch` around it.

**`fetch_optional` and `transpose`.** The by-id lookup:

```rust
let row = sqlx::query(r#"SELECT "Id", "Name" FROM "Genre" WHERE "Id" = ?"#)
    .bind(id)
    .fetch_optional(&self.pool)
    .await
    .map_err(common::database)?;

row.as_ref()
    .map(rows::genre)
    .transpose()
    .map_err(common::database)
```

`fetch_optional` yields `Option<SqliteRow>`. Mapping it with a fallible
decoder gives `Option<Result<Genre, _>>`, which is the wrong nesting for the
return type. `.transpose()` flips it to `Result<Option<Genre>, _>`. Once you
know this method exists you will use it constantly.

**`.bind(id)` is parameter binding.** Values are never interpolated. The one
place the port uses `format!` in SQL is for a *table name* in `common.rs`,
and the comment says why that is safe: SQL has no parameter slot for an
identifier, and every caller passes a module-level constant.

---

## 3. `let ... else` for the "not found" early return

```rust
let Some(header) = sqlx::query(/* album with artist name */)
    .bind(id)
    .fetch_optional(&self.pool)
    .await
    .map_err(common::database)?
else {
    return Ok(None);
};
```

`let PATTERN = EXPR else { DIVERGE };` binds if the pattern matches and
otherwise runs the block, which must return, break, or panic. It is the
cleanest way to say "if this is `None`, we are done" without an extra
indentation level, and the graph-building repositories use it for every
header row.

---

## 4. Decoding rows: `rows.rs`

Every column read goes through one of a few small accessors:

```rust
pub(crate) fn int(row: &SqliteRow, column: &str) -> Result<Option<i32>, Error> {
    let value: Option<i64> = row.try_get(column)?;
    #[allow(clippy::cast_possible_truncation)]
    Ok(value.map(|number| number as i32))
}
```

- `row.try_get(column)` is generic over the target type, so the `let value:
  Option<i64>` annotation is what tells it what to decode. SQLite integers are
  64-bit; the entities use `i32` to match the C# `int`.
- `as i32` is a numeric cast that truncates silently on overflow. Clippy
  flags it, and the port acknowledges the lint with an explicit `allow`
  rather than hiding it globally. That is the right way to use a cast: on
  purpose, with a comment or an allow at the site.
- The function returns `Result<Option<i32>, sqlx::Error>` and the caller
  adds `?`. Errors from the driver stay driver errors until the repository
  boundary, where `common::database` erases them.

**Money** is declared `decimal(10,2)` but stored as `real`, so `decimal()`
tries `f64` first, then `i64`, then text — three `if let Ok(...)` attempts in
a row, because `try_get` with the wrong type returns an error rather than
panicking. Writes go back out through `money()` as `f64` so a row this port
writes looks like every other row.

**Timestamps** are the subtle one, and the doc comments on
`STORED_TIMESTAMP_FORMAT`, `timestamp_text`, and `parse_timestamp` tell the
whole story of fix F16: sqlx's own `DateTime` encoder writes RFC 3339, the
file holds EF Core's `2007-01-02 00:00:00.000000 +00:00`, and a column holding
two encodings sorts by text. So writes render the stored form explicitly and
the reader accepts both. The test
`a_written_timestamp_reads_back_as_the_instant_that_was_written` closes the
round trip that did not close before.

`chrono` vocabulary: `NaiveDateTime` has no zone; `DateTime<Utc>` does.
`DateTime::parse_from_str` with a format containing `%:z` yields a zoned
value, and `.naive_local()` drops the zone *without shifting the clock*, which
is what the original does and the test `a_non_utc_offset_keeps_the_written_wall_clock`
pins. `.and_utc()` goes the other way.

**The shared projection.** `TRACK_MODEL_SELECT` is a `const &str` holding a
`SELECT ... LEFT JOIN ...` prefix, and callers append their `WHERE` with
`format!`. This is how the port gets one decoder (`track_model`) for every
track-shaped endpoint.

---

## 5. Query shapes worth studying

`music.rs` contains the three deliberate shapes the crate doc lists:

- **Album by id: two statements.** Header with a `LEFT JOIN` to the artist,
  then tracks. The original's `AsSplitQuery()` comment calls this "important on
  SQLite to avoid cartesian explosion."
- **Artist by id: three statements, grouped in memory.** All of the artist's
  tracks come back in one query and are assigned to albums with
  `tracks.iter().filter(|t| t.album_id == Some(album.id)).cloned().collect()`.
  The comment notes this keeps the statement count O(1) rather than O(albums).
- **Tracks by invoice: `EXISTS`.** Matches the LINQ `Any(...)` and avoids
  the duplicates a join would produce.

`SqliteTrackRepository::tracks_where` shows a private helper taking a SQL
fragment: six by-foreign-key queries differ only in their `WHERE`/`JOIN`, so
one method takes the clause and the key. The same trick appears as
`employees_where` in `administration.rs`.

`administration.rs` also holds the two fidelity quirks P17 and P18. Read the
comment on `SqliteCustomerRepository::get_by_id` — the nine `None`s are there
on purpose, and the test in `tests/repositories.rs` pins both the nulls and
the fact that the collection endpoint returns the same customer populated.

---

## 6. Writes: `RETURNING` and `rows_affected`

```rust
async fn add(&self, entity: Genre) -> RepositoryResult<Genre> {
    let row = sqlx::query(r#"INSERT INTO "Genre" ("Name") VALUES (?) RETURNING "Id", "Name""#)
        .bind(&entity.name)
        .fetch_one(&self.pool)
        .await
        .map_err(common::database)?;
    rows::genre(&row).map_err(common::database)
}

async fn update(&self, entity: Genre) -> RepositoryResult<bool> {
    let updated = sqlx::query(r#"UPDATE "Genre" SET "Name" = ? WHERE "Id" = ?"#)
        .bind(&entity.name)
        .bind(entity.id)
        .execute(&self.pool)
        .await
        .map_err(common::database)?;
    Ok(updated.rows_affected() > 0)
}
```

`.bind(&entity.name)` borrows the `Option<String>`; `.bind(entity.id)` copies
the `i32`. `entity` is taken **by value** (`entity: Genre`) because the trait
says so, and the method may consume it.

`rows_affected()` is fix F11: the original asks "does it exist?" and then
writes, with no transaction, so the answer is stale by the time the write
runs. `common::delete_by_id`'s doc comment explains the race and the cure:
one statement that reports what it did. Notice `common::exists` is kept
because it is part of the contract, but the comment says no write uses it any
more.

---

## 7. The pool and the probe

`pool.rs` is a builder in the usual Rust style:

```rust
let options = SqliteConnectOptions::new()
    .filename(database)
    .create_if_missing(false)
    .foreign_keys(true);

SqlitePoolOptions::new()
    .max_connections(MAX_POOL_CONNECTIONS)
    .connect_with(options)
    .await
```

`create_if_missing(false)` is a deliberate divergence: the original lets
SQLite create an empty database and fail later; this fails at startup and
names the file. The doc comment also records why journal mode is left alone.

`probe.rs` implements the Phase 1 `DatabaseProbe` trait with `SELECT 1`, and
its `match` shows the pattern for "log the error, return a boolean":

```rust
match sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&self.pool).await {
    Ok(_) => true,
    Err(error) => {
        tracing::warn!(%error, "data-health probe could not reach the database");
        false
    }
}
```

`query_scalar::<_, i64>` reads one column of one row; the `_` lets inference
fill the database type. This is fix F9 — same answer on the wire as the C#
bare `catch`, but the reason is logged.

---

## 8. Module privacy inside a crate

```rust
pub mod pool;
pub mod probe;
pub mod repositories;
mod rows;
```

`rows` is private to the crate: its functions are `pub(crate)`, callable from
`repositories/*.rs` but invisible to `api`. `repositories/mod.rs` re-exports
the ten structs with `pub use`, so a consumer writes
`shared_data_sqlite::SqliteGenreRepository` without knowing which file it is
in. `mod.rs` is the conventional entry point for a directory module; the
alternative (`repositories.rs` beside a `repositories/` directory) is
equivalent.

---

## 9. Tests on a private copy of the database

`tests/repositories.rs` opens with the fixture that every writing test uses:

```rust
struct ScratchDatabase { path: PathBuf }

impl ScratchDatabase {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("chinook-{}-{label}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::copy(bundled_database(), &path).expect("the database should be copyable");
        Self { path }
    }
    async fn pool(&self) -> SqlitePool { create_pool(&self.path).await.expect("...") }
}

impl Drop for ScratchDatabase {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.path); }
}
```

The process id in the name keeps parallel test binaries apart — `cargo test`
runs test functions in threads and test binaries as separate processes. The
`Drop` impl deletes the copy however the test ends. `.gitignore` excludes
`**/chinook-*.db` in case something goes very wrong.

`tests/seed_integrity.rs` is a different kind of test: it asserts the
*committed* database holds exactly stock Chinook (a `CENSUS` of eleven row
counts and a list of `LIKE` patterns that leftover test rows tend to match).
Its doc comment records two occasions the file was polluted, once upstream and
once here. It is a good example of a test written to protect an invariant
rather than a function.

---

## Try it

1. **Add a query.** Implement `get_by_composer(&self, composer: &str)` on
   `SqliteTrackRepository` using `tracks_where`, add it to the trait, and
   write a test that finds "Angus Young" tracks on the bundled file.
2. **Break the timestamp round trip.** In `rows.rs`, make `timestamp_text`
   return `value.map(|i| i.to_rfc3339())` and run
   `cargo test -p shared-data-sqlite a_written_timestamp`. The failure shows
   why the stored format is rendered by hand.
3. **See `transpose` in the error message.** Remove `.transpose()` from
   `get_by_id` and read the type mismatch the compiler reports; it names both
   nestings.
4. **Watch the pool refuse a missing file.** Run
   `cargo test -p shared-data-sqlite opening_a_missing_database`, then flip
   `create_if_missing` to `true` and see the test's second assertion fail.
5. **Count statements.** Add an `eprintln!` before each `sqlx::query` in
   `SqliteArtistRepository::get_by_id` and run
   `cargo test -p shared-data-sqlite artist -- --nocapture`. (Tests install
   no `tracing` subscriber, so `tracing::debug!` would print nothing here;
   `eprintln!` goes straight to stderr.)
6. **Pollute the seed and get caught.** Copy `data/chinook.db` somewhere,
   insert a genre named `TestGenre_x` into the copy with the `sqlite3` CLI,
   point `seed_integrity` at it (temporarily), and watch the census fail.

## Vocabulary for this phase

| C# / .NET | Rust | Notes |
|---|---|---|
| EF Core LINQ | Hand-written SQL through `sqlx::query` | Or `query!` macros with a DB at build time |
| `DbContext` pool | `sqlx::SqlitePool` | `Clone` is a cheap handle |
| `SqliteConnectionStringBuilder` | `SqliteConnectOptions` builder | |
| `ExecuteScalarAsync` | `query_scalar::<_, T>(...).fetch_one` | |
| `ToListAsync` | `fetch_all` + collect | |
| `FirstOrDefaultAsync` | `fetch_optional` → `Option<Row>` | |
| `SaveChangesAsync` result | `execute(...).rows_affected()` | |
| `reader.GetInt32("Col")` | `row.try_get::<i64, _>("Col")?` then `as i32` | Fallible, typed |
| `(int)longValue` | `long_value as i32` | Truncates; clippy warns |
| `DateTime.ParseExact` | `NaiveDateTime::parse_from_str(s, fmt)` | `%`-style format strings |
| `try { ... } catch (SqliteException)` | `.map_err(common::database)?` | At the boundary only |
| `List<Result>` → fail fast | `.collect::<Result<Vec<_>, _>>()` | |
| `Option<Result>` ↔ `Result<Option>` | `.transpose()` | |
| Early return on null | `let Some(x) = expr else { return ...; };` | |
| `@"..."` verbatim string | `r#"..."#` raw string | |
| Test fixture with `IDisposable` | Struct with `impl Drop` | |
| `internal` helper module | `mod rows;` (no `pub`) + `pub(crate) fn` | |

**Next:** [Phase 4 — the host](phase-4-host.md): axum, the middleware stack,
and the first HTTP responses.
