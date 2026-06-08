# Phase 1 — IR-Layer Hardening & Slimming Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the IR-layer illegal-state escape hatches and one over-typed surface identified in the 2026-06 architecture review (decisions 4, 6, 12, 13a), without changing any observable migration behavior.

**Architecture:** `pgevolve-core/src/ir/` is the load-bearing domain model; both the parser and the catalog reader lower into it and both pass through `ir::canon`. These are *type-shape* changes: each task changes one IR type so an illegal state becomes unrepresentable (or collapses an over-typed field), then fixes every construction/match site the Rust compiler flags. Because Rust's exhaustiveness and type checking enumerate every affected site, "fix every site `cargo build` reports" is a complete instruction, not a placeholder — the grep lists below are provided so the worker knows the blast radius up front.

**Tech Stack:** Rust (edition 2021), `serde`, `thiserror`. Workspace lints are strict (`clippy::pedantic` + `nursery`, `-D warnings`); no `unwrap`/`expect` in production code. Tests run with `cargo test`; type changes are driven by `cargo build`.

**Scope boundary:** This is the *IR-layer-only* slice. The changeset/diff illegal-state fixes (grant `signature`, direction-bools, `AlterObjectOwner`) were moved to Phase 3 because they live in `diff/` and share construction sites with the owner/grants dedup. The macro removal (decisions 2/3) is Phase 2 and runs *after* this phase so it writes hand-written impls against these finalized types.

**Decision-log note:** Decision 13a was revised during planning — the Subscription `connect`/`create_slot`/`copy_data` fields are **kept** (they are write-only CREATE options the testkit depends on, not dead surface). So 13a here is *only* the autovacuum collapse. Update the `ARCHITECTURE-REVIEW-2026-06.md` §0 row 13a accordingly (Task 0).

---

## Pre-flight

- [ ] **Step P1: Confirm a green baseline**

Run: `cargo test -p pgevolve-core && cargo clippy -p pgevolve-core --all-targets`
Expected: PASS / no warnings. If red, stop — do not build on a broken baseline.

- [ ] **Step P2: Note the commit convention**

This project commits directly to `main` (CLAUDE.md directive 9); there is no feature branch. Each task below ends in its own commit and **must leave `main` green** — run `cargo test -p pgevolve-core` + `cargo clippy -p pgevolve-core --all-targets` before every commit. The full workspace gate (fmt, clippy, test, `RUSTDOCFLAGS="-D warnings" cargo doc`, `cargo deny`) runs once at the end (Task 6). Every commit ends with the co-author trailer:
```
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```

---

### Task 0: Amend the decision-log for the 13a revision

**Files:**
- Modify: `docs/ARCHITECTURE-REVIEW-2026-06.md` (§0, row 13a)

- [ ] **Step 1: Edit the 13a row to reflect "subscription fields kept"**

Replace the row-13a cell text `Collapse the typed autovacuum reloption matrix into the existing \`extra\` map; drop the Subscription CREATE-only fields that never round-trip.` with:

```
**Autovacuum only.** Collapse the typed autovacuum reloption matrix into the existing `extra` map. Subscription `connect`/`create_slot`/`copy_data` are **kept** — planning showed they are intentional write-only CREATE options the testkit depends on, not dead surface; the read-asymmetry is already handled without bugs.
```

- [ ] **Step 2: Commit**

```bash
git add docs/ARCHITECTURE-REVIEW-2026-06.md
git commit -m "docs: record 13a revision (keep subscription CREATE-only fields)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 1: `ColumnType::Numeric` — make scale-without-precision unrepresentable

**Why:** `Numeric { precision: Option<u16>, scale: Option<i16> }` allows `precision: None, scale: Some(_)`, an illegal state guarded only by a runtime `unreachable!` in `render_sql` (`ir/column_type.rs:183-186`). Postgres `numeric` cannot have a scale without a precision.

**Files:**
- Modify: `crates/pgevolve-core/src/ir/column_type.rs` (enum variant, `render_sql`, `parse_canonical`, in-file tests)
- Modify (ripple, ~6 sites): `crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs:135,538`, `crates/pgevolve-core/src/plan/rewrite/emit/aggregate.rs:412`, `crates/pgevolve-core/src/parse/builder/create_composite_type_stmt.rs:175`, `crates/pgevolve-core/src/diff/aggregates.rs:223`, `crates/pgevolve-testkit/src/ir_generator/table.rs:208`

- [ ] **Step 1: Add the `NumericPrecision` newtype and change the variant**

In `column_type.rs`, replace the `Numeric` variant (lines 27-33) with:

```rust
    /// `numeric` / `decimal` with optional precision (and, only then, optional scale).
    /// `None` = unbounded `numeric`.
    Numeric(Option<NumericPrecision>),
