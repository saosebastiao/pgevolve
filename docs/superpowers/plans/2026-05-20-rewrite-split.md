# `plan/rewrite/mod.rs` Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce `crates/pgevolve-core/src/plan/rewrite/mod.rs` from 2694 lines to ~120 lines by moving per-object-family `Change` dispatcher logic into a new `emit/` submodule (one file per family) and moving the ~1294-line test block to a sibling `tests.rs`.

**Architecture:** New `plan/rewrite/emit/` directory holds one file per object family (`schema.rs`, `table.rs`, `sequence.rs`, `index.rs`, `constraint.rs`, `view.rs`, `mv.rs`, `user_type.rs`, `function.rs`, `procedure.rs`, `deferred_fk.rs`). `mod.rs` keeps the public `rewrite` / `rewrite_with_source` API, the `Ctx` struct, the `emit_change` dispatcher (now one line per arm), and the small `destructive_reason` / `schema_target` helpers. Existing sibling files (`sql.rs`, `functions.rs`, `views.rs`, `types.rs` for SQL emission; `concurrent_index.rs`, `refresh_mv_concurrently.rs`, etc. for post-emit rewrites) are unchanged.

**Tech Stack:** Pure code motion within `pgevolve-core`. No new dependencies, no public API change.

**Reference design:** `docs/superpowers/specs/2026-05-20-rewrite-split-design.md`.

---

## File structure (final state)

```
plan/rewrite/
  mod.rs              # public API + Ctx + emit_change dispatcher + tiny helpers (~120 lines)
  tests.rs            # the 40 existing end-to-end tests (~1294 lines, moved verbatim)
  emit/
    mod.rs            # pub(super) mod {schema, table, sequence, index, constraint, view, mv, user_type, function, procedure, deferred_fk};
    schema.rs         # CreateSchema, DropSchema, AlterSchema arms
    table.rs          # CreateTable, DropTable, AlterTable arms + emit_table_op
    sequence.rs       # CreateSequence, DropSequence, AlterSequence arms + emit_sequence_op
    index.rs          # CreateIndex, DropIndex, ReplaceIndex, RecreateIndex arms
    constraint.rs     # ValidateConstraint arm (drift recovery)
    view.rs           # emit_view_change body
    mv.rs             # emit_mv_change body
    user_type.rs      # emit_user_type_change body
    function.rs       # emit_function_change body
    procedure.rs      # emit_procedure_change body
    deferred_fk.rs    # emit_deferred_fk body
```

**Unchanged neighbors (do NOT modify):**
- SQL helpers: `sql.rs`, `functions.rs`, `views.rs`, `types.rs`
- Post-emit rewrites: `check_not_valid_validate.rs`, `concurrent_index.rs`, `fk_not_valid_validate.rs`, `refresh_mv_concurrently.rs`, `set_not_null_check_pattern.rs`

---

## Mapping (line ranges in pre-migration `mod.rs`)

| Family | Source lines | Destination |
|---|---|---|
| Schema arms | 105–148 (3 arms) | `emit/schema.rs::{create, drop_, alter}` |
| CreateTable arm | 150–202 | `emit/table.rs::create` |
| DropTable arm | 203–212 | `emit/table.rs::drop_` |
| AlterTable arm | 213–217 | `emit/table.rs::alter` (inline loop calls `op`) |
| emit_table_op | 975–1151 | `emit/table.rs::op` |
| CreateIndex arm | 219–252 | `emit/index.rs::create` |
| DropIndex arm | 253–269 | `emit/index.rs::drop_` |
| ReplaceIndex arm | 270–306 | `emit/index.rs::replace` |
| CreateSequence arm | 308–336 | `emit/sequence.rs::create` |
| DropSequence arm | 337–346 | `emit/sequence.rs::drop_` |
| AlterSequence arm | 347–351 | `emit/sequence.rs::alter` (inline loop calls `op`) |
| emit_sequence_op | 1152–1175 | `emit/sequence.rs::op` |
| ValidateConstraint arm | 354–365 | `emit/constraint.rs::validate` |
| RecreateIndex arm | 366–407 | `emit/index.rs::recreate` |
| View arm + emit_view_change | 409, 419–573 | `emit/view.rs::emit` |
| Mv arm + emit_mv_change | 410, 574–716 | `emit/mv.rs::emit` |
| UserType arm + emit_user_type_change | 411–413, 717–974 | `emit/user_type.rs::emit` |
| Function arm + emit_function_change | 414, 1197–1303 | `emit/function.rs::emit` |
| Procedure arm + emit_procedure_change | 415, 1304–1391 | `emit/procedure.rs::emit` |
| emit_deferred_fk | 1176–1192 | `emit/deferred_fk.rs::emit` |
| `destructive_reason`, `schema_target` | 1392–1399, 1193–1196 | stay in `mod.rs` (widened to `pub(super)`) |
| `Ctx<'a>` | 41–45 | stays in `mod.rs` (widened to `pub(super)`) |
| Tests (`#[cfg(test)] mod tests`) | 1400–2694 | `tests.rs` |

