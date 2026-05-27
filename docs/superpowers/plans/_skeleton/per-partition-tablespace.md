---
status: skeleton
target_version: v0.4.0
sub_spec: per-partition-tablespace
---

# Per-partition `TABLESPACE` — implementation plan (skeleton)

## Problem
A partitioned-table parent may specify a default tablespace; individual
partitions may override it. pgevolve's `Table` IR has a `tablespace`
field, but the per-partition override path isn't fully exercised and
the diff path for tablespace-only changes isn't implemented.

## Scope
- In: `CREATE TABLE ... PARTITION OF ... TABLESPACE foo`; `ALTER TABLE
  partition SET TABLESPACE foo`; diff path for partition-level tablespace
  changes.
- Out: cluster-level `CREATE TABLESPACE` — that's in the v0.4.2 plan.

## IR sketch
TBD — `Table::tablespace: Option<QualifiedName>` already exists.
Confirm the catalog reader returns it correctly for partitions, and
that diff emits `ALTER TABLE ... SET TABLESPACE`.

## Catalog reader notes
TBD — `pg_class.reltablespace` → `pg_tablespace.spcname`. Zero
`reltablespace` means inherit default.

## Conformance fixtures
TBD — `objects/partitions/create-with-tablespace`,
`alter-set-tablespace`, `partition-with-different-tablespace-than-parent`.

## Open questions
- Should a tablespace move be considered destructive? In Postgres it
  rewrites the partition's storage; intent-required flag is likely needed.

## Dependencies on other roadmap items
- Pairs with cluster `TABLESPACE` (v0.4.2) for the full flow, but
  per-partition assignment can ship first against existing tablespaces.