```

And add this type just below the `ColumnType` enum (after line 103):

```rust
/// Precision/scale for a constrained `numeric(p[, s])`.
///
/// Scale is representable only *with* a precision — Postgres has no
/// `numeric(,s)` form — so the previous `precision: None, scale: Some(_)`
/// illegal state cannot be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NumericPrecision {
    /// Total digits (1..=1000).
    pub precision: u16,
    /// Digits to the right of the decimal point; `None` = scale 0.
    pub scale: Option<i16>,
}
```

- [ ] **Step 2: Update `render_sql`**

Replace the four `Self::Numeric { .. }` arms (lines 171-186), including the `unreachable!`, with:

```rust
            Self::Numeric(None) => "numeric".into(),
            Self::Numeric(Some(NumericPrecision { precision, scale: None })) => {
                format!("numeric({precision})")
            }
            Self::Numeric(Some(NumericPrecision { precision, scale: Some(s) })) => {
                format!("numeric({precision},{s})")
            }
```

The `unreachable!` is now gone — the illegal state is unrepresentable.

- [ ] **Step 3: Update `parse_canonical`**

Bare `numeric`/`decimal` (lines 311-314) → `Some(ColumnType::Numeric(None))`. The parameterized arm (lines 355-368) becomes:

```rust
        "numeric" | "decimal" => {
            let mut parts = args.split(',').map(str::trim);
            let precision: u16 = parts.next()?.parse().ok()?;
            let scale = parts.next().map(str::trim).map(str::parse).transpose().ok()?;
            Some(ColumnType::Numeric(Some(NumericPrecision { precision, scale })))
        }
```

- [ ] **Step 4: Update `has_default_btree_opclass`**

Line 146: `Self::Numeric { .. }` → `Self::Numeric(_)`.

- [ ] **Step 5: Build to enumerate the ripple, fix every flagged site**

Run: `cargo build -p pgevolve-core -p pgevolve-testkit`
Expected: FAIL with type errors at the ~6 ripple sites listed above plus the in-file tests. Rewrite each construction/match from the old struct form to the new form, e.g.:
- `ColumnType::Numeric { precision: Some(10), scale: Some(2) }` → `ColumnType::Numeric(Some(NumericPrecision { precision: 10, scale: Some(2) }))`
- `ColumnType::Numeric { precision: None, scale: None }` → `ColumnType::Numeric(None)`
- `ColumnType::Numeric { .. }` (match) → `ColumnType::Numeric(_)`

The testkit generator (`ir_generator/table.rs:208`) currently produces `precision`/`scale` independently — make it generate `Option<NumericPrecision>` so it can never emit scale-without-precision. Re-run `cargo build` until clean.

- [ ] **Step 6: Update the in-file tests and add the invariant note**

In `column_type.rs` tests, update every `Numeric { .. }` literal (lines ~547,557,562,637,641,704,708) to the new form. The existing `render_sql_round_trips_canonical` and `parameterized_types_parse` tests now also prove the illegal state is gone (there is no longer a 4th `Numeric` arm). Add:

```rust
    #[test]
    fn numeric_scale_requires_precision_by_construction() {
        // Compile-time proof: NumericPrecision has no way to set scale without
        // precision. This test documents the invariant and round-trips serde.
        let n = ColumnType::Numeric(Some(NumericPrecision { precision: 10, scale: Some(2) }));
        let j = serde_json::to_string(&n).unwrap();
        assert_eq!(ColumnType::parse_from_pg_type_string(&n.render_sql()).unwrap(), n);
        assert_eq!(serde_json::from_str::<ColumnType>(&j).unwrap(), n);
    }
