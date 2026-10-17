# Architecture Decision Records

Each file in this folder records one decision: what was decided, why, and how
to undo it. Every cross-module boundary gets a record here before the code
that implements it merges.

ADR-0001 to ADR-0020 port the original's records
([aspnetcore-min-api-modular-monolithic](https://github.com/cwoodruff/aspnetcore-min-api-modular-monolithic),
`docs/adr/`) with the same numbers, titles and decisions, rewritten for the
crates, types and mechanisms of this port. Section 3 of
[`docs/upgrade-plan.md`](../upgrade-plan.md) is the mapping they follow. Each
one starts with the line `Ports upstream ADR-NNNN.` naming its original.
Records from 0021 on exist only in this port.

Wire-visible facts (routes, policy names, configuration keys and defaults,
persisted strings) are the original's. Where a record says something
different from its upstream counterpart, the difference is in how the
decision is implemented, and the record says so.

## Writing a new ADR

1. Copy `0000-template.md` to `NNNN-short-slug.md` using the next free number.
2. Fill in every section. For a cross-module boundary, the Consistency section
   must name exactly one of: "shared transaction", "outbox, eventual", or
   "read only".
3. Add the record to the index below in the same PR as the code it describes.

Records are not edited after they merge. If a decision changes, write a new
record that supersedes the old one and update the old one's Status line.

## Index

| ADR | Title | Status |
|---|---|---|
| [0001](0001-module-map-and-ownership.md) | Module map and entity ownership | Superseded by 0018 |
| [0002](0002-database-engine.md) | PostgreSQL, one schema per owning module | Accepted |
| [0003](0003-one-dbcontext-per-module.md) | One database pool, schema and migration history per module | Accepted |
| [0004](0004-orders-to-administration-customer.md) | Orders refers to Customer by id only | Accepted |
| [0005](0005-orders-to-catalog-track.md) | Orders refers to Track by id only | Accepted |
| [0006](0006-catalog-to-administration-genre.md) | Catalog refers to Genre by id only | Accepted |
| [0007](0007-catalog-to-administration-mediatype.md) | Catalog refers to MediaType by id only | Accepted |
| [0008](0008-integration-events-and-outbox.md) | Hand-rolled integration events through a per-module outbox | Accepted |
| [0009](0009-orders-to-catalog-track-sales.md) | Catalog counts track sales from InvoiceFinalized | Accepted |
| [0010](0010-orders-to-administration-purchase-summary.md) | Administration summarizes purchases from InvoiceFinalized | Accepted |
| [0011](0011-authorization-names-are-compiled-contracts.md) | Authorization names are compiled constants | Accepted |
| [0012](0012-shared-kernel-budget.md) | Shared kernel budget and dependency fences | Accepted |
| [0013](0013-per-module-bulkheads.md) | Per-module bulkheads inside one process | Accepted |
| [0014](0014-reporting-read-model-as-views.md) | Reporting reads across modules through views, as a read-only role | Accepted |
| [0015](0015-orphan-detection.md) | Detect orphaned cross-module references; do not repair them | Accepted |
| [0016](0016-a-test-host-per-module.md) | Test each module on a host of its own | Accepted |
| [0017](0017-catalog-host-reads-orders-outbox-by-cursor.md) | catalog-host reads Orders' outbox as a log, by cursor | Accepted |
| [0018](0018-module-map-after-the-upgrade.md) | Module map and entity ownership after the upgrade | Accepted |
| [0019](0019-dependencies-added-outside-the-phase-lists.md) | Dependencies added during the upgrade that no step listed | Accepted |
| [0020](0020-catalog-host-validates-identitys-tokens-over-http.md) | catalog-host validates Identity's tokens over HTTP and keeps its own policy copies | Accepted |
| [0021](0021-port-mapping-of-the-module-contract.md) | Port the module contract as a typed `Services` object and request-time policies | Accepted (port only) |

The file name of ADR-0003 keeps the upstream slug (`one-dbcontext-per-module`)
so the numbers and slugs line up with the original; the title says what this
port does instead.
