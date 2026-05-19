# Canonicalization Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move every "filter PG defaults to None" rule out of the catalog reader and source-side parser builders into a single ordered pipeline under `crates/pgevolve-core/src/ir/canon/`, with `Catalog::canonicalize` becoming a thin wrapper. Pure refactor: zero behavior change.

**Architecture:** A new module `ir/canon/` exposes `pub fn canonicalize(&mut Catalog) -> Result<(), IrError>` that runs four named passes in fixed order — `filter_pg_defaults`, `sentinel_view_columns`, `renumber_enum_sort_orders`, `sort_and_dedupe`. Each pass lives in its own file with its own unit tests. The existing `Catalog::canonicalize` keeps its public signature and delegates.

**Tech Stack:** Rust 2024 edition; no new dependencies.

**Reference design:** `docs/superpowers/specs/2026-05-19-canon-consolidation-design.md`.

---

## File Structure

**Created:**
- `crates/pgevolve-core/src/ir/canon/mod.rs` — pipeline entry point.
- `crates/pgevolve-core/src/ir/canon/sort_and_dedupe.rs` — sort + duplicate detection.
- `crates/pgevolve-core/src/ir/canon/sentinel_view_columns.rs` — view/MV column-type sentinel.
- `crates/pgevolve-core/src/ir/canon/renumber_enum_sort_orders.rs` — enum sort_order renumbering.
- `crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs` — default-elision rules.

**Modified:**
- `crates/pgevolve-core/src/ir/mod.rs` — add `pub mod canon;`.
- `crates/pgevolve-core/src/ir/catalog.rs` — body of `Catalog::canonicalize` shrinks to one call.
- `crates/pgevolve-core/src/ir/sequence.rs` — delete `Sequence::canonicalize` and the `default_bounds` helper.
- `crates/pgevolve-core/src/catalog/assemble.rs` — three default-elision filters removed (sequence min/max, function cost/rows, column collation `pg_catalog.default` → None).
- `crates/pgevolve-core/src/parse/builder/create_function_stmt.rs` — cost/rows default filters removed.

---

## Task 1: Scaffold `ir/canon/` as a no-op pipeline

**Files:**
- Create: `crates/pgevolve-core/src/ir/canon/mod.rs`
- Modify: `crates/pgevolve-core/src/ir/mod.rs`
- Modify: `crates/pgevolve-core/src/ir/catalog.rs`

- [ ] **Step 1: Create the empty pipeline module**

Create `crates/pgevolve-core/src/ir/canon/mod.rs`:

```rust
//! `Catalog` canonicalization pipeline.
//!
//! Every IR-value normalization rule that must apply to both the
//! source-built `Catalog` and the catalog-reader-built `Catalog` lives
//! here, behind a single entry point. The pipeline runs in a fixed
//! documented order; new rules go into the appropriate file in this
//! module (or get a new file if they're a new kind of rule).
//!
//! Today's order:
//!
//! 1. [`filter_pg_defaults`] — values that equal PG's documented
//!    defaults become `None` (sequence min/max, function cost/rows,
//!    column collation `pg_catalog.default`).
//! 2. [`sentinel_view_columns`] — view/MV column types collapse to the
//!    `view_column` sentinel.
//! 3. [`renumber_enum_sort_orders`] — every enum's `sort_order` values
//!    are re-indexed to `1.0, 2.0, 3.0, …` in current order.
//! 4. [`sort_and_dedupe`] — every collection is sorted by its canonical
//!    key and duplicates raise [`IrError`]. Runs last so duplicate
//!    detection sees post-normalization values.
//!
//! See `docs/superpowers/specs/2026-05-19-canon-consolidation-design.md`.

use crate::ir::IrError;
use crate::ir::catalog::Catalog;

/// Run every canonicalization pass on `cat` in order.
///
/// Only [`sort_and_dedupe`] is fallible; the other passes mutate in
/// place and cannot fail.
pub fn canonicalize(_cat: &mut Catalog) -> Result<(), IrError> {
    // Passes land in subsequent commits — Task 2 onward.
    Ok(())
}
```

- [ ] **Step 2: Register the module**