```

- [ ] **Step 7: Verify**

Run: `cargo test -p pgevolve-core -p pgevolve-testkit && cargo clippy -p pgevolve-core --all-targets`
Expected: PASS / clean.

> **Note on serde shape:** this changes `Numeric`'s JSON from `{"kind":"numeric","precision":10,"scale":2}` to `{"kind":"numeric","precision":{"precision":10,"scale":2}}`. The catalog snapshot JSON schema is explicitly unstable (v1.md §2), so this is acceptable — but any committed snapshot fixtures with `numeric(p,s)` must be re-blessed. Run `cargo test` and if snapshot tests fail on a pure shape change, re-bless via the xtask bless command and inspect the diff to confirm it is shape-only.

- [ ] **Step 8: Commit**

```bash
git add crates/pgevolve-core/src/ir/column_type.rs crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs crates/pgevolve-core/src/plan/rewrite/emit/aggregate.rs crates/pgevolve-core/src/parse/builder/create_composite_type_stmt.rs crates/pgevolve-core/src/diff/aggregates.rs crates/pgevolve-testkit/src/ir_generator/table.rs
git commit -m "refactor(ir): make numeric scale-without-precision unrepresentable

Replace ColumnType::Numeric { precision, scale } with
Numeric(Option<NumericPrecision>), removing the render_sql unreachable!.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `ViewColumn` — remove the `"unresolved"` string sentinel

**Why:** an unresolved alias-list column type is currently `ColumnType::Other { raw: "unresolved" }` (`ir/view.rs:39-55`), a serializable magic string the docs warn "must never appear in a serialized catalog." Make it `Option<ColumnType>` (`None` = unresolved) so the sentinel cannot serialize, and enforce resolution at canon time.

**Files:**
- Modify: `crates/pgevolve-core/src/ir/view.rs` (`ViewColumn.column_type`, doc, tests)
- Modify (construction): `crates/pgevolve-core/src/parse/builder/create_view_stmt.rs:131-134`, `crates/pgevolve-core/src/parse/builder/create_materialized_view_stmt.rs:141-144`
- Modify (resolver): `crates/pgevolve-core/src/parse/ast_canon.rs` (the pass that fills resolved types — search for where `ViewColumn.column_type` is assigned the resolved type)
- Modify (readers, compiler-flagged): `crates/pgevolve-core/src/catalog/assemble/views.rs`, `crates/pgevolve-core/src/render/` view paths, any diff/view reader

- [ ] **Step 1: Change the field type**

In `view.rs`, line 55: `pub column_type: ColumnType,` → `pub column_type: Option<ColumnType>,`. Rewrite the doc comment (lines 34-57) to:

```rust
/// A single named column in a view or materialized view.
///
/// `column_type` is `None` while unresolved — when `ViewColumn` is built from
/// an explicit alias list during parsing, the type requires resolving the
/// SELECT body against the catalog. The AST-canonicalization pass fills it in.
/// Resolution is enforced by `Catalog::canonicalize`: a `None` that survives
/// to canon is an error, so a serialized catalog never carries an unresolved
/// column type. When built from the live catalog the type is always `Some`.
```

- [ ] **Step 2: Update the two parse construction sites**

In `create_view_stmt.rs:131-134` and `create_materialized_view_stmt.rs:141-144`, replace the `ColumnType::Other { raw: "unresolved".to_string() }` sentinel with `None`, and the comment with `// type resolved later by ast_canon`.

- [ ] **Step 3: Update the resolver to write `Some(_)`**

In `ast_canon.rs`, where the resolved type is assigned to `ViewColumn.column_type`, wrap it in `Some(...)`. (`cargo build` will flag the type mismatch precisely.)

- [ ] **Step 4: Add the canon-time resolution check**

Find where view columns are validated in `ir/canon/` (the same pass that runs `sort_and_dedupe`). Add a check that returns an error if any `ViewColumn.column_type` is `None` after resolution. Use the new error variant from Task 3 if available, else a focused `IrError`. Minimal form:

```rust
    for view in &catalog.views {
        for col in &view.columns {
            if col.column_type.is_none() {
                return Err(IrError::UnresolvedViewColumn {
                    view: view.qname.clone(),
                    column: col.name.clone(),
                });
            }
        }
    }
```

Add the variant to `ir/mod.rs`:

```rust
    /// A view column's type was still unresolved at canon time.
    #[error("view {view}: column {column} has an unresolved type (internal resolver bug)")]
    UnresolvedViewColumn {
        /// The view whose column is unresolved.
        view: crate::identifier::QualifiedName,
        /// The unresolved column.
        column: crate::identifier::Identifier,
    },
```

