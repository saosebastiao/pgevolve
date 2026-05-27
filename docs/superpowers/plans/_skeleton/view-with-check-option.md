---
status: skeleton
target_version: v0.3.7
sub_spec: view-with-check-option
---

# `CREATE VIEW ... WITH CHECK OPTION` — implementation plan (skeleton)

## Problem
Updatable views can be created with `WITH [LOCAL | CASCADED] CHECK
OPTION` so that DML through the view enforces the view's predicate.
pgevolve currently ignores this clause; it's marked 🔮 in `objects.md`.

## Scope
- In: parse `WITH LOCAL CHECK OPTION` and `WITH CASCADED CHECK OPTION`;
  model on `View` as `check_option: Option<CheckOption>` enum; emit via
  `CREATE OR REPLACE VIEW`; round-trip via `pg_views.viewdef` or
  `pg_rewrite`.
- Out: `WITH CHECK OPTION` on materialized views — Postgres does not
  support it there.

## IR sketch
TBD — add `check_option: Option<CheckOption>` to `crates/pgevolve-core/src/ir/view.rs`
with `CheckOption::{Local, Cascaded}`.

## Catalog reader notes
TBD — check-option setting lives in `pg_rewrite` rule deps or is encoded
in the rewritten action's `WithCheckOption` node; verify by querying a
known-good view.

## Conformance fixtures
TBD — `objects/views/create-with-local-check-option`,
`create-with-cascaded-check-option`, `toggle-check-option`.

## Open questions
- Does a check-option change require `CREATE OR REPLACE` or a drop +
  recreate? (Likely `CREATE OR REPLACE` works.)

## Dependencies on other roadmap items
None.
