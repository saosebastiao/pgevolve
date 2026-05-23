# Table/Index/MV Storage Parameters (Reloptions) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship v0.3.3 — declarative `WITH (storage_parameter = …)` reloptions across `Table`, `Index`, and `MaterializedView`, with typed fields for the well-known keys (fillfactor, autovacuum_*, parallel_workers, GIN/GiST/BRIN/B-tree specifics) plus an `extra: BTreeMap<String, String>` for extension keys.

**Architecture:** Ten sequential stages. Per-relkind typed `*StorageOptions` structs share an `AutovacuumOptions` substruct between `Table` and `MaterializedView` (PG documents identical key sets). `MaterializedViewStorageOptions` is a type alias for `TableStorageOptions`. Drift is **lenient** — source `None` always means "skip"; no `Reset*` Change variants because they'd be unreachable. Per-relkind validation (fillfactor ranges per access method) happens at parse time.

**Tech Stack:** Rust 1.95+, `pg_query` 6.x, `tokio_postgres`, `serde`, `proptest`. Builds on the v0.3.x trilogy infrastructure (no new cross-cutting concerns).

**Source spec:** `docs/superpowers/specs/2026-05-22-table-reloptions-design.md`.

---

## Pre-flight

- [ ] **Step 1: Confirm clean baseline**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --tests
```

All green. v0.3.2 is committed; main is clean.

- [ ] **Step 2: Skim spec sections relevant to each stage**

Open `docs/superpowers/specs/2026-05-22-table-reloptions-design.md` once. Each stage below cites the section it implements.

---

## File structure

```
crates/pgevolve-core/src/
├── ir/
│   ├── reloptions.rs            NEW — Stage 1 — Table/Index/MV storage options + AutovacuumOptions + BufferingMode + NotNanF64
│   ├── table.rs                 MODIFY — Stage 2 — add storage field
│   ├── index.rs                 MODIFY — Stage 2 — add storage field
│   ├── view.rs                  MODIFY — Stage 2 — add storage field to MaterializedView
│   └── canon/
│       └── reloptions.rs        NEW — Stage 3 — sort extra bags (mostly no-op)
├── catalog/
│   ├── reloptions.rs            NEW — Stage 4 — decode_table_reloptions, decode_index_reloptions
│   ├── queries/
│   │   ├── shared.rs            MODIFY — Stage 4 — add reloptions to tables/MVs/indexes queries
│   │   └── pg14.rs              MODIFY — Stage 4 — same additions
│   └── assemble/
│       ├── tables.rs            MODIFY — Stage 4 — decode + assign storage
│       └── indexes.rs           MODIFY — Stage 4 — decode + assign storage (find actual file path)
├── parse/
│   └── builder/
│       ├── reloptions.rs        NEW — Stage 5 — shared WITH (...) decoder
│       ├── create_stmt.rs       MODIFY — Stage 5 — wire decoder into CREATE TABLE
│       ├── create_index_stmt.rs MODIFY — Stage 5 — wire decoder into CREATE INDEX (find file)
│       ├── create_materialized_view_stmt.rs MODIFY — Stage 5
│       └── alter_table_stmt.rs  MODIFY — Stage 5 — AT_SetRelOptions handler
├── diff/
│   ├── reloptions.rs            NEW — Stage 6 — sparse-delta diff per relkind
│   ├── change.rs                MODIFY — Stage 6 — 3 new variants
│   ├── tables.rs                MODIFY — Stage 6 — call diff_reloptions for tables
│   ├── indexes.rs               MODIFY — Stage 6 — call diff_reloptions for indexes
│   └── views.rs                 MODIFY — Stage 6 — call diff_reloptions for MVs
├── plan/
│   ├── raw_step.rs              MODIFY — Stage 7 — 3 new StepKind variants
│   └── rewrite/
│       └── reloptions.rs        NEW — Stage 7 — SQL helpers + emit handlers
└── lint/
    └── rules/
        └── unmanaged_reloption.rs  NEW — Stage 8

crates/pgevolve-conformance/tests/cases/objects/
└── reloptions/                  NEW — Stage 9 — 11 fixtures
```

---

## Stage 1 — IR foundation

Pure data types in `ir::reloptions`. No behavior beyond derives.

**Files created:** `crates/pgevolve-core/src/ir/reloptions.rs`.
**Files modified:** `crates/pgevolve-core/src/ir/mod.rs`.

### Task 1.1: Create the module

- [ ] **Step 1: Write `crates/pgevolve-core/src/ir/reloptions.rs`**

```rust
//! Storage parameters / reloptions for Table, Index, MaterializedView.
//!
//! Typed fields for well-known keys + `extra: BTreeMap<String, String>` for
//! extension-registered or otherwise-unknown options. Tables and MVs share
//! the autovacuum substruct because PG documents identical key sets.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

/// `f64` wrapper that excludes NaN — required so `Option<f64>` reloptions
/// can participate in `Eq` / `Hash` / `Ord` derived implementations.
///
/// The catalog reader and source parser both reject NaN values explicitly.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct NotNanF64(f64);

impl NotNanF64 {
    /// Construct, rejecting NaN.
    ///
    /// # Errors
    ///
    /// Returns the input value when NaN.
    pub fn new(v: f64) -> Result<Self, f64> {
        if v.is_nan() { Err(v) } else { Ok(Self(v)) }
    }

    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl PartialEq for NotNanF64 {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}
impl Eq for NotNanF64 {}
impl Hash for NotNanF64 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}
impl PartialOrd for NotNanF64 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for NotNanF64 {
    fn cmp(&self, other: &Self) -> Ordering {
        // Safe because NaN is excluded by construction.
        self.0.partial_cmp(&other.0).unwrap_or(Ordering::Equal)
    }
}

/// Shared autovacuum options — apply to both `Table` and `MaterializedView`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct AutovacuumOptions {
    pub enabled: Option<bool>,
    pub vacuum_threshold: Option<u64>,
    pub vacuum_scale_factor: Option<NotNanF64>,
    pub vacuum_cost_delay: Option<u64>,
    pub vacuum_cost_limit: Option<u64>,
    pub analyze_threshold: Option<u64>,
    pub analyze_scale_factor: Option<NotNanF64>,
    pub freeze_max_age: Option<u64>,
    pub freeze_min_age: Option<u64>,
    pub freeze_table_age: Option<u64>,
    pub multixact_freeze_max_age: Option<u64>,
    pub multixact_freeze_min_age: Option<u64>,
    pub multixact_freeze_table_age: Option<u64>,
    /// `autovacuum_vacuum_insert_threshold` (PG 13+).
    pub vacuum_insert_threshold: Option<u64>,
    /// `autovacuum_vacuum_insert_scale_factor` (PG 13+).
    pub vacuum_insert_scale_factor: Option<NotNanF64>,
    /// `log_autovacuum_min_duration` — `-1` disables. Stored as i64.
    pub log_min_duration: Option<i64>,
}

impl AutovacuumOptions {
    /// `true` iff every field is `None`. Used by the differ to short-circuit.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.vacuum_threshold.is_none()
            && self.vacuum_scale_factor.is_none()
            && self.vacuum_cost_delay.is_none()
            && self.vacuum_cost_limit.is_none()
            && self.analyze_threshold.is_none()
            && self.analyze_scale_factor.is_none()
            && self.freeze_max_age.is_none()
            && self.freeze_min_age.is_none()
            && self.freeze_table_age.is_none()
            && self.multixact_freeze_max_age.is_none()
            && self.multixact_freeze_min_age.is_none()
            && self.multixact_freeze_table_age.is_none()
            && self.vacuum_insert_threshold.is_none()
            && self.vacuum_insert_scale_factor.is_none()
            && self.log_min_duration.is_none()
    }
}

/// Storage options for tables. MV reuses via type alias since PG documents
/// identical reloptions for both.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct TableStorageOptions {
    pub fillfactor: Option<u32>,            // 10..=100
    pub autovacuum: AutovacuumOptions,
    pub parallel_workers: Option<u32>,      // 0..=1024
    pub toast_tuple_target: Option<u32>,    // 128..=8160
    pub user_catalog_table: Option<bool>,
    pub vacuum_truncate: Option<bool>,      // PG 12+
    /// Unknown / extension-registered options. Always sorted by key (BTreeMap).
    pub extra: BTreeMap<String, String>,
}

impl TableStorageOptions {
    /// `true` iff every typed field is `None` and `extra` is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fillfactor.is_none()
            && self.autovacuum.is_empty()
            && self.parallel_workers.is_none()
            && self.toast_tuple_target.is_none()
            && self.user_catalog_table.is_none()
            && self.vacuum_truncate.is_none()
            && self.extra.is_empty()
    }
}

/// MVs share table reloption semantics in PG.
pub type MaterializedViewStorageOptions = TableStorageOptions;

/// Storage options for indexes. Valid keys depend on access method;
/// parse-time validation enforces per-AM rules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct IndexStorageOptions {
    pub fillfactor: Option<u32>,             // range per AM
    // GIN-specific:
    pub fastupdate: Option<bool>,
    pub gin_pending_list_limit: Option<u64>,
    // GiST/SP-GiST:
    pub buffering: Option<BufferingMode>,
    // B-tree (PG 13+):
    pub deduplicate_items: Option<bool>,
    // BRIN:
    pub pages_per_range: Option<u32>,
    pub autosummarize: Option<bool>,
    pub extra: BTreeMap<String, String>,
}

impl IndexStorageOptions {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fillfactor.is_none()
            && self.fastupdate.is_none()
            && self.gin_pending_list_limit.is_none()
            && self.buffering.is_none()
            && self.deduplicate_items.is_none()
            && self.pages_per_range.is_none()
            && self.autosummarize.is_none()
            && self.extra.is_empty()
    }
}

/// `buffering` setting for GiST/SP-GiST index builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferingMode {
    On,
    Off,
    Auto,
}

impl BufferingMode {
    #[must_use]
    pub const fn sql_keyword(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
            Self::Auto => "auto",
        }
    }

    /// Parse from `pg_class.reloptions` text or source SQL value.
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_nan_rejects_nan() {
        assert!(NotNanF64::new(f64::NAN).is_err());
        assert!(NotNanF64::new(1.5).is_ok());
        assert!(NotNanF64::new(0.0).is_ok());
        assert!(NotNanF64::new(f64::INFINITY).is_ok());
    }

    #[test]
    fn not_nan_equality_is_bit_exact() {
        let a = NotNanF64::new(0.1).unwrap();
        let b = NotNanF64::new(0.1).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn default_storage_is_empty() {
        assert!(TableStorageOptions::default().is_empty());
        assert!(IndexStorageOptions::default().is_empty());
        assert!(AutovacuumOptions::default().is_empty());
    }

    #[test]
    fn non_empty_storage_detected() {
        let s = TableStorageOptions { fillfactor: Some(80), ..Default::default() };
        assert!(!s.is_empty());
    }

    #[test]
    fn buffering_mode_roundtrips() {
        for m in [BufferingMode::On, BufferingMode::Off, BufferingMode::Auto] {
            assert_eq!(BufferingMode::from_str(m.sql_keyword()), Some(m));
        }
        assert!(BufferingMode::from_str("bogus").is_none());
    }

    #[test]
    fn extra_is_sorted_via_btreemap() {
        let mut s = TableStorageOptions::default();
        s.extra.insert("zebra".into(), "1".into());
        s.extra.insert("alpha".into(), "2".into());
        let keys: Vec<_> = s.extra.keys().cloned().collect();
        assert_eq!(keys, vec!["alpha", "zebra"]);
    }
}
```

- [ ] **Step 2: Wire into `crates/pgevolve-core/src/ir/mod.rs`**

Add `pub mod reloptions;` in alphabetical position (between `policy` and `procedure`, or wherever alphabetical fits — read the existing module list).

- [ ] **Step 3: Run + commit**

```bash
cargo test -p pgevolve-core --lib ir::reloptions
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add -p crates/pgevolve-core/src/ir/
git commit -m "$(cat <<'EOF'
feat(ir): reloptions — TableStorageOptions, IndexStorageOptions, AutovacuumOptions

