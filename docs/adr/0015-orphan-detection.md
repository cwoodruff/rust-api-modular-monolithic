Ports upstream ADR-0015.

# 0015. Detect orphaned cross-module references; do not repair them

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 6; port step 5 (see `docs/upgrade-plan.md`, section 5)

## Context

The port never creates the four cross-module foreign keys the original
removed (ADR-0004 to ADR-0007). The ids stay, validated only for presence, so
a row can end up pointing at nothing, for example after a genre in use is
deleted (ADR-0006). Those records promise a check.

## Decision

- `IntegrityCheckJob` in `module-reporting` runs one check per missing
  foreign key:
  - `invoice-line-track`: `orders."InvoiceLine"."TrackId"` with no `catalog."Track"`;
  - `invoice-customer`: `orders."Invoice"."CustomerId"` with no `administration."Customer"`;
  - `track-genre`: `catalog."Track"."GenreId"` with no `administration."Genre"`;
  - `track-media-type`: `catalog."Track"."MediaTypeId"` with no `administration."MediaType"`.
- Each orphan becomes a row in `reporting."IntegrityFinding"` (`Id`,
  `CheckName`, `SourceSchema`, `SourceTable`, `SourceId`,
  `MissingReference`, `DetectedAt`, `ResolvedAt`). A partial unique index on
  (`CheckName`, `SourceId`) for open findings makes a re-run add nothing new;
  a finding is marked resolved when the reference exists again or the source
  row is gone.
- **It repairs nothing.** Which fix is right (restore the referenced row,
  re-point the reference, delete the source) is a business decision for the
  owning module, not something Reporting may do; it has no write access to
  other schemas anyway (ADR-0014).
- It runs daily at `Reporting:Integrity:RunAtUtc` (default 02:00 UTC;
  `Reporting:Integrity:Enabled` turns it off) as a hosted task registered
  through `Services`. It computes the next run from the `Clock` trait and
  waits with `tokio::time`, rather than using a cron crate. It also runs on
  demand via `POST /api/reporting/integrity/run` (admin). Every run goes
  through Reporting's `ModuleWorkQueue`, so runs never overlap. Open
  findings: `GET /api/reporting/integrity/findings` (admin).

## Consistency

Read only, with respect to the other modules.

## Consequences

- Orphans are visible within a day, or immediately on demand, with enough
  detail to find the row.
- Someone has to look at the findings; nothing alerts yet (the job logs a
  summary per run through `tracing`).
- The checks read whole tables; at this data size that is fine, and they
  run behind the work queue.

## How to reverse

Disable the job (`Reporting:Integrity:Enabled=false`), or add the foreign
keys, which needs the modules back on one pool and migration set (ADR-0003).