Note: `drop` is a Rust keyword; functions named "drop" use the `drop_` convention.

---

## Task 1: Scaffold `emit/` with empty modules

**Files:**
- Create: `crates/pgevolve-core/src/plan/rewrite/emit/mod.rs`
- Create: empty stubs `crates/pgevolve-core/src/plan/rewrite/emit/{schema,table,sequence,index,constraint,view,mv,user_type,function,procedure,deferred_fk}.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Create `emit/mod.rs` with module declarations**

Create `crates/pgevolve-core/src/plan/rewrite/emit/mod.rs`:

```rust
//! Per-object-family dispatchers for the rewrite pass.
//!
//! Each file in this module handles the `Change::*` variants for one
//! family. The top-level [`super::emit_change`] dispatcher routes each
//! variant to a `pub(super) fn` here. Future v0.2 sub-specs (extensions,
//! triggers, partitioning) add new files in this directory.

pub(super) mod constraint;
pub(super) mod deferred_fk;
pub(super) mod function;
pub(super) mod index;
pub(super) mod mv;
pub(super) mod procedure;
pub(super) mod schema;
pub(super) mod sequence;
pub(super) mod table;
pub(super) mod user_type;
pub(super) mod view;
```

- [ ] **Step 2: Create empty per-family stub files**

For each of the 11 family modules, create a stub file with a placeholder doc comment. Example for `crates/pgevolve-core/src/plan/rewrite/emit/schema.rs`:

```rust
//! Dispatchers for `Change::CreateSchema`, `Change::DropSchema`,
//! `Change::AlterSchema`. Bodies move from `mod.rs` in Task 2.
```

Repeat for: `table.rs`, `sequence.rs`, `index.rs`, `constraint.rs`, `view.rs`, `mv.rs`, `user_type.rs`, `function.rs`, `procedure.rs`, `deferred_fk.rs`. Each is a 2-line file.

- [ ] **Step 3: Widen `Ctx`, `destructive_reason`, `schema_target` visibility and register `emit` module**

In `crates/pgevolve-core/src/plan/rewrite/mod.rs`:

(a) Add `pub mod emit;` near the other `pub mod` declarations (line ~18, alphabetical order):

```rust
pub mod check_not_valid_validate;
pub mod concurrent_index;
pub mod emit;
pub mod fk_not_valid_validate;
pub mod functions;
pub mod refresh_mv_concurrently;
pub mod set_not_null_check_pattern;
pub mod sql;
pub mod types;
pub mod views;
```

(b) Change `struct Ctx<'a>` (line 41) to `pub(super) struct Ctx<'a>` and make every field `pub(super)`:

```rust
/// Context passed to every emitter — read-only.
pub(super) struct Ctx<'a> {
    pub(super) target: &'a Catalog,
    pub(super) source: &'a Catalog,
    pub(super) policy: &'a PlannerPolicy,
}
```

(c) Change `fn destructive_reason` (line 1392) and `fn schema_target` (line 1193) to `pub(super) fn`:

```rust
pub(super) fn schema_target(name: &crate::identifier::Identifier) -> QualifiedName {
    QualifiedName::new(name.clone(), name.clone())  // existing body unchanged
}

// ... (and at line 1392)
pub(super) fn destructive_reason(d: &Destructiveness) -> Option<String> {
    // existing body unchanged
}
```

Copy the existing bodies verbatim; only the visibility changes.

- [ ] **Step 4: Build, test, clippy**

Run:
```
cargo build -p pgevolve-core
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green. The new module is empty so no behavior change.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/
git commit -m "$(cat <<'EOF'
refactor(rewrite): scaffold emit/ submodule (no-op)

Empty per-family stub files and a mod.rs that wires them. `Ctx`,
`destructive_reason`, and `schema_target` widened to `pub(super)` so
the upcoming per-family dispatchers can reach them. No behavior change.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Move schema dispatchers

