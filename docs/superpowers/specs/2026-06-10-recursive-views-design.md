---
status: implemented
target_version: v0.5.3
sub_spec: recursive-views
supersedes: docs/superpowers/plans/_skeleton/recursive-views.md
---

# `WITH RECURSIVE` views — design (revised after empirical spike)

## TL;DR

**Recursive views already work.** The roadmap premise — that they "require
cycle-aware dep-graph handling" because a recursive view "appears to depend on
itself, failing the topological sort" — is **incorrect** for the current code.
An empirical spike (a `CREATE RECURSIVE VIEW` fixture run through the full
conformance pipeline — parse → diff → plan → apply to ephemeral Postgres →
re-introspect → round-trip) **passed on PG 14, 17, and 18** with no code
changes. The feature is therefore **conformance coverage to lock in the
behavior**, not a dep-graph fix.

## Why it already works

Two facts the original framing missed:

1. **Both view body-dependency walkers skip *unqualified* references.**
   `parse/ast_canon.rs::walk_node` and `catalog/assemble/views.rs::walk_node_for_deps`
   both gate on `!rv.schemaname.is_empty()` — they only emit a `DepEdge` for a
   schema-qualified `FROM schema.rel`. A recursive CTE's self-reference is
   **unqualified** (`FROM v`; CTEs cannot be schema-qualified), so **neither
   walker ever emits the self-edge.** A recursive view's edges point only to the
   real, schema-qualified tables it joins. The topological sort sees no cycle.

2. **`body_dependencies` is not part of view equivalence.** `View::differences`
   (the `Equiv` impl) destructures `body_dependencies` as `_` (uncompared), and
   the migration diff compares only `body_canonical.canonical_hash()`. So
   `body_dependencies` is *ordering-only* — it cannot produce a spurious diff.
   A recursive view's equivalence rides entirely on `body_canonical`, the
   deparsed `WITH RECURSIVE …` text, which round-trips through `pg_query::deparse`
   and matches `pg_get_viewdef`'s form across PG majors (the Phase-5 CTE-aware
   qualifier-stripping further hardened this).

Both `CREATE RECURSIVE VIEW v(cols) AS …` and `CREATE VIEW v AS WITH RECURSIVE …`
parse to the same `ViewStmt` (PG grammar desugars the former), so both source
forms canonicalize identically — confirmed by fixtures using each form.

## What this feature delivers

Pure conformance coverage (no production code change):

- `objects/views/create-recursive-cte` — `CREATE RECURSIVE VIEW`, CTE name == view
  name (the exact case the skeleton feared).
- `objects/views/create-with-recursive-cte` — direct `CREATE VIEW … WITH RECURSIVE`,
  CTE name != view name.
- `objects/materialized_views/create-with-recursive-cte` — recursive materialized
  view.
- `objects/views/replace-recursive-body` — change a recursive view's body
  (`CREATE OR REPLACE`).

All four pass the full apply + round-trip pipeline (verified via `xtask
diagnose-pg-version`; goldens blessed via `xtask bless --conformance`).

Docs: move recursive views from the roadmap active matrix to "shipped," drop the
inaccurate "requires cycle-aware dep-graph handling" note, and add a CHANGELOG
entry.

## Explicitly NOT done (and why)

- **No CTE-scoping change.** The originally-designed fix targeted a self-edge
  that never materializes. Skipped — there is nothing to fix.
- **No new IR fields, parser changes, or catalog-reader changes.**
- **The latent CTE-shadowing ordering bug is tracked separately.** Because the
  walkers don't scope CTE names, a *non-recursive* CTE named like a real managed
  table would emit a spurious *ordering* edge (not an equivalence diff, and
  unrelated to recursive views). It is rare and out of scope here; tracked as a
  follow-up.
- **Infinite-recursion linting** (missing base case) — left to Postgres.

## Discovered during the spike (general, pre-existing)

A view with an **unnamed expression column** (e.g. `SELECT id, 0 FROM …`)
round-trips inconsistently: the source deparse keeps `0`, but `pg_get_viewdef`
names it `0 AS "?column?"`, so `body_canonical` differs between source and
catalog → a spurious (no-op) diff. This is **general view-normalization, not
recursive-specific** — the recursive fixtures simply tripped it by using bare
literals for `depth`. The fixtures were changed to use named columns
(`0 AS depth`), matching the project convention for view SELECT lists. The
normalization gap itself is recorded as a known limitation (`v1.md` §8) and
tracked as a follow-up; fixing it (normalizing the `?column?` alias) is out of
scope for recursive views.

## Testing

The four conformance fixtures above (Tier-A/B/C) are the coverage. Each is
exercised against PG 14–18 by the conformance suite; a future regression that
breaks recursive-view round-trip or ordering would fail them.
