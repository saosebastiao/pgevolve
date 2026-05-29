---
status: skeleton
target_version: v0.5.3
sub_spec: recursive-views
---

# `WITH RECURSIVE` views — implementation plan (skeleton)

## Problem
`CREATE VIEW v AS WITH RECURSIVE … SELECT …` defines a view whose body
references itself via a recursive CTE. pgevolve's current view
parser + canonicalizer accepts WITH RECURSIVE syntactically, but the
dep-graph builder doesn't handle the self-reference cleanly — the
view appears to depend on itself, which fails the topological sort.

## Scope
- In: `CREATE VIEW … WITH RECURSIVE …`, `CREATE MATERIALIZED VIEW …
  WITH RECURSIVE …`, `DROP`, `COMMENT`, dep edges that correctly skip
  the self-reference.
- Out: PG 14+ already supports WITH RECURSIVE everywhere; no
  version gating needed.

## IR sketch
TBD — likely no new fields on `View`; the recursion is internal to
the canonicalized body. The dep-graph builder needs to detect the
self-reference and not emit an edge from the view to itself.

## Catalog reader notes
TBD — `pg_get_viewdef` already returns the WITH RECURSIVE form
verbatim; no reader work expected beyond confirming round-trip.

## Conformance fixtures
TBD — `objects/views/create-with-recursive-cte`,
`replace-recursive-body`, dep-graph test that confirms no self-edge.

## Open questions
- Should the linter warn on infinite recursion (no terminating base
  case)? Probably out of scope — leave to PG.

## Dependencies
- Internal: planner cycle-aware dep-graph handling (no other roadmap
  row).