Edit `crates/pgevolve-core/src/ir/mod.rs`. Add `pub mod canon;` in alphabetical position (between `catalog` and `column` looks like the right slot — confirm by reading the existing list and inserting where it fits the file's existing convention).

The final block should contain a line:

```rust
pub mod canon;
```

- [ ] **Step 3: Make `Catalog::canonicalize` call the new pipeline (still a no-op for now)**

In `crates/pgevolve-core/src/ir/catalog.rs`, at the **end** of `Catalog::canonicalize` (just before the existing `Ok(self)` line), add:

```rust
        // Delegate to the unified canon pipeline. Currently a no-op;
        // existing rules below still run inline (they move into the
        // pipeline in subsequent commits).
        crate::ir::canon::canonicalize(&mut self)?;
```

Do NOT touch any other code in the function — every existing rule stays inline. The added call is a no-op today; later tasks shrink the inline code in this function as each rule moves into the canon module.

- [ ] **Step 4: Build and run tests**

Run:
```
cargo build -p pgevolve-core
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green. Test count unchanged (~707).

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/ir/canon/mod.rs crates/pgevolve-core/src/ir/mod.rs crates/pgevolve-core/src/ir/catalog.rs
git commit -m "$(cat <<'EOF'
refactor(ir): scaffold ir::canon pipeline (no-op)

Empty `canonicalize()` entry that Catalog::canonicalize now calls. Rules
move into named passes in subsequent commits; this commit only sets up
the module and the delegation point.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Move sort + dedupe into `canon::sort_and_dedupe`

**Files:**
- Create: `crates/pgevolve-core/src/ir/canon/sort_and_dedupe.rs`
- Modify: `crates/pgevolve-core/src/ir/canon/mod.rs`
- Modify: `crates/pgevolve-core/src/ir/catalog.rs`

- [ ] **Step 1: Create the new pass file**

Create `crates/pgevolve-core/src/ir/canon/sort_and_dedupe.rs`:

```rust
//! Sort each `Catalog` collection by its canonical key and reject
//! duplicate keys.
//!
//! Runs last in the pipeline so that any rule that may rewrite IR
//! values (e.g., `filter_pg_defaults`) has already completed — duplicate
//! detection sees the post-normalization state.

use crate::ir::IrError;
use crate::ir::catalog::Catalog;
use crate::ir::column_type::ColumnType;

/// Sort + dedupe every `Catalog` collection. Fallible: returns
/// [`IrError::InvalidIdentifier`] on the first duplicate key.
pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    cat.schemas.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(dupe) = first_duplicate(cat.schemas.iter().map(|s| s.name.as_str())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate schema: {dupe}"
        )));
    }

    cat.tables.sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) = first_duplicate(cat.tables.iter().map(|t| t.qname.to_string())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate table: {dupe}"
        )));
    }

    cat.indexes.sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) = first_duplicate(cat.indexes.iter().map(|i| i.qname.to_string())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate index: {dupe}"
        )));
    }

    cat.sequences.sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) = first_duplicate(cat.sequences.iter().map(|s| s.qname.to_string())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate sequence: {dupe}"
        )));
    }

    cat.views.sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) = first_duplicate(cat.views.iter().map(|v| v.qname.to_string())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate view: {dupe}"
        )));
    }

    cat.materialized_views
        .sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) =
        first_duplicate(cat.materialized_views.iter().map(|m| m.qname.to_string()))
    {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate materialized view: {dupe}"
        )));
    }

    cat.types.sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) = first_duplicate(cat.types.iter().map(|t| t.qname.to_string())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate type: {dupe}"
        )));
    }

    // Functions: identity is (qname, arg_types_normalized.canonical_hash).
    // Overloads with the same qname but different arg types are permitted.
    cat.functions.sort_by(|a, b| {
        a.qname.cmp(&b.qname).then_with(|| {
            a.arg_types_normalized
                .canonical_hash
                .cmp(&b.arg_types_normalized.canonical_hash)
        })
    });
    if let Some(dupe) = first_duplicate(cat.functions.iter().map(|f| {
        format!(
            "{}({})",
            f.qname,
            f.arg_types_normalized
                .types
                .iter()
                .map(ColumnType::render_sql)
                .collect::<Vec<_>>()
                .join(",")
        )
    })) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate function: {dupe}"
        )));
    }

    cat.procedures.sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) = first_duplicate(cat.procedures.iter().map(|p| p.qname.to_string())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate procedure: {dupe}"
        )));
    }

    Ok(())
}

fn first_duplicate<T: Ord, I: IntoIterator<Item = T>>(items: I) -> Option<T> {
    let mut seen: Vec<T> = items.into_iter().collect();
    seen.sort();
    let mut iter = seen.into_iter();
    let mut prev = iter.next()?;
    for cur in iter {
        if cur == prev {
            return Some(cur);
        }
        prev = cur;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::schema::Schema;
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    #[test]
    fn sorts_schemas_by_name() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("billing")));
        cat.schemas.push(Schema::new(id("app")));
        run(&mut cat).expect("must canonicalize");
        let names: Vec<_> = cat.schemas.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["app", "billing"]);
    }

    #[test]
    fn rejects_duplicate_table() {
        let mut cat = Catalog::empty();
        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![],
            constraints: vec![],
            comment: None,
        });
        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![],
            constraints: vec![],
            comment: None,
        });
        let err = run(&mut cat).expect_err("duplicate must error");
        assert!(err.to_string().contains("duplicate table"));
    }
}
```

- [ ] **Step 2: Wire the pass into the pipeline**

Edit `crates/pgevolve-core/src/ir/canon/mod.rs`. Add `pub mod sort_and_dedupe;` near the top (right after the existing `use` block) and update `canonicalize` to call it last:

```rust
//! [docstring unchanged]

pub mod sort_and_dedupe;

use crate::ir::IrError;
use crate::ir::catalog::Catalog;

pub fn canonicalize(cat: &mut Catalog) -> Result<(), IrError> {
    sort_and_dedupe::run(cat)?;
    Ok(())
}
```

- [ ] **Step 3: Delete the inline sort/dedupe code from `Catalog::canonicalize`**

In `crates/pgevolve-core/src/ir/catalog.rs`:

(a) Delete every `cat.<x>.sort_by(...)` + `first_duplicate(...)` block inside `Catalog::canonicalize` (lines ~71–214 in the current file). The function should keep ONLY the rules NOT yet moved (sentinel_view_columns block, enum sort_order renumbering, the eventual cost/rows + min/max + collation filters that haven't moved yet, and the final delegation to `canon::canonicalize`).

(b) Delete the standalone `fn first_duplicate` (around lines 220–232) since it's no longer used.

(c) Confirm the function body now begins with the comment block about view/MV sentinels, then the sentinel rewrite loop, then the enum renumbering loop, then `crate::ir::canon::canonicalize(&mut self)?;`, then `Ok(self)`.

(d) Remove the `#[allow(clippy::too_many_lines)]` if the function is now under 100 lines.

