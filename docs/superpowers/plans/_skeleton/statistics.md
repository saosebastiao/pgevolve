---
status: skeleton
target_version: v0.3.7
sub_spec: statistics
---

# `STATISTICS` — implementation plan (skeleton)

## Problem
Postgres `CREATE STATISTICS` declares multi-column statistics objects
(`ndistinct`, `dependencies`, `mcv`) that the planner uses for correlated
columns. pgevolve does not yet manage these. Currently 📋 Planned in
`objects.md`.

## Scope
- In: `CREATE STATISTICS`, `DROP STATISTICS`, `ALTER STATISTICS ...
  RENAME TO`, `ALTER STATISTICS ... SET STATISTICS n`, all three kinds
  (`ndistinct`, `dependencies`, `mcv`), expression statistics (PG 14+),
  `COMMENT ON STATISTICS`.
- Out: `CREATE STATISTICS ... INCLUDE` (PG 18+) until v0.4.x.

## IR sketch
TBD — likely a `Catalog::statistics: Vec<Statistic>` collection with
fields for kinds bitset, columns/expressions, target table, optional
`statistics_target`.

## Catalog reader notes
TBD — primary table is `pg_statistic_ext` joined with `pg_namespace` and
`pg_class`. Kind information is in `stxkind`. Expression statistics
require `pg_statistic_ext_data`.

## Conformance fixtures
TBD — `objects/statistics/create-simple`, `add-kind`, `drop`,
`alter-set-target`, `expression-stats`, `comment-on`.

## Open questions
- How are statistics that reference dropped columns handled by the
  catalog? Do we need a cascade-drop path?
- Lint rule for unmanaged statistics in managed schemas?

## Dependencies on other roadmap items
None — independent surface.
