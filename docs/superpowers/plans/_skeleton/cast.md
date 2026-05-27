---
status: skeleton
target_version: v0.5.2
sub_spec: cast
---

# `CAST` — implementation plan (skeleton)

## Problem
User-defined casts between custom types (or between built-ins via a
user function) are not managed. Common with custom types that want to
participate in coercion paths.

## Scope
- In: `CREATE CAST (source AS target) WITH FUNCTION fn`,
  `CREATE CAST ... WITHOUT FUNCTION`, `CREATE CAST ... WITH INOUT`,
  `AS ASSIGNMENT` / `AS IMPLICIT` flags, `DROP CAST`,
  `COMMENT ON CAST`.
- Out: cast removal on built-in types (catalog reader excludes them).

## IR sketch
TBD — `Catalog::casts: Vec<Cast>` with fields `source: QualifiedName`,
`target: QualifiedName`, `method: CastMethod` (`Function(QualifiedName)`
| `Inout` | `Binary`), `context: CastContext` (`Explicit` | `Assignment`
| `Implicit`), `comment`. Identity: `(source, target)`.

## Catalog reader notes
TBD — `pg_cast` joined with `pg_type` (twice) and `pg_proc`. Filter out
built-in casts (`castcontext = 'i'` from built-ins or `castsource`/`casttarget`
in `pg_catalog`).

## Conformance fixtures
TBD — `objects/casts/create-with-function`, `create-without-function`,
`create-with-inout`, `drop`, `comment-on`,
`scenarios/custom-type-implicit-cast-roundtrip`.

## Open questions
- Identity collision when source = target = same type (legal in PG for
  domain → base coercion); ensure `Cast` identity handles this.

## Dependencies on other roadmap items
- Hard dep on the function surface and custom types (both already in).
- Soft dep on `OPERATOR` family (operators sometimes drive implicit
  casts).
