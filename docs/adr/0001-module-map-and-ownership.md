Ports upstream ADR-0001.

# 0001. Record the module map and entity ownership

- Status: Superseded by [0018](0018-module-map-after-the-upgrade.md)
- Date: 2026-10-16
- Phase: upstream phase 0; port step 1 (see `docs/upgrade-plan.md`, section 5)

## Context

Before the upgrade, the host (`crates/api`) composes five modules from an
explicit list. Each implements `shared_kernel::Module` and nests its router
under its own prefix. Persistence is not split: `crates/shared-persistence`
holds every entity row type and `crates/shared-data-sqlite` holds every
repository, all against one SQLite file, and one `AppState` carries every
repository to every module. Ownership is therefore a convention, not
something the compiler or the database enforces. Later steps move each entity
into its owner's crate and schema; they need one written answer to "who owns
this table".

## Decision

Five modules, each owning the entities listed. Only the owner writes an
entity. Other modules reach it through the owner's public contract (from step
2 onward, its `*-contracts` crate).

| Module | Crate | Route group | Owns |
|---|---|---|---|
| Catalog | `module-catalog` (was `module-music`) | `/api/catalog` | Artist, Album, Track, Playlist, PlaylistTrack |
| Orders | `module-orders` | `/api/orders` | Invoice, InvoiceLine |
| Administration | `module-admin` | `/api/admin` | Customer, Employee, Genre, MediaType |
| Identity | `module-identity` | `/api/identity` | No persisted entities (users, refresh tokens and key material are held in memory or in configuration) |
| Reporting | `module-reporting` | `/api/reporting` | None (it owns a read model from step 5) |

Cross-module references that exist today as joined columns in shared row
types, and that later steps replace with id-only references:

- `Invoice.Customer` (Orders to Administration)
- `InvoiceLine.Track` (Orders to Catalog)
- `Track.Genre` and `Track.MediaType` (Catalog to Administration)

## Consistency

Today every cross-module write goes through one SQLite pool, and the
repositories in `shared-data-sqlite` can join any table to any other. That is
recorded here as the starting point, not endorsed. Each reference above gets
its own ADR with an explicit consistency choice when it is cut (step 3).

## Consequences

- Reviewers have one table to check ownership against.
- `architecture-tests` enforce the parts that can be checked today: module
  crates do not depend on each other (`module_boundaries.rs`), and every
  route that serves data takes an authorization guard (`route_guards.rs`).
- Nothing yet stops a module from calling another module's repository
  through the shared `AppState`; step 3 closes that by giving each module its
  own pool and state (ADR-0003, ADR-0021).

## How to reverse

Edit this table in a superseding ADR. Moving an entity between modules means
moving its row type, validator, service and endpoints into the new owner's
crate and updating every ADR that references it.