New ir::reloptions module. Typed per-relkind storage option structs
with shared AutovacuumOptions (Tables + MVs use identical autovacuum
keys per PG docs; MaterializedViewStorageOptions is a type alias for
TableStorageOptions).

NotNanF64 wrapper supports Eq/Hash/Ord on f64-typed reloptions by
rejecting NaN at construction. BufferingMode enum for GiST/SP-GiST
buffering.

is_empty() helpers short-circuit the differ when nothing is set.

Stage 1 of docs/superpowers/plans/2026-05-22-table-reloptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 2 — Add `storage` field to Table/Index/MaterializedView

Mirrors v0.3.2 Stage 1 (add three fields to Table). Here we add one field to each of three relkind IRs, plus backfill all literals.

**Files modified:** `crates/pgevolve-core/src/ir/{table,index,view}.rs`, plus every workspace site that constructs one of these literals.

### Task 2.1: Add fields

- [ ] **Step 1: Extend `Table`**

In `crates/pgevolve-core/src/ir/table.rs::Table`, after the last existing field (currently `policies`), add:

```rust
    /// Storage parameters (`WITH (fillfactor = …, autovacuum_* = …, …)`).
    /// Default is the empty/no-overrides state.
    pub storage: crate::ir::reloptions::TableStorageOptions,
```

Extend the hand-rolled `Diff for Table` impl with a `diff_field` call:

```rust
        out.extend(diff_field(
            "storage",
            &format!("{:?}", self.storage),
            &format!("{:?}", other.storage),
        ));
```

- [ ] **Step 2: Extend `Index`**

In `crates/pgevolve-core/src/ir/index.rs::Index`, after the last existing field, add:

```rust
    /// Storage parameters (`WITH (fillfactor = …, fastupdate = …, …)`).
    /// Valid keys depend on `method`; parser enforces per-AM ranges.
    pub storage: crate::ir::reloptions::IndexStorageOptions,
```

If `Index` derives `DiffMacro`, add `#[diff(via_debug)]` on the new field. If it's hand-rolled, add a `diff_field` call.

- [ ] **Step 3: Extend `MaterializedView`**

In `crates/pgevolve-core/src/ir/view.rs::MaterializedView`, after the last existing field, add:

```rust
    /// Storage parameters. Same key set as Table.
    pub storage: crate::ir::reloptions::MaterializedViewStorageOptions,
```

Extend the `MaterializedView` `Diff` impl with the new field (hand-rolled, mirror existing).

### Task 2.2: Workspace-wide backfill

- [ ] **Step 1: Run `cargo check --workspace --all-targets` and iterate**

```bash
cargo check --workspace --all-targets 2>&1 | grep -E "missing field" | head -30
```

Each missing-field error: add `storage: <DefaultsType>::default()` to the literal. Likely sites:

- `crates/pgevolve-core/src/ir/{table,index,view}.rs::tests::base()` (test helpers).
- `crates/pgevolve-core/src/diff/{tables,indexes,views}.rs` test helpers.
- `crates/pgevolve-core/src/catalog/assemble/{tables,indexes,views}.rs` (Stage 4 populates them properly; for now `Default::default()` placeholder).
- `crates/pgevolve-core/src/parse/builder/{create_stmt,create_index_stmt,create_materialized_view_stmt}.rs` (Stage 5 populates; placeholder for now).
- `crates/pgevolve-testkit/src/ir_generator.rs` (Stage 10 extends with arbitrary; placeholder for now).
- `crates/pgevolve-core/src/render/{table,index,view}.rs` test fixtures.
- Conformance test fixtures that inline-build the IR (unlikely — they go through parse).

v0.3.2 Stage 1 backfilled 120 `Table` literals. This stage touches 3 struct types, so likely ~200 backfill sites across the workspace. Iterate until clean.

- [ ] **Step 2: Per-type Diff tests**

Add to each of `Table`, `Index`, `MaterializedView` test modules:

```rust
    #[test]
    fn storage_change_diffs() {
        let mut b = base();
        b.storage = /* construct a non-default storage value */;
        assert!(base().diff(&b).iter().any(|x| x.path == "storage"));
    }
```

For `Table` / `MaterializedView`, the non-default value can be:
```rust
crate::ir::reloptions::TableStorageOptions {
    fillfactor: Some(80),
    ..Default::default()
}
```

For `Index`:
```rust
crate::ir::reloptions::IndexStorageOptions {
    fillfactor: Some(70),
    ..Default::default()
}
```

### Task 2.3: Run + commit

```bash
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add -p crates/
git commit -m "$(cat <<'EOF'
feat(ir): add storage field to Table, Index, MaterializedView

Three IR types each gain a `storage: *StorageOptions` field.
TableStorageOptions::default() = the empty/no-overrides state
(matches PG's pg_class.reloptions IS NULL semantics).

Backfilled all Table/Index/MaterializedView literals across the
workspace (~200 sites; compiler-enforced). Diff impls extended.

Stage 2 of docs/superpowers/plans/2026-05-22-table-reloptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 3 — Canon

`BTreeMap` is already sorted, so canon is mostly a no-op. But wire it into the orchestrator for future-proofing — if extra-key normalization (e.g., lowercase keys) becomes needed, this is where it goes.

**Files created:** `crates/pgevolve-core/src/ir/canon/reloptions.rs`.
**Files modified:** `crates/pgevolve-core/src/ir/canon/mod.rs`.

### Task 3.1: Canon module

- [ ] **Step 1: Create `crates/pgevolve-core/src/ir/canon/reloptions.rs`**

```rust
//! Canon rules for reloptions.
//!
//! `extra` is `BTreeMap<String, String>` (already key-ordered). This module
//! is currently a no-op pass-through, intentionally; it exists so future
//! normalization (lowercase keys, value trimming, etc.) has an obvious home.

use crate::ir::catalog::Catalog;

pub fn run(cat: &mut Catalog) {
    // Tables, indexes, MVs — each storage struct's `extra` is BTreeMap,
    // already ordered. Nothing to do today. If future PG quirks require
    // value-normalization (e.g., '1'/'true'/'on' canonicalization on bool
    // reloptions, or lowercasing extra-bag keys), add it here.
    let _ = cat;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;

    #[test]
    fn run_on_empty_catalog_is_no_op() {
        let mut c = Catalog::empty();
        run(&mut c);
        assert!(c.tables.is_empty());
    }

    #[test]
    fn run_is_idempotent() {
        let mut c = Catalog::empty();
        // Build a minimal table-with-storage (use a helper if it exists,
        // otherwise inline a Table literal).
        run(&mut c);
        let snap1 = format!("{c:?}");
        run(&mut c);
        let snap2 = format!("{c:?}");
        assert_eq!(snap1, snap2);
    }
}
```

- [ ] **Step 2: Wire into orchestrator**

In `crates/pgevolve-core/src/ir/canon/mod.rs`:

```rust
pub mod reloptions;
```

(Alphabetical position — likely between `policies` and `renumber_enum_sort_orders`.)

In `canonicalize(cat: &mut Catalog)`, after the existing canon passes, add:

```rust
    reloptions::run(cat);
```

Position it after `policies::run_on_table` calls, before `sort_and_dedupe::run`.

### Task 3.2: Run + commit

```bash
cargo test -p pgevolve-core --lib ir::canon::reloptions
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve-core/src/ir/canon/
git commit -m "$(cat <<'EOF'
feat(canon): reloptions — no-op placeholder + orchestrator wiring

`BTreeMap` already orders extra-bag keys, so canon is a no-op today.
The module exists so future normalization (lowercase keys, bool
value canonicalization, etc.) has an obvious home that's already
wired into Catalog::canonicalize.

Stage 3 of docs/superpowers/plans/2026-05-22-table-reloptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 4 — Catalog reader

Decode `pg_class.reloptions::text[]` into typed structs. Wire into tables/MVs/indexes assemblers. The tables query already reads the column (used for view reloptions); extend to also assign on Table/MV. Indexes query needs the column added.

**Files created:** `crates/pgevolve-core/src/catalog/reloptions.rs`.
**Files modified:** `crates/pgevolve-core/src/catalog/{mod.rs, queries/shared.rs, queries/pg14.rs, assemble/tables.rs, assemble/indexes.rs (or wherever)}.rs`.

### Task 4.1: Shared decoder

- [ ] **Step 1: Create `crates/pgevolve-core/src/catalog/reloptions.rs`**

