---
status: design
target: v1.0
sub_project: B (object-coverage roadmap)
---

# Roadmap revision for v1.0 — design

Sub-project **B** of the v1.0 path. Output: an edited
`docs/spec/roadmap.md` that aligns with the v1.0 charter
([`docs/v1.md`](../../v1.md)) and resolves the one ordering bug found
during dep-edge analysis. Pure docs sub-project.

---

## §1. Scope

This sub-project edits `docs/spec/roadmap.md` and (in the same
commit) flips the recursive-views row in `docs/spec/objects.md` from
`🔮 Future` to `📋 Planned, v0.5.3` — one synchronized line of work
across the two docs. It does NOT add object kinds beyond what already
appears in the charter §4 checklist plus the recursive-views row also
introduced by the charter. Per-release status flips (`📋` → `✅`)
happen when each feature ships, not in this sub-project.

The charter cross-reference paragraph already landed in commit
`e093139` (Sub-project A); this spec does not re-introduce it.

---

## §2. The 5 changes

### Change 1 — Swap the tablespace slots (the ordering bug)

Per-partition TABLESPACE was at v0.4.0; cluster TABLESPACE at v0.4.2.
But per-partition needs cluster (you can't set a partition's
`TABLESPACE` to a user-defined tablespace pgevolve doesn't manage).
Swap so cluster ships first:

- v0.4.0 row → `TABLESPACE` (cluster object), plan
  [`_skeleton/cluster-tablespace.md`](../plans/_skeleton/cluster-tablespace.md)
- v0.4.2 row → Per-partition `TABLESPACE`, plan
  [`_skeleton/per-partition-tablespace.md`](../plans/_skeleton/per-partition-tablespace.md)

The Notes column updates accordingly (see Change 3).

### Change 2 — Add the v0.5.3 recursive-views row

The charter §4 added recursive views as a v1.0 blocker. Materialize
this as an active-matrix row in roadmap.md and remove from the
"Future" section:

| Target | Item | Plan | Notes |
|---|---|---|---|
| v0.5.3 | Recursive views (`WITH RECURSIVE`) | `_skeleton/recursive-views.md` (new stub) | Depends on: planner cycle-aware dep-graph work (internal, no roadmap row) |

The "Future" section's "Recursive views (`WITH RECURSIVE`) — Requires
cycle-aware dep-graph handling" row is deleted.

### Change 3 — Add `Depends on:` notes per row

Update the Notes column for every active-matrix row that has a real
dep. Rows without internal deps stay as-is. The full updated Notes
column text after this sub-project lands:

| Target | Item | Notes (new) |
|---|---|---|
| v0.4.0 | EVENT TRIGGER | Independent surface |
| v0.4.0 | `TABLESPACE` (cluster object) | Reverses the "out of scope" stance in `objects.md`; see design doc. Independent (no internal deps). |
| v0.4.0 | `TABLE ... USING <access method>` | New `access_method` field on `Table`. Independent (no internal deps). |
| v0.4.1 | `AGGREGATE` (SQL/plpgsql state) | Constrained: v0.4.1 rejects non-readable state-function languages. Soft dep on PL-language wiring (v0.4.2) — non-SQL state-function support lands in a v0.4.2 follow-up. |
| v0.4.1 | PG 18 virtual generated columns | New `GeneratedKind` variant. Depends on: PG 18 catalog support (shipped v0.3.6). |
| v0.4.2 | Per-partition `TABLESPACE` | `tablespace` override on partition children. Depends on: `TABLESPACE` (cluster object), shipped v0.4.0. |
| v0.4.2 | PL-language wiring → non-SQL `FUNCTION` bodies | Enables PL/Python, PL/Perl, etc. Depends on: `CREATE EXTENSION` (shipped v0.2.x) for the language extension. |
| v0.4.3 | `TEXT SEARCH` family | Configuration / dictionary / parser / template. Depends on: `CREATE COLLATION` (shipped v0.3.8). |
| v0.5.0 | FDW family | `FDW`, `SERVER`, `USER MAPPING`, `FOREIGN TABLE`, `IMPORT FOREIGN SCHEMA`; includes secrets handling. Internal slot order within v0.5.0: FDW → SERVER → USER MAPPING → FOREIGN TABLE → IMPORT FOREIGN SCHEMA. |
| v0.5.1 | `OPERATOR` / `OPERATOR CLASS` / `OPERATOR FAMILY` | Heavy admin surface. Depends on: functions + custom types (both shipped v0.2.x). |
| v0.5.2 | `CAST` | Depends on: custom types + functions (both shipped v0.2.x). |
| v0.5.3 | Recursive views (`WITH RECURSIVE`) | Depends on: planner cycle-aware dep-graph work (internal, no roadmap row). |

### Change 4 — One-line v1.0 reminder above the Active matrix

Immediately above the `## Active matrix` heading, add a single line:

> **The 1.0 cut happens when this matrix is empty.** See
> [`../v1.md`](../v1.md) §4 for the full v1.0 feature checklist (this
> matrix is the source of truth; the charter restates it).

### Change 5 — Create the recursive-views plan stub

Add `docs/superpowers/plans/_skeleton/recursive-views.md` with the
standard skeleton-stub frontmatter and a short body naming the
internal cycle-aware-dep-graph prerequisite. Modeled on existing
stubs like `_skeleton/event-trigger.md`.

Skeleton content:

```markdown
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
```

---

## §3. What this design produces

One commit modifying:
- `docs/spec/roadmap.md` — Changes 1, 2, 3, 4 above.
- `docs/superpowers/plans/_skeleton/recursive-views.md` — Change 5 (new file).

No code, no tests beyond the standard "everything still builds" gate.

---

## §4. What this design does NOT do

- Touch the v1.0 charter (`docs/v1.md`). The charter §4 table already
  lists v0.5.3 recursive views; the roadmap edit just brings the
  source-of-truth document into alignment.
- Promote any other "🔮 Future" item into the v1.0 active matrix.
  The charter's §7 parking lot is the canonical post-1.0 list;
  recursive views was the only intentional promotion.
- Add any tooling (e.g., a check that roadmap dep-edges are
  consistent). Manual notes only, per the brainstorming Q&A.
