Ports upstream ADR-0011.

# 0011. Authorization names are compiled constants in identity-contracts

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 4; port step 2 (see `docs/upgrade-plan.md`, section 5)

## Context

Identity decides what each authorization policy means; every other module
applies policies to its endpoints. Before the upgrade the port kept the
names in `shared_kernel::auth::policies` (`MUSIC_READ`, `ORDERS_READ`,
`ADMINISTRATION_READ`, ...) and wrapped each route's requirements in a marker
type (`guards::MusicRead`, `guards::OrdersRead`, ...) whose
`Policy::REQUIREMENTS` list a handler named through the `Authorized<P>`
extractor. Identity's vocabulary therefore lived in the shared kernel, not in
Identity, and the Music-to-Catalog rename changes every one of those names.
The original had the same problem as string literals: a rename on one side
compiled and failed at runtime as a 403 on the other.

From step 2 the port applies policies by name and evaluates them at request
time instead of encoding them as types (ADR-0021). That makes the name itself
the contract, so it needs one definition, owned by Identity.

## Decision

- `identity-contracts` holds every name as a `pub const &str`: module
  `permissions`, one constant per permission (each permission is also the
  name of the policy that requires it, for example
  `permissions::CATALOG_READ = "catalog.read"`), and module `policies`, with
  `policies::ADMIN` (`role.admin`) and `policies::TENANT_SCOPED`
  (`tenant.scoped`). The strings are the original's.
- Identity's `PolicyEvaluator` recognizes exactly these names. Every other
  module depends on `identity-contracts` and applies policies only through
  them: `.require_authorization(&[permissions::CATALOG_READ,
  policies::TENANT_SCOPED])` on the endpoint builder. No string literal
  reaches `require_authorization` in `crates/`.
- Enforcement:
  - a guardrail test in `architecture-tests` reads every recorded
    `EndpointDescriptor` and fails on a policy name that is not one of those
    constants (a literal that happens to be spelled right still passes, but
    cannot drift once the constant changes);
  - a second guardrail test, and the startup check
    `ModuleComposition::validate_endpoints` (Development and Test), fail on a
    name the registered `PolicyEvaluator` does not know.

## Consistency

Not a data boundary. Modules depend on Identity's names at compile time
through `identity-contracts`, never on `module-identity`.

## Consequences

- Renaming a permission is one change in `identity-contracts`; every use
  follows or fails to compile.
- `identity-contracts` becomes a dependency of every module that protects an
  endpoint. It holds constants only (the fence tests enforce the contracts
  shape, ADR-0012), so the dependency carries no behavior.
- The claim values in issued tokens are the same strings, so existing tokens,
  including tokens the original issues with the same key, keep working.

## How to reverse

Inline the constant values back as strings at each call site and delete the
two modules from `identity-contracts`. Nothing else depends on them.
Returning to marker types as well means reversing ADR-0021.
