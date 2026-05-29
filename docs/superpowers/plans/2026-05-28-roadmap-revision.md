# Roadmap Revision Implementation Plan (sub-project B of v1.0 path)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Edit `docs/spec/roadmap.md`, `docs/spec/objects.md`, and add `docs/superpowers/plans/_skeleton/recursive-views.md` so the source-of-truth roadmap aligns with the v1.0 charter (`docs/v1.md` §4) and resolves the per-partition-TABLESPACE / cluster-TABLESPACE ordering bug.

**Architecture:** Pure documentation change. Two file edits + one new skeleton stub, all in a single commit. No code; verify gate is fmt + clippy + cargo doc (sanity).

**Tech Stack:** Markdown.

**Spec:** [`../specs/2026-05-28-roadmap-revision-design.md`](../specs/2026-05-28-roadmap-revision-design.md)

---

## Pre-flight

1. Confirm `main` is green: `git log --oneline -1`, `gh run list --branch main --limit 1` → ✅.
2. Read the spec end-to-end. The five changes listed in spec §2 are your work.
3. Read `docs/spec/roadmap.md`, `docs/spec/objects.md` so you know the current Markdown layout.

## Verify gate (run before committing)

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

No `cargo test` — docs only.

---

## File structure

### Created

- `docs/superpowers/plans/_skeleton/recursive-views.md` — skeleton stub for the new v0.5.3 row.

### Modified

- `docs/spec/roadmap.md` — Changes 1, 2, 3, 4 from spec §2 (swap tablespace slots, add v0.5.3 row, add Depends-on notes, add v1.0 reminder above the matrix).
- `docs/spec/objects.md` — one-line flip of the recursive-views row from `🔮 Future` to `📋 Planned, v0.5.3` (per spec §1, in-scope).

All three changes land in ONE commit.

---

## Task 1: Roadmap + objects + recursive-views stub (single commit)

**Files:**
- Modify: `docs/spec/roadmap.md`
- Modify: `docs/spec/objects.md`
- Create: `docs/superpowers/plans/_skeleton/recursive-views.md`

### Step 1.1: Edit `docs/spec/roadmap.md` — replace the Active matrix + Future section

- [ ] Find the existing block in `docs/spec/roadmap.md` that begins with the `## Active matrix` heading and ends just before the `## Future (no version commitment)` heading. Replace **the matrix table** (the rows from `v0.4.0 | EVENT TRIGGER ...` through `v0.5.2 | CAST | ...`) with the new content below. Keep the surrounding `## Active matrix` heading and any prose lines above the table.

Replacement matrix:

```markdown
**The 1.0 cut happens when this matrix is empty.** See
[`../v1.md`](../v1.md) §4 for the full v1.0 feature checklist (this
matrix is the source of truth; the charter restates it).

| Target | Object / sub-feature | Plan | Notes |
|---|---|---|---|
| v0.4.0 | `EVENT TRIGGER` | [`_skeleton/event-trigger.md`](../superpowers/plans/_skeleton/event-trigger.md) | Independent surface |
| v0.4.0 | `TABLESPACE` (cluster object) | [`_skeleton/cluster-tablespace.md`](../superpowers/plans/_skeleton/cluster-tablespace.md) | Reverses the "out of scope" stance in `objects.md`; see design doc. Independent (no internal deps). |
| v0.4.0 | `TABLE ... USING <access method>` | [`_skeleton/table-access-method.md`](../superpowers/plans/_skeleton/table-access-method.md) | New `access_method` field on `Table`. Independent (no internal deps). |
| v0.4.1 | `AGGREGATE` (SQL/plpgsql state) | [`_skeleton/aggregate.md`](../superpowers/plans/_skeleton/aggregate.md) | Constrained: v0.4.1 rejects non-readable state-function languages. Soft dep on PL-language wiring (v0.4.2) — non-SQL state-function support lands in a v0.4.2 follow-up. |
| v0.4.1 | PG 18 virtual generated columns | [`_skeleton/virtual-generated-columns.md`](../superpowers/plans/_skeleton/virtual-generated-columns.md) | New `GeneratedKind` variant. Depends on: PG 18 catalog support (shipped v0.3.6). |
| v0.4.2 | Per-partition `TABLESPACE` | [`_skeleton/per-partition-tablespace.md`](../superpowers/plans/_skeleton/per-partition-tablespace.md) | `tablespace` override on partition children. Depends on: `TABLESPACE` (cluster object), shipped v0.4.0. |
| v0.4.2 | PL-language wiring → non-SQL `FUNCTION` bodies | [`_skeleton/pl-language-wiring.md`](../superpowers/plans/_skeleton/pl-language-wiring.md) | Enables PL/Python, PL/Perl, etc. Depends on: `CREATE EXTENSION` (shipped v0.2.x) for the language extension. |
| v0.4.3 | `TEXT SEARCH` family | [`_skeleton/text-search.md`](../superpowers/plans/_skeleton/text-search.md) | Configuration / dictionary / parser / template. Depends on: `CREATE COLLATION` (shipped v0.3.8). |
| v0.5.0 | FDW family | [`_skeleton/fdw-family.md`](../superpowers/plans/_skeleton/fdw-family.md) | `FDW`, `SERVER`, `USER MAPPING`, `FOREIGN TABLE`, `IMPORT FOREIGN SCHEMA`; includes secrets handling. Internal slot order within v0.5.0: FDW → SERVER → USER MAPPING → FOREIGN TABLE → IMPORT FOREIGN SCHEMA. |
| v0.5.1 | `OPERATOR` / `OPERATOR CLASS` / `OPERATOR FAMILY` | [`_skeleton/operator-family.md`](../superpowers/plans/_skeleton/operator-family.md) | Heavy admin surface. Depends on: functions + custom types (both shipped v0.2.x). |
| v0.5.2 | `CAST` | [`_skeleton/cast.md`](../superpowers/plans/_skeleton/cast.md) | Depends on: custom types + functions (both shipped v0.2.x). |
| v0.5.3 | Recursive views (`WITH RECURSIVE`) | [`_skeleton/recursive-views.md`](../superpowers/plans/_skeleton/recursive-views.md) | Depends on: planner cycle-aware dep-graph work (internal, no roadmap row). |
```