```rust
//! Decode `pg_class.reloptions::text[]` into typed *StorageOptions.

use std::collections::BTreeMap;

use crate::catalog::error::CatalogError;
use crate::catalog::queries::CatalogQuery;
use crate::ir::reloptions::{
    AutovacuumOptions, BufferingMode, IndexStorageOptions, NotNanF64,
    TableStorageOptions,
};

/// Decode reloptions for a table or materialized view.
pub(crate) fn decode_table_reloptions(
    raw: &[String],
    q: CatalogQuery,
) -> Result<TableStorageOptions, CatalogError> {
    let mut out = TableStorageOptions::default();
    for entry in raw {
        let (key, value) = split_kv(entry, q)?;
        if assign_autovacuum(&mut out.autovacuum, key, &value, q)? {
            continue;
        }
        match key {
            "fillfactor" => out.fillfactor = Some(parse_u32(&value, key, q)?),
            "parallel_workers" => out.parallel_workers = Some(parse_u32(&value, key, q)?),
            "toast_tuple_target" => out.toast_tuple_target = Some(parse_u32(&value, key, q)?),
            "user_catalog_table" => out.user_catalog_table = Some(parse_bool(&value, key, q)?),
            "vacuum_truncate" => out.vacuum_truncate = Some(parse_bool(&value, key, q)?),
            _ => {
                out.extra.insert(key.to_owned(), value);
            }
        }
    }
    Ok(out)
}

/// Decode reloptions for an index. Unknown keys land in `extra` regardless
/// of access method — validation is parser-side, not reader-side.
pub(crate) fn decode_index_reloptions(
    raw: &[String],
    q: CatalogQuery,
) -> Result<IndexStorageOptions, CatalogError> {
    let mut out = IndexStorageOptions::default();
    for entry in raw {
        let (key, value) = split_kv(entry, q)?;
        match key {
            "fillfactor" => out.fillfactor = Some(parse_u32(&value, key, q)?),
            "fastupdate" => out.fastupdate = Some(parse_bool(&value, key, q)?),
            "gin_pending_list_limit" => {
                out.gin_pending_list_limit = Some(parse_u64(&value, key, q)?);
            }
            "buffering" => {
                out.buffering = Some(BufferingMode::from_str(&value).ok_or_else(|| {
                    CatalogError::BadColumnType {
                        query: q,
                        column: "reloptions",
                        message: format!("buffering value {value:?} invalid"),
                    }
                })?);
            }
            "deduplicate_items" => out.deduplicate_items = Some(parse_bool(&value, key, q)?),
            "pages_per_range" => out.pages_per_range = Some(parse_u32(&value, key, q)?),
            "autosummarize" => out.autosummarize = Some(parse_bool(&value, key, q)?),
            _ => {
                out.extra.insert(key.to_owned(), value);
            }
        }
    }
    Ok(out)
}

fn assign_autovacuum(
    out: &mut AutovacuumOptions,
    key: &str,
    value: &str,
    q: CatalogQuery,
) -> Result<bool, CatalogError> {
    match key {
        "autovacuum_enabled" => out.enabled = Some(parse_bool(value, key, q)?),
        "autovacuum_vacuum_threshold" => out.vacuum_threshold = Some(parse_u64(value, key, q)?),
        "autovacuum_vacuum_scale_factor" => out.vacuum_scale_factor = Some(parse_notnan(value, key, q)?),
        "autovacuum_vacuum_cost_delay" => out.vacuum_cost_delay = Some(parse_u64(value, key, q)?),
        "autovacuum_vacuum_cost_limit" => out.vacuum_cost_limit = Some(parse_u64(value, key, q)?),
        "autovacuum_analyze_threshold" => out.analyze_threshold = Some(parse_u64(value, key, q)?),
        "autovacuum_analyze_scale_factor" => out.analyze_scale_factor = Some(parse_notnan(value, key, q)?),
        "autovacuum_freeze_max_age" => out.freeze_max_age = Some(parse_u64(value, key, q)?),
        "autovacuum_freeze_min_age" => out.freeze_min_age = Some(parse_u64(value, key, q)?),
        "autovacuum_freeze_table_age" => out.freeze_table_age = Some(parse_u64(value, key, q)?),
        "autovacuum_multixact_freeze_max_age" => out.multixact_freeze_max_age = Some(parse_u64(value, key, q)?),
        "autovacuum_multixact_freeze_min_age" => out.multixact_freeze_min_age = Some(parse_u64(value, key, q)?),
        "autovacuum_multixact_freeze_table_age" => out.multixact_freeze_table_age = Some(parse_u64(value, key, q)?),
        "autovacuum_vacuum_insert_threshold" => out.vacuum_insert_threshold = Some(parse_u64(value, key, q)?),
        "autovacuum_vacuum_insert_scale_factor" => out.vacuum_insert_scale_factor = Some(parse_notnan(value, key, q)?),
        "log_autovacuum_min_duration" => out.log_min_duration = Some(parse_i64(value, key, q)?),
        _ => return Ok(false),
    }
    Ok(true)
}

fn split_kv<'a>(entry: &'a str, q: CatalogQuery) -> Result<(&'a str, String), CatalogError> {
    let (k, v) = entry.split_once('=').ok_or_else(|| CatalogError::BadColumnType {
        query: q,
        column: "reloptions",
        message: format!("malformed reloption {entry:?}"),
    })?;
    Ok((k, v.to_owned()))
}

fn parse_u32(v: &str, key: &str, q: CatalogQuery) -> Result<u32, CatalogError> {
    v.parse().map_err(|e| CatalogError::BadColumnType {
        query: q,
        column: "reloptions",
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })
}

fn parse_u64(v: &str, key: &str, q: CatalogQuery) -> Result<u64, CatalogError> {
    v.parse().map_err(|e| CatalogError::BadColumnType {
        query: q,
        column: "reloptions",
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })
}

fn parse_i64(v: &str, key: &str, q: CatalogQuery) -> Result<i64, CatalogError> {
    v.parse().map_err(|e| CatalogError::BadColumnType {
        query: q,
        column: "reloptions",
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })
}

fn parse_bool(v: &str, key: &str, q: CatalogQuery) -> Result<bool, CatalogError> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "on" | "1" => Ok(true),
        "false" | "off" | "0" => Ok(false),
        _ => Err(CatalogError::BadColumnType {
            query: q,
            column: "reloptions",
            message: format!("reloption {key} = {v:?} not a recognized bool"),
        }),
    }
}

fn parse_notnan(v: &str, key: &str, q: CatalogQuery) -> Result<NotNanF64, CatalogError> {
    let f: f64 = v.parse().map_err(|e| CatalogError::BadColumnType {
        query: q,
        column: "reloptions",
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })?;
    NotNanF64::new(f).map_err(|_| CatalogError::BadColumnType {
        query: q,
        column: "reloptions",
        message: format!("reloption {key} value is NaN"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const Q: CatalogQuery = CatalogQuery::Indexes; // arbitrary; doesn't affect decode logic

    #[test]
    fn decodes_fillfactor() {
        let s = decode_table_reloptions(&["fillfactor=80".into()], Q).unwrap();
        assert_eq!(s.fillfactor, Some(80));
    }

    #[test]
    fn decodes_autovacuum_enabled() {
        let s = decode_table_reloptions(&["autovacuum_enabled=false".into()], Q).unwrap();
        assert_eq!(s.autovacuum.enabled, Some(false));
    }

    #[test]
    fn decodes_autovacuum_scale_factor() {
        let s = decode_table_reloptions(&["autovacuum_vacuum_scale_factor=0.05".into()], Q).unwrap();
        assert_eq!(s.autovacuum.vacuum_scale_factor.unwrap().get(), 0.05);
    }

    #[test]
    fn unknown_keys_flow_into_extra() {
        let s = decode_table_reloptions(&["pg_partman.something=value".into()], Q).unwrap();
        assert_eq!(s.extra.get("pg_partman.something").map(String::as_str), Some("value"));
    }

    #[test]
    fn index_decode_buffering_on() {
        let s = decode_index_reloptions(&["buffering=auto".into()], Q).unwrap();
        assert_eq!(s.buffering, Some(BufferingMode::Auto));
    }

    #[test]
    fn malformed_entry_errors() {
        assert!(decode_table_reloptions(&["no_equals".into()], Q).is_err());
    }

    #[test]
    fn nan_value_errors() {
        assert!(decode_table_reloptions(&["autovacuum_vacuum_scale_factor=NaN".into()], Q).is_err());
    }

    #[test]
    fn bool_accepts_on_off() {
        let s = decode_table_reloptions(&["autovacuum_enabled=on".into()], Q).unwrap();
        assert_eq!(s.autovacuum.enabled, Some(true));
    }
}
```

- [ ] **Step 2: Wire `pub(crate) mod reloptions;` into `crates/pgevolve-core/src/catalog/mod.rs`**

### Task 4.2: Extend tables/MV catalog queries to populate `storage`

Most table catalog queries already select `c.reloptions::text[]` (used for view reloptions). Confirm by reading `catalog/queries/shared.rs` and `pg14.rs`:

```bash
grep -n "reloptions" crates/pgevolve-core/src/catalog/queries/*.rs
```

If the column isn't already in TABLES_QUERY, add it. Then in `crates/pgevolve-core/src/catalog/assemble/tables.rs::build_tables` (where Stage 1 set `storage: Default::default()` as a placeholder), decode the column:

```rust
let reloptions: Vec<String> = row.get_text_array(q, "reloptions")?;
let storage = crate::catalog::reloptions::decode_table_reloptions(&reloptions, q)?;
```

Then assign `storage` in the Table literal (replacing the Stage 1 `Default::default()` placeholder).

Same for materialized views (their assembler likely lives at the same path or in a `views.rs` file).

### Task 4.3: Extend INDEXES_QUERY + indexes assembler

- [ ] **Step 1: Add `reloptions` column to INDEXES_QUERY in both `shared.rs` and `pg14.rs`**

Add `coalesce(c.reloptions, '{}'::text[]) AS reloptions,` near the other column selections (after `relname` / `indrelid` / wherever it fits in the existing query).

- [ ] **Step 2: Decode in the indexes assembler**

Find the indexes assembler (likely `catalog/assemble/indexes.rs`; if not, find via `grep -rn "build_index\|fn build_indexes" crates/pgevolve-core/src/catalog/assemble/`):

```rust
let reloptions: Vec<String> = row.get_text_array(q, "reloptions")?;
let storage = crate::catalog::reloptions::decode_index_reloptions(&reloptions, q)?;
```

Assign in the Index literal.

### Task 4.4: Docker-gated integration test

- [ ] **Step 1: Create `crates/pgevolve-core/tests/catalog_reloptions.rs`**

Mirror the v0.3.1 / v0.3.2 Docker test pattern (`docker_available()` runtime check + `ephemeral_pg()`).

```rust
#[tokio::test]
async fn reads_table_fillfactor() {
    if !docker_available() { return; }
    let pg = ephemeral_pg().await;
    pg.exec("CREATE SCHEMA app").await;
    pg.exec("CREATE TABLE app.t (id bigint) WITH (fillfactor = 80)").await;
    let cat = read_catalog(pg.querier(), &["app".to_string()]).await.unwrap();
    let t = cat.tables.iter().find(|t| t.qname.name.as_str() == "t").unwrap();
    assert_eq!(t.storage.fillfactor, Some(80));
}

#[tokio::test]
async fn reads_autovacuum_disabled() {
    if !docker_available() { return; }
    let pg = ephemeral_pg().await;
    pg.exec("CREATE SCHEMA app").await;
    pg.exec("CREATE TABLE app.t (id bigint) WITH (autovacuum_enabled = false)").await;
    let cat = read_catalog(pg.querier(), &["app".to_string()]).await.unwrap();
    let t = cat.tables.iter().find(|t| t.qname.name.as_str() == "t").unwrap();
    assert_eq!(t.storage.autovacuum.enabled, Some(false));
}

#[tokio::test]
async fn reads_index_fillfactor() {
    if !docker_available() { return; }
    let pg = ephemeral_pg().await;
    pg.exec("CREATE SCHEMA app").await;
    pg.exec("CREATE TABLE app.t (id bigint)").await;
    pg.exec("CREATE INDEX i ON app.t (id) WITH (fillfactor = 70)").await;
    let cat = read_catalog(pg.querier(), &["app".to_string()]).await.unwrap();
    let i = cat.indexes.iter().find(|i| i.name.as_str() == "i").unwrap();
    assert_eq!(i.storage.fillfactor, Some(70));
}

#[tokio::test]
async fn reads_gin_fastupdate() {
    if !docker_available() { return; }
    let pg = ephemeral_pg().await;
    pg.exec("CREATE EXTENSION btree_gin").await;
    pg.exec("CREATE SCHEMA app").await;
    pg.exec("CREATE TABLE app.t (id bigint)").await;
    pg.exec("CREATE INDEX i ON app.t USING gin (id) WITH (fastupdate = false)").await;
    let cat = read_catalog(pg.querier(), &["app".to_string()]).await.unwrap();
    let i = cat.indexes.iter().find(|i| i.name.as_str() == "i").unwrap();
    assert_eq!(i.storage.fastupdate, Some(false));
}
```

### Task 4.5: Run + commit

```bash
cargo test -p pgevolve-core --lib catalog::reloptions
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
# Docker:
cargo test -p pgevolve-core --tests catalog_reloptions
git add -p crates/pgevolve-core/src/catalog/ crates/pgevolve-core/tests/
git commit -m "$(cat <<'EOF'
feat(catalog): decode pg_class.reloptions into typed storage

New catalog::reloptions module decodes pg_class.reloptions text[]
into TableStorageOptions / IndexStorageOptions. Known keys populate
typed fields; unknown keys flow into the extra bag (no per-AM
validation reader-side — that's parser territory).

NaN values rejected. Bool values accept on/off/true/false/1/0.

Tables/MVs queries already selected reloptions for view-side use;
extend the assembler to also assign on Table/MaterializedView.
INDEXES_QUERY gains the column.

Stage 4 of docs/superpowers/plans/2026-05-22-table-reloptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 5 — Source parser

`WITH (...)` decoder shared across `CREATE TABLE`, `CREATE INDEX`, `CREATE MATERIALIZED VIEW`. `ALTER ... SET (...)` handler. `RESET (...)` rejected.

**Files created:** `crates/pgevolve-core/src/parse/builder/reloptions.rs`.
**Files modified:** `parse/builder/{create_stmt, create_index_stmt, create_materialized_view_stmt, alter_table_stmt}.rs` (find actual paths — names may differ slightly).

### Task 5.1: Shared `WITH (...)` decoder

- [ ] **Step 1: Create `crates/pgevolve-core/src/parse/builder/reloptions.rs`**

```rust
//! Decode `WITH (key = value, ...)` reloption clauses from `DefElem` nodes.

use crate::ir::reloptions::{
    AutovacuumOptions, BufferingMode, IndexStorageOptions, NotNanF64,
    TableStorageOptions,
};
use crate::parse::error::{ParseError, SourceLocation};