- [ ] **Step 4: Build, test, lint**

Run:
```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all tests pass (~707 + 2 new sort_and_dedupe unit tests = ~709). Clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/ir/canon/ crates/pgevolve-core/src/ir/catalog.rs
git commit -m "$(cat <<'EOF'
refactor(canon): move sort+dedupe into ir::canon::sort_and_dedupe

Pure code motion. Catalog::canonicalize stops inlining the per-vec sort
calls and the first_duplicate helper; both live in their own
single-responsibility module. Public behavior identical.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Move view/MV column sentinel into `canon::sentinel_view_columns`

**Files:**
- Create: `crates/pgevolve-core/src/ir/canon/sentinel_view_columns.rs`
- Modify: `crates/pgevolve-core/src/ir/canon/mod.rs`
- Modify: `crates/pgevolve-core/src/ir/catalog.rs`

- [ ] **Step 1: Create the new pass file**

Create `crates/pgevolve-core/src/ir/canon/sentinel_view_columns.rs`:

```rust
//! Collapse view and materialized-view column types to a shared
//! sentinel.
//!
//! Source-side parsing produces placeholder `ColumnType::Other` values
//! for view columns because static type-resolution of an arbitrary
//! SELECT body without running it through PG is non-trivial. The
//! catalog reader produces real types via `format_type` on the view's
//! `pg_class` row. Body-level changes are already captured by
//! `body_canonical` (a canonicalized AST hash), so per-output-column
//! types are redundant info derived from the body. We normalize them
//! to a single sentinel on both sides so byte-equality holds without
//! a source-side analyzer.

use crate::ir::catalog::Catalog;
use crate::ir::column_type::ColumnType;