**Files:**
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/schema.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Populate `emit/schema.rs`**

Read lines 105–148 of `crates/pgevolve-core/src/plan/rewrite/mod.rs` (the three `Change::*Schema*` arms). Each arm becomes a `pub(super) fn` in `emit/schema.rs`. Replace the file with:

```rust
//! Dispatchers for `Change::CreateSchema`, `Change::DropSchema`,
//! `Change::AlterSchema`.

use crate::identifier::Identifier;
use crate::ir::schema::Schema;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::sql;

use super::super::{destructive_reason, schema_target};

pub(super) fn create(
    s: Schema,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::CreateSchema,
        destructive,
        destructive_reason: destructive_reason_text,
        intent_id: None,
        targets: vec![schema_target(&s.name)],
        sql: sql::create_schema(&s),
        transactional: TransactionConstraint::InTransaction,
    });
    if let Some(c) = &s.comment {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterSchemaComment,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![schema_target(&s.name)],
            sql: sql::comment_on_schema(&s.name, Some(c)),
            transactional: TransactionConstraint::InTransaction,
        });
    }
}

pub(super) fn drop_(
    name: Identifier,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::DropSchema,
        destructive,
        destructive_reason: destructive_reason_text,
        intent_id: None,
        targets: vec![schema_target(&name)],
        sql: sql::drop_schema(&name),
        transactional: TransactionConstraint::InTransaction,
    });
}

pub(super) fn alter(
    name: Identifier,
    comment: Option<String>,
    out: &mut Vec<RawStep>,
) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::AlterSchemaComment,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![schema_target(&name)],
        sql: sql::comment_on_schema(&name, comment.as_deref()),
        transactional: TransactionConstraint::InTransaction,
    });
}
```

The `destructive_reason` import line is included for completeness even though only the `_text` parameter is used directly — keep it consistent with future per-family files that may need it.

If the existing arm bodies in `mod.rs` differ in any detail (extra match guards, additional steps), preserve those details verbatim.

- [ ] **Step 2: Replace the schema arms in `emit_change` with one-line dispatches**

In `crates/pgevolve-core/src/plan/rewrite/mod.rs`, replace lines 105–148 (the three schema arms) with:

```rust
        Change::CreateSchema(s) => emit::schema::create(s, destructive, destructive_reason, out),
        Change::DropSchema(name) => emit::schema::drop_(name, destructive, destructive_reason, out),
        Change::AlterSchema { name, comment } => emit::schema::alter(name, comment, out),
```

- [ ] **Step 3: Build, test, clippy**

Run:
```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/emit/schema.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
refactor(rewrite): move schema dispatchers into emit/schema.rs

CreateSchema, DropSchema, AlterSchema arms move out of emit_change
into per-variant functions. Pure code motion.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Move sequence dispatchers

**Files:**
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/sequence.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Populate `emit/sequence.rs`**

Read lines 308–351 (the three `Change::*Sequence*` arms) and lines 1152–1175 (`emit_sequence_op`) of `mod.rs`. Replace the contents of `crates/pgevolve-core/src/plan/rewrite/emit/sequence.rs` with:

```rust
//! Dispatchers for `Change::CreateSequence`, `Change::DropSequence`,
//! `Change::AlterSequence`, plus the per-`SequenceOp` emitter.

use crate::diff::sequence_op::{SequenceOp, SequenceOpEntry};
use crate::identifier::QualifiedName;
use crate::ir::sequence::Sequence;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::sql;

pub(super) fn create(
    s: Sequence,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from the existing CreateSequence arm
    // (lines 308–336 of mod.rs pre-migration).
    // ... preserve every step the existing arm emits ...
}

pub(super) fn drop_(
    qname: QualifiedName,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from the existing DropSequence arm
    // (lines 337–346 of mod.rs pre-migration).
    // ... preserve every step the existing arm emits ...
}

pub(super) fn alter(
    qname: QualifiedName,
    ops: Vec<SequenceOpEntry>,
    out: &mut Vec<RawStep>,
) {
    for op in ops {
        op_(&qname, op, out);
    }
}