/// Decode reloption clauses for a table or materialized view.
pub(crate) fn decode_table_options(
    options: &[pg_query::protobuf::Node],
    loc: &SourceLocation,
) -> Result<TableStorageOptions, ParseError> {
    let mut out = TableStorageOptions::default();
    for opt_node in options {
        let Some(pg_query::NodeEnum::DefElem(def)) = opt_node.node.as_ref() else {
            continue;
        };
        if assign_autovacuum(&mut out.autovacuum, def, loc)? {
            continue;
        }
        let key = def.defname.as_str();
        let value = extract_value(def, loc)?;
        match key {
            "fillfactor" => {
                let n = parse_u32(&value, key, loc)?;
                validate_range(n, 10..=100, "fillfactor (table)", loc)?;
                out.fillfactor = Some(n);
            }
            "parallel_workers" => {
                let n = parse_u32(&value, key, loc)?;
                validate_range(n, 0..=1024, "parallel_workers", loc)?;
                out.parallel_workers = Some(n);
            }
            "toast_tuple_target" => {
                let n = parse_u32(&value, key, loc)?;
                validate_range(n, 128..=8160, "toast_tuple_target", loc)?;
                out.toast_tuple_target = Some(n);
            }
            "user_catalog_table" => out.user_catalog_table = Some(parse_bool(&value, key, loc)?),
            "vacuum_truncate" => out.vacuum_truncate = Some(parse_bool(&value, key, loc)?),
            _ => {
                out.extra.insert(key.to_owned(), value);
            }
        }
    }
    Ok(out)
}

/// Decode reloption clauses for an index. `access_method` is the `USING ...`
/// clause from the surrounding CreateIndexStmt; needed for fillfactor range
/// validation (B-tree 50–100, GiST 10–100, SP-GiST 90–100, etc.).
pub(crate) fn decode_index_options(
    options: &[pg_query::protobuf::Node],
    access_method: &str,
    loc: &SourceLocation,
) -> Result<IndexStorageOptions, ParseError> {
    let mut out = IndexStorageOptions::default();
    for opt_node in options {
        let Some(pg_query::NodeEnum::DefElem(def)) = opt_node.node.as_ref() else {
            continue;
        };
        let key = def.defname.as_str();
        let value = extract_value(def, loc)?;
        match key {
            "fillfactor" => {
                let n = parse_u32(&value, key, loc)?;
                validate_index_fillfactor(n, access_method, loc)?;
                out.fillfactor = Some(n);
            }
            "fastupdate" => out.fastupdate = Some(parse_bool(&value, key, loc)?),
            "gin_pending_list_limit" => {
                out.gin_pending_list_limit = Some(parse_u64(&value, key, loc)?);
            }
            "buffering" => {
                out.buffering = Some(BufferingMode::from_str(&value).ok_or_else(|| {
                    ParseError::Structural {
                        location: loc.clone(),
                        message: format!("buffering value {value:?} invalid; expected on/off/auto"),
                    }
                })?);
            }
            "deduplicate_items" => out.deduplicate_items = Some(parse_bool(&value, key, loc)?),
            "pages_per_range" => {
                let n = parse_u32(&value, key, loc)?;
                validate_range(n, 1..=131072, "pages_per_range", loc)?;
                out.pages_per_range = Some(n);
            }
            "autosummarize" => out.autosummarize = Some(parse_bool(&value, key, loc)?),
            _ => {
                out.extra.insert(key.to_owned(), value);
            }
        }
    }
    Ok(out)
}

fn validate_index_fillfactor(
    n: u32,
    method: &str,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    let valid_range = match method.to_ascii_lowercase().as_str() {
        "btree" => 50..=100,
        "gist" => 10..=100,
        "hash" => 10..=100,
        "spgist" => 90..=100,
        "brin" | "gin" => {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!("fillfactor is not supported for {method} indexes"),
            });
        }
        _ => 10..=100, // unknown method — accept any sensible range; PG will reject if invalid
    };
    validate_range(n, valid_range, &format!("fillfactor ({method} index)"), loc)
}

fn validate_range(
    n: u32,
    range: std::ops::RangeInclusive<u32>,
    label: &str,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    if !range.contains(&n) {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "{label} = {n} out of range; valid: {}..={}",
                range.start(), range.end()
            ),
        });
    }
    Ok(())
}

fn assign_autovacuum(
    out: &mut AutovacuumOptions,
    def: &pg_query::protobuf::DefElem,
    loc: &SourceLocation,
) -> Result<bool, ParseError> {
    let key = def.defname.as_str();
    let value = extract_value(def, loc)?;
    match key {
        "autovacuum_enabled" => out.enabled = Some(parse_bool(&value, key, loc)?),
        "autovacuum_vacuum_threshold" => out.vacuum_threshold = Some(parse_u64(&value, key, loc)?),
        "autovacuum_vacuum_scale_factor" => out.vacuum_scale_factor = Some(parse_notnan(&value, key, loc)?),
        "autovacuum_vacuum_cost_delay" => out.vacuum_cost_delay = Some(parse_u64(&value, key, loc)?),
        "autovacuum_vacuum_cost_limit" => out.vacuum_cost_limit = Some(parse_u64(&value, key, loc)?),
        "autovacuum_analyze_threshold" => out.analyze_threshold = Some(parse_u64(&value, key, loc)?),
        "autovacuum_analyze_scale_factor" => out.analyze_scale_factor = Some(parse_notnan(&value, key, loc)?),
        "autovacuum_freeze_max_age" => out.freeze_max_age = Some(parse_u64(&value, key, loc)?),
        "autovacuum_freeze_min_age" => out.freeze_min_age = Some(parse_u64(&value, key, loc)?),
        "autovacuum_freeze_table_age" => out.freeze_table_age = Some(parse_u64(&value, key, loc)?),
        "autovacuum_multixact_freeze_max_age" => out.multixact_freeze_max_age = Some(parse_u64(&value, key, loc)?),
        "autovacuum_multixact_freeze_min_age" => out.multixact_freeze_min_age = Some(parse_u64(&value, key, loc)?),
        "autovacuum_multixact_freeze_table_age" => out.multixact_freeze_table_age = Some(parse_u64(&value, key, loc)?),
        "autovacuum_vacuum_insert_threshold" => out.vacuum_insert_threshold = Some(parse_u64(&value, key, loc)?),
        "autovacuum_vacuum_insert_scale_factor" => out.vacuum_insert_scale_factor = Some(parse_notnan(&value, key, loc)?),
        "log_autovacuum_min_duration" => out.log_min_duration = Some(parse_i64(&value, key, loc)?),
        _ => return Ok(false),
    }
    Ok(true)
}

fn extract_value(
    def: &pg_query::protobuf::DefElem,
    loc: &SourceLocation,
) -> Result<String, ParseError> {
    let Some(arg) = def.arg.as_ref().and_then(|n| n.node.as_ref()) else {
        // Some boolean reloptions are declared bareword (`WITH (autovacuum_enabled)`)
        // meaning true. PG accepts this; we treat absence-of-arg as "true".
        return Ok("true".to_string());
    };
    match arg {
        pg_query::NodeEnum::Integer(i) => Ok(i.ival.to_string()),
        pg_query::NodeEnum::Float(f) => Ok(f.fval.clone()),
        pg_query::NodeEnum::String(s) => Ok(s.sval.clone()),
        pg_query::NodeEnum::Boolean(b) => Ok(if b.boolval { "true".into() } else { "false".into() }),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("reloption {}: unexpected value node {other:?}", def.defname),
        }),
    }
}

fn parse_u32(v: &str, key: &str, loc: &SourceLocation) -> Result<u32, ParseError> {
    v.parse().map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })
}

fn parse_u64(v: &str, key: &str, loc: &SourceLocation) -> Result<u64, ParseError> {
    v.parse().map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })
}

fn parse_i64(v: &str, key: &str, loc: &SourceLocation) -> Result<i64, ParseError> {
    v.parse().map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })
}

fn parse_bool(v: &str, key: &str, loc: &SourceLocation) -> Result<bool, ParseError> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "on" | "1" => Ok(true),
        "false" | "off" | "0" => Ok(false),
        _ => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("reloption {key} = {v:?} not a recognized bool"),
        }),
    }
}

fn parse_notnan(v: &str, key: &str, loc: &SourceLocation) -> Result<NotNanF64, ParseError> {
    let f: f64 = v.parse().map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })?;
    NotNanF64::new(f).map_err(|_| ParseError::Structural {
        location: loc.clone(),
        message: format!("reloption {key} value is NaN"),
    })
}
```

Add unit tests covering: fillfactor in range, fillfactor out of range, B-tree fillfactor < 50 errors, GIN/BRIN fillfactor errors entirely, autovacuum_enabled bool, unknown key flows into extra, malformed bool errors, NaN rejected.

- [ ] **Step 2: Wire into `parse/builder/mod.rs`**

```rust
pub mod reloptions;
```

### Task 5.2: Wire decoder into CREATE statements

- [ ] **Step 1: CREATE TABLE — extend `create_stmt.rs`**

In the existing `build_table` (or wherever `CreateStmt` is consumed), after the columns/constraints are decoded, decode `s.options`:

```rust
let storage = crate::parse::builder::reloptions::decode_table_options(&s.options, location)?;
```

And populate the Table literal's `storage` field (replacing the Stage 2 `Default::default()` placeholder).

- [ ] **Step 2: CREATE INDEX — extend the index parser**

Find the `IndexStmt` handler (search `grep -rn "IndexStmt" crates/pgevolve-core/src/parse/builder/`). Access method comes from `s.access_method` (verify name in pg_query). Decode:

```rust
let storage = crate::parse::builder::reloptions::decode_index_options(
    &s.options, &s.access_method, location,
)?;
```

Populate the Index literal.

- [ ] **Step 3: CREATE MATERIALIZED VIEW — extend the MV parser**

Find the `CreateTableAsStmt` handler (PG models CREATE MATERIALIZED VIEW as a variant of CreateTableAs). The options live on `s.into.options` (verify path). Decode using `decode_table_options` (MVs use the same key set as tables).

### Task 5.3: ALTER TABLE/INDEX/MV SET (...) — extend `alter_table_stmt.rs`

In the existing `alter_table_stmt.rs` (which already handles `AT_ChangeOwner`, RLS toggles, etc.):

```rust
AlterTableType::AtSetRelOptions => {
    let opts = crate::parse::builder::reloptions::decode_table_options(&cmd.def_list, &loc)?;
    // Merge opts into the existing Table.storage. Same pending-action pattern
    // as PendingOwner from v0.3.1.
}
AlterTableType::AtResetRelOptions => {
    return Err(ParseError::Structural {
        location: loc.clone(),
        message: "ALTER TABLE ... RESET (...) in source is not supported — \
                  clear options out-of-band, then remove from source".into(),
    });
}
AlterTableType::AtReplaceRelOptions => {
    return Err(ParseError::Structural {
        location: loc.clone(),
        message: "ALTER TABLE ... RESET () in source is not supported".into(),
    });
}
```

Verify the `AlterTableType` variant names against pg_query bindings. For indexes and MVs, `ALTER INDEX SET (...)` and `ALTER MATERIALIZED VIEW SET (...)` use `AlterTableStmt` with `relkind` discriminating — handle uniformly through the same dispatch (the existing alter-table handler already does this for ALTER VIEW).

If pgevolve has separate alter-index / alter-materialized-view parsers, extend those analogously.

### Task 5.4: Tests

Add ~10 tests across the parser:

- `create_table_with_fillfactor` — fillfactor populates.
- `create_table_fillfactor_out_of_range_errors` — fillfactor = 200 → ParseError.
- `create_table_autovacuum_disabled` — autovacuum.enabled = Some(false).
- `create_table_unknown_extra_key` — pg_partman.foo flows into extra.
- `create_index_btree_fillfactor_too_low_errors` — fillfactor = 40 → ParseError.
- `create_index_gist_fillfactor_in_range` — fillfactor = 80 works.
- `create_index_brin_fillfactor_errors` — BRIN doesn't support fillfactor.
- `create_index_gin_fastupdate` — fastupdate = false works.
- `create_mv_inherits_table_keys` — autovacuum_enabled on MV.
- `alter_table_set_reloption` — ALTER … SET (fillfactor = 80) populates.
- `alter_table_reset_reloption_errors` — RESET rejected.

### Task 5.5: Run + commit

```bash
cargo test -p pgevolve-core --lib parse::builder::reloptions
cargo test -p pgevolve-core --lib parse
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add -p crates/pgevolve-core/src/parse/
git commit -m "$(cat <<'EOF'
feat(parse): WITH (...) reloptions + ALTER SET on Table/Index/MV