Notes for the replacer:
- The two changed rows: `v0.4.0 | TABLESPACE (cluster object)` was at v0.4.2; `v0.4.2 | Per-partition TABLESPACE` was at v0.4.0. The bullet text in the Notes column updates accordingly.
- The new v0.5.3 row is appended at the bottom.
- The "1.0 cut happens when this matrix is empty" line goes immediately above the table (after the existing intro and the "Shipped" section, before the `## Active matrix` heading's body).

### Step 1.2: Edit `docs/spec/roadmap.md` — remove recursive-views from Future

- [ ] Find this exact row in the `## Future (no version commitment)` table:

```markdown
| Recursive views (`WITH RECURSIVE`) | Requires cycle-aware dep-graph handling |
```

Delete the whole row. Leave the surrounding "Future" table heading and other rows intact.

### Step 1.3: Edit `docs/spec/objects.md` — flip recursive-views status

- [ ] In `docs/spec/objects.md`, find this row (currently at approximately line 36):

```markdown
| Recursive views (`WITH RECURSIVE`) | 🔮 Future | Requires cycle-aware dep-graph handling. |
```

Replace with:

```markdown
| Recursive views (`WITH RECURSIVE`) | 📋 Planned, v0.5.3 | Requires cycle-aware dep-graph handling. See [`roadmap.md`](./roadmap.md). |
```

(Status flips `🔮 Future` → `📋 Planned, v0.5.3`; appends a link to the roadmap so a reader of objects.md can find the slot.)

### Step 1.4: Create the recursive-views skeleton stub

- [ ] Create `docs/superpowers/plans/_skeleton/recursive-views.md` with this exact content:

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

### Step 1.5: Run verify gate

Run:
```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```
Expected: all three pass.

### Step 1.6: Commit

```bash
git add docs/spec/roadmap.md docs/spec/objects.md docs/superpowers/plans/_skeleton/recursive-views.md
git commit -m "$(cat <<'EOF'
docs(spec): roadmap revision for v1.0 (sub-project B of v1.0 path)

Five changes per the design at
docs/superpowers/specs/2026-05-28-roadmap-revision-design.md:

1. Swap tablespace slots — cluster TABLESPACE now v0.4.0 (was v0.4.2),
   per-partition TABLESPACE now v0.4.2 (was v0.4.0). Fixes ordering
   bug: per-partition needs cluster's user-defined tablespace surface
   to exist first.
2. Add v0.5.3 row for recursive views (`WITH RECURSIVE`). Promoted
   from "Future" to "Active matrix" by the v1.0 charter §4.
3. Add `Depends on:` notes to each row in the Active matrix that has
   a real internal or prior-release dep. Independent rows stay plain.
4. Add "The 1.0 cut happens when this matrix is empty" line above the
   Active matrix, cross-referencing v1.0 charter §4.
5. Flip recursive-views row in objects.md from `🔮 Future` to
   `📋 Planned, v0.5.3`; add new `_skeleton/recursive-views.md` stub.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Self-review

Quick sanity check the commit hangs together:

- [ ] `git log --oneline -1` shows the one new commit.
- [ ] `git show HEAD --stat` shows three files: roadmap.md modified, objects.md modified, recursive-views.md new file.
- [ ] Read the updated `docs/spec/roadmap.md`:
  - Confirm the "Shipped" section is untouched.
  - Confirm the Active matrix has exactly 12 rows (was 11 before this commit).
  - Confirm both `TABLESPACE` rows have the correct version slot (cluster=v0.4.0, per-partition=v0.4.2).
  - Confirm every `📋 Planned` row's Notes column either says "Independent" or has a `Depends on:` / `Soft dep on:` clause.
  - Confirm the `## Future` section no longer contains the recursive-views row.
  - Confirm the "1.0 cut happens when this matrix is empty" reminder is present above the Active matrix.
- [ ] Read the updated `docs/spec/objects.md` line 36-ish: confirm the row reads `📋 Planned, v0.5.3` not `🔮 Future`.
- [ ] Read the new `docs/superpowers/plans/_skeleton/recursive-views.md`: confirm the frontmatter has `status: skeleton`, `target_version: v0.5.3`, `sub_spec: recursive-views`.

No code changes; nothing to test beyond the verify gate (already run) and this manual audit.

---

## Self-review (plan author's pass)

**1. Spec coverage:**
- Spec §2 Change 1 (swap tablespace slots) → Step 1.1 ✓
- Spec §2 Change 2 (add v0.5.3 recursive-views row) → Step 1.1 ✓
- Spec §2 Change 3 (add Depends-on notes) → Step 1.1 ✓
- Spec §2 Change 4 (v1.0 reminder above matrix) → Step 1.1 ✓
- Spec §2 Change 5 (recursive-views skeleton stub) → Step 1.4 ✓
- Spec §1 (synchronized objects.md one-liner flip) → Step 1.3 ✓
- Spec §3 (one commit) → Step 1.6 ✓
- Spec §4 (does NOT touch charter or promote anything else from Future) → respected; no charter edit in this plan ✓

All covered.

**2. Placeholder scan:** No TBD / TODO / "fill in" markers in the plan
itself. The skeleton stub content (Step 1.4) contains `TBD` markers in
its IR/reader/fixtures sections — these are intentional skeleton
content following the existing `_skeleton/*.md` convention (see
existing `_skeleton/event-trigger.md` for the precedent). Not plan
failures; they're stub-template content.

**3. Type consistency:** N/A — no code.

---

## Execution handoff

After Task 2 self-review passes, **do not push** automatically — per
CLAUDE.md directive 11, the user handles `git push origin main` (or
explicitly delegates). Surface the commit with `git log -1 --stat`
and wait for the explicit "push" or "yes push" confirmation.