/// Replace every view and MV column's `column_type` with the
/// `view_column` sentinel.
pub fn run(cat: &mut Catalog) {
    let sentinel = ColumnType::Other {
        raw: "view_column".to_string(),
    };
    for v in &mut cat.views {
        for c in &mut v.columns {
            c.column_type = sentinel.clone();
        }
    }
    for m in &mut cat.materialized_views {
        for c in &mut m.columns {
            c.column_type = sentinel.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::view::{View, ViewColumn};
    use crate::parse::normalize_body::NormalizedBody;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn replaces_bigint_view_column_with_sentinel() {
        let mut cat = Catalog::empty();
        cat.views.push(View {
            qname: QualifiedName::new(id("app"), id("v")),
            columns: vec![ViewColumn {
                name: id("id"),
                column_type: ColumnType::BigInt,
                comment: None,
            }],
            body_canonical: NormalizedBody::empty(),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            comment: None,
            raw_body: String::new(),
        });
        run(&mut cat);
        assert!(matches!(
            &cat.views[0].columns[0].column_type,
            ColumnType::Other { raw } if raw == "view_column",
        ));
    }
}
```

- [ ] **Step 2: Wire the pass into the pipeline**

Edit `crates/pgevolve-core/src/ir/canon/mod.rs`. Add `pub mod sentinel_view_columns;` and call it in `canonicalize` BEFORE `sort_and_dedupe`:

```rust
pub mod sentinel_view_columns;
pub mod sort_and_dedupe;

use crate::ir::IrError;
use crate::ir::catalog::Catalog;

pub fn canonicalize(cat: &mut Catalog) -> Result<(), IrError> {
    sentinel_view_columns::run(cat);
    sort_and_dedupe::run(cat)?;
    Ok(())
}
```

- [ ] **Step 3: Delete the inline sentinel block from `Catalog::canonicalize`**

In `crates/pgevolve-core/src/ir/catalog.rs`, delete the block beginning with the comment `// View / MV column types: source-side parsing ...` and ending after the `for m in &mut self.materialized_views { ... }` loop (the `let sentinel = ...` line through the second loop's closing brace).

- [ ] **Step 4: Build, test, lint**

Run:
```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green (one more new test, ~710).

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/ir/canon/ crates/pgevolve-core/src/ir/catalog.rs
git commit -m "$(cat <<'EOF'
refactor(canon): move view-column sentinel into ir::canon

Pure code motion. The view_column sentinel rewrite that used to live
inline in Catalog::canonicalize now lives in its own pass with a focused
unit test.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Move enum sort_order renumbering into `canon::renumber_enum_sort_orders`

**Files:**
- Create: `crates/pgevolve-core/src/ir/canon/renumber_enum_sort_orders.rs`
- Modify: `crates/pgevolve-core/src/ir/canon/mod.rs`
- Modify: `crates/pgevolve-core/src/ir/catalog.rs`

- [ ] **Step 1: Create the new pass file**

Create `crates/pgevolve-core/src/ir/canon/renumber_enum_sort_orders.rs`:

```rust
//! Re-index every enum's `sort_order` values to `1.0, 2.0, 3.0, …` in
//! ascending order.
//!
//! PG stores enum sort orders as float4 and `ALTER TYPE … ADD VALUE`
//! can produce fractional or 0-indexed values. The source parser
//! assigns `1.0, 2.0, …` in declaration order. The IR-level
//! equivalence we care about is the value names AND their relative
//! order — the floats are storage detail. Re-numbering on both sides
//! makes byte-equality work without a custom `Eq` impl.

use crate::ir::catalog::Catalog;
use crate::ir::user_type::UserTypeKind;

/// Sort each enum's values by current `sort_order`, then renumber to
/// `1.0, 2.0, 3.0, …`.
pub fn run(cat: &mut Catalog) {
    for t in &mut cat.types {
        if let UserTypeKind::Enum { values } = &mut t.kind {
            // Preserve relative order (sort by current sort_order
            // ascending) then assign 1-indexed floats.
            values.sort_by(|a, b| {
                a.sort_order
                    .partial_cmp(&b.sort_order)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            #[allow(clippy::cast_precision_loss)]
            for (i, v) in values.iter_mut().enumerate() {
                v.sort_order = (i as f32) + 1.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::user_type::{EnumValue, UserType, UserTypeKind};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn renumbers_fractional_orders_to_sequential_floats() {
        let mut cat = Catalog::empty();
        cat.types.push(UserType {
            qname: QualifiedName::new(id("app"), id("status")),
            kind: UserTypeKind::Enum {
                values: vec![
                    EnumValue { name: "open".into(), sort_order: 0.5 },
                    EnumValue { name: "closed".into(), sort_order: 1.7 },
                    EnumValue { name: "pending".into(), sort_order: 0.1 },
                ],
            },
            comment: None,
        });
        run(&mut cat);
        let kind = &cat.types[0].kind;
        let UserTypeKind::Enum { values } = kind else {
            panic!("expected Enum kind, got {kind:?}");
        };
        let orders: Vec<f32> = values.iter().map(|v| v.sort_order).collect();
        let names: Vec<&str> = values.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(orders, vec![1.0, 2.0, 3.0]);
        // Sorted by original sort_order: pending (0.1), open (0.5), closed (1.7).
        assert_eq!(names, vec!["pending", "open", "closed"]);
    }
}
```

- [ ] **Step 2: Wire the pass into the pipeline**

Edit `crates/pgevolve-core/src/ir/canon/mod.rs`. Add `pub mod renumber_enum_sort_orders;` and run it between `sentinel_view_columns` and `sort_and_dedupe`:

```rust
pub mod renumber_enum_sort_orders;
pub mod sentinel_view_columns;
pub mod sort_and_dedupe;

use crate::ir::IrError;
use crate::ir::catalog::Catalog;

pub fn canonicalize(cat: &mut Catalog) -> Result<(), IrError> {
    sentinel_view_columns::run(cat);
    renumber_enum_sort_orders::run(cat);
    sort_and_dedupe::run(cat)?;
    Ok(())
}
```

- [ ] **Step 3: Delete the inline enum-renumber block from `Catalog::canonicalize`**

In `crates/pgevolve-core/src/ir/catalog.rs`, delete the block beginning with the comment `// Normalize enum sort_order to sequential 1.0, 2.0, ...` through the closing brace of the outer `for t in &mut self.types` loop.

- [ ] **Step 4: Build, test, lint**

Run:
```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green (one more new test).

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/ir/canon/ crates/pgevolve-core/src/ir/catalog.rs
git commit -m "$(cat <<'EOF'
refactor(canon): move enum sort_order renumber into ir::canon

Pure code motion. The renumber-to-1.0,2.0,… pass that used to live
inline in Catalog::canonicalize now lives in its own pass with a focused
unit test.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Move sequence + column-collation default elision into `canon::filter_pg_defaults`

**Files:**
- Create: `crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs`
- Modify: `crates/pgevolve-core/src/ir/canon/mod.rs`
- Modify: `crates/pgevolve-core/src/ir/sequence.rs`
- Modify: `crates/pgevolve-core/src/catalog/assemble.rs`

- [ ] **Step 1: Create the new pass file with sequence + column-collation rules**

Create `crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs`:

```rust
//! Drop IR field values that match PG's documented defaults — turning
//! them into `None`.
//!
//! Why: PG stores explicit values for things the user often didn't
//! declare (e.g., `MINVALUE`/`MAXVALUE` derived from the sequence's
//! type; `COST 100` for SQL/PLpgSQL functions; the implicit
//! `pg_catalog.default` collation for every text column). The source
//! parser uses `None` to mean "no explicit clause." This pass
//! normalizes both sides so a function declared without `COST` and the
//! catalog reading of the same function are byte-equal.
//!
//! Rules are order-insensitive — each runs over disjoint IR fields.

use crate::ir::catalog::Catalog;
use crate::ir::column_type::ColumnType;
use crate::ir::sequence::Sequence;

/// Run every default-elision rule.
pub fn run(cat: &mut Catalog) {
    for seq in &mut cat.sequences {
        normalize_sequence_defaults(seq);
    }
    for table in &mut cat.tables {
        for col in &mut table.columns {
            normalize_column_collation(col);
        }
    }
    // Function cost/rows lands in Task 6 (phase B).
}

/// Normalize `min_value` / `max_value` to `None` when they equal the
/// PG-implied default for the sequence's `(data_type, increment)`.
fn normalize_sequence_defaults(seq: &mut Sequence) {
    let (default_min, default_max) = sequence_default_bounds(&seq.data_type, seq.increment);
    if seq.min_value == Some(default_min) {
        seq.min_value = None;
    }
    if seq.max_value == Some(default_max) {
        seq.max_value = None;
    }
}

/// PG's per-type defaults for `MINVALUE`/`MAXVALUE` when not explicitly
/// set. For ascending sequences (`increment > 0`), `MINVALUE` defaults
/// to `1` and `MAXVALUE` to the type's max. For descending sequences,
/// the roles flip.
fn sequence_default_bounds(ty: &ColumnType, increment: i64) -> (i64, i64) {
    let (ty_min, ty_max) = match ty {
        ColumnType::SmallInt => (i64::from(i16::MIN), i64::from(i16::MAX)),
        ColumnType::Integer => (i64::from(i32::MIN), i64::from(i32::MAX)),
        // BigInt or anything else we treat as bigint-shaped.
        _ => (i64::MIN, i64::MAX),
    };
    if increment >= 0 {
        (1, ty_max)
    } else {
        (ty_min, -1)
    }
}

/// Strip the implicit `pg_catalog.default` collation that PG attaches
/// to every text-typed column. Source IR uses `None` to mean "no
/// explicit COLLATE clause"; this pass aligns the catalog read.
fn normalize_column_collation(col: &mut crate::ir::column::Column) {
    if let Some(qname) = &col.collation
        && qname.schema.as_str() == "pg_catalog"
        && qname.name.as_str() == "default"
    {
        col.collation = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::column::Column;
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn ascending_bigint_seq() -> Sequence {
        Sequence {
            qname: qn("app", "s"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: Some(1),
            max_value: Some(i64::MAX),
            cache: 1,
            cycle: false,
            owned_by: None,
            comment: None,
        }
    }

    #[test]
    fn strips_pg_default_min_max_on_ascending_bigint() {
        let mut cat = Catalog::empty();
        cat.sequences.push(ascending_bigint_seq());
        run(&mut cat);
        let s = &cat.sequences[0];
        assert_eq!(s.min_value, None);
        assert_eq!(s.max_value, None);
    }

    #[test]
    fn keeps_explicit_non_default_min_max() {
        let mut cat = Catalog::empty();
        let mut s = ascending_bigint_seq();
        s.min_value = Some(5);
        s.max_value = Some(1000);
        cat.sequences.push(s);
        run(&mut cat);
        assert_eq!(cat.sequences[0].min_value, Some(5));
        assert_eq!(cat.sequences[0].max_value, Some(1000));
    }

    #[test]
    fn strips_pg_catalog_default_collation() {
        let mut cat = Catalog::empty();
        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![Column {
                name: id("email"),
                ty: ColumnType::Text,
                nullable: false,
                default: None,
                identity: None,
                generated: None,
                collation: Some(QualifiedName::new(id("pg_catalog"), id("default"))),
                comment: None,
            }],
            constraints: vec![],
            comment: None,
        });
        run(&mut cat);
        assert_eq!(cat.tables[0].columns[0].collation, None);
    }

    #[test]
    fn keeps_explicit_collation() {
        let mut cat = Catalog::empty();
        let explicit = QualifiedName::new(id("pg_catalog"), id("ucs_basic"));
        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![Column {
                name: id("email"),
                ty: ColumnType::Text,
                nullable: false,
                default: None,
                identity: None,
                generated: None,
                collation: Some(explicit.clone()),
                comment: None,
            }],
            constraints: vec![],
            comment: None,
        });
        run(&mut cat);
        assert_eq!(cat.tables[0].columns[0].collation, Some(explicit));
    }
}
```

- [ ] **Step 2: Wire the pass into the pipeline (runs first)**

Edit `crates/pgevolve-core/src/ir/canon/mod.rs`. Add `pub mod filter_pg_defaults;` and call it FIRST in `canonicalize`:

```rust
pub mod filter_pg_defaults;
pub mod renumber_enum_sort_orders;
pub mod sentinel_view_columns;
pub mod sort_and_dedupe;

use crate::ir::IrError;
use crate::ir::catalog::Catalog;

pub fn canonicalize(cat: &mut Catalog) -> Result<(), IrError> {
    filter_pg_defaults::run(cat);
    sentinel_view_columns::run(cat);
    renumber_enum_sort_orders::run(cat);
    sort_and_dedupe::run(cat)?;
    Ok(())
}
```

- [ ] **Step 3: Delete `Sequence::canonicalize` and its helper**

In `crates/pgevolve-core/src/ir/sequence.rs`, delete:

- The `impl Sequence { ... canonicalize ... }` block (the `#[must_use] pub fn canonicalize(mut self) -> Self { ... }` method and its enclosing `impl Sequence` block — keep nothing else in that impl since it had only one method).
- The standalone `fn default_bounds(ty: &ColumnType, increment: i64) -> (i64, i64)` helper.

Also delete the now-unused `use` for `ColumnType` in this file if removing `default_bounds` orphans it. (Check with `grep ColumnType crates/pgevolve-core/src/ir/sequence.rs` — if `ColumnType` is still referenced from the struct definition, leave the import; if not, drop it.)

In `crates/pgevolve-core/src/ir/catalog.rs`, delete the inline sequence-canonicalize call:

```rust
        self.sequences = self
            .sequences
            .into_iter()
            .map(Sequence::canonicalize)
            .collect();
```

This block must be removed because the rule is now in `filter_pg_defaults::run` (called via `canon::canonicalize`).

If after this change `Sequence` is no longer imported in `crates/pgevolve-core/src/ir/catalog.rs`, drop the now-unused `use` line.

- [ ] **Step 4: Delete the catalog-side min/max filter in `build_sequence`**

In `crates/pgevolve-core/src/catalog/assemble.rs`, find `fn build_sequence` (around line 685). Replace the block:

```rust
    // PG stores explicit `min_value`/`max_value` even when the source didn't
    // specify them — the value defaults to the type's full range plus
    // direction-aware start adjustments. Treat the type-default values as
    // "unspecified" so the IR doesn't gain phantom MIN/MAX clauses.
    let raw_min = r.get_int(q, "min_value")?;
    let raw_max = r.get_int(q, "max_value")?;
    let (default_min, default_max) = sequence_default_bounds(&data_type, increment);
    let min_value = (raw_min != default_min).then_some(raw_min);
    let max_value = (raw_max != default_max).then_some(raw_max);
```

with:

```rust
    // PG stores explicit `min_value`/`max_value` even when the source
    // didn't specify them. The catalog reader returns those raw values;
    // `ir::canon::filter_pg_defaults` normalizes the type-default
    // values to None on both sides.
    let min_value = Some(r.get_int(q, "min_value")?);
    let max_value = Some(r.get_int(q, "max_value")?);
```

Then DELETE the `fn sequence_default_bounds` helper (around lines 727–743 — its definition is no longer used here).

- [ ] **Step 5: Delete the column-collation filter in the column-reader**

In `crates/pgevolve-core/src/catalog/assemble.rs`, find the `let collation = match ...` block (around line 264). Replace:

```rust
    let collation = match (
        r.get_opt_text(q, "collation_schema")?,
        r.get_opt_text(q, "collation_name")?,
    ) {
        // `pg_catalog.default` is PG's implicit collation for every
        // text-typed column. Treat it as "no explicit collation" so that
        // the IR doesn't gain a phantom collation that nobody declared.
        (Some(s), Some(n)) if s == "pg_catalog" && n == "default" => None,
        (Some(s), Some(n)) => Some(QualifiedName::new(ident_required(&s)?, ident_required(&n)?)),
        _ => None,
    };
```

with:

```rust
    let collation = match (
        r.get_opt_text(q, "collation_schema")?,
        r.get_opt_text(q, "collation_name")?,
    ) {
        (Some(s), Some(n)) => Some(QualifiedName::new(ident_required(&s)?, ident_required(&n)?)),
        _ => None,
    };
```

`ir::canon::filter_pg_defaults` now drops the `pg_catalog.default` collation.

- [ ] **Step 6: Build, test, lint**

Run:
```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green. Test count rises by 4 new unit tests.

If any existing test asserts that the catalog reader produces `collation = None` for `pg_catalog.default` (i.e., before canonicalization), it may now fail because the reader produces `Some(pg_catalog.default)`. Such tests need updating to either (a) assert post-canonicalize state or (b) accept the raw `Some` value. Adjust them in place.

- [ ] **Step 7: Commit**

```bash
git add crates/pgevolve-core/src/ir/canon/ crates/pgevolve-core/src/ir/catalog.rs crates/pgevolve-core/src/ir/sequence.rs crates/pgevolve-core/src/catalog/assemble.rs
git commit -m "$(cat <<'EOF'
refactor(canon): move sequence + collation defaults into ir::canon

Catalog reader and source builders now produce raw IR values for
sequence min/max and column collation. `ir::canon::filter_pg_defaults`
turns the PG-implied defaults into `None` on both sides. Drops the
duplicated `default_bounds` helper that lived in both ir/sequence.rs
and catalog/assemble.rs. Public behavior identical.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Move function cost/rows default elision into `canon::filter_pg_defaults`

**Files:**
- Modify: `crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs`
- Modify: `crates/pgevolve-core/src/catalog/assemble.rs`
- Modify: `crates/pgevolve-core/src/parse/builder/create_function_stmt.rs`

- [ ] **Step 1: Extend `filter_pg_defaults` with function rules**

In `crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs`:

(a) At the top, add the import:

```rust
use crate::ir::function::Function;
```

(b) Update `run` to iterate functions too:

```rust
pub fn run(cat: &mut Catalog) {
    for seq in &mut cat.sequences {
        normalize_sequence_defaults(seq);
    }
    for table in &mut cat.tables {
        for col in &mut table.columns {
            normalize_column_collation(col);
        }
    }
    for f in &mut cat.functions {
        normalize_function_defaults(f);
    }
}
```

(c) Add the helper function:

```rust
/// PG defaults `procost = 100` for SQL/PLpgSQL functions and
/// `prorows = 1000` for SETOF (`0` otherwise). Source IR uses `None`
/// for the default in both cases; this pass aligns the catalog read.
fn normalize_function_defaults(f: &mut Function) {
    if let Some(v) = f.cost
        && (v - 100.0).abs() <= f32::EPSILON
    {
        f.cost = None;
    }
    if let Some(v) = f.rows
        && (v <= 0.0 || (v - 1000.0).abs() <= f32::EPSILON)
    {
        f.rows = None;
    }
}
```

(d) Add unit tests inside the existing `mod tests`:

```rust
    use crate::ir::function::{
        Function, FunctionLanguage, ParallelSafety, SecurityMode, Volatility,
    };
    use crate::ir::function::NormalizedArgTypes;
    use crate::parse::normalize_body::NormalizedBody;

    fn sample_function() -> Function {
        Function {
            qname: qn("app", "f"),
            args: vec![],
            arg_types_normalized: NormalizedArgTypes::from_args(&[]),
            return_type: crate::ir::function::ReturnType::Void,
            language: FunctionLanguage::Sql,
            body: NormalizedBody::empty(),
            body_dependencies: vec![],
            volatility: Volatility::Volatile,
            strict: false,
            security: SecurityMode::Invoker,
            parallel: ParallelSafety::Unsafe,
            leakproof: false,
            cost: None,
            rows: None,
            comment: None,
        }
    }

    #[test]
    fn strips_pg_default_cost() {
        let mut cat = Catalog::empty();
        let mut f = sample_function();
        f.cost = Some(100.0);
        cat.functions.push(f);
        run(&mut cat);
        assert_eq!(cat.functions[0].cost, None);
    }

    #[test]
    fn keeps_non_default_cost() {
        let mut cat = Catalog::empty();
        let mut f = sample_function();
        f.cost = Some(50.0);
        cat.functions.push(f);
        run(&mut cat);
        assert_eq!(cat.functions[0].cost, Some(50.0));
    }

    #[test]
    fn strips_pg_default_rows_setof() {
        let mut cat = Catalog::empty();
        let mut f = sample_function();
        f.rows = Some(1000.0);
        cat.functions.push(f);
        run(&mut cat);
        assert_eq!(cat.functions[0].rows, None);
    }

    #[test]
    fn strips_pg_default_rows_zero() {
        let mut cat = Catalog::empty();
        let mut f = sample_function();
        f.rows = Some(0.0);
        cat.functions.push(f);
        run(&mut cat);
        assert_eq!(cat.functions[0].rows, None);
    }

    #[test]
    fn keeps_non_default_rows() {
        let mut cat = Catalog::empty();
        let mut f = sample_function();
        f.rows = Some(42.0);
        cat.functions.push(f);
        run(&mut cat);
        assert_eq!(cat.functions[0].rows, Some(42.0));
    }
```

If `ReturnType::Void` is not the actual variant name in the codebase, run `grep -n "pub enum ReturnType" crates/pgevolve-core/src/ir/function.rs` first and use whichever variant has no arguments.

- [ ] **Step 2: Remove the cost/rows filter from the catalog reader**

In `crates/pgevolve-core/src/catalog/assemble.rs`, find the block (around lines 1311–1325):

```rust
                // PG defaults procost = 100 for SQL/PL/pgSQL functions (vs.
                // 1 for C). Source IR uses None to mean "no explicit COST
                // clause"; we normalize the catalog read to None when the
                // value matches the PG default for SQL/plpgsql, so a
                // function declared without COST round-trips byte-equal.
                let cost: Option<f32> = cost_str
                    .as_deref()
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|&v| (v - 100.0).abs() > f32::EPSILON);
                // PG defaults prorows = 1000 for SETOF functions, 0 otherwise.
                // Source IR uses None for the default in both cases.
                let rows: Option<f32> = rows_str
                    .as_deref()
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|&v| v > 0.0 && (v - 1000.0).abs() > f32::EPSILON);
```

Replace with:

```rust
                // Catalog returns raw cost/rows; `ir::canon::filter_pg_defaults`
                // normalizes the PG-defaults (procost=100, prorows=1000 for
                // SETOF, prorows=0 otherwise) to None on both sides.
                let cost: Option<f32> = cost_str
                    .as_deref()
                    .and_then(|s| s.parse::<f32>().ok());
                let rows: Option<f32> = rows_str
                    .as_deref()
                    .and_then(|s| s.parse::<f32>().ok());
```

- [ ] **Step 3: Remove the cost/rows filter from the source builder**

In `crates/pgevolve-core/src/parse/builder/create_function_stmt.rs`, find the two relevant branches in the DefElem match (around lines 280–302):

```rust
            "cost" => {
                if is_procedure {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!("{qname}: COST is not valid on procedures"),
                    });
                }
                // Normalize explicit-but-default `COST 100` to None so it
                // round-trips byte-equal with catalog (which also stores
                // None for the SQL/PLpgSQL default).
                cost = float_from_def_elem(de).filter(|&v| (v - 100.0).abs() > f32::EPSILON);
            }
            "rows" => {
                if is_procedure {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!("{qname}: ROWS is not valid on procedures"),
                    });
                }
                // Normalize explicit-but-default `ROWS 1000` to None.
                rows = float_from_def_elem(de)
                    .filter(|&v| v > 0.0 && (v - 1000.0).abs() > f32::EPSILON);
            }
```

Replace with:

```rust
            "cost" => {
                if is_procedure {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!("{qname}: COST is not valid on procedures"),
                    });
                }
                // Raw parse: `ir::canon::filter_pg_defaults` strips the
                // PG-default value of 100 to None on both sides.
                cost = float_from_def_elem(de);
            }
            "rows" => {
                if is_procedure {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!("{qname}: ROWS is not valid on procedures"),
                    });
                }
                // Raw parse: `ir::canon::filter_pg_defaults` strips the
                // PG-default (1000 for SETOF, 0 otherwise) to None.
                rows = float_from_def_elem(de);
            }
```

- [ ] **Step 4: Build, test, lint**

Run:
```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green. Plus the 5 new function-related unit tests.

If a test in `parse::builder::create_function_stmt::tests` or `catalog::assemble::tests` asserts the raw `cost`/`rows` value being filtered at read/parse time, update it: either (a) check post-`canon` state via `catalog.canonicalize()`, or (b) accept the raw value pre-canon.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs crates/pgevolve-core/src/catalog/assemble.rs crates/pgevolve-core/src/parse/builder/create_function_stmt.rs
git commit -m "$(cat <<'EOF'
refactor(canon): move function cost/rows defaults into ir::canon

Source builder and catalog reader now produce raw cost/rows. The
canon pipeline normalizes the PG defaults (procost=100,
prorows=1000-or-0) to None on both sides. With this commit, all the
per-builder default-elision filters are gone — every PG-default rule
lives in one place.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Workspace-wide verification

**Files:** none modified — verification only.

- [ ] **Step 1: Run the full workspace test suite**

Run: `cargo test --workspace --lib --tests`
Expected: all tests pass. No conformance fixture has regressed because public behavior is unchanged.

- [ ] **Step 2: Run clippy with `-D warnings` on the whole workspace**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Run fmt check**

Run: `cargo fmt --check`
Expected: no output.

- [ ] **Step 4: Run the conformance suite**

Run: `cargo test -p pgevolve-conformance --test run`
Expected: PASS.

- [ ] **Step 5: Verify the inline rules are gone from `Catalog::canonicalize`**

Run: `grep -c "first_duplicate\|sentinel\|sort_order\|min_value\|cost\|EPSILON\|default_bounds" crates/pgevolve-core/src/ir/catalog.rs`

Expected: 0 (or only matches inside the now-trivial `Catalog::canonicalize` doc-comment, which is fine — the function body itself should be tiny).

Inspect `crates/pgevolve-core/src/ir/catalog.rs::Catalog::canonicalize` visually: it should be a thin wrapper around `crate::ir::canon::canonicalize(&mut self)?; Ok(self)`. Any remaining inline rule = bug, fix before declaring done.

- [ ] **Step 6: Verify the source builder + catalog reader are filter-free**

Run:
```
grep -n "EPSILON\|default_bounds\|pg_catalog.default" crates/pgevolve-core/src/catalog/assemble.rs crates/pgevolve-core/src/parse/builder/create_function_stmt.rs crates/pgevolve-core/src/ir/sequence.rs
```

Expected: zero matches. If any survived, they were missed in earlier tasks — go back to the relevant task and finish the migration.

- [ ] **Step 7: Run the property tests (Docker-gated, ~70s each)**

Run:
```
cargo test -p pgevolve-core --test property_tests -- --include-ignored
cargo test -p pgevolve --test pg_property_tests -- --include-ignored
```

Expected: PASS. These cover round-trip equivalence and would catch any normalization regression that the unit tests missed.

- [ ] **Step 8: If any of Steps 1–7 produced fixes, commit them**

If you found a missed filter in Step 6 or a clippy/fmt fixup in Steps 2–3, commit it as a follow-up:

```bash
git add -A
git commit -m "$(cat <<'EOF'
chore(canon): post-consolidation cleanup

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

If no fixes were needed, skip the commit.

---

## Self-review pre-flight checklist for the implementing agent

Before declaring the plan complete:

- [ ] `Catalog::canonicalize` body is a thin wrapper (≤5 lines): a call to `canon::canonicalize` and an `Ok(self)`.
- [ ] `git grep "fn canonicalize" crates/pgevolve-core/src/ir/` returns exactly two matches: the wrapper on `Catalog` and `pub fn canonicalize` in `ir/canon/mod.rs`.
- [ ] `ir/canon/` contains 5 files: `mod.rs`, `filter_pg_defaults.rs`, `sentinel_view_columns.rs`, `renumber_enum_sort_orders.rs`, `sort_and_dedupe.rs`.
- [ ] No call site in `catalog/assemble.rs` or `parse/builder/` filters PG-default values.
- [ ] `Sequence::canonicalize` and `fn default_bounds` (both in `ir/sequence.rs`) are deleted.
- [ ] `sequence_default_bounds` in `catalog/assemble.rs` is deleted.

---

## Out-of-scope (do NOT touch)

- `parse/ast_canon.rs` — its passes (`canonicalize_view_bodies`, `promote_mv_index_parents`) operate on source-side raw AST, not IR values.
- `parse/normalize_body.rs`, `parse/normalize_expr.rs` — text/AST-level canonicalization, not IR-value canonicalization.
- `Catalog::diff`, any `Diff` impl, or `ir/eq.rs` — unrelated subsystem.
- The shape of any IR struct — pure code motion, no field signatures change.
