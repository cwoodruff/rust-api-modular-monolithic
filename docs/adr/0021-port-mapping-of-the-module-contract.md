Port-only; no upstream ADR.

# 0021. Port the module contract as a typed `Services` object and request-time policies

- Status: Accepted
- Date: 2026-10-16
- Phase: port step 2 (see `docs/upgrade-plan.md`, sections 3 and 5)

## Context

The original's module contract is `IModule { Name; RegisterServices;
MapEndpoints }`. `RegisterServices` adds to an `IServiceCollection`, and the
upgrade leans on it: each module registers its own `DbContext`, hosted
services, keyed event handlers, health checks, cache, work queue, gate,
meter and rate-limit policy, and nothing outside the module knows they
exist. Endpoints name policies with `RequireAuthorization("...")`, and the
framework resolves the name against registered policies on each request; an
unknown name is a runtime failure, which is why the original added a startup
check for it (upstream phase 0).

Before the upgrade this port had dropped `register_services` (see the
comment in `shared_kernel::module`): the host built every repository itself
and handed them to every module in one shared `AppState`. Authorization was
the `Authorized<P>` extractor, where `P` is a marker type in
`shared_kernel::auth::guards` carrying a compile-time `REQUIREMENTS` list.
Both choices stop working once the upgrade lands:

- With no registration hook, a module's pool, dispatcher, subscriptions and
  bulkheads would have to be built by the host, which then has to know every
  module's internals. A shared `AppState` holding every module's services
  also lets any handler reach any module's services, the Rust form of one
  module injecting another's repository.
- With marker types, an endpoint cannot name a policy that does not exist:
  the program would not compile. The startup check for unknown policies
  would test nothing. The policy vocabulary also lived in the shared kernel
  rather than in Identity, and `catalog-host` (ADR-0020) and the module test
  hosts (ADR-0016) need to supply their own meaning for the same names
  without depending on `module-identity`.

## Decision

- The trait is `Module { name(); register_services(&mut self, &mut
  Services, &AppConfig); map_endpoints(&self, &mut Endpoints) }`.
- `Services` is a typed composition object, not a dependency-injection
  container. It has one method per kind of thing a module may contribute:
  hosted tasks, event subscriptions (keyed by module, ADR-0012), keyed module
  database pools, health checks, caches, work queues, gates, meters,
  rate-limit policies, and the authenticator and `PolicyEvaluator`. There is
  no lookup by type and no lifetime management: the host reads what was
  registered and wires it once at startup. A module keeps its own state in
  its own struct and gives its handlers that state; there is no `AppState`
  holding every module's services.
- `Endpoints` records each route as an `EndpointDescriptor { method, path,
  name, tag, policies, rate_limit, description, responses }` as well as
  building the axum router. The descriptors drive
  `ModuleComposition::validate_endpoints` (duplicate routes and unknown
  policies, in Development and Test), the per-module OpenAPI documents, and
  the guardrail tests, which read descriptors instead of parsing source.
- Authorization is by name: `.require_authorization(&[permissions::X,
  policies::Y])` with constants from `identity-contracts` (ADR-0011). At
  request time a route layer asks the registered `PolicyEvaluator` whether
  the caller satisfies every named policy. `module-identity` registers the
  real evaluator; `catalog-host` and `test-support` register their own.
  The `Authorized<P>` extractor and the marker types are removed.

## Consistency

Not a boundary decision. It is the mechanism by which the boundary decisions
in ADR-0003, ADR-0008 and ADR-0013 are kept inside each module.

## Consequences

- Each module builds and owns its pool, tasks and bulkheads; the host only
  composes. A module test host (ADR-0016) and `catalog-host` (ADR-0017) can
  compose one module the same way the monolith composes five.
- A handler can reach only its own module's state, so the compiler, not a
  convention, stops one module from calling another's services.
- `Services` must grow a method for each new kind of contribution. That is
  more code than a generic container, but each addition is visible in
  `shared-kernel` and counts against its budget (ADR-0012).
- An unknown policy name is now possible, and so is a real failure: a typo
  that bypassed the constants would be denied on every request. The startup
  check and the guardrail tests are what catch it, and they are no longer
  vacuous.
- A handler no longer receives proof of authorization in its argument list.
  The check runs in a route layer before the handler, as the original's
  middleware does; a route registered without `require_authorization` is
  caught by the guardrail test over descriptors, not by the type checker.
- Policy evaluation costs a lookup by name per request, which is negligible
  next to token validation.

## How to reverse

Remove `register_services` and build each module's services in the host
again, restoring a shared state type; reintroduce marker types with
compile-time requirement lists and the `Authorized<P>` extractor, and delete
the unknown-policy half of `validate_endpoints`, which would have nothing
left to find. ADR-0016, ADR-0017 and ADR-0020 would each need another way to
supply authorization without `module-identity`.
