Ports upstream ADR-0002.

# 0002. Use PostgreSQL, with one schema per owning module

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 1; port step 3 (see `docs/upgrade-plan.md`, section 5)

## Context

Until step 3 the port ran on a committed SQLite file (`data/chinook.db`)
through `sqlx` with the `sqlite` feature, and had no migrations: the schema
was whatever the file held.

SQLite has no schemas. Everything later steps need in order to give each
module its own data depends on them: a schema per module, a migration history
table per module, dropping cross-schema foreign keys, and an orphan check
across schemas (ADR-0003, ADR-0015). None of it can be tried on SQLite. The
original made the same move in its phase 1, and a port that stays on SQLite
could not share a database with it or reproduce its behavior.

## Decision

- PostgreSQL 17 is the only database engine. Local development runs it from
  `docker-compose.yml` (init scripts in `docker/postgres-init/`); tests run it
  in one shared container, `rmm-test-postgres`, or against the server named
  by `TEST_POSTGRES_URL`.
- The schema is defined by `sqlx` migrations kept in each owning module's
  crate (ADR-0003). There is no shared migration set; the port moves to
  PostgreSQL and to per-module migrations in the same step.
- Every table is created in the schema of the module that owns it (ADR-0001):
  `catalog`, `orders`, `administration`. Identity has no tables; Reporting's
  schema arrives in step 5 (ADR-0014). Table and column names are the
  original's, quoted where it quotes them, so a Rust host and a C# host can
  read the same database.
- Column types are PostgreSQL-native: `varchar(n)`, `timestamp with time
  zone`, `numeric(10,2)` (read as `rust_decimal::Decimal`). Dates without a
  time zone are stored as UTC.
- Seed data is `data/chinook-postgres-seed.sql`, the original's file byte for
  byte; a test pins its SHA-256. `DbSeeder` in `crates/api` applies
  migrations and loads it in Development and Test only. Other environments
  apply migrations as a deployment step.
- There is no second engine. `sqlx`'s `sqlite` feature and `data/chinook.db`
  are removed. A zero-dependency demo mode would need its own ADR; carrying
  SQLite next to PostgreSQL would mean two dialects to test.

## Consistency

Not a boundary decision. The schemas are split here, and ADR-0003 gives each
module its own pool in the same step, so no cross-module write shares a
transaction. The four cross-schema foreign keys the original carried through
its phase 1 never exist in this port; ADR-0004 to ADR-0007 record each
reference.

## Consequences

- Running the app needs Docker, or a reachable PostgreSQL set in
  `ConnectionStrings:AppDatabase`. A fresh clone no longer runs on its own.
- Tests need Docker, or `TEST_POSTGRES_URL`. Each test host gets its own
  clone of a seeded template database (`CREATE DATABASE ... TEMPLATE ...
  STRATEGY FILE_COPY`, serialized under a lock), so test binaries still run
  in parallel (ADR-0016).
- Service tests now run on the real engine, so constraints, SQL and identity
  sequences are actually tested.
- PostgreSQL string comparison is case-sensitive, unlike the old SQLite
  `LIKE`.

## How to reverse

Switch the `sqlx` feature from `postgres` to another engine, rewrite each
module's migrations and queries for that dialect, regenerate the seed script,
and replace the fixture in `test-support`. The per-module schemas have to be
flattened or emulated as well.
