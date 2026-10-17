Ports upstream ADR-NNNN. (Or: Port-only; no upstream ADR.)

# NNNN. Title in the imperative

- Status: Proposed | Accepted | Superseded by NNNN
- Date: YYYY-MM-DD
- Phase: upstream phase N; port step M (see `docs/upgrade-plan.md`, section 5)

## Context

What is true today, and what problem forces a decision. Name the crates,
modules, types and files involved.

## Decision

What we will do, stated so a reviewer can check the code against it.

## Consistency

For a cross-module boundary, exactly one of:

- **Shared transaction**: both modules' writes commit or roll back together.
- **Outbox, eventual**: the owning module commits, then publishes an
  integration event through its outbox; consumers catch up later.
- **Read only**: the consuming module reads the owner's data and never
  writes it.

State what a caller or UI may and may not assume as a result. Write "Not a
boundary decision" if this ADR does not cross a module line.

## Consequences

What gets easier, what gets harder, and what new failure modes appear.

## How to reverse

The concrete steps to undo this decision, and what data or code would need
to move.
