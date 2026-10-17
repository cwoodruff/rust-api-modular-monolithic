Ports upstream ADR-0020.

# 0020. catalog-host validates Identity's tokens over HTTP and keeps its own policy copies

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 9; port step 6 (see `docs/upgrade-plan.md`, section 5)

## Context

Inside the monolith, `module-identity` issues and validates tokens and
registers the `PolicyEvaluator` that gives meaning to the policy names every
module applies (ADR-0011, ADR-0021). `samples/catalog-host` runs Catalog
without `module-identity` (ADR-0017), yet Catalog's endpoints still require
those policies. The sample must accept the monolith's tokens without
depending on `module-identity`, or the "builds from module-catalog and
shared-kernel alone" check it exists for would be false.

## Decision

- `samples/catalog-host`'s remote identity module validates JWTs with
  `jsonwebtoken`, reading the signing keys from Identity's published key set
  (`CatalogHost:IdentityJwksUrl`, by default
  `http://localhost:5043/api/identity/.well-known/jwks.json`). It fetches the
  set with `reqwest`, parses it as `jsonwebtoken::jwk::JwkSet`, caches it and
  refreshes it. Issuer and audience come from `Jwt:Issuer` and
  `Jwt:Audience`, with Identity's defaults.
- It registers its own authenticator and `PolicyEvaluator` with `Services`.
  The evaluator recognizes every `identity_contracts::permissions` constant,
  `policies::ADMIN` and `policies::TENANT_SCOPED`, with the rules Identity's
  evaluator applies: the `permissions` claim, the Admin role, and a tenant
  claim that must match `X-Tenant-Id` when sent. The startup check for
  unknown policies (ADR-0011) therefore runs against this evaluator in the
  sample.
- `catalog-host` depends on no Identity crate besides `identity-contracts`,
  which comes through `module-catalog`. The extraction-readiness tests in
  `architecture-tests` check its dependency graph with `cargo metadata`.

## Consistency

Read only. `catalog-host` reads Identity's public keys over HTTP and never
writes anything Identity owns. A key Identity rotates in is picked up when
the cached key set refreshes; until then a token signed with it is rejected.

## Consequences

- The policy rules now exist in two places outside Identity: here and in
  `test-support`'s test evaluator (ADR-0016). A change to an Identity rule
  must be copied to both; the full-host authorization tests in `crates/api`
  cover only Identity's own evaluator.
- `catalog-host` cannot authenticate anyone while the monolith's key set is
  unreachable. The sample is not deployed, so this is recorded rather than
  mitigated; the extraction playbook covers key rotation and cache lifetime
  for a real extraction.

## How to reverse

Depend on `module-identity` from `catalog-host` and register Identity's
authenticator and evaluator, accepting that the sample then no longer proves
Catalog builds without Identity; or delete the sample (ADR-0017).