Shared parse::builder::reloptions decoder handles DefElem lists from:
  CREATE TABLE ... WITH (...)
  CREATE INDEX ... WITH (...)
  CREATE MATERIALIZED VIEW ... WITH (...)
  ALTER TABLE/INDEX/MATERIALIZED VIEW ... SET (...)

Per-AM fillfactor validation enforces PG's stricter ranges:
  Tables/MVs: 10..=100
  B-tree:     50..=100
  GiST/Hash:  10..=100
  SP-GiST:    90..=100
  BRIN/GIN:   not supported (parse error)

ALTER ... RESET (...) rejected in source — the lenient differ
policy means RESET in source has no semantics. Operators clear
options out-of-band.

Stage 5 of docs/superpowers/plans/2026-05-22-table-reloptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 6 — Differ

Sparse-delta diff per relkind. 3 new Change variants. No `Reset*` per spec (lenient policy).

**Files created:** `crates/pgevolve-core/src/diff/reloptions.rs`.
**Files modified:** `crates/pgevolve-core/src/diff/{change.rs, tables.rs, indexes.rs, views.rs, mod.rs}`.

### Task 6.1: Add 3 Change variants

- [ ] **Step 1: Extend `crates/pgevolve-core/src/diff/change.rs`**

```rust
    /// Set table reloptions. `options` carries the sparse delta — only the
    /// fields whose source value differs from the catalog.
    SetTableStorage {
        qname: QualifiedName,
        options: crate::ir::reloptions::TableStorageOptions,
    },
    /// Set index reloptions. Sparse delta.
    SetIndexStorage {
        qname: QualifiedName,
        options: crate::ir::reloptions::IndexStorageOptions,
    },
    /// Set materialized view reloptions. Sparse delta.
    SetMaterializedViewStorage {
        qname: QualifiedName,
        options: crate::ir::reloptions::TableStorageOptions,
    },
```

### Task 6.2: Sparse-delta computation

- [ ] **Step 1: Create `crates/pgevolve-core/src/diff/reloptions.rs`**

```rust
//! Sparse-delta diffing for storage reloptions. Lenient policy:
//! source `None` always means "skip"; catalog values not in source surface
//! as `unmanaged-reloption` lint, never RESET.

use crate::ir::reloptions::{
    AutovacuumOptions, IndexStorageOptions, TableStorageOptions,
};

/// Build the sparse delta — only the fields where source is `Some(_)` AND
/// catalog disagrees. Used by Stage 6's per-relkind diff functions.
#[must_use]
pub fn table_delta(target: &TableStorageOptions, source: &TableStorageOptions) -> TableStorageOptions {
    let mut out = TableStorageOptions::default();

    macro_rules! diff_field {
        ($field:ident) => {
            if let Some(src) = &source.$field {
                if target.$field.as_ref() != Some(src) {
                    out.$field = Some(src.clone());
                }
            }
        };
    }

    diff_field!(fillfactor);
    diff_field!(parallel_workers);
    diff_field!(toast_tuple_target);
    diff_field!(user_catalog_table);
    diff_field!(vacuum_truncate);

    out.autovacuum = autovacuum_delta(&target.autovacuum, &source.autovacuum);

    // Extra bag: only keys present in source and (missing-or-different) in catalog.
    for (k, src_v) in &source.extra {
        if target.extra.get(k) != Some(src_v) {
            out.extra.insert(k.clone(), src_v.clone());
        }
    }

    out
}

#[must_use]
pub fn index_delta(target: &IndexStorageOptions, source: &IndexStorageOptions) -> IndexStorageOptions {
    let mut out = IndexStorageOptions::default();

    macro_rules! diff_field {
        ($field:ident) => {
            if let Some(src) = &source.$field {
                if target.$field.as_ref() != Some(src) {
                    out.$field = Some(src.clone());
                }
            }
        };
    }

    diff_field!(fillfactor);
    diff_field!(fastupdate);
    diff_field!(gin_pending_list_limit);
    diff_field!(buffering);
    diff_field!(deduplicate_items);
    diff_field!(pages_per_range);
    diff_field!(autosummarize);

    for (k, src_v) in &source.extra {
        if target.extra.get(k) != Some(src_v) {
            out.extra.insert(k.clone(), src_v.clone());
        }
    }

    out
}

fn autovacuum_delta(target: &AutovacuumOptions, source: &AutovacuumOptions) -> AutovacuumOptions {
    let mut out = AutovacuumOptions::default();

    macro_rules! diff_field {
        ($field:ident) => {
            if let Some(src) = &source.$field {
                if target.$field.as_ref() != Some(src) {
                    out.$field = Some(src.clone());
                }
            }
        };
    }

    diff_field!(enabled);
    diff_field!(vacuum_threshold);
    diff_field!(vacuum_scale_factor);
    diff_field!(vacuum_cost_delay);
    diff_field!(vacuum_cost_limit);
    diff_field!(analyze_threshold);
    diff_field!(analyze_scale_factor);
    diff_field!(freeze_max_age);
    diff_field!(freeze_min_age);
    diff_field!(freeze_table_age);
    diff_field!(multixact_freeze_max_age);
    diff_field!(multixact_freeze_min_age);
    diff_field!(multixact_freeze_table_age);
    diff_field!(vacuum_insert_threshold);
    diff_field!(vacuum_insert_scale_factor);
    diff_field!(log_min_duration);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_source_yields_empty_delta() {
        let t = TableStorageOptions { fillfactor: Some(80), ..Default::default() };
        let s = TableStorageOptions::default();
        let delta = table_delta(&t, &s);
        assert!(delta.is_empty(), "lenient: source None means skip");
    }

    #[test]
    fn source_only_fillfactor_emits_one_field() {
        let t = TableStorageOptions::default();
        let s = TableStorageOptions { fillfactor: Some(80), ..Default::default() };
        let delta = table_delta(&t, &s);
        assert_eq!(delta.fillfactor, Some(80));
        assert!(delta.autovacuum.is_empty());
    }

    #[test]
    fn matching_source_and_target_yields_empty_delta() {
        let t = TableStorageOptions { fillfactor: Some(80), ..Default::default() };
        let s = TableStorageOptions { fillfactor: Some(80), ..Default::default() };
        let delta = table_delta(&t, &s);
        assert!(delta.is_empty());
    }

    #[test]
    fn source_extra_key_not_in_target_emits() {
        let t = TableStorageOptions::default();
        let mut s = TableStorageOptions::default();
        s.extra.insert("pg_partman.foo".into(), "value".into());
        let delta = table_delta(&t, &s);
        assert_eq!(delta.extra.get("pg_partman.foo").map(String::as_str), Some("value"));
    }

    #[test]
    fn target_extra_key_not_in_source_does_not_emit() {
        let mut t = TableStorageOptions::default();
        t.extra.insert("pg_partman.foo".into(), "value".into());
        let s = TableStorageOptions::default();
        let delta = table_delta(&t, &s);
        assert!(delta.is_empty(), "lenient: unmanaged extra-bag keys ignored");
    }

    #[test]
    fn index_delta_fillfactor_change() {
        let t = IndexStorageOptions { fillfactor: Some(70), ..Default::default() };
        let s = IndexStorageOptions { fillfactor: Some(80), ..Default::default() };
        let delta = index_delta(&t, &s);
        assert_eq!(delta.fillfactor, Some(80));
    }
}
```

### Task 6.3: Wire into per-relkind diffs

- [ ] **Step 1: Tables diff (`diff/tables.rs`)**

At the end of the per-table diff function (after existing columns/constraints/grants/owner/policies work):

```rust
    let delta = crate::diff::reloptions::table_delta(&target.storage, &source.storage);
    if !delta.is_empty() {
        out.push(Change::SetTableStorage {
            qname: source.qname.clone(),
            options: delta,
        });
    }
```

- [ ] **Step 2: Indexes diff (`diff/indexes.rs`)**

Same pattern, calling `index_delta` and pushing `SetIndexStorage`.

- [ ] **Step 3: Views diff (`diff/views.rs`)** — only for `MaterializedView`, not regular views

```rust
    let delta = crate::diff::reloptions::table_delta(&target.storage, &source.storage);
    if !delta.is_empty() {
        out.push(Change::SetMaterializedViewStorage {
            qname: source.qname.clone(),
            options: delta,
        });
    }
```

### Task 6.4: Wire mod.rs + stub emit handlers

- [ ] **Step 1: `pub mod reloptions;` in `diff/mod.rs`**

- [ ] **Step 2: Stub emit arms in `plan/rewrite/mod.rs`**

Stage 7 fills in the real SQL. For now, stub arms so the workspace compiles:

```rust
// Stage 7 wires these into real SQL.
Change::SetTableStorage { .. }
| Change::SetIndexStorage { .. }
| Change::SetMaterializedViewStorage { .. } => {
    // no-op for Stage 6; Stage 7 emits ALTER ... SET (...) statements
}
```

Also stub `change_kind_name` in `plan/plan.rs` if its match is exhaustive, and any other Change consumers (CLI display, ordering).

### Task 6.5: Run + commit

```bash
cargo test -p pgevolve-core --lib diff::reloptions
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add -p crates/pgevolve-core/src/diff/ crates/pgevolve-core/src/plan/
git commit -m "$(cat <<'EOF'
feat(diff): sparse-delta reloption diffing for Table/Index/MV

New diff::reloptions module computes sparse deltas: only the fields
where source is Some(_) AND target disagrees flow into the Change.

Three new Change variants (Set* only — no Reset* because the lenient
policy makes source-removal a no-op). Per-relkind diff functions
push SetTableStorage / SetIndexStorage / SetMaterializedViewStorage
when the delta is non-empty.

Stage 7 emit stubs in plan/rewrite/mod.rs let the workspace compile.

Stage 6 of docs/superpowers/plans/2026-05-22-table-reloptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 7 — Render + emit + 3 new StepKinds

**Files created:** `crates/pgevolve-core/src/plan/rewrite/reloptions.rs`.
**Files modified:** `crates/pgevolve-core/src/plan/raw_step.rs`, `crates/pgevolve-core/src/plan/rewrite/mod.rs`, `crates/pgevolve-core/src/plan/plan.rs`, `crates/pgevolve/src/commands/diff.rs`.

### Task 7.1: StepKind variants

- [ ] **Step 1: Extend `crates/pgevolve-core/src/plan/raw_step.rs::StepKind`**

```rust
    SetTableStorage,
    SetIndexStorage,
    SetMaterializedViewStorage,
```

Extend the round-trip serialization test (every variant must appear).

- [ ] **Step 2: Extend `kind_name` / `parse_kind_name` in `crates/pgevolve-core/src/plan/plan.rs`**

Add mappings: `"set_table_storage"`, `"set_index_storage"`, `"set_materialized_view_storage"`.

### Task 7.2: SQL helpers

- [ ] **Step 1: Create `crates/pgevolve-core/src/plan/rewrite/reloptions.rs`**

```rust
//! SQL rendering for reloption SET statements.

use crate::identifier::QualifiedName;
use crate::ir::reloptions::{
    AutovacuumOptions, BufferingMode, IndexStorageOptions, NotNanF64,
    TableStorageOptions,
};

/// `ALTER TABLE qname SET (key = value, ...);`
#[must_use]
pub fn alter_table_set_storage(qname: &QualifiedName, opts: &TableStorageOptions) -> String {
    format!("ALTER TABLE {} SET ({});", qname.render_sql(), render_table_options(opts))
}

