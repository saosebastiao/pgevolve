# Phase 4 — Type-replacement CASCADE auditability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the type `DROP … CASCADE` replacement **auditable** — the plan must name every dependent (columns, views, functions, embedding composites/domains/ranges) that the CASCADE will destroy, instead of hiding it. (Decision 8, reshaped: enumeration, not a full recreation engine — see below.)

**Architecture:** An incompatible enum/composite/range/domain change has no in-place `ALTER`, so the planner emits `DROP TYPE … CASCADE` + `CREATE TYPE` (gated behind destructive-approval). The CASCADE silently drops dependents. This phase adds a **plan-layer enumerator** that walks the full target catalog (available at emit time via `Ctx.target`) to list every dependent of the type being replaced, and enriches the step's `destructive_reason` (and optionally `targets`) so the operator sees exactly what will be destroyed. The teardown stays `CASCADE` (controlled); only the auditability changes.

**Tech Stack:** Rust, serde. STRICT lints (clippy pedantic+nursery, `-D warnings`); no `unwrap`/`expect` in production. Per-commit gate INCLUDES `cargo fmt --check` AND `RUSTDOCFLAGS="-D warnings" cargo doc -p pgevolve-core --no-deps` (Phase 3 lesson — a private intra-doc link slipped past clippy/test). Commits go directly to `main`. Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Prerequisite:** Phase 3 CI green across all 5 PG majors (confirmed).

**Why this shape (record in the decision log, Task 0):** decision 8 said "kill CASCADE, reuse the view walker." Investigation showed that premise is inaccurate: the view walker (`recreate_views.rs`) walks SQL `body_dependencies`, but a type's dependents are **table columns** (and views/functions/embedding types), a different dependency the view walker doesn't model. The destruction is also inherent — an incompatible enum/composite change must drop dependent columns either way (the CASCADE is already destructive-gated). So the achievable, simplicity-aligned goal is **auditability** (name what's destroyed), not a recreation engine. Full explicit type-dependent recreation is deferred to the post-1.0 parking lot.

---

### Task 0: Record the decision-8 reshape

**Files:** `docs/ARCHITECTURE-REVIEW-2026-06.md` (§0 row 8), `docs/v1.md` (§7 parking lot)

- [ ] **Step 1:** Edit the §0 row-8 resolution cell to:
```
**Auditability via enumeration** (reshaped from "reuse the view walker", which was inaccurate — types are depended on by table columns, not view bodies). The plan now enumerates every dependent of a type being CASCADE-replaced and names them in the destructive warning; the CASCADE teardown is kept (already destructive-gated). Full explicit type-dependent recreation deferred to post-1.0 (v1.md §7).
```
- [ ] **Step 2:** Add a `v1.md` §7 parking-lot row:
```
| **Full explicit type-dependent recreation** (DROP each dependent + recreate views/functions, replacing the type CASCADE entirely) | The 1.0 work makes the CASCADE auditable (names every dependent); the larger recreation engine is deferred. |
```
- [ ] **Step 3: Commit.** `docs: record decision-8 reshape (type CASCADE auditability)`.

---

### Task 1: The type-dependent enumerator

**Why:** one function that, given a type's `QualifiedName` and the target `Catalog`, returns every object that depends on it (i.e. what `DROP TYPE … CASCADE` would destroy).

**Files:** create `crates/pgevolve-core/src/plan/type_dependents.rs` (wire into `plan/mod.rs`). Reuse the `ColumnType::UserDefined` detection idiom already used in `plan/edges.rs` (~line 467).

- [ ] **Step 1: Define the dependent type + enumerator.**
```rust
//! Enumerate every catalog object that depends on a user-defined type — i.e.
//! exactly what `DROP TYPE <t> CASCADE` would destroy. Used to make the type
//! replacement auditable (the destruction is inherent; this names it).

use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::ir::column_type::ColumnType;

/// One object that depends on a type being CASCADE-replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypeDependent {
    /// `table.column` whose type is (or contains) the dropped type. DATA LOSS.
    Column { table: QualifiedName, column: crate::identifier::Identifier },
    /// A view / materialized view with a column of the dropped type.
    View(QualifiedName),
    MaterializedView(QualifiedName),
    /// A function/procedure whose argument or return type is the dropped type.
    Routine(QualifiedName),
    /// Another user-defined type embedding the dropped type (composite attr,
    /// domain base, range subtype).
    Type(QualifiedName),
}

/// Every dependent of `ty` in `target`, in a deterministic order (sorted).
pub(crate) fn enumerate_type_dependents(ty: &QualifiedName, target: &Catalog) -> Vec<TypeDependent> {
    // walk target.tables[*].columns[*].column_type
    // walk target.views / materialized_views[*].columns[*].column_type
    // walk target.functions / procedures arg_types + return_type
    // walk target.types[*] composite attrs / domain base / range subtype
    // For each, if the type (directly, or as an Array element, or a domain whose
    // base is `ty`) matches `ty`, push the corresponding TypeDependent.
    // Then sort + dedup for determinism.
}

/// `ColumnType` references `ty` directly or as an array element. (Domains are
/// separate catalog objects, surfaced as Type dependents, so this is the direct
/// column/attr case.)
fn column_type_references(ct: &ColumnType, ty: &QualifiedName) -> bool {
    match ct {
        ColumnType::UserDefined(q) => q == ty,
        ColumnType::Array { element, .. } => column_type_references(element, ty),
        _ => false,
    }
}
```
Read the actual IR shapes (`Catalog` fields, `ViewColumn.column_type` is `Option<ColumnType>` after Phase 1, function `arg_types`/`return_type` shapes, `UserTypeKind::{Composite,Domain,Range}`) and fill the walk bodies to match. Use the exact field names — `cargo build` will guide.

- [ ] **Step 2: Unit tests** (no DB needed — pure IR walk). Build a `Catalog` with a type `app.color` and: a table column of that type, a view column, a function arg, a composite embedding it, an array column of it. Assert `enumerate_type_dependents` returns all of them, sorted/deduped. Add a negative test (a type with no dependents → empty).

- [ ] **Step 3: Verify + commit.** `cargo fmt --check && cargo clippy -p pgevolve-core --all-targets && cargo test -p pgevolve-core type_dependents && RUSTDOCFLAGS="-D warnings" cargo doc -p pgevolve-core --no-deps`. Commit:
```
feat(plan): enumerate type dependents (what DROP TYPE CASCADE destroys)
```

---

### Task 2: Surface the dependents in the CASCADE destructive warning

**Why:** when the planner emits a type/domain `DROP … CASCADE` replacement, the plan must NAME the dependents the enumerator found.

**Files:** `crates/pgevolve-core/src/plan/rewrite/emit/user_type.rs` (the `ReplaceWithCascade` arm ~L66 and any domain-CASCADE arm); `Ctx` already carries `target: &Catalog`.

- [ ] **Step 1: Find every CASCADE-emitting arm.** In `emit/user_type.rs`, the `U::ReplaceWithCascade { source, catalog }` arm emits `emit_drop_type_cascade`. Check for a domain-cascade path too (diff/types.rs:349 emits a domain collation-change `ReplaceWithCascade`; confirm whether it routes through the same arm or a separate one — `grep -rn 'cascade\|CASCADE' crates/pgevolve-core/src/plan/rewrite/`).

- [ ] **Step 2: Enrich the destructive_reason.** In the CASCADE arm, before pushing the `DropType` step, call `enumerate_type_dependents(&catalog.qname, ctx.target)`. Build an enriched reason that appends the dependent list to the incoming `destructive_reason`, e.g.:
```rust
let deps = crate::plan::type_dependents::enumerate_type_dependents(&catalog.qname, ctx.target);
let enriched = render_cascade_reason(destructive_reason.as_deref(), &catalog.qname, &deps);
// render_cascade_reason: base reason + "; DROP TYPE app.color CASCADE will destroy: column app.t.c, view app.v, function app.f(integer)"
```
Apply `enriched` to BOTH the `DropType` (CASCADE) step and the destructive `CreateType` recreation step (it already shares the destructive gate). Keep the format human-readable and deterministic (deps are sorted). If `deps` is empty (a type with no dependents — CASCADE is then a plain drop), the reason is unchanged / notes "no dependents".

- [ ] **Step 3 (optional but recommended): add dependents to `targets`.** The `DropType` step's `targets: Vec<QualifiedName>` currently lists only the type. Consider adding the dependent object qnames (columns' tables, views, routines, types) so they appear in the plan's machine-readable affected-objects list. Only do this if it doesn't break existing `targets`-consuming code/tests (check `touches_only`/audit assertions in conformance); if it does, keep it string-only in the reason.

