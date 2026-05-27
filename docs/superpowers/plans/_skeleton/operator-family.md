---
status: skeleton
target_version: v0.5.1
sub_spec: operator-family
---

# `OPERATOR` / `OPERATOR CLASS` / `OPERATOR FAMILY` — implementation plan (skeleton)

## Problem
User-defined operators and their opclass/family membership (driving
index access methods' understanding of custom types) are unmanaged.
The most common use case is custom types that want to be indexable —
without managing the opclass/family, an index on a custom-typed column
silently breaks.

## Scope
- In: `CREATE OPERATOR`, `ALTER OPERATOR`, `DROP OPERATOR`,
  `CREATE OPERATOR CLASS`, `ALTER OPERATOR CLASS`, `DROP OPERATOR CLASS`,
  `CREATE OPERATOR FAMILY`, `ALTER OPERATOR FAMILY` (add/drop members),
  `DROP OPERATOR FAMILY`, `COMMENT ON` all three.
- Out: hash opclasses for non-standard hash functions in the first
  iteration; revisit if demand surfaces.

## IR sketch
TBD — three new `Catalog::operators*` collections. Identity for operators
is `(schema, name, left_type, right_type)`.

## Catalog reader notes
TBD — `pg_operator`, `pg_opclass`, `pg_opfamily`, `pg_amop` (operator
membership in a family), `pg_amproc` (support procedures).

## Conformance fixtures
TBD — `objects/operators/create-simple`,
`create-opclass-for-custom-type`, `alter-family-add-operator`,
`scenarios/custom-type-with-btree-opclass-roundtrip`.

## Open questions
- Cross-schema operator references in index DDL — verify the dep graph
  catches these.
- Should we lint operators with no matching opclass (i.e., unusable for
  indexing)?

## Dependencies on other roadmap items
- Loose dep on user-defined types being solid (already true for v0.2).
- Loose dep on `CAST` (v0.5.2) for some operator-driven implicit casts.
