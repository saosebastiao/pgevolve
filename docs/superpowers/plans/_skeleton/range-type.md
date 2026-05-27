---
status: skeleton
target_version: v0.3.8
sub_spec: range-type
---

# `RANGE TYPE` — implementation plan (skeleton)

## Problem
`CREATE TYPE ... AS RANGE` defines a user range type (subtype + optional
subtype_opclass, collation, canonical, subtype_diff, multirange_type_name).
pgevolve handles enums, domains, and composites today but not ranges;
range-typed columns currently fail at IR-build with an "unknown type"
error.

## Scope
- In: `CREATE TYPE ... AS RANGE`, `DROP TYPE`, `COMMENT ON TYPE`. Range
  types are immutable once created (no `ALTER TYPE` for ranges).
- Out: multirange type customization beyond `multirange_type_name`.

## IR sketch
TBD — add `RangeType` variant to the existing `UserType` enum, with
fields `subtype: QualifiedName`, `subtype_opclass: Option<QualifiedName>`,
`collation: Option<QualifiedName>`, `canonical: Option<QualifiedName>`,
`subtype_diff: Option<QualifiedName>`,
`multirange_type_name: Option<Identifier>`.

## Catalog reader notes
TBD — primary table is `pg_range`, joined with `pg_type` for the range
type's `oid` and `pg_type` for the subtype.

## Conformance fixtures
TBD — `objects/ranges/create-simple`, `create-with-opclass`,
`create-with-canonical-fn`, `drop`, `comment-on`,
`column-with-range-type`.

## Open questions
- Drop semantics: a range type drop cascades to the multirange type;
  diff should account for this.

## Dependencies on other roadmap items
- Independent. May expose latent bugs in the column-type system for
  range-typed columns; tier-C fixture should cover that path.