- [ ] **Step 4: Tests.** Add a plan-level test: a target catalog with a type used by a column + a view, a source that changes the type incompatibly (forcing `ReplaceWithCascade`); run the planner; assert the emitted CASCADE step's `destructive_reason` NAMES the column and the view. Run the existing user-type/conformance tests — the emitted SQL is UNCHANGED (still `DROP TYPE … CASCADE`), only the reason string grows, so no plan.sql SQL-body golden should change. The `destructive_reason` may appear in plan manifests/goldens — if a conformance golden records the reason text, re-bless and confirm the only change is the added dependent enumeration.

- [ ] **Step 5: Verify + commit.** Full per-task gate (fmt + clippy + test + doc). Commit:
```
feat(plan): name CASCADE dependents in the type-replacement destructive warning
```

---

### Task 3: Phase wrap — full workspace gate + push
- [ ] **Step 1:** `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps && cargo deny check`. (If a local `cargo test --workspace` flakes on Docker ephemeral-PG contention — many DB-backed binaries in parallel — re-run the specific failed test in isolation to confirm it's contention, not a regression; CI's per-major jobs avoid this.)
- [ ] **Step 2:** `git push origin main`; wait for CI green across all 5 PG majors before Phase 4 is done.

---

## Self-review notes
- **Spec coverage:** Task 1 = the enumerator; Task 2 = surfacing it in the destructive warning (decision 8 reshaped goal); Task 0 = record the reshape + defer the full recreation engine.
- **No emitted-SQL change** — the teardown stays `DROP TYPE … CASCADE`; only the `destructive_reason` text (and optionally `targets`) grows. The only golden that can change is a manifest recording the reason text (re-bless, confirm it's only the added enumeration).
- **The enumerator must be complete** — a missed dependent category means the audit under-reports what CASCADE destroys. Cover: table columns (incl. arrays), view/MV columns, function/procedure arg+return types, composite attrs, domain bases, range subtypes. Cross-check against what `DROP TYPE CASCADE` actually cascades in Postgres.
- Per-task gate INCLUDES `cargo fmt --check` AND `cargo doc -D warnings` (Phase 3 lessons).
