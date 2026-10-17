Ports upstream ADR-0006.

# 0006. Catalog refers to Administration's Genre by id only

- Status: Accepted
- Date: 2026-10-16
- Phase: upstream phase 2; port step 3 (see `docs/upgrade-plan.md`, section 5)

## Context

`catalog."Track"."GenreId"` points at `administration."Genre"`. Before step
3, track responses (directly, and nested in albums, artists and playlists)
carried `GenreName` and a nested `Genre`, and genre responses carried
`Tracks`, all through joined queries. Genre is the one entity with write
endpoints, including `DELETE /api/admin/genres/{id}`. The original enforced
the reference with the foreign key `FK_Track_Genre_GenreId`.

## Decision

- No foreign key between the two tables. Catalog's migration creates
  `GenreId` as an indexed column only; `GET /api/catalog/tracks/genre/{id}`
  filters on it.
- Track responses carry `GenreId` only; `GenreName` and the nested genre are
  gone. Genre responses lose `Tracks`.
- Names, if needed again, come from a genre read contract in
  `administration-contracts`.

## Consistency

Application-validated id, no database foreign key, eventual. Today Catalog's
track validator requires a non-null `GenreId`; nothing checks that the genre
exists.

## Consequences

- Behavior change: `DELETE /api/admin/genres/{id}` on a genre that tracks
  still use succeeds. With the foreign key it was rejected and the request
  failed with a 500. After the delete those tracks keep a `GenreId` that
  refers to nothing, and `/api/catalog/tracks/genre/{id}` still returns them.
- Reporting's orphan check reports tracks whose genre is gone (ADR-0015). If
  that turns out to be common, Administration can refuse to delete a genre in
  use by asking Catalog through a contract, or Catalog can react to a
  genre-deleted event.

## How to reverse

Give one pool and migration set both tables (undoing ADR-0003 for Catalog
and Administration); remove or reassign orphaned tracks first, then add the
foreign key.
