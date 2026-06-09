# Phase 2 — Remove the `#[derive(Diff)]` proc-macro + rename the equivalence trait Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete the `pgevolve-core-macros` crate (decision 2) by replacing all `#[derive(DiffMacro)]` derives with hand-written impls, then rename the equivalence trait so "diff" unambiguously means the migration change-engine (decision 3). No behavior change — the equivalence output must be byte-identical.

**Architecture:** `ir::eq::Diff` is an *equivalence* trait (`diff() -> Vec<Difference>`, `canonical_eq()`); it is unrelated to the `diff::ChangeSet` migration engine and is used mainly by round-trip/validate/testkit equivalence checks. The macro (`pgevolve-core-macros`, a published 2nd crate) only generates the trivial flat-struct impls; 9 hard impls are already hand-written. We replace the 13 derived impls with hand-written ones (the macro's per-field attributes are the exact spec), delete the macro crate, then rename `Diff`→`Equiv` / `diff`→`differences` / the two helpers. Rust's exhaustiveness (`let Self { .. }` destructuring) guarantees no field is dropped; the existing per-type `canonical_eq` tests + round-trip/conformance suites guarantee output equivalence.

**Tech Stack:** Rust, serde, thiserror. STRICT lints (clippy pedantic+nursery, `-D warnings`); no `unwrap`/`expect` in production. Commits go directly to `main`; per-task local gate is `cargo fmt --check && cargo clippy -p pgevolve-core --all-targets && cargo test -p pgevolve-core` (note: **`cargo fmt --check` is mandatory per commit** — Phase 1 accumulated fmt drift by omitting it). Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Order rationale:** hand-write impls FIRST (Task 1, still using the existing `Diff`/`diff`/`diff_field` names so the 9 existing hand-written impls serve as copy-paste templates), DELETE the macro SECOND (Task 2), RENAME LAST (Task 3, one mechanical compiler-guided sweep). This keeps each task green and avoids touching the soon-deleted macro.

**Prerequisite:** Phase 1's CI run must be green across all 5 PG majors before starting (it finalizes the IR types these impls compare).

---

## The 13 derived types (replace each derive with a hand-written impl)

From `grep -rn 'DiffMacro' crates/pgevolve-core/src`:
- `ir/procedure.rs` — `Procedure`
- `ir/sequence.rs` — `Sequence`
- `ir/constraint.rs` — `Constraint` (L14) and `ForeignKey` (L62) [note: `ConstraintKind` in the same file is ALREADY hand-written — leave it]
- `ir/index.rs` — `Index`
- `ir/schema.rs` — `Schema`
- `ir/trigger.rs` — `Trigger`
- `ir/column.rs` — `Column`
- `ir/default_privileges.rs` — the derived rule struct
- `ir/extension.rs` — `Extension`
- `ir/cluster/role.rs` — two structs (L9, L30)
- `ir/cluster/tablespace.rs` — `Tablespace` (L40)

The authoritative count is "every `#[derive(..., DiffMacro)]`" — after Task 1, `grep -rn 'DiffMacro' crates/` must return ONLY the re-export line in `ir/eq.rs` (removed in Task 2).

## The 9 already-hand-written impls (templates; Task 1 does NOT change these, Task 3 renames them)

`ConstraintKind` (constraint.rs:134), `ColumnType` (column_type.rs:457), `UserType` (user_type.rs:112), `Catalog` (catalog.rs:115), `Table` (table.rs:49), `Function` (function.rs:106), `View` (view.rs:123), `MaterializedView` (view.rs:216), `DefaultExpr` (default_expr.rs:103). **`Table::diff` (table.rs:49) is the canonical template** — copy its shape.

## The macro's field-attribute → hand-written translation (this IS the spec)

The macro (`crates/pgevolve-core-macros/src/lib.rs`) emits, per field, exactly one of:

| `#[diff(...)]` attribute | Generated line (replicate verbatim) |
|---|---|
| *(none / plain)* | `out.extend(diff_field("FIELD", &self.FIELD, &other.FIELD));` |
| `#[diff(via_debug)]` | `out.extend(diff_field("FIELD", &format!("{:?}", self.FIELD), &format!("{:?}", other.FIELD)));` |
| `#[diff(nested)]` | `out.extend(prefix_diffs("FIELD", Diff::diff(&self.FIELD, &other.FIELD)));` |
| `#[diff(skip)]` | *(omit the field entirely)* |

So a hand-written impl is a flat list of these lines, one per non-skipped field, wrapped in `fn diff(&self, other: &Self) -> Vec<Difference> { let mut out = Vec::new(); <lines> out }`.

---

## Pre-flight

- [ ] **Step P1: Confirm Phase 1 CI is green across all 5 PG majors** (`gh run list --branch main --limit 3` — the latest `ci` run `success`). Do not start until green.
- [ ] **Step P2: Confirm a green baseline:** `cargo fmt --check && cargo test -p pgevolve-core && cargo clippy -p pgevolve-core --all-targets` → all pass/clean.

---

### Task 1: Replace all 13 `#[derive(DiffMacro)]` with hand-written `impl Diff`

**Goal:** Every derived type gets a hand-written `impl Diff` matching the macro's output exactly. Use exhaustive `let Self { .. } = self;` destructuring at the top of each `diff()` so the compiler errors if a future field is added without a diff line (this is the one real benefit the macro provided; preserve it).

**Files:** the 11 files listed above (procedure, sequence, constraint, index, schema, trigger, column, default_privileges, extension, cluster/role, cluster/tablespace).

**Per-type procedure (repeat for each of the 13 structs):**

- [ ] **Step 1: Read the struct + its field attributes.** Open the file, read the struct definition and every field's `#[diff(...)]` attribute. These attributes are the exact spec for the impl.

- [ ] **Step 2: Write the hand-written impl** directly below the struct (matching where `Table::diff` sits relative to `Table`). Pattern — worked example for `Column` (`ir/column.rs`: field `name` plain, the other ~9 fields `#[diff(via_debug)]`):

```rust
impl Diff for Column {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        // Exhaustive destructure: compiler errors if a field is added without a
        // diff line below. Bindings are intentionally unused (we read via self).
        let Self {
            name: _,
            ty: _,
            nullable: _,
            default: _,
            // ... every remaining field, each `: _`
        } = self;
        let mut out = Vec::new();
        out.extend(diff_field("name", &self.name, &other.name));
        out.extend(diff_field("ty", &format!("{:?}", self.ty), &format!("{:?}", other.ty)));
        out.extend(diff_field("nullable", &format!("{:?}", self.nullable), &format!("{:?}", other.nullable)));
        // ... one line per field, per the attribute→line table
        out
    }
}
```
Translate each field via the table above. For `#[diff(skip)]` fields, still list them in the destructure as `field: _` but emit NO diff line (add a `// skip` comment). For `#[diff(nested)]`, use the `prefix_diffs("field", Diff::diff(&self.field, &other.field))` form.

- [ ] **Step 3: Update the file's imports.** Remove `use crate::ir::eq::DiffMacro;`. Add the helpers the impl needs: `use crate::ir::eq::{Diff, diff_field};` (+ `prefix_diffs` if any field is `nested`). Remove `DiffMacro` from the `#[derive(...)]` list, leaving the other derives intact.

- [ ] **Step 4: Compile + run that file's tests.** `cargo test -p pgevolve-core <module>` (e.g. `column::`). Most derived types already have a `canonical_eq` test in their `#[cfg(test)]` module — it now exercises the hand-written impl and MUST still pass (proving output equivalence). If a type has NO such test, add a minimal one: two equal values → `diff().is_empty()`; one differing field → exactly one `Difference` with the expected `path`.

**After all 13:**

- [ ] **Step 5: Confirm the macro is fully unused.** `grep -rn 'DiffMacro' crates/` returns ONLY `ir/eq.rs:9` (the re-export). `grep -rn 'derive(.*DiffMacro\|DiffMacro.*)' crates/` returns nothing in the `#[derive(...)]` position.

- [ ] **Step 6: Full local gate + commit.** `cargo fmt --check && cargo clippy -p pgevolve-core --all-targets && cargo test -p pgevolve-core` (all green). If clippy flags the unused destructure bindings, the `field: _` form (not `ref field`) avoids it; if it still complains, `let Self { .. } = self;` plain (bare `..`) is the fallback that keeps the "added field" guard only when you remove the `..` — prefer enumerating `field: _` so the guard is real. Commit:
```bash
git add -A
git commit -m "refactor(ir): hand-write Diff impls, drop the DiffMacro derive

Replace all 13 #[derive(DiffMacro)] with explicit impls (exhaustive
let Self { .. } destructure preserves the compile-time field-completeness
guard). Output is byte-identical to the macro; existing canonical_eq tests
verify equivalence.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

> **Reviewer note for this task:** the critical check is OUTPUT EQUIVALENCE — every hand-written impl must produce the same `Vec<Difference>` (same paths, same via_debug-vs-display formatting) the macro did. Verify by (a) confirming each field's translation matches the attribute table, and (b) the per-type `canonical_eq` tests + the `dump_round_trip`/`parser_corpus` tests (which use `ir::eq::Diff`) still pass.

---

### Task 2: Delete the `pgevolve-core-macros` crate

**Files:**
- Delete dir: `crates/pgevolve-core-macros/`
- Modify: `Cargo.toml` (workspace `members` — remove `"crates/pgevolve-core-macros"`; remove any `[workspace.dependencies]` entry for it)
- Modify: `crates/pgevolve-core/Cargo.toml` (remove the `pgevolve-core-macros = { ... }` dependency line)
- Modify: `crates/pgevolve-core/src/ir/eq.rs` (remove the `pub use pgevolve_core_macros::Diff as DiffMacro;` re-export at L9 and its doc comment)

- [ ] **Step 1: Remove the re-export** in `ir/eq.rs` (lines 5-9: the doc comment + the `pub use`).
- [ ] **Step 2: Remove the dependency** from `crates/pgevolve-core/Cargo.toml` (the `pgevolve-core-macros = { version = "0.2.1", path = "../pgevolve-core-macros" }` line, ~L21).
- [ ] **Step 3: Remove the workspace member** in root `Cargo.toml` (`"crates/pgevolve-core-macros"`, ~L5) and any `[workspace.dependencies]` reference.
- [ ] **Step 4: Delete the crate directory:** `git rm -r crates/pgevolve-core-macros`.
- [ ] **Step 5: Verify nothing references it.** `grep -rn 'pgevolve_core_macros\|pgevolve-core-macros\|DiffMacro' . --include='*.rs' --include='*.toml'` (excluding `target/`, `docs/`) returns nothing.
- [ ] **Step 6: Gate + commit.** `cargo fmt --check && cargo clippy -p pgevolve-core --all-targets && cargo test -p pgevolve-core && cargo deny check` (the `deny` check confirms the dependency tree shrank cleanly). Commit:
```bash
git add -A
git commit -m "build: delete pgevolve-core-macros crate (no longer used)

Removes a published second crate + its syn/quote/proc-macro2 deps and the
two-crate publish-ordering footgun. The DiffMacro re-export is gone; all
Diff impls are hand-written.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

> **Note:** this removes a crate that was published to crates.io. That is fine — `pgevolve-core-macros` simply stops receiving new versions; the old versions remain on crates.io harmlessly. No yank needed.

---

### Task 3: Rename the equivalence trait so "diff" means only the migration engine

**Goal:** Remove "diff" from the equivalence API. Exact end-state renames (mechanical, compiler-guided):

| Old | New |
|---|---|
| trait `Diff` | trait `Equiv` |
| method `Diff::diff(&self, other)` | `Equiv::differences(&self, other)` |
| `Diff::canonical_eq` | `Equiv::canonical_eq` *(unchanged)* |
| fn `diff_field` | fn `field_difference` |
| fn `prefix_diffs` | fn `prefix_differences` |
| struct `Difference` | *unchanged* (a noun, not confusing with migration `diff`) |
| module `ir::eq` | *unchanged* (already not named "diff") |

Scope (from grep): ~87 `.diff(` trait-method call sites, 32 `canonical_eq` (unchanged but their imports move), ~20 `use crate::ir::eq::Diff` / `eq::Diff` imports across `crates/pgevolve-core/src` AND tests in `crates/pgevolve-core/tests/` (`dump_round_trip.rs`, `parser_corpus.rs`) AND any use in `crates/pgevolve` / `crates/pgevolve-testkit` / `crates/pgevolve-conformance`.

- [ ] **Step 1: Rename in `ir/eq.rs`.** Trait `Diff` → `Equiv`; method `fn diff` → `fn differences` (update the `canonical_eq` default body `self.differences(other).is_empty()`); `fn diff_field` → `fn field_difference`; `fn prefix_diffs` → `fn prefix_differences`. Update the module + item doc comments (the header says "Diff trait" → "Equiv trait"; remove the now-stale DiffMacro paragraph if any survived Task 2). Update the in-file tests.

- [ ] **Step 2: Compiler-guided sweep.** `cargo build -p pgevolve-core` will now error at every reference. Mechanically update:
  - `impl Diff for X` → `impl Equiv for X` (all 9 hand-written + the 13 from Task 1 = 22 impls), and inside each, `fn diff` → `fn differences`, recursive `Diff::diff(...)` → `Equiv::differences(...)`, `diff_field` → `field_difference`, `prefix_diffs` → `prefix_differences`.
  - `use crate::ir::eq::{Diff, ...}` → `{Equiv, ...}` with the renamed helpers; `use crate::ir::eq::Diff;` → `use crate::ir::eq::Equiv;`.
  - `.diff(&other)` trait-method calls → `.differences(&other)`. **CAUTION:** do NOT touch the unrelated migration `diff(...)` free function (in `crate::diff`) — it is called as `diff(target, source, drift)` (free fn, no receiver) and as `diff::` paths. Only receiver-style `x.diff(&y)` where `x: impl Equiv` renames. The compiler disambiguates: if a `.diff(` site doesn't error after the trait rename, it wasn't the trait method — leave it.
  - `canonical_eq` callers: only their `use` imports change (the method name is unchanged).

- [ ] **Step 3: Extend the sweep to the other crates' tests.** Build the workspace: `cargo build --workspace --all-targets`. Fix the same renames in `crates/pgevolve-core/tests/*`, and any references in `crates/pgevolve`, `crates/pgevolve-testkit` (e.g. `assert_canonical_eq`), `crates/pgevolve-conformance`. `grep -rn '\bDiff\b' crates/ --include='*.rs' | grep -v 'difference\|Difference\|diff::\|ChangeSet'` helps find stragglers; reason about each (the migration engine's `diff` module/`ChangeSet` must NOT be renamed).