/// `ALTER INDEX qname SET (key = value, ...);`
#[must_use]
pub fn alter_index_set_storage(qname: &QualifiedName, opts: &IndexStorageOptions) -> String {
    format!("ALTER INDEX {} SET ({});", qname.render_sql(), render_index_options(opts))
}

/// `ALTER MATERIALIZED VIEW qname SET (key = value, ...);`
#[must_use]
pub fn alter_mv_set_storage(qname: &QualifiedName, opts: &TableStorageOptions) -> String {
    format!("ALTER MATERIALIZED VIEW {} SET ({});", qname.render_sql(), render_table_options(opts))
}

fn render_table_options(opts: &TableStorageOptions) -> String {
    let mut parts = Vec::new();
    if let Some(v) = opts.fillfactor { parts.push(format!("fillfactor = {v}")); }
    render_autovacuum(&opts.autovacuum, &mut parts);
    if let Some(v) = opts.parallel_workers { parts.push(format!("parallel_workers = {v}")); }
    if let Some(v) = opts.toast_tuple_target { parts.push(format!("toast_tuple_target = {v}")); }
    if let Some(v) = opts.user_catalog_table { parts.push(format!("user_catalog_table = {v}")); }
    if let Some(v) = opts.vacuum_truncate { parts.push(format!("vacuum_truncate = {v}")); }
    for (k, v) in &opts.extra {
        parts.push(format!("{k} = {v}"));
    }
    parts.join(", ")
}

fn render_index_options(opts: &IndexStorageOptions) -> String {
    let mut parts = Vec::new();
    if let Some(v) = opts.fillfactor { parts.push(format!("fillfactor = {v}")); }
    if let Some(v) = opts.fastupdate { parts.push(format!("fastupdate = {v}")); }
    if let Some(v) = opts.gin_pending_list_limit { parts.push(format!("gin_pending_list_limit = {v}")); }
    if let Some(v) = opts.buffering { parts.push(format!("buffering = {}", v.sql_keyword())); }
    if let Some(v) = opts.deduplicate_items { parts.push(format!("deduplicate_items = {v}")); }
    if let Some(v) = opts.pages_per_range { parts.push(format!("pages_per_range = {v}")); }
    if let Some(v) = opts.autosummarize { parts.push(format!("autosummarize = {v}")); }
    for (k, v) in &opts.extra {
        parts.push(format!("{k} = {v}"));
    }
    parts.join(", ")
}

fn render_autovacuum(opts: &AutovacuumOptions, parts: &mut Vec<String>) {
    if let Some(v) = opts.enabled { parts.push(format!("autovacuum_enabled = {v}")); }
    if let Some(v) = opts.vacuum_threshold { parts.push(format!("autovacuum_vacuum_threshold = {v}")); }
    if let Some(v) = opts.vacuum_scale_factor { parts.push(format!("autovacuum_vacuum_scale_factor = {}", render_f64(v))); }
    if let Some(v) = opts.vacuum_cost_delay { parts.push(format!("autovacuum_vacuum_cost_delay = {v}")); }
    if let Some(v) = opts.vacuum_cost_limit { parts.push(format!("autovacuum_vacuum_cost_limit = {v}")); }
    if let Some(v) = opts.analyze_threshold { parts.push(format!("autovacuum_analyze_threshold = {v}")); }
    if let Some(v) = opts.analyze_scale_factor { parts.push(format!("autovacuum_analyze_scale_factor = {}", render_f64(v))); }
    if let Some(v) = opts.freeze_max_age { parts.push(format!("autovacuum_freeze_max_age = {v}")); }
    if let Some(v) = opts.freeze_min_age { parts.push(format!("autovacuum_freeze_min_age = {v}")); }
    if let Some(v) = opts.freeze_table_age { parts.push(format!("autovacuum_freeze_table_age = {v}")); }
    if let Some(v) = opts.multixact_freeze_max_age { parts.push(format!("autovacuum_multixact_freeze_max_age = {v}")); }
    if let Some(v) = opts.multixact_freeze_min_age { parts.push(format!("autovacuum_multixact_freeze_min_age = {v}")); }
    if let Some(v) = opts.multixact_freeze_table_age { parts.push(format!("autovacuum_multixact_freeze_table_age = {v}")); }
    if let Some(v) = opts.vacuum_insert_threshold { parts.push(format!("autovacuum_vacuum_insert_threshold = {v}")); }
    if let Some(v) = opts.vacuum_insert_scale_factor { parts.push(format!("autovacuum_vacuum_insert_scale_factor = {}", render_f64(v))); }
    if let Some(v) = opts.log_min_duration { parts.push(format!("log_autovacuum_min_duration = {v}")); }
}

fn render_f64(v: NotNanF64) -> String {
    // PG accepts both 0.05 and 5e-2; render the most readable form.
    // Avoid trailing zeros, but always show at least one decimal place
    // so PG doesn't interpret as an integer.
    let f = v.get();
    if f == f.trunc() {
        format!("{f}.0")
    } else {
        format!("{f}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(
            Identifier::from_unquoted(schema).unwrap(),
            Identifier::from_unquoted(name).unwrap(),
        )
    }

    #[test]
    fn renders_alter_table_fillfactor() {
        let opts = TableStorageOptions { fillfactor: Some(80), ..Default::default() };
        let sql = alter_table_set_storage(&qn("app", "t"), &opts);
        assert_eq!(sql, "ALTER TABLE app.t SET (fillfactor = 80);");
    }

    #[test]
    fn renders_alter_table_multiple_keys() {
        let mut opts = TableStorageOptions { fillfactor: Some(80), ..Default::default() };
        opts.autovacuum.enabled = Some(false);
        let sql = alter_table_set_storage(&qn("app", "t"), &opts);
        assert!(sql.contains("fillfactor = 80"));
        assert!(sql.contains("autovacuum_enabled = false"));
    }

    #[test]
    fn renders_alter_index_buffering() {
        let opts = IndexStorageOptions { buffering: Some(BufferingMode::Auto), ..Default::default() };
        let sql = alter_index_set_storage(&qn("app", "i"), &opts);
        assert_eq!(sql, "ALTER INDEX app.i SET (buffering = auto);");
    }

    #[test]
    fn renders_alter_mv_autovacuum() {
        let mut opts = TableStorageOptions::default();
        opts.autovacuum.enabled = Some(false);
        let sql = alter_mv_set_storage(&qn("app", "m"), &opts);
        assert_eq!(sql, "ALTER MATERIALIZED VIEW app.m SET (autovacuum_enabled = false);");
    }

    #[test]
    fn f64_with_integer_value_renders_with_decimal() {
        let v = NotNanF64::new(5.0).unwrap();
        assert_eq!(render_f64(v), "5.0");
    }

    #[test]
    fn f64_with_decimal_renders_compactly() {
        let v = NotNanF64::new(0.05).unwrap();
        assert_eq!(render_f64(v), "0.05");
    }

    #[test]
    fn extra_bag_keys_rendered() {
        let mut opts = TableStorageOptions::default();
        opts.extra.insert("pg_partman.foo".into(), "bar".into());
        let sql = alter_table_set_storage(&qn("app", "t"), &opts);
        assert!(sql.contains("pg_partman.foo = bar"));
    }
}
```

### Task 7.3: Replace Stage 6 stub emit arms

- [ ] **Step 1: Update `plan/rewrite/mod.rs`**

Replace the stub:

```rust
Change::SetTableStorage { qname, options } => {
    raw_steps.push(RawStep {
        step_no: 0,
        kind: StepKind::SetTableStorage,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![qname.clone()],
        sql: reloptions::alter_table_set_storage(qname, options),
        transactional: TransactionConstraint::InTransaction,
    });
}
Change::SetIndexStorage { qname, options } => {
    raw_steps.push(RawStep {
        step_no: 0,
        kind: StepKind::SetIndexStorage,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![qname.clone()],
        sql: reloptions::alter_index_set_storage(qname, options),
        transactional: TransactionConstraint::InTransaction,
    });
}
Change::SetMaterializedViewStorage { qname, options } => {
    raw_steps.push(RawStep {
        step_no: 0,
        kind: StepKind::SetMaterializedViewStorage,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![qname.clone()],
        sql: reloptions::alter_mv_set_storage(qname, options),
        transactional: TransactionConstraint::InTransaction,
    });
}
```

Add `pub mod reloptions;` to `plan/rewrite/mod.rs`.

### Task 7.4: Update CLI display

In `crates/pgevolve/src/commands/diff.rs`, replace stub arms:

```rust
Change::SetTableStorage { qname, .. } => format!("~ ALTER TABLE {qname} SET (...)"),
Change::SetIndexStorage { qname, .. } => format!("~ ALTER INDEX {qname} SET (...)"),
Change::SetMaterializedViewStorage { qname, .. } => format!("~ ALTER MATERIALIZED VIEW {qname} SET (...)"),
```

Update `change_kind_name` accordingly.

### Task 7.5: Run + commit

```bash
cargo test -p pgevolve-core --lib plan
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add -p crates/pgevolve-core/src/plan/ crates/pgevolve/src/commands/
git commit -m "$(cat <<'EOF'
feat(plan): reloptions render + emit — 3 new StepKinds

plan::rewrite::reloptions renders ALTER TABLE/INDEX/MATERIALIZED
VIEW ... SET (...) statements. One step per relkind per diff (PG
accepts multiple options per SET, so batching is cheaper).

3 new StepKind variants: SetTableStorage, SetIndexStorage,
SetMaterializedViewStorage. All InTransaction, non-destructive.

f64 reloption values always render with at least one decimal place
(5.0 not 5) so PG doesn't reinterpret as integer.

Stage 7 of docs/superpowers/plans/2026-05-22-table-reloptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 8 — `unmanaged-reloption` lint

**Files created:** `crates/pgevolve-core/src/lint/rules/unmanaged_reloption.rs`.
**Files modified:** `crates/pgevolve-core/src/lint/rules/mod.rs`, `crates/pgevolve-core/src/lint/universal.rs`.

### Task 8.1: Implement the rule

- [ ] **Step 1: Create `crates/pgevolve-core/src/lint/rules/unmanaged_reloption.rs`**

