---
status: skeleton
target_version: v0.4.1
sub_spec: virtual-generated-columns
---

# Virtual generated columns (PG 18) — implementation plan (skeleton)

## Problem
PG 18 introduces `GENERATED ALWAYS AS (...) VIRTUAL`, computed on read
instead of stored. The current `Column::generated` field only models
stored generated columns. Source files using `VIRTUAL` fail to parse;
catalog reads of PG 18 virtual columns produce incorrect IR.

## Scope
- In: parse `GENERATED ALWAYS AS (expr) VIRTUAL`; add
  `GeneratedKind::Virtual` variant; gate via `[managed].min_pg_version
  >= 18`; diff path between `Virtual` and `Stored` triggers
  `ReplaceWithCascade` (changing storage strategy rewrites the table).
- Out: anything PG 17-and-earlier-compatible (must be a hard error
  when source uses `VIRTUAL` against PG < 18).

## IR sketch
TBD — refactor `Column::generated: Option<NormalizedExpr>` into
`Column::generated: Option<Generated>` where
`Generated { expr: NormalizedExpr, kind: GeneratedKind }` and
`GeneratedKind::{Stored, Virtual}`. Default in parser for
`GENERATED ALWAYS AS (expr) STORED` is `Stored`.

## Catalog reader notes
TBD — `pg_attribute.attgenerated` is `'s'` for stored; PG 18 adds `'v'`
for virtual. Verify the column exists in older PG versions too (it
does, just never returns `'v'`).

## Conformance fixtures
TBD — `objects/columns/create-virtual-generated`,
`alter-stored-to-virtual` (`ReplaceWithCascade` path),
`failure/columns/virtual-on-pg17` (lint).

## Open questions
- Lint name for "VIRTUAL requires PG 18+"?
  `column-virtual-generated-requires-pg-version` follows the publication
  pattern.

## Dependencies on other roadmap items
- Depends on the v0.3.6 PG 18 catalog work (`pg18.rs` must dispatch).