- [ ] **Step 4: Confirm the disambiguation is complete.** `grep -rn 'ir::eq::Diff\b\|impl Diff for\|DiffMacro' crates/` returns nothing. The only remaining `Diff`-ish identifiers should be the migration `diff` module and `Difference` (intentional).

- [ ] **Step 5: Full gate + commit.** `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`. Commit:
```bash
git add -A
git commit -m "refactor(ir): rename equivalence trait Diff -> Equiv (differences/field_difference)

\"diff\" now unambiguously means the migration change-engine; the
equivalence trait is Equiv with differences()/canonical_eq(). Pure rename,
no behavior change.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Phase wrap — full workspace gate + push

- [ ] **Step 1:** `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps && cargo deny check` — all green.
- [ ] **Step 2:** Confirm artifacts: `grep -rn 'DiffMacro\|pgevolve-core-macros' . --include='*.rs' --include='*.toml' | grep -v target/` → empty; `ls crates/` no longer shows `pgevolve-core-macros`.
- [ ] **Step 3:** `git push origin main`, then wait for CI green across all 5 PG majors before considering Phase 2 done / starting Phase 3.

---

## Self-review notes (for the executor)

- **Spec coverage:** Task 1 = decision 2 (remove macro, hand-write impls); Task 2 = decision 2 (delete crate + wiring); Task 3 = decision 3 (rename). 
- **The load-bearing risk is OUTPUT EQUIVALENCE in Task 1** — the hand-written impls must match the macro's `Vec<Difference>` exactly (paths + via_debug `{:?}` formatting). The attribute→line table is the deterministic spec; the existing per-type `canonical_eq` tests + `dump_round_trip`/`parser_corpus` + conformance are the safety net. Any divergence fails a test.
- **Do not rename the migration `diff`** module / `diff::ChangeSet` / the free `diff(target, source, drift)` fn — only the equivalence trait/method/helpers. The whole point is to make those two no longer share a name.
- **`Difference` and module `ir::eq` keep their names** (neither is confusing with migration "diff"); renaming them is out of scope churn.
- Per-commit gate INCLUDES `cargo fmt --check` (Phase 1 lesson).
