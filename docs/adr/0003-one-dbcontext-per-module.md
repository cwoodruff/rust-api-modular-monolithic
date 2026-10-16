Ports upstream ADR-0003.

# 0003. Give each module its own database pool, schema and migration history

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 2; port step 3 (see `docs/upgrade-plan.md`, section 5)

## Context

Before step 3, `crates/shared-persistence` held every entity's row type and
`crates/shared-data-sqlite` held a repository per entity, with queries that
joined across module lines. Every module could reach every table through the
shared `AppState`, and the shared crates owned the domain. The original
answered the same problem with one EF Core `DbContext` per module; this port
has no ORM, so the unit of ownership is a connection pool plus a migration
set.

## Decision

- Catalog, Orders and Administration (and Reporting, from step 5) each own a
  `sqlx::PgPool`, built in `Module::register_services` through
  `shared_kernel::persistence` against the one shared connection string
  `ConnectionStrings:AppDatabase`. Each pool's connections set `search_path`
  to the module's schema. The pool is registered with `Services` keyed by the
  module (ADR-0021) and is held in the module's own state; the host builds no
  pool and no repository for any module.
- Each module crate embeds its migrations with `sqlx::migrate!` from its own
  `migrations/` directory. They run on a connection whose `search_path` is the
  module's schema, so each schema has its own `_sqlx_migrations` history
  table: the counterpart of upstream's `<schema>.__EFMigrationsHistory`.
- Row types, API models, validators and mappers move into their module crate
  as private or `pub(crate)` items. The repository layer is dissolved:
  services query their own pool with `sqlx::query_as`. Services stay behind
  traits so handlers can be tested with fakes.
- Each module's first migration creates the original's phase 2 schema for
  that module: the original's tables and columns minus the four cross-schema
  foreign keys (ADR-0004 to ADR-0007). Verified by comparing `pg_dump
  --schema-only` output with the original's, history tables excepted.
- In Development and Test the host migrates Administration, Catalog, then
  Orders, then Reporting (ADR-0014), then loads the seed.
- `crates/shared-persistence` and `crates/shared-data-sqlite` are deleted.

## Consistency

Not a boundary decision in itself; it is what makes each boundary explicit.
A write goes through one module's pool and commits one module's tables.
Nothing shares a transaction across modules. Each cross-module reference has
its own ADR.

## Consequences

- A module cannot query another module's tables through the type system: it
  holds no other module's pool, and module crates do not depend on each other
  (`architecture-tests`, `module_boundaries.rs`). It could still write raw SQL
  naming another schema on its own pool; the boundary test that scans each
  module's SQL for other modules' schema names is what catches that.
- Each module's migrations evolve independently and can be extracted with the
  module.
- Queries that used to join across modules can no longer do so. Responses
  that embedded another module's data now carry ids only (customer on an
  invoice, track names on invoice lines, genre and media type names on
  tracks, invoices on a customer).
- A database migrated by the original carries EF history tables, not
  `_sqlx_migrations`. The two hosts can read and write one database, but only
  one of them may own migrations for it.
- Service tests run against PostgreSQL with each module's real pool, because
  there is no repository left to substitute.

## How to reverse

Recreate a shared crate that holds every module's row types and one pool,
merge the per-module migration sets into one directory and one history
table, and move the repositories back. The per-module schemas can stay as
they are.