```rust
//! Warns when the catalog has reloptions not declared in source.
//!
//! Per the lenient drift policy in `diff::reloptions`, catalog reloptions
//! that don't appear in source (or appear with different values that source
//! doesn't override) are NOT reset by the differ. This lint surfaces them
//! so operators can decide whether to bring under management or accept
//! the drift.

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "unmanaged-reloption";

pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    let mut findings = Vec::new();

    // Pair source and target tables by qname.
    for src_t in &source.tables {
        let Some(tgt_t) = target.tables.iter().find(|t| t.qname == src_t.qname) else {
            continue;
        };
        check_table_storage(&src_t.qname, &src_t.storage, &tgt_t.storage, &mut findings);
    }

    // Same for indexes.
    for src_i in &source.indexes {
        // Adapt to Index's identifier (likely qname or name).
        let Some(tgt_i) = target.indexes.iter().find(|i| i.name == src_i.name) else {
            continue;
        };
        check_index_storage(&src_i.name, &src_i.storage, &tgt_i.storage, &mut findings);
    }

    // Same for materialized views.
    for src_m in &source.materialized_views {
        let Some(tgt_m) = target.materialized_views.iter().find(|m| m.qname == src_m.qname) else {
            continue;
        };
        check_table_storage(&src_m.qname, &src_m.storage, &tgt_m.storage, &mut findings);
    }

    findings
}

fn check_table_storage(
    qname: &crate::identifier::QualifiedName,
    src: &crate::ir::reloptions::TableStorageOptions,
    tgt: &crate::ir::reloptions::TableStorageOptions,
    out: &mut Vec<Finding>,
) {
    macro_rules! check_field {
        ($field:ident, $label:literal) => {
            if src.$field.is_none() && tgt.$field.is_some() {
                out.push(Finding {
                    rule: RULE_ID,
                    severity: Severity::Warning,
                    message: format!(
                        "table {qname}: catalog has reloption {} not declared in source",
                        $label
                    ),
                    location: None,
                });
            }
        };
    }

    check_field!(fillfactor, "fillfactor");
    check_field!(parallel_workers, "parallel_workers");
    check_field!(toast_tuple_target, "toast_tuple_target");
    check_field!(user_catalog_table, "user_catalog_table");
    check_field!(vacuum_truncate, "vacuum_truncate");

    check_autovacuum(qname, &src.autovacuum, &tgt.autovacuum, out);

    // Extra bag: catalog keys not in source extra.
    for k in tgt.extra.keys() {
        if !src.extra.contains_key(k) {
            out.push(Finding {
                rule: RULE_ID,
                severity: Severity::Warning,
                message: format!(
                    "table {qname}: catalog has reloption {k} not declared in source"
                ),
                location: None,
            });
        }
    }
}

fn check_index_storage(
    name: &crate::identifier::Identifier,
    src: &crate::ir::reloptions::IndexStorageOptions,
    tgt: &crate::ir::reloptions::IndexStorageOptions,
    out: &mut Vec<Finding>,
) {
    macro_rules! check_field {
        ($field:ident, $label:literal) => {
            if src.$field.is_none() && tgt.$field.is_some() {
                out.push(Finding {
                    rule: RULE_ID,
                    severity: Severity::Warning,
                    message: format!(
                        "index {name}: catalog has reloption {} not declared in source",
                        $label
                    ),
                    location: None,
                });
            }
        };
    }
    check_field!(fillfactor, "fillfactor");
    check_field!(fastupdate, "fastupdate");
    check_field!(gin_pending_list_limit, "gin_pending_list_limit");
    check_field!(buffering, "buffering");
    check_field!(deduplicate_items, "deduplicate_items");
    check_field!(pages_per_range, "pages_per_range");
    check_field!(autosummarize, "autosummarize");

    for k in tgt.extra.keys() {
        if !src.extra.contains_key(k) {
            out.push(Finding {
                rule: RULE_ID,
                severity: Severity::Warning,
                message: format!(
                    "index {name}: catalog has reloption {k} not declared in source"
                ),
                location: None,
            });
        }
    }
}

fn check_autovacuum(
    qname: &crate::identifier::QualifiedName,
    src: &crate::ir::reloptions::AutovacuumOptions,
    tgt: &crate::ir::reloptions::AutovacuumOptions,
    out: &mut Vec<Finding>,
) {
    macro_rules! check_field {
        ($field:ident, $label:literal) => {
            if src.$field.is_none() && tgt.$field.is_some() {
                out.push(Finding {
                    rule: RULE_ID,
                    severity: Severity::Warning,
                    message: format!(
                        "table {qname}: catalog has reloption {} not declared in source",
                        $label
                    ),
                    location: None,
                });
            }
        };
    }
    check_field!(enabled, "autovacuum_enabled");
    check_field!(vacuum_threshold, "autovacuum_vacuum_threshold");
    check_field!(vacuum_scale_factor, "autovacuum_vacuum_scale_factor");
    check_field!(vacuum_cost_delay, "autovacuum_vacuum_cost_delay");
    check_field!(vacuum_cost_limit, "autovacuum_vacuum_cost_limit");
    check_field!(analyze_threshold, "autovacuum_analyze_threshold");
    check_field!(analyze_scale_factor, "autovacuum_analyze_scale_factor");
    check_field!(freeze_max_age, "autovacuum_freeze_max_age");
    check_field!(freeze_min_age, "autovacuum_freeze_min_age");
    check_field!(freeze_table_age, "autovacuum_freeze_table_age");
    check_field!(multixact_freeze_max_age, "autovacuum_multixact_freeze_max_age");
    check_field!(multixact_freeze_min_age, "autovacuum_multixact_freeze_min_age");
    check_field!(multixact_freeze_table_age, "autovacuum_multixact_freeze_table_age");
    check_field!(vacuum_insert_threshold, "autovacuum_vacuum_insert_threshold");
    check_field!(vacuum_insert_scale_factor, "autovacuum_vacuum_insert_scale_factor");
    check_field!(log_min_duration, "log_autovacuum_min_duration");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::reloptions::TableStorageOptions;
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn empty_table(qname: QualifiedName) -> Table {
        Table {
            qname,
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: TableStorageOptions::default(),
        }
    }

    #[test]
    fn empty_catalogs_silent() {
        let cat = Catalog::empty();
        assert!(check(&cat, &cat).is_empty());
    }

    #[test]
    fn unmanaged_fillfactor_fires() {
        let mut source = Catalog::empty();
        source.tables.push(empty_table(qn("app", "t")));
        let mut target = Catalog::empty();
        let mut t = empty_table(qn("app", "t"));
        t.storage.fillfactor = Some(80);
        target.tables.push(t);
        let f = check(&source, &target);
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("fillfactor"));
        assert_eq!(f[0].rule, RULE_ID);
        assert_eq!(f[0].severity, Severity::Warning);
    }

    #[test]
    fn matching_storage_silent() {
        let mut source = Catalog::empty();
        let mut s = empty_table(qn("app", "t"));
        s.storage.fillfactor = Some(80);
        source.tables.push(s);
        let mut target = Catalog::empty();
        let mut t = empty_table(qn("app", "t"));
        t.storage.fillfactor = Some(80);
        target.tables.push(t);
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn unmanaged_extra_bag_key_fires() {
        let mut source = Catalog::empty();
        source.tables.push(empty_table(qn("app", "t")));
        let mut target = Catalog::empty();
        let mut t = empty_table(qn("app", "t"));
        t.storage.extra.insert("pg_partman.x".into(), "y".into());
        target.tables.push(t);
        let f = check(&source, &target);
        assert!(f.iter().any(|f| f.message.contains("pg_partman.x")));
    }
}
```

### Task 8.2: Register

- [ ] **Step 1: Add to `lint/rules/mod.rs`**

```rust
pub mod unmanaged_reloption;
```

- [ ] **Step 2: Wire into the lint dispatcher**

`unmanaged-reloption` operates on `(source, target)` Catalogs — it's catalog-pair-level, not source-tree-only or changeset-only. The closest existing dispatcher is whatever runs after drift detection. Read `crates/pgevolve-core/src/lint/universal.rs` to find the right entry point. If a "drift lint" function exists, add the rule there. If not, the rule may need to be invoked directly from `build_plan`'s drift step.

Mirror the wiring for the `check_changeset` family from v0.2.1 Stage 6 + v0.3.2 Stage 8's `check_plan_time_catalog` bridge.

### Task 8.3: Run + commit

```bash
cargo test -p pgevolve-core --lib lint
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add -p crates/pgevolve-core/src/lint/
git commit -m "$(cat <<'EOF'
feat(lint): unmanaged-reloption (warning)

Fires when catalog has typed reloption fields or extra-bag keys
that source doesn't declare. Per the lenient drift policy, the
differ doesn't reset these; the lint surfaces them so operators
can decide whether to bring under management or accept the drift.

Operates on (source, target) Catalogs. Wired into the drift-aware
lint dispatcher.

Stage 8 of docs/superpowers/plans/2026-05-22-table-reloptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 9 — Conformance fixtures (11)

Under `crates/pgevolve-conformance/tests/cases/objects/reloptions/`. All `authoring = "objects"`. Mirror v0.3.x fixture patterns.

### Task 9.1: Create fixtures

For each fixture: directory with `before.sql`, `after.sql`, `fixture.toml`, empty `expected/` (bless populates).

**1. `table-fillfactor/`**

```sql
-- before.sql
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint);

-- after.sql
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint) WITH (fillfactor = 80);
```

`fixture.toml`:
```toml
[meta]
title = "CREATE TABLE WITH (fillfactor = 80)"
authoring = "objects"
spec_refs = ["objects.reloptions.table"]
[pg]
min = 14
max = 17
[expect.plan]
steps = 1
```

**2. `table-autovacuum-disabled/`**

`after.sql` adds: `CREATE TABLE app.t (id bigint) WITH (autovacuum_enabled = false);`

**3. `table-multi-set/`**

```sql
-- after.sql
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint) WITH (fillfactor = 80, autovacuum_enabled = false, parallel_workers = 4);
```

Single SET step with all three keys.

**4. `alter-table-set-after-create/`**

```sql
-- before.sql
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint);

-- after.sql
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint);
ALTER TABLE app.t SET (fillfactor = 80);
```

Single SET step.

**5. `index-fillfactor/`**

```sql
-- after.sql
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint);
CREATE INDEX i ON app.t (id) WITH (fillfactor = 70);
```

**6. `index-gin-fastupdate/`**

```sql
-- after.sql
CREATE SCHEMA app;
CREATE EXTENSION IF NOT EXISTS btree_gin;
CREATE TABLE app.t (id bigint);
CREATE INDEX i ON app.t USING gin (id) WITH (fastupdate = false);
```

**7. `index-brin-pages-per-range/`**

```sql
-- after.sql
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint);
CREATE INDEX i ON app.t USING brin (id) WITH (pages_per_range = 32);
```

**8. `mv-fillfactor/`**

```sql
-- after.sql
CREATE SCHEMA app;
CREATE TABLE app.base (id bigint);
CREATE MATERIALIZED VIEW app.m WITH (fillfactor = 90) AS SELECT * FROM app.base;
```

**9. `partition-inherits-reloptions/`**

Partition table gets its own reloptions. Verifies the spec claim that partitions inherit since they're `Table` in IR.

```sql
-- after.sql
CREATE SCHEMA app;
CREATE TABLE app.parent (id bigint, ts timestamptz) PARTITION BY RANGE (ts);
CREATE TABLE app.child_2026 PARTITION OF app.parent
    FOR VALUES FROM ('2026-01-01') TO ('2027-01-01')
    WITH (fillfactor = 80);
```

**10. `lint/unmanaged-reloption/`**

The conformance harness uses `before.sql` as the **current live state** and `after.sql` as the **desired source**. To trigger `unmanaged-reloption`, the catalog must have a reloption that the source doesn't declare:

```sql
-- before.sql (seeds catalog state)
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint) WITH (fillfactor = 80);
```

```sql
-- after.sql (desired source — fillfactor NOT declared)
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint);
```

`fixture.toml`:
```toml
[meta]
title = "Catalog has fillfactor not declared in source → unmanaged-reloption warning"
authoring = "objects"
spec_refs = ["lint.unmanaged-reloption"]
[pg]
min = 14
max = 17
[expect.plan]
steps = 0
[expect.advisory]
rule_ids = ["unmanaged-reloption"]
```

The differ sees source `fillfactor = None`, catalog `fillfactor = Some(80)` → per lenient policy, no SET emitted → `steps = 0`. The lint fires.

**11. `extra-bag/`**

Use a deliberately-fake key the parser doesn't recognize (the typed-fields set is finite; anything outside lands in `extra`).

```sql
-- after.sql
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint) WITH (fillfactor = 80, "pgevolve.test_extra_key" = 'hello');
```

`pgevolve.test_extra_key` is not a known PG reloption — but **PG itself will reject** unknown keys at apply time. So this fixture can't apply through the live-DB path; it's a parser-only fixture (the conformance harness can verify the parsed IR's `extra` bag without applying to Postgres). If the harness requires every fixture to apply successfully, **skip this fixture and document the gap** — the unit tests in Stage 5 already cover the extra-bag round-trip at parser level.

### Task 9.2: Bless + verify

```bash
cargo xtask bless --conformance
cargo test -p pgevolve-conformance
```

Inspect each `expected/plan.sql` to confirm:
- `table-fillfactor/`: contains `ALTER TABLE app.t SET (fillfactor = 80);` (since CREATE TABLE WITH (...) ALSO produces the inline form — actually the differ probably emits an ALTER for the storage delta because the CREATE TABLE step is separately handled. Verify what actually gets blessed.)
- `table-multi-set/`: single ALTER TABLE SET with three keys (no separate SETs).
- `partition-inherits-reloptions/`: partition's own ALTER TABLE SET on the child table.
- `lint/unmanaged-reloption/`: advisory finding produced.

### Task 9.3: Commit

```bash
git add -p crates/pgevolve-conformance/tests/cases/
git commit -m "$(cat <<'EOF'
test(conformance): 11 reloption fixtures