- [ ] **Step 5: Build, fix flagged readers**

Run: `cargo build -p pgevolve-core`
Expected: FAIL at every site that reads `ViewColumn.column_type` as a bare `ColumnType`. The catalog reader (`assemble/views.rs`) builds from live data — wrap its assignment in `Some`. Render/diff readers that need the type after canon can rely on it being `Some`; use `.as_ref()` with an explicit handling path (never `unwrap`/`expect` in production — return `IrError`/render an empty string per the local contract). Fix each until clean.

- [ ] **Step 6: Update tests + add a guard test**

Update `view.rs` tests that build `ViewColumn`. Add:

```rust
    #[test]
    fn unresolved_view_column_rejected_by_canon() {
        // A ViewColumn with column_type: None must not survive canonicalize.
        // (Construct a minimal Catalog with one such view and assert canonicalize errs.)
        // ... build catalog ...
        assert!(matches!(cat.canonicalize(), Err(IrError::UnresolvedViewColumn { .. })));
    }
```

- [ ] **Step 7: Verify**

Run: `cargo test -p pgevolve-core && cargo clippy -p pgevolve-core --all-targets`
Expected: PASS / clean. Confirm no `"unresolved"` literals remain: `grep -rn '"unresolved"' crates/` returns nothing.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(ir): replace ViewColumn unresolved sentinel with Option

column_type is now Option<ColumnType> (None = unresolved); canonicalize
rejects any None that survives resolution, so the sentinel can never
serialize.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `IrError` — dedicated duplicate-object variant

**Why:** duplicate-object detection in `ir/canon/sort_and_dedupe.rs` (10 sites) reports `IrError::InvalidIdentifier`, conflating "this string isn't a valid identifier" with "two objects share a name." The taxonomy already has typed `Duplicate*` variants for event-triggers/aggregates/casts/tablespaces — extend the same treatment to the generic keyed collections.

**Files:**
- Modify: `crates/pgevolve-core/src/ir/mod.rs` (add variant)
- Modify: `crates/pgevolve-core/src/ir/canon/sort_and_dedupe.rs` (10 duplicate-key sites: lines 17,24,31,38,45,52,60,67,93,100,107)
- Modify: `crates/pgevolve-core/src/ir/catalog.rs:321` (test expectation)

> **Do NOT touch** the ~80 *genuine* `InvalidIdentifier` uses in `catalog/assemble/*` and `parse/builder/*` — those are correct (a real malformed identifier). Only the `sort_and_dedupe.rs` duplicate-detection sites change.

- [ ] **Step 1: Add the variant**

In `ir/mod.rs` `IrError`, add:

```rust
    /// Two objects in the same collection share a key (name / qualified name /
    /// overload identity).
    #[error("duplicate {kind}: {key}")]
    DuplicateObject {
        /// Human-readable object kind, e.g. "table", "index", "schema".
        kind: &'static str,
        /// The duplicated key, formatted for display.
        key: String,
    },
```

- [ ] **Step 2: Read `sort_and_dedupe.rs` and rewrite each duplicate site**

Run: `cargo build -p pgevolve-core` after editing. For each of the 10 `return Err(IrError::InvalidIdentifier(format!("duplicate ... {key}")))` sites, switch to `IrError::DuplicateObject { kind: "<object>", key: format!("{key}") }`, using the object name the surrounding function dedupes (e.g. `"table"`, `"index"`, `"sequence"`, `"view"`, `"schema"`, …; the function names / comments indicate which). Keep the message content equivalent.

- [ ] **Step 3: Fix the test expectation**

`catalog.rs:321` `assert!(matches!(r, Err(IrError::InvalidIdentifier(_))))` (the `canonicalize_rejects_duplicate_table` test) → `Err(IrError::DuplicateObject { kind: "table", .. })`. Check whether `user_type.rs:312`, `view.rs:442/455`, `function.rs:512`, `procedure.rs:117` test *duplicate* detection (→ update) or genuine *invalid identifier* (→ leave). Use the test body to decide; only change the duplicate-detection assertions.

- [ ] **Step 4: Verify**

