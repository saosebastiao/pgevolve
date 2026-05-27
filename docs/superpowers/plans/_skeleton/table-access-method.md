---
status: skeleton
target_version: v0.4.0
sub_spec: table-access-method
---

# `TABLE ... USING <access method>` — implementation plan (skeleton)

## Problem
Postgres tables can specify a non-default table access method (`heap`,
`zheap`, columnar AMs from extensions). The IR currently assumes `heap`
implicitly; mismatches between source and catalog are silent.

## Scope
- In: parse `USING method` in `CREATE TABLE`; model as
  `Table::access_method: Option<Identifier>`; diff via `ALTER TABLE ...
  SET ACCESS METHOD method` (PG 15+) or `ReplaceWithCascade` on PG 14
  (PG 14 lacks the ALTER form).
- Out: `CREATE ACCESS METHOD` itself (extension-provided AMs only;
  pgevolve doesn't manage AM definitions).

## IR sketch
TBD — `Table::access_method: Option<Identifier>`; `None` means inherit
cluster default (`heap` in practice).

## Catalog reader notes
TBD — `pg_class.relam` → `pg_am.amname`. Filter out `heap` to keep IR
canonical (or always include — TBD during brainstorm).

## Conformance fixtures
TBD — `objects/tables/create-with-access-method` (needs an extension
that provides a non-heap AM available in the test image — likely
`pg_columnar`, or skip via `requires-extension` fixture flag),
`alter-set-access-method`.

## Open questions
- PG 14 lacks `ALTER TABLE ... SET ACCESS METHOD`; pre-PG 15 path must
  be drop + recreate. Confirm.
- Do we lint when an unknown AM is referenced (i.e., extension not
  declared)?

## Dependencies on other roadmap items
- Loose pairing with the extensions surface (extensions provide AMs).
