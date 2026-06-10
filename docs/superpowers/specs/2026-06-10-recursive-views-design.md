---
status: design
target_version: v0.5.3
sub_spec: recursive-views
supersedes: docs/superpowers/plans/_skeleton/recursive-views.md
---

# `WITH RECURSIVE` views — design

## Problem

A recursive view references itself through a recursive CTE. Postgres's
`CREATE RECURSIVE VIEW v(cols) AS query` desugars (in the grammar) to
`CREATE VIEW v AS WITH RECURSIVE v(cols) AS (query) SELECT * FROM v`, and
`pg_get_viewdef` always returns this `WITH RECURSIVE` form. pgevolve already
parses and canonicalizes `WITH RECURSIVE` bodies verbatim, **but** the
view body-dependency walkers resolve every `FROM <name>` into a dependency
edge without knowing which names are CTEs. For a recursive view the CTE is
named the same as the view, so the walker resolves the self-reference to the
view itself and emits a **self-edge** in the dependency graph; `Graph::topological_sort`
correctly rejects the resulting cycle, so the plan fails.

A latent bug shares the same root cause: because the walkers don't scope CTE
names, *any* CTE whose name collides with a real managed relation (recursive or
not) emits a **spurious dependency edge** to that relation.

## Goal

Support recursive views and materialized views by making the view
body-dependency walkers **CTE-aware**: a reference that resolves to an in-scope
CTE name is a local reference, not an external dependency, and produces no edge.
This eliminates the recursive self-edge and the latent shadowing bug in one
principled change.

## Non-goals

- **Infinite-recursion linting** (warning on a missing base case): out of scope
  — Postgres enforces recursion semantics at apply time.
- **Function/procedure body CTE-scoping**: the same latent shadowing bug may
  exist in the PL/pgSQL body walker, but recursive views don't require it.
  Tracked as a separate follow-up.
- **Unifying the duplicated view-dep walkers**: kept as a separate cleanup; this
  design fixes both walkers and guards their agreement with a test (see
  Symmetry).
- **New source syntax handling**: none needed — see Source forms.

## The core change — CTE scoping

There are two view body-dependency walkers that must produce identical output:

- **Source side:** `crates/pgevolve-core/src/parse/ast_canon.rs::walk_node`
  (runs in `canonicalize_view_bodies`).
- **Catalog side:** `crates/pgevolve-core/src/catalog/assemble/views.rs::walk_node_for_deps`
  (runs over `pg_get_viewdef` text during `read_catalog`).

Apply the identical change to each:

1. **Collect CTE names per scope.** When the walker enters a `SelectStmt` that
   has a `with_clause`, collect that clause's CTE names (`CommonTableExpr.ctename`)
   into an **in-scope CTE name set**. Accumulate it (do not replace) and thread it
   into every nested walk — the CTEs' own queries (so a `WITH RECURSIVE` self-
   reference and prior-sibling references are in scope) and the main query.
2. **Skip CTE references when emitting edges.** Before emitting a `DepEdge` for a
   relation reference, check: if the reference is **unqualified** (bare name, no
   schema) **and** its name matches a name in the in-scope CTE set, skip it — it
   is a CTE reference, not an external dependency. A schema-qualified reference
   (`schema.relation`) is always a real relation (CTEs cannot be schema-qualified)
   and is never skipped.

This is SQL's scoping rule: a CTE name shadows a relation of the same (bare)
name within the WITH's scope.

**Resulting behavior:**

| Reference | Before | After |
|---|---|---|
| `FROM v` inside `WITH RECURSIVE v AS (…)` on view `v` | self-edge → topo-sort failure | no edge (CTE ref) |
| `FROM cte` where `cte` is a non-recursive CTE AND a real table `cte` exists | spurious edge to the table | no edge (CTE ref) |
| `FROM app.users` (real table the recursive query joins) | edge to `app.users` | edge to `app.users` (unchanged) |
| `FROM users` (real table, no CTE named `users` in scope) | edge to resolved `users` | edge (unchanged) |

No new IR fields. No parser changes. No catalog-reader changes beyond the walker.

## Scope: views, MVs, both source forms

- **Views and materialized views** are both supported — they share the walkers,
  so the fix covers both. MVs may contain `WITH RECURSIVE`.
- **Source forms come for free.** `CREATE RECURSIVE VIEW v(cols) AS …` and
  `CREATE VIEW v AS WITH RECURSIVE …` both parse to the same `ViewStmt` (PG
  grammar desugars the recursive form), and the body is captured verbatim and
  canonicalized to the `pg_get_viewdef` form. A round-trip fixture confirms both
  forms produce the same canonical IR.
- **CREATE / DROP / REPLACE-body / COMMENT** require no new code: once the
  dependency graph is correct (no self-edge), the existing planner orders
  creation, drops, and dependent-view recreation off the corrected graph.

## Symmetry (safety-critical)

The source-side and catalog-side walkers must emit **identical**
`body_dependencies` for the same view, or `diff(parsed, catalog)` is non-empty
and pgevolve reports a spurious change on every plan. Because the two walkers are
duplicated (different resolution contexts), the CTE-scoping logic is added to
both and their agreement is guarded by an explicit test:

> Parse a recursive view from source SQL and read the same view from a live
> ephemeral-Postgres catalog; assert the two `body_dependencies` are identical
> and that neither contains a self-edge.

## Error handling

No new error modes. The change only *removes* incorrect edges. A recursive view
that genuinely depends on a missing managed object still produces the existing
unresolved-reference error from the walker (the CTE-name skip only suppresses
edges for names that match an in-scope CTE). A cyclic dependency among *distinct*
real views remains a reported `Cycle` from the topological sort (unchanged).

## Testing

- **Unit (pure, no DB), in `ast_canon` tests:**
  - Recursive view `v` (`WITH RECURSIVE v AS (… FROM v …)`) → no self-edge; edges
    only to real objects the query joins.
  - Non-recursive CTE shadowing a real table → no edge to the shadowed table.
  - Recursive view joining a real table `app.t` → edge to `app.t` present.
  - Nested CTEs → inner CTE names in scope for inner refs; outer scope preserved.
  - Schema-qualified reference matching a CTE name by last segment → edge still
    emitted (qualified ≠ CTE).
- **Catalog-side mirror tests** for `walk_node_for_deps` covering the same cases.
- **Symmetry test** (Docker-gated): the source/catalog `body_dependencies`
  agreement described above.
- **Conformance fixtures (Tier-A/B/C):**
  - `objects/views/create-with-recursive-cte` — create a recursive view; apply;
    round-trip.
  - `objects/views/create-recursive-view-syntax` — the `CREATE RECURSIVE VIEW`
    source form canonicalizes identically to the `WITH RECURSIVE` form.
  - `objects/views/replace-recursive-body` — change a recursive view's body.
  - `objects/materialized-views/create-with-recursive-cte` — MV variant.
  - A dep-graph assertion fixture confirming **no self-edge** for a recursive view.

## Affected files

- `crates/pgevolve-core/src/parse/ast_canon.rs` — CTE-scoping in `walk_node`
  (thread the in-scope CTE-name set; skip unqualified CTE-name references).
- `crates/pgevolve-core/src/catalog/assemble/views.rs` — the identical change in
  `walk_node_for_deps`.
- `crates/pgevolve-conformance/` fixtures (above).
- Possibly a small shared helper for "is this bare reference an in-scope CTE
  name" if it reads cleanly in both files (without unifying the walkers).

## Open questions

None. (Source-form handling, IR shape, and catalog-reader behavior were all
resolved during design: no new IR fields, no parser/reader changes, both source
forms desugar at parse time.)