Run: `cargo test -p pgevolve-core && cargo clippy -p pgevolve-core --all-targets`
Expected: PASS / clean.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/ir/mod.rs crates/pgevolve-core/src/ir/canon/sort_and_dedupe.rs crates/pgevolve-core/src/ir/catalog.rs
git commit -m "refactor(ir): dedicated IrError::DuplicateObject for dup keys

Stop overloading InvalidIdentifier for duplicate-object detection in
sort_and_dedupe; genuine invalid-identifier uses are unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Fix the `lib.rs` "I/O-free" doc claim

**Why:** `crates/pgevolve-core/src/lib.rs:2-3` claims the crate is "I/O-free", but `parse_directory` reads the filesystem. The accurate guarantee is *no network I/O*; DB access is injected via `CatalogQuerier`.

**Files:**
- Modify: `crates/pgevolve-core/src/lib.rs:2-5`

- [ ] **Step 1: Edit the module doc**

Replace lines 2-5:

```rust
//! This crate performs no network I/O: live-database access is injected by the
//! caller via a `CatalogQuerier` implementation, and the crate returns IR,
//! diffs, and plans as data. (It does read source `.sql` files from disk in
//! `parse::parse_directory`.) See `docs/superpowers/specs/` for the design.
```

- [ ] **Step 2: Verify doc builds**

Run: `RUSTDOCFLAGS="-D warnings" cargo doc -p pgevolve-core --no-deps`
Expected: PASS (no broken intra-doc links).

- [ ] **Step 3: Commit**

```bash
git add crates/pgevolve-core/src/lib.rs
git commit -m "docs(core): correct the I/O-free claim (no network I/O; reads source files)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Collapse the typed autovacuum reloption matrix into `extra`

**Why (decision 13a):** `AutovacuumOptions` models ~16 autovacuum knobs as individually-typed `Option<u64>` / `Option<NotNanF64>` fields plus a bespoke `NotNanF64` newtype — high-effort, low-value surface most users never touch. `TableStorageOptions.extra: BTreeMap<String, String>` already exists for the long tail. Route `autovacuum_*` keys through `extra`, delete `AutovacuumOptions` and (if now unused) `NotNanF64`.

**Trade-off (accept, per decision):** parse-time *typed* validation of autovacuum values is lost — they become free-form strings in `extra`, like every other unknown reloption. This is the understood cost of the collapse. Existing range-validation lint rules (if any reference autovacuum keys) continue to operate on `extra` strings.

**Files (9, all flagged by `cargo build`):**
- `crates/pgevolve-core/src/ir/reloptions.rs` — delete `AutovacuumOptions`, remove `autovacuum` field from `TableStorageOptions`, delete `NotNanF64` if unused elsewhere (grep first)
- `crates/pgevolve-core/src/parse/builder/reloptions.rs` — route `autovacuum_*` keys into `extra`
- `crates/pgevolve-core/src/parse/builder/alter_table_stmt.rs`
- `crates/pgevolve-core/src/catalog/reloptions.rs` — read `autovacuum_*` from `pg_class.reloptions` into `extra`
- `crates/pgevolve-core/src/diff/reloptions.rs` — drop autovacuum-specific diffing (extra is already diffed as a map)
- `crates/pgevolve-core/src/plan/rewrite/reloptions.rs` — emit autovacuum keys from `extra`
- `crates/pgevolve-core/src/lint/rules/unmanaged_reloption.rs`
- `crates/pgevolve-testkit/src/ir_generator/reloptions.rs` — generate autovacuum keys as `extra` entries
- `crates/pgevolve-core/tests/catalog_reloptions.rs` — update expectations

- [ ] **Step 1: Confirm `NotNanF64`'s only consumer is autovacuum**

Run: `grep -rn 'NotNanF64' crates/`
Expected: uses only in `reloptions.rs` (def + autovacuum scale-factor fields + tests). If anything else uses it, keep it; otherwise it will be deleted in Step 3.

- [ ] **Step 2: Read the three "produce reloptions" sites to learn the `extra` insertion idiom**

Read `parse/builder/reloptions.rs`, `catalog/reloptions.rs`, and `plan/rewrite/reloptions.rs`. Note how non-autovacuum unknown keys already flow into / out of `extra` (the canonical key→string handling). The autovacuum keys will use the identical path; the goal is to delete the special-cased typed branch and let autovacuum keys fall through to the generic `extra` handling.

- [ ] **Step 3: Edit `ir/reloptions.rs`**

Remove the `autovacuum: AutovacuumOptions` field from `TableStorageOptions` (line 128) and its references in `TableStorageOptions::is_empty` (line 146). Delete the `AutovacuumOptions` struct + impl (lines 61-119) and, if Step 1 confirmed it, the `NotNanF64` struct + all its trait impls (lines 14-59). Update the `is_empty` doc/test.

- [ ] **Step 4: Build, route autovacuum keys through `extra` at each site**

Run: `cargo build -p pgevolve-core -p pgevolve-testkit`
Expected: FAIL in the parse/catalog/plan/diff/lint/testkit sites that referenced `.autovacuum`. At each: delete the typed-autovacuum branch so `autovacuum_*` keys are inserted into / read from `extra` like any other reloption. In `diff/reloptions.rs`, remove the autovacuum sub-diff (the `extra` `BTreeMap` is already structurally diffed). Re-run until clean.

- [ ] **Step 5: Update tests**

`tests/catalog_reloptions.rs` and the in-file `reloptions.rs` tests: assert autovacuum settings now appear as `extra["autovacuum_vacuum_scale_factor"] = "0.1"` (string) rather than typed fields. Add one round-trip test proving an `autovacuum_enabled=false` table parses → an `extra` entry and renders back to the same `WITH (autovacuum_enabled=false)`.

- [ ] **Step 6: Verify (incl. conformance reloption fixtures if Docker available)**

Run: `cargo test -p pgevolve-core -p pgevolve-testkit && cargo clippy -p pgevolve-core --all-targets`
Expected: PASS / clean. If any catalog snapshot fixtures recorded typed autovacuum fields, re-bless and confirm the diff is shape-only (typed field → `extra` entry, same values).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(ir): collapse typed autovacuum reloptions into extra map

Route autovacuum_* keys through TableStorageOptions.extra; delete
AutovacuumOptions and NotNanF64. Trades typed validation for a smaller
IR surface (decision 13a, autovacuum only).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Full workspace gate + phase wrap

**Files:** none (verification only)

- [ ] **Step 1: Run the full local gate**

Run:
```bash
cargo fmt --check \
 && cargo clippy --workspace --all-targets -- -D warnings \
 && cargo test --workspace \
 && RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps \
 && cargo deny check