New fixture root: cases/objects/reloptions/. Covers:
  table-fillfactor / table-autovacuum-disabled / table-multi-set
  alter-table-set-after-create
  index-fillfactor / index-gin-fastupdate / index-brin-pages-per-range
  mv-fillfactor
  partition-inherits-reloptions
  lint/unmanaged-reloption
  extra-bag

Stage 9 of docs/superpowers/plans/2026-05-22-table-reloptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 10 — Proptest + docs + v0.3.3 release

### Task 10.1: Property test extensions

- [ ] **Step 1: Extend testkit generators**

In `crates/pgevolve-testkit/src/ir_generator.rs`, add:

```rust
fn arb_autovacuum_options() -> impl Strategy<Value = AutovacuumOptions> {
    // 0-3 fields randomly populated.
    (
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))],   // enabled
        prop_oneof![Just(None), (0u64..1000).prop_map(Some)],            // vacuum_threshold
        prop_oneof![Just(None), (0.0f64..1.0).prop_map(|f| Some(NotNanF64::new(f).unwrap()))],  // vacuum_scale_factor
    ).prop_map(|(enabled, vacuum_threshold, vacuum_scale_factor)| {
        AutovacuumOptions {
            enabled,
            vacuum_threshold,
            vacuum_scale_factor,
            ..Default::default()
        }
    })
}

fn arb_table_storage() -> impl Strategy<Value = TableStorageOptions> {
    (
        prop_oneof![Just(None), (10u32..=100).prop_map(Some)],    // fillfactor
        arb_autovacuum_options(),
        prop_oneof![Just(None), (0u32..=64).prop_map(Some)],      // parallel_workers
    ).prop_map(|(fillfactor, autovacuum, parallel_workers)| TableStorageOptions {
        fillfactor,
        autovacuum,
        parallel_workers,
        ..Default::default()
    })
}

fn arb_index_storage() -> impl Strategy<Value = IndexStorageOptions> {
    (
        prop_oneof![Just(None), (50u32..=100).prop_map(Some)],    // fillfactor (B-tree-friendly range)
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))],
    ).prop_map(|(fillfactor, fastupdate)| IndexStorageOptions {
        fillfactor,
        fastupdate,
        ..Default::default()
    })
}
```

Plumb into `arbitrary_table` / `arbitrary_index` / `arbitrary_materialized_view`. The existing strategy returns a tuple; add the storage strategies to it.

- [ ] **Step 2: Run 10× per constitution §9**

```bash
for i in 1 2 3 4 5 6 7 8 9 10; do
    echo "=== Run $i ==="
    PROPTEST_CASES=512 cargo test --workspace --release 2>&1 | tail -3
done
```

All 10 green.

- [ ] **Step 3: Commit**

```
test(proptest): reloptions in arbitrary_table / arbitrary_index

arb_table_storage / arb_index_storage / arb_autovacuum_options
generate well-typed reloption configurations. Range-bounded
strategies (fillfactor 10..=100 for tables, 50..=100 for indexes)
prevent generating PG-invalid combinations. 10× per §9; all green.

Stage 10.1 of docs/superpowers/plans/2026-05-22-table-reloptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

### Task 10.2: Docs

- [ ] **Step 1: Update `docs/spec/objects.md`**

Find line 269 (table reloptions row) and replace:

```markdown
| `WITH (storage_parameter = ...)` (table reloptions) | 🟡 Partial | The IR doesn't yet model `fillfactor`, autovacuum overrides, etc. Planned for v0.2. change_kinds: [alter] |
```

with:

```markdown
| `WITH (storage_parameter = ...)` (table reloptions) | ✅ Supported | Typed fields for fillfactor + autovacuum_* + parallel_workers + toast_tuple_target + user_catalog_table + vacuum_truncate; `extra: BTreeMap` for unknown/extension keys. Lenient drift policy. change_kinds: [alter] |
| Index reloptions | ✅ Supported | Per-AM validation: B-tree 50..=100 fillfactor, GiST 10..=100, SP-GiST 90..=100, BRIN/GIN no fillfactor; fastupdate (GIN), gin_pending_list_limit (GIN), buffering (GiST), deduplicate_items (B-tree), pages_per_range + autosummarize (BRIN). change_kinds: [alter] |
| Materialized view reloptions | ✅ Supported | Same key set as tables (autovacuum_*, fillfactor, etc.). change_kinds: [alter] |
```

Also update line 240 (per-partition storage note):

```markdown
- **Per-partition `TABLESPACE` and storage parameters** — partition bounds + reloptions are modeled (partitions are `Table` in IR, so they inherit table reloptions automatically). Per-partition `TABLESPACE` overrides are still 🔮 Future.
```

- [ ] **Step 2: Create `docs/spec/reloptions.md`**

```markdown
# Storage parameters / reloptions

pgevolve models PG `WITH (storage_parameter = …)` reloptions on tables,
indexes, and materialized views. Each relkind has a typed `*StorageOptions`
struct with named fields for the well-known options plus an `extra:
BTreeMap<String, String>` for extension-registered or otherwise-unknown
keys.

## Semantics — `None` always means "unmanaged"

Every typed field is `Option<T>`. The semantics follow v0.3.1's owner
pattern:

| source | catalog | differ action |
|---|---|---|
| `None` | `None` | no-op |
| `None` | `Some(x)` | **no-op** — surface as `unmanaged-reloption` lint warning |
| `Some(x)` | `None` | `ALTER … SET (key = x);` |
| `Some(x)` | `Some(x)` | no-op |
| `Some(x)` | `Some(y)` (x ≠ y) | `ALTER … SET (key = x);` |
| `Some(x)` removed from source → `None` | `Some(x)` | **no-op** (lenient) |

**Removing a reloption from source does NOT issue `RESET`.** To clear a
managed reloption:

1. Issue `ALTER TABLE t RESET (fillfactor)` out-of-band.
2. On the next plan run, catalog reads `None`, source is also `None`,
   diff is empty.

This is the same trade-off as v0.3.1's `owner: Option<Identifier>` —
"unmanaged" must be safe to declare without triggering destructive resets.

## Source surface

```sql
-- Inline at creation:
CREATE TABLE app.t (id bigint) WITH (fillfactor = 80, autovacuum_enabled = false);

-- ALTER post-creation:
ALTER TABLE app.t SET (parallel_workers = 4);

-- Indexes:
CREATE INDEX i ON app.t (id) WITH (fillfactor = 70);

-- Materialized views:
CREATE MATERIALIZED VIEW m WITH (fillfactor = 90) AS SELECT * FROM ...;
```

`ALTER ... RESET (...)` and `ALTER ... RESET ()` are **rejected** in source.

## Per-relkind validation

Parser enforces PG's documented ranges at parse time:

- **Tables / MVs `fillfactor`**: 10..=100
- **B-tree index `fillfactor`**: 50..=100
- **GiST / Hash index `fillfactor`**: 10..=100
- **SP-GiST index `fillfactor`**: 90..=100
- **BRIN / GIN index `fillfactor`**: not supported → `ParseError`
- **`parallel_workers`**: 0..=1024
- **`toast_tuple_target`**: 128..=8160
- **`pages_per_range`** (BRIN): 1..=131072
- **Numeric scale factors**: NaN rejected

## Supported keys

### Tables / Materialized Views

`fillfactor`, `parallel_workers`, `toast_tuple_target`, `user_catalog_table`,
`vacuum_truncate`, plus all 16 `autovacuum_*` keys including
`log_autovacuum_min_duration`.

### Indexes

`fillfactor`, `fastupdate` (GIN), `gin_pending_list_limit` (GIN),
`buffering` (GiST/SP-GiST, values: on/off/auto), `deduplicate_items`
(B-tree, PG 13+), `pages_per_range` + `autosummarize` (BRIN).

## Lint

- **`unmanaged-reloption`** (warning, waivable) — catalog has a typed
  reloption or extra-bag key not declared in source. Per the lenient
  drift policy, the differ doesn't RESET; the lint surfaces the drift
  so operators can decide.

## Out of scope

- `toast.*` prefixed options (apply to TOAST tables). Rare; deferred.
- Active RESET via source. Operators clear out-of-band.
- Per-partition tablespace overrides. Per-partition reloptions are
  supported (partitions are `Table` in IR).
```

- [ ] **Step 3: CHANGELOG**

Add new `[0.3.3]` section above `[0.3.2]`:

```markdown
## [Unreleased]

## [0.3.3] — 2026-05-22

### Added

- **Storage parameters / reloptions on tables, indexes, materialized views.** Typed `Option<T>` fields for the well-known keys (fillfactor, autovacuum_*, parallel_workers, fastupdate, buffering, pages_per_range, etc.) plus `extra: BTreeMap<String, String>` for extension-registered or unknown keys. Tables and MVs share the autovacuum substruct since PG documents identical key sets.
- **Per-AM fillfactor validation** at parse time: B-tree 50..=100, GiST 10..=100, SP-GiST 90..=100, BRIN/GIN reject fillfactor.
- **Lenient drift policy**: source `None` always means "unmanaged" — never triggers `RESET`. `unmanaged-reloption` lint surfaces catalog reloptions not in source.
- **3 new StepKind variants**: `SetTableStorage`, `SetIndexStorage`, `SetMaterializedViewStorage`. One ALTER step per relkind per diff (batches multiple keys into one SET).
- **`unmanaged-reloption` lint** (warning, waivable).
- **Source parser** for `WITH (...)` on CREATE TABLE/INDEX/MATERIALIZED VIEW and `ALTER ... SET (...)`. `RESET (...)` rejected in source.
- **Catalog reader** decodes `pg_class.reloptions::text[]` into typed structs.
- **11 conformance fixtures.**

### Closes

Slipped v0.2 commitment from `docs/spec/objects.md:269` (table reloptions row marked 🟡 Partial). Per-partition storage parameters at `objects.md:240` also satisfied (partitions inherit since they're `Table` in IR).

## [0.3.2] — 2026-05-22
```

### Task 10.3: Version bump

```bash
# Root Cargo.toml → 0.3.3
# Each per-crate Cargo.toml → 0.3.3
cargo build --workspace

v=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
echo "version: $v"
grep -q "^## \[$v\] — " CHANGELOG.md && echo OK || echo MISMATCH
```

### Task 10.4: §9 verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo doc --workspace --no-deps 2>&1 | grep -cE "^warning"  # expect 0
```

### Task 10.5: Re-bless conformance

```bash
cargo xtask bless --conformance
cargo test -p pgevolve-conformance
```

### Task 10.6: Release commit

```bash
git add docs/spec/objects.md docs/spec/reloptions.md CHANGELOG.md Cargo.toml Cargo.lock crates/*/Cargo.toml crates/pgevolve-conformance/tests/cases/
git commit -m "$(cat <<'EOF'
release: v0.3.3 — storage parameters / reloptions

Tables, indexes, and materialized views gain typed
*StorageOptions structs with named fields for well-known reloptions
plus an extra: BTreeMap for extension-registered or unknown keys.
Per-AM fillfactor validation at parse time. Lenient drift policy
(source None = unmanaged; never RESET).

Closes the slipped v0.2 commitment for table reloptions (objects.md:269)
and per-partition storage parameters (objects.md:240).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 10.7: STOP

Do NOT `git tag`, `git push`, or close GH issues. The user pushes independently. Report DONE.

---

## Done.

After Stage 10, v0.3.3 is committed locally and ready for tagging.

Next plan target: **PUBLICATION / SUBSCRIPTION** (logical replication source-side + consumer-side metadata) per the agreed roadmap.
