Ports upstream ADR-0007.

# 0007. Catalog refers to Administration's MediaType by id only

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 2; port step 3 (see `docs/upgrade-plan.md`, section 5)

## Context

`catalog."Track"."MediaTypeId"` points at `administration."MediaType"`.
Before step 3, track responses carried `MediaTypeName` and a nested
`MediaType`, and media type responses carried `Tracks`, through joined
queries. The original enforced the reference with the foreign key
`FK_Track_MediaType_MediaTypeId`.

## Decision

- No foreign key between the two tables. Catalog's migration creates
  `MediaTypeId` as an indexed column only; `GET
  /api/catalog/tracks/mediatype/{id}` filters on it.
- Track responses carry `MediaTypeId` only; `MediaTypeName` and the nested
  media type are gone. Media type responses lose `Tracks`.
- Names, if needed again, come from a read contract in
  `administration-contracts`.

## Consistency

Application-validated id, no database foreign key, eventual. Today Catalog's
track validator requires a non-null `MediaTypeId`; nothing checks that the
media type exists.

## Consequences

- A track can name a media type id that does not exist. There is no media
  type delete endpoint today, so the realistic way to orphan a track is a
  direct database change; if a delete endpoint is added it behaves like genre
  deletion in ADR-0006.
- A missing media type leaves tracks with a dangling `MediaTypeId`;
  Reporting's orphan check reports them (ADR-0015).

## How to reverse

Give one pool and migration set both tables (undoing ADR-0003 for Catalog
and Administration); remove or reassign orphaned tracks first, then add the
foreign key.