```
Expected: all PASS. If the conformance suite needs Docker and it is available, it runs here; if not, note that CI will run it across PG 14–18 on push.

- [ ] **Step 2: Confirm the illegal states are gone**

Run:
```bash
grep -rn 'unreachable!("scale without precision' crates/ ; \
grep -rn '"unresolved"' crates/ ; \
grep -rn 'AutovacuumOptions\|NotNanF64' crates/
```
Expected: all three return nothing (the first two empty; the third empty unless `NotNanF64` was retained for a non-autovacuum consumer found in Task 5 Step 1).

- [ ] **Step 3: Push**

```bash
git push origin main
```
Then wait for the per-push CI to go green across all 5 PG majors before considering Phase 1 done (CLAUDE.md directive 11). Do not start Phase 2 until CI is green — Phase 2 builds hand-written `Diff` impls against these finalized types.

---

## Self-review notes (for the executor)

- **Spec coverage:** Task 1 = decision 4 (Numeric); Task 2 = decision 4 (ViewColumn sentinel); Task 3 = decision 4 (IrError overload); Task 4 = decision 12 (lib.rs doc); Task 5 = decision 13a (autovacuum); Task 0 records the 13a subscription revision. The other three decision-4 items (grant `signature`, direction-bools, `AlterObjectOwner`) are intentionally deferred to Phase 3 (diff-layer) — not omitted.
- **Type consistency:** `NumericPrecision { precision: u16, scale: Option<i16> }` is referenced identically in Tasks 1's variant, render, parse, and tests. `IrError::DuplicateObject { kind: &'static str, key: String }` and `IrError::UnresolvedViewColumn { view, column }` are the exact shapes used in Tasks 2 and 3.
- **No behavior change:** every task is a representation change; migration output is unchanged except the deliberate serde-shape shifts (Numeric, ViewColumn, autovacuum→extra) on the explicitly-unstable catalog JSON. Snapshot re-bless, where needed, must be inspected to confirm it is shape-only.