pub(super) fn op_(qname: &QualifiedName, entry: SequenceOpEntry, out: &mut Vec<RawStep>) {
    // Body copied verbatim from emit_sequence_op
    // (lines 1152–1175 of mod.rs pre-migration).
}
```

**Important:** the placeholder "Body copied verbatim from..." comments above are NOT acceptable in the final file. Read the actual line ranges from the current `mod.rs` and paste the exact code into each function body. The signatures shown match the existing arms' destructuring patterns and the existing `emit_sequence_op` signature. If the existing CreateSequence arm uses `&s` references that need conversion to `s.clone()` (because the new function takes owned `Sequence`), adjust accordingly — most likely the arm already takes ownership via destructuring.

The function name `op_` (with trailing underscore) avoids conflict with the `op` parameter in `alter`; alternatively name it `emit_op` for clarity. Pick one convention and use it consistently across all `emit/*.rs` files.

- [ ] **Step 2: Replace the sequence arms in `emit_change` and remove `emit_sequence_op`**

In `mod.rs`:

(a) Replace lines 308–351 (the three sequence arms) with:

```rust
        Change::CreateSequence(s) => emit::sequence::create(s, destructive, destructive_reason, out),
        Change::DropSequence(qname) => emit::sequence::drop_(qname, destructive, destructive_reason, out),
        Change::AlterSequence { qname, ops } => emit::sequence::alter(qname, ops, out),
```

(b) Delete the standalone `fn emit_sequence_op` (lines 1152–1175).

- [ ] **Step 3: Build, test, clippy**

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/emit/sequence.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
refactor(rewrite): move sequence dispatchers + emit_sequence_op into emit/sequence.rs

Pure code motion.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Move deferred-FK dispatcher

**Files:**
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/deferred_fk.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Populate `emit/deferred_fk.rs`**

Read lines 1176–1192 of `mod.rs` (`fn emit_deferred_fk`). Replace `crates/pgevolve-core/src/plan/rewrite/emit/deferred_fk.rs`:

```rust
//! Dispatcher for deferred FK additions emitted at the end of the plan.

use crate::plan::ordered::DeferredFkAdd;
use crate::plan::raw_step::RawStep;

pub(super) fn emit(fk: &DeferredFkAdd, ctx: &super::super::Ctx<'_>, out: &mut Vec<RawStep>) {
    // Body copied verbatim from the existing `fn emit_deferred_fk` at
    // lines 1176–1192 of mod.rs pre-migration. The `_ctx` parameter
    // there was unused; if it remains unused after the move, prefix
    // with `_` or drop the parameter entirely. Keeping it preserves the
    // signature in case future work needs ctx.
}
```

Read the actual function body from `mod.rs` and paste it in place of the comment. If the original `_ctx: &Ctx<'_>` was unused (the leading underscore confirms it), keep the parameter with the underscore to preserve the call shape from `rewrite_with_source`.

- [ ] **Step 2: Update `rewrite_with_source` to call the new function and remove the old one**

In `mod.rs`:

(a) Replace `emit_deferred_fk(&fk, &ctx, &mut out);` at line ~88 with:

```rust
        emit::deferred_fk::emit(&fk, &ctx, &mut out);
```

(b) Delete the standalone `fn emit_deferred_fk` (lines 1176–1192).

- [ ] **Step 3: Build, test, clippy**

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/emit/deferred_fk.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
refactor(rewrite): move emit_deferred_fk into emit/deferred_fk.rs

Pure code motion.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Move index dispatchers

**Files:**
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/index.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Populate `emit/index.rs`**

Read these arms from `mod.rs`:
- `Change::CreateIndex(idx)` arm (lines 219–252)
- `Change::DropIndex(qname)` arm (lines 253–269)
- `Change::ReplaceIndex { from, to }` arm (lines 270–306)
- `Change::RecreateIndex { qname }` arm (lines 366–407)

Replace `crates/pgevolve-core/src/plan/rewrite/emit/index.rs`:

```rust
//! Dispatchers for index changes: Create, Drop, Replace, Recreate.
//!
//! `Replace` and `Recreate` cover online-rewrite paths (gated on
//! `PlannerPolicy::create_index_concurrent`) and drift recovery for
//! INVALID indexes, respectively.

use crate::identifier::QualifiedName;
use crate::ir::index::Index;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::{concurrent_index, sql};

pub(super) fn create(
    idx: Index,
    ctx: &super::super::Ctx<'_>,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from the existing CreateIndex arm.
    // Preserves the policy check for concurrent_index.
}

pub(super) fn drop_(
    qname: QualifiedName,
    ctx: &super::super::Ctx<'_>,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from the existing DropIndex arm.
}

pub(super) fn replace(
    from: Index,
    to: Index,
    ctx: &super::super::Ctx<'_>,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from the existing ReplaceIndex arm.
}

pub(super) fn recreate(
    qname: QualifiedName,
    ctx: &super::super::Ctx<'_>,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from the existing RecreateIndex arm.
}
```

**Read the actual line ranges from `mod.rs` and paste the bodies in.** The `ctx` parameter is required because the existing arms call into `concurrent_index::should_rewrite_create(&idx, ctx.target, ctx.policy)` etc., and `RecreateIndex` consults `ctx.source` for the index definition.

If any arm has no use for `ctx`, drop the parameter from that function's signature. Inspect each arm's body to decide.

- [ ] **Step 2: Replace the four index arms in `emit_change`**

In `mod.rs`:

(a) Replace lines 219–306 (Create/Drop/Replace) with:

```rust
        Change::CreateIndex(idx) => emit::index::create(idx, ctx, destructive, destructive_reason, out),
        Change::DropIndex(qname) => emit::index::drop_(qname, ctx, destructive, destructive_reason, out),
        Change::ReplaceIndex { from, to } => emit::index::replace(from, to, ctx, destructive, destructive_reason, out),
```

(b) Replace lines 366–407 (RecreateIndex) with:

```rust
        Change::RecreateIndex { qname } => emit::index::recreate(qname, ctx, destructive, destructive_reason, out),
```

Drop `ctx` from any per-variant call where the function signature dropped it.

- [ ] **Step 3: Build, test, clippy**

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green. The `concurrent_index` import in `mod.rs` may become unused; remove if so.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/emit/index.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
refactor(rewrite): move index dispatchers into emit/index.rs

CreateIndex, DropIndex, ReplaceIndex, RecreateIndex arms move out of
emit_change. Pure code motion; policy-gated concurrent index path
preserved.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Move constraint dispatcher

**Files:**
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/constraint.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Populate `emit/constraint.rs`**

Read lines 354–365 of `mod.rs` (the `Change::ValidateConstraint { table, constraint }` arm). Replace `crates/pgevolve-core/src/plan/rewrite/emit/constraint.rs`:

```rust
//! Dispatcher for drift-recovery constraint changes.
//!
//! Today this module handles `Change::ValidateConstraint` only. Other
//! constraint operations are expressed as `TableOp`s and handled by
//! `emit/table.rs`.

use crate::identifier::QualifiedName;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::sql;

pub(super) fn validate(
    table: QualifiedName,
    constraint: QualifiedName,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from the existing ValidateConstraint arm
    // at lines 354–365 of mod.rs pre-migration.
}
```

Paste the actual body.

- [ ] **Step 2: Replace the arm in `emit_change`**

In `mod.rs`, replace lines 354–365 with:

```rust
        Change::ValidateConstraint { table, constraint } => {
            emit::constraint::validate(table, constraint, destructive, destructive_reason, out);
        }
```

- [ ] **Step 3: Build, test, clippy**

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/emit/constraint.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
refactor(rewrite): move ValidateConstraint dispatcher into emit/constraint.rs

Pure code motion.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Move view dispatcher (`emit_view_change`)

**Files:**
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/view.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Populate `emit/view.rs`**

Read the entire `fn emit_view_change` body (lines 419–573) from `mod.rs`. Replace `crates/pgevolve-core/src/plan/rewrite/emit/view.rs`:

```rust
//! Dispatcher for `Change::View(ViewChange)`.

use crate::diff::change::ViewChange;
use crate::plan::raw_step::RawStep;

pub(super) fn emit(
    vc: ViewChange,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from `fn emit_view_change` at lines 419–573
    // of mod.rs pre-migration.
}
```

Paste the actual body. The existing signature is `fn emit_view_change(vc: ViewChange, destructive: bool, destructive_reason: Option<String>, out: &mut Vec<RawStep>)` — no `ctx` parameter, per the call site at line 409. Match imports to whatever the existing body references (likely `crate::plan::rewrite::{sql, views}` and various IR types).

- [ ] **Step 2: Replace the arm and delete the standalone function**

In `mod.rs`:

(a) Replace line 409 with:

```rust
        Change::View(vc) => emit::view::emit(vc, destructive, destructive_reason, out),
```

(b) Delete `fn emit_view_change` (lines 419–573).

- [ ] **Step 3: Build, test, clippy**

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/emit/view.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
refactor(rewrite): move emit_view_change into emit/view.rs

Pure code motion.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Move MV dispatcher (`emit_mv_change`)

**Files:**
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/mv.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Populate `emit/mv.rs`**

Read the entire `fn emit_mv_change` body (lines 574–716) from `mod.rs`. Replace `crates/pgevolve-core/src/plan/rewrite/emit/mv.rs`:

```rust
//! Dispatcher for `Change::Mv(MvChange)`.

use crate::diff::change::MvChange;
use crate::plan::raw_step::RawStep;

pub(super) fn emit(
    mc: MvChange,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from `fn emit_mv_change` at lines 574–716
    // of mod.rs pre-migration.
}
```

Paste the actual body. Match imports.

- [ ] **Step 2: Replace the arm and delete the standalone function**

In `mod.rs`:

(a) Replace line 410 with:

```rust
        Change::Mv(mc) => emit::mv::emit(mc, destructive, destructive_reason, out),
```

(b) Delete `fn emit_mv_change` (lines 574–716).

- [ ] **Step 3: Build, test, clippy**

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/emit/mv.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
refactor(rewrite): move emit_mv_change into emit/mv.rs

Pure code motion.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Move user-type dispatcher (`emit_user_type_change`)

**Files:**
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/user_type.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Populate `emit/user_type.rs`**

Read `fn emit_user_type_change` (lines 717–974) of `mod.rs`. Replace `crates/pgevolve-core/src/plan/rewrite/emit/user_type.rs`:

```rust
//! Dispatcher for `Change::UserType(UserTypeChange)`.

use crate::diff::change::UserTypeChange;
use crate::plan::raw_step::RawStep;

#[allow(clippy::too_many_lines)]
pub(super) fn emit(
    utc: UserTypeChange,
    destructive: bool,
    destructive_reason_text: Option<String>,
    ctx: &super::super::Ctx<'_>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from `fn emit_user_type_change` at lines
    // 717–974 of mod.rs pre-migration.
}
```

Paste the actual body. The existing signature includes `ctx: &Ctx<'_>` per the call site at line 412. The `#[allow(clippy::too_many_lines)]` is required because the function exceeds 100 lines; verify whether the existing function had a similar allow at the function level or relied on the module-level one.

- [ ] **Step 2: Replace the arm and delete the standalone function**

In `mod.rs`:

(a) Replace lines 411–413 with:

```rust
        Change::UserType(utc) => emit::user_type::emit(utc, destructive, destructive_reason, ctx, out),
```

(b) Delete `fn emit_user_type_change` (lines 717–974).

- [ ] **Step 3: Build, test, clippy**

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/emit/user_type.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
refactor(rewrite): move emit_user_type_change into emit/user_type.rs

Pure code motion.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Move function dispatcher (`emit_function_change`)

**Files:**
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/function.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Populate `emit/function.rs`**

Read `fn emit_function_change` (lines 1197–1303) of `mod.rs`. Replace `crates/pgevolve-core/src/plan/rewrite/emit/function.rs`:

```rust
//! Dispatcher for `Change::Function(FunctionChange)`.

use crate::diff::change::FunctionChange;
use crate::plan::raw_step::RawStep;

pub(super) fn emit(
    fc: FunctionChange,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from `fn emit_function_change` at lines
    // 1197–1303 of mod.rs pre-migration.
}
```

Paste the actual body. Match imports.

- [ ] **Step 2: Replace the arm and delete the standalone function**

In `mod.rs`:

(a) Replace line 414 with:

```rust
        Change::Function(fc) => emit::function::emit(fc, destructive, destructive_reason, out),
```

(b) Delete `fn emit_function_change` (lines 1197–1303).

- [ ] **Step 3: Build, test, clippy**

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/emit/function.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
refactor(rewrite): move emit_function_change into emit/function.rs

Pure code motion.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Move procedure dispatcher (`emit_procedure_change`)

**Files:**
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/procedure.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Populate `emit/procedure.rs`**

Read `fn emit_procedure_change` (lines 1304–1391) of `mod.rs`. Replace `crates/pgevolve-core/src/plan/rewrite/emit/procedure.rs`:

```rust
//! Dispatcher for `Change::Procedure(ProcedureChange)`.

use crate::diff::change::ProcedureChange;
use crate::plan::raw_step::RawStep;

pub(super) fn emit(
    pc: ProcedureChange,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from `fn emit_procedure_change` at lines
    // 1304–1391 of mod.rs pre-migration.
}
```

Paste the actual body. Match imports.

- [ ] **Step 2: Replace the arm and delete the standalone function**

In `mod.rs`:

(a) Replace line 415 with:

```rust
        Change::Procedure(pc) => emit::procedure::emit(pc, destructive, destructive_reason, out),
```

(b) Delete `fn emit_procedure_change` (lines 1304–1391).

- [ ] **Step 3: Build, test, clippy**

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/emit/procedure.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
refactor(rewrite): move emit_procedure_change into emit/procedure.rs

Pure code motion.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Move table dispatcher (`emit_table_op` + Create/Drop/AlterTable arms)

This is the largest single move (~320 lines combined). Done intentionally last so simpler families have validated the pattern.

**Files:**
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/table.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Populate `emit/table.rs`**

Read these regions of `mod.rs`:
- `Change::CreateTable(t)` arm (lines 150–202)
- `Change::DropTable { qname, .. }` arm (lines 203–212)
- `Change::AlterTable { qname, ops }` arm (lines 213–217)
- `fn emit_table_op` (lines 975–1151)

Replace `crates/pgevolve-core/src/plan/rewrite/emit/table.rs`:

```rust
//! Dispatchers for table changes: `CreateTable`, `DropTable`,
//! `AlterTable`, plus the per-`TableOp` emitter.

use crate::diff::table_op::{TableOp, TableOpEntry};
use crate::identifier::QualifiedName;
use crate::ir::table::Table;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::sql;

pub(super) fn create(
    t: Table,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from the CreateTable arm.
}

pub(super) fn drop_(
    qname: QualifiedName,
    destructive: bool,
    destructive_reason_text: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from the DropTable arm.
}

pub(super) fn alter(
    qname: QualifiedName,
    ops: Vec<TableOpEntry>,
    ctx: &super::super::Ctx<'_>,
    out: &mut Vec<RawStep>,
) {
    for op_entry in ops {
        op(&qname, op_entry, ctx, out);
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn op(
    qname: &QualifiedName,
    entry: TableOpEntry,
    ctx: &super::super::Ctx<'_>,
    out: &mut Vec<RawStep>,
) {
    // Body copied verbatim from `fn emit_table_op` at lines 975–1151.
}
```

Paste the actual bodies. The `ctx` parameter is required for `op` because `emit_table_op`'s body consults `ctx.target` / `ctx.policy` for online-rewrite gating. If the original `DropTable` arm had a `{ qname, .. }` destructure pattern with discarded fields, the new `drop_` function only needs `qname` — drop the unused fields.

If the `CreateTable` arm body iterates `t.columns` and `t.constraints` for nested comment steps, preserve that loop verbatim.

- [ ] **Step 2: Replace the table arms and delete `emit_table_op`**

In `mod.rs`:

(a) Replace lines 150–217 with:

```rust
        Change::CreateTable(t) => emit::table::create(t, destructive, destructive_reason, out),
        Change::DropTable { qname, .. } => emit::table::drop_(qname, destructive, destructive_reason, out),
        Change::AlterTable { qname, ops } => emit::table::alter(qname, ops, ctx, out),
```

(b) Delete `fn emit_table_op` (lines 975–1151).

- [ ] **Step 3: Build, test, clippy**

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/emit/table.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
refactor(rewrite): move table dispatchers + emit_table_op into emit/table.rs

CreateTable, DropTable, AlterTable arms move out of emit_change, and
the per-TableOp emitter (~177 lines) moves with them. Pure code
motion.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Move tests to `tests.rs`

**Files:**
- Create: `crates/pgevolve-core/src/plan/rewrite/tests.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Move the test module body into `tests.rs`**

In `crates/pgevolve-core/src/plan/rewrite/mod.rs`, locate the `#[cfg(test)] mod tests { ... }` block (starts at the line that was 1400 in the pre-migration file; the line number will have shifted considerably by now — grep `#\[cfg(test)\]` to find it).

Copy the entire block's body (everything between `mod tests {` and the matching `}`) into a new file `crates/pgevolve-core/src/plan/rewrite/tests.rs`. The file's top should be:

```rust
//! End-to-end tests for the rewrite pass. Exercises the public
//! `rewrite()` / `rewrite_with_source()` entry points via the
//! `OrderedChangeSet → Vec<RawStep>` contract.
//!
//! Tests are end-to-end across all object families rather than
//! per-family, so they live as a single block here rather than under
//! `emit/<family>.rs::tests`.

use super::*;
// ... rest of imports copied from inside the previous `mod tests` block ...
// ... rest of test bodies ...
```

The existing tests use `use super::*;` (probably already at the top of the moved block). After the move into `tests.rs`, `super` refers to `plan::rewrite`, exactly the scope the tests need.

- [ ] **Step 2: Replace the inline test module in `mod.rs` with a file declaration**

In `mod.rs`, delete the entire `#[cfg(test)] mod tests { ... }` block (the body has moved into `tests.rs`). In its place add:

```rust
#[cfg(test)]
mod tests;
```

This tells Rust to look for `tests.rs` as a sibling.

- [ ] **Step 3: Build, test, clippy**

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green, all 40 tests in `plan::rewrite::tests` still pass.

If a test in the moved block uses `use super::super::*;` patterns, those resolve identically because file-based modules have the same `super` semantics as inline modules.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/tests.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
refactor(rewrite): move test module into sibling tests.rs

The 40 end-to-end rewrite tests (~1294 lines) move out of mod.rs
into tests.rs. mod.rs shrinks to the API + Ctx + dispatcher +
helpers. Pure code motion.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Workspace verification

**Files:** none modified — verification only.

- [ ] **Step 1: Sanity-check `mod.rs` size**

Run: `wc -l crates/pgevolve-core/src/plan/rewrite/mod.rs`
Expected: around 120 lines (well under 200). If it's still > 200, an earlier task left code that should have moved — investigate before declaring done.

- [ ] **Step 2: Verify `emit/` directory contents**

Run: `ls crates/pgevolve-core/src/plan/rewrite/emit/`
Expected: 12 files — `mod.rs`, `schema.rs`, `table.rs`, `sequence.rs`, `index.rs`, `constraint.rs`, `view.rs`, `mv.rs`, `user_type.rs`, `function.rs`, `procedure.rs`, `deferred_fk.rs`.

- [ ] **Step 3: Verify the dispatcher is now one line per arm**

Run: `grep -nc "=> emit::" crates/pgevolve-core/src/plan/rewrite/mod.rs`
Expected: matches the number of `Change::*` variants (~15–17).

- [ ] **Step 4: Full workspace test suite**

Run: `cargo test --workspace --lib --tests`
Expected: all green.

- [ ] **Step 5: Workspace clippy with `-D warnings`**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Format check**

Run: `cargo fmt --check`
Expected: no output.

- [ ] **Step 7: Conformance suite**

Run: `cargo test -p pgevolve-conformance --test run`
Expected: PASS.

- [ ] **Step 8: Conformance across all PG majors**

Run:
```
for v in 14 15 16 17; do
  echo "=== PG $v ==="
  PGEVOLVE_TEST_PG_VERSION=$v cargo test -p pgevolve-conformance --test run 2>&1 | tail -3
done
```
Expected: all 4 versions PASS.

- [ ] **Step 9: If any of Steps 1–8 produced fmt/clippy fixups, commit them**

If `cargo fmt` produced changes, commit:

```bash
git add -A
git commit -m "$(cat <<'EOF'
chore(rewrite): post-split fmt fixups

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

If no changes, skip the commit.

---

## Self-review pre-flight checklist for the implementing agent

Before declaring the plan complete:

- [ ] `crates/pgevolve-core/src/plan/rewrite/mod.rs` is around 120 lines (it should fit in one screen).
- [ ] `emit/` has 12 files: `mod.rs`, 11 family files.
- [ ] `tests.rs` exists; `mod.rs` no longer has an inline `mod tests` block.
- [ ] `grep -n "^fn emit_" crates/pgevolve-core/src/plan/rewrite/mod.rs` returns no matches (every standalone emit helper moved out).
- [ ] All 40 existing rewrite tests still pass.
- [ ] Conformance suite passes on PG 14, 15, 16, 17.
- [ ] Clippy clean with `-D warnings`. Fmt clean.

---

## Out-of-scope (do NOT touch)

- The SQL-emission helpers `sql.rs`, `functions.rs`, `views.rs`, `types.rs` — they are unchanged neighbors of `emit/`.
- The post-emit rewrite passes `concurrent_index.rs`, `refresh_mv_concurrently.rs`, `check_not_valid_validate.rs`, `fk_not_valid_validate.rs`, `set_not_null_check_pattern.rs`.
- Any public API. `rewrite`, `rewrite_with_source`, and the existing `pub mod` re-exports are byte-identical.
- The `Change` enum or any of its variants. Same names, same shapes.
- Splitting `tests.rs` per family. Out of scope for this refactor.
