# Storage parameters / reloptions for Tables, Indexes, Materialized Views (v0.3.3)

**Status:** Design accepted 2026-05-22.
**Closes:** Slipped v0.2 commitment at `docs/spec/objects.md:269` (table reloptions row marked 🟡 Partial / "Planned for v0.2"). Also closes the per-partition storage-overrides gap at line 240 (partitions inherit since they're `Table` in the IR).
**Not closing a GH issue:** no explicit issue exists; this design supersedes the spec-table TODO.

## Summary

Model Postgres `WITH (storage_parameter = …)` options on tables, indexes, and materialized views. Each relkind gains a typed `*StorageOptions` struct with named fields for the well-known keys (fillfactor, autovacuum_*, parallel_workers, fastupdate, etc.) plus an `extra: BTreeMap<String, String>` for extension-registered or otherwise-unknown keys. Tables and MVs share an `AutovacuumOptions` substruct because PG documents identical reloptions for them. Drift policy is **lenient** — catalog reloptions that aren't in source surface as a `unmanaged-reloption` warning, never silently RESET. Parser enforces per-relkind range validation (e.g. `fillfactor` 50–100 for B-tree indexes; 90–100 for SP-GiST).

## Scope

**In scope:**

- New `crates/pgevolve-core/src/ir/reloptions.rs` with `TableStorageOptions`, `IndexStorageOptions`, `AutovacuumOptions`, `BufferingMode`.
- `Table`, `Index`, `MaterializedView` IR structs gain a `storage: *StorageOptions` field. MV reuses `TableStorageOptions` directly (PG documents identical key sets).
- Source parser handles `WITH (...)` clauses in `CREATE TABLE`, `CREATE INDEX`, `CREATE MATERIALIZED VIEW`, plus `ALTER TABLE/INDEX/MATERIALIZED VIEW SET (...)` and `RESET (...)`.
- Per-relkind range validation at parse time (`fillfactor` ranges per access method; numeric/bool type-checking for typed fields).
- Catalog reader decodes `pg_class.reloptions` for tables/MVs (already partially in use for views) and adds the same column to `INDEXES_QUERY`.
- Differ + render: 6 new `Change` variants (3 set + 3 reset, one each per relkind) and 6 corresponding `StepKind` variants.
- One new lint: `unmanaged-reloption` (warning, waivable).
- Conformance fixtures (~10).
- Property test extensions for the new IR fields.

**Explicitly out of scope:**

- `toast.*` prefixed options (apply to the associated TOAST table; rare; deferred to a future sub-spec).
- Per-partition reloption overrides as a *separate* feature — partitions inherit automatically since they're `Table` in the IR. Tested via at least one partition fixture.
- `STORAGE` access method on tables/indexes (`USING heap`, `USING zheap`, etc.) — already 🔮 Future per `objects.md:268`.
- Sequence reloptions (`pg_sequence` has none in modern PG).

User confirmed scope decisions during 2026-05-22 brainstorming.

## IR — `crates/pgevolve-core/src/ir/reloptions.rs` (new)

```rust
use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

/// Shared autovacuum options — apply to both `Table` and `MaterializedView`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// `autovacuum_vacuum_insert_threshold` (PG 13+). On PG 12 the column
    /// doesn't exist; reader emits `None`.
    pub vacuum_insert_threshold: Option<u64>,
    pub vacuum_insert_scale_factor: Option<NotNanF64>,
    /// `log_autovacuum_min_duration` — `-1` disables. Stored as `i64`.
    pub log_min_duration: Option<i64>,
}

/// Wrapper around `f64` that excludes NaN — needed because `Option<f64>` can't
/// derive `Eq`/`Hash`. The reader rejects NaN catalog values; the parser
/// rejects NaN source values.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct NotNanF64(f64);
// Hand-rolled Eq + Hash + Ord since f64 doesn't have them.

/// Storage options for tables (and materialized views — see type alias).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TableStorageOptions {
    pub fillfactor: Option<u32>,            // 10..=100
    pub autovacuum: AutovacuumOptions,
    pub parallel_workers: Option<u32>,      // 0..=1024
    pub toast_tuple_target: Option<u32>,    // 128..=8160
    pub user_catalog_table: Option<bool>,
    pub vacuum_truncate: Option<bool>,      // PG 12+
    /// Unknown / extension-registered options. Stored as raw strings.
    /// Canon sorts by key.
    pub extra: BTreeMap<String, String>,
}

/// MVs share table reloption semantics in PG — same key set, same validators.
pub type MaterializedViewStorageOptions = TableStorageOptions;

/// Storage options for indexes. Valid keys depend on the access method.
/// Parse-time validation accepts only the keys valid for the index's `method`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndexStorageOptions {
    pub fillfactor: Option<u32>,             // range per AM (see Validation)
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
    /// Unknown / extension-registered options.
    pub extra: BTreeMap<String, String>,
}

/// `buffering` setting for GiST/SP-GiST index builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferingMode {
    On,
    Off,
    Auto,
}
```

Each of `Table`, `Index`, `MaterializedView` gains one field at the end of its struct:

```rust
pub struct Table {
    /* ... existing fields ... */
    pub storage: TableStorageOptions,
}
pub struct Index {
    /* ... */
    pub storage: IndexStorageOptions,
}
pub struct MaterializedView {
    /* ... */
    pub storage: MaterializedViewStorageOptions,
}
```

Default state (`TableStorageOptions::default()`) = all fields `None` + empty `extra` map. Matches PG's `pg_class.reloptions IS NULL`. Backfill across the workspace mirrors v0.3.x patterns.

## Semantics — `None` always means "unmanaged"

This is the most surprising part of the design. It deserves explicit documentation in `docs/spec/reloptions.md`.

| source side | catalog side | differ action |
|---|---|---|
| `None` | `None` | no-op |
| `None` | `Some(x)` | **no-op** — surface as `unmanaged-reloption` lint warning |
| `Some(x)` | `None` | `ALTER … SET (key = x);` |
| `Some(x)` | `Some(x)` | no-op |
| `Some(x)` | `Some(y)` (x ≠ y) | `ALTER … SET (key = x);` |
| `Some(x)` removed from source → `None` | `Some(x)` | **no-op** (lenient) |

The last row is the consequential one: **removing an option from source does NOT issue `RESET`**. To clear a managed reloption, the operator must:

1. Issue `ALTER TABLE t RESET (fillfactor)` out-of-band (or via a directly-authored migration step).
2. Then on the next plan run, the catalog reads `None`, source also `None`, and the diff is empty.

Rationale: same as v0.3.1's `owner: Option<Identifier>` — "unmanaged" must be safe to declare without triggering destructive resets. Adding a "remove" intent would require tracking per-field "previously managed" state, which the IR doesn't have a place for. The simpler model is: source declares what is managed; absence means uninvolved.

Document this trade-off prominently in `docs/spec/reloptions.md`.

## Source parser

### `WITH (...)` clause on CREATE statements

Three sites:
- `CreateStmt` (CREATE TABLE) — `s.options: Vec<Node>` carries the `DefElem`s.
- `IndexStmt` (CREATE INDEX) — `s.options: Vec<Node>`.
- `CreateTableAsStmt` (CREATE MATERIALIZED VIEW … AS …) — `s.into.options: Vec<Node>` (the `IntoClause` carries them).

Each `DefElem` has `defname: String` (key, lowercase) and `arg: Option<Node>`. The `arg` is typically an `Integer`, `Float`, `String`, or `Boolean` node, or absent (when the key is a bool defaulting to true).

Shared decoder at `crates/pgevolve-core/src/parse/builder/reloptions.rs::decode_options` dispatches to per-relkind decoders that populate the typed struct. Unknown keys flow into `extra`.

### `ALTER ... SET (...)` and `RESET (...)`

Extend `parse/builder/alter_table_stmt.rs` (which already handles `AT_ChangeOwner`, the four RLS toggles, `AT_SetStorage`, etc.). Add three subcommand handlers:

- `AT_SetRelOptions` → merge keyed values into the table's `TableStorageOptions`.
- `AT_ResetRelOptions` → ParseError ("RESET in source is not supported — clear options out-of-band, then remove from source").
- `AT_ReplaceRelOptions` → ParseError (PG-internal, not user-facing).

Same shape for `AlterIndexStmt` and `AlterMaterializedViewStmt` paths.

**Why reject `RESET` in source:** consistent with v0.3.1's "REVOKE in source rejected — revokes happen via diff." But the differ here doesn't emit RESET either (per the lenient policy). So RESET source has no meaningful semantics: it doesn't add to managed state, doesn't trigger a diff change. Rejecting parser-side is the honest call.

### Validation (parser-side, stricter)

Per-key checks at parse time:

- **`fillfactor`** range varies per relkind:
  - Tables / MVs: `10..=100`
  - B-tree index: `50..=100`
  - GiST index: `10..=100`
  - Hash index: `10..=100`
  - SP-GiST index: `90..=100`
  - BRIN index: not supported (PG rejects) → `ParseError`
  - GIN index: not supported (PG rejects) → `ParseError`

  Index-side validation needs to know the access method, which is the existing `Index.method` field — available at parse time since `WITH (...)` is parsed alongside `USING method` in the same statement.

- **`parallel_workers`**: `0..=1024`.
- **`toast_tuple_target`**: `128..=8160`.
- **`autovacuum_*` integer/scale-factor**: non-negative.
- **`log_autovacuum_min_duration`**: any `i64` (negative = disabled).
- **`pages_per_range`** (BRIN): `1..=131072`.
- **Unknown keys**: parsed into `extra` bag without validation (we have no schema to check against).
- **Type mismatches** (e.g., `fillfactor = "high"`): `ParseError::Structural`.

## Catalog reader

### Shared aclitem-style decoder for reloptions

New `crates/pgevolve-core/src/catalog/reloptions.rs`. Two entry points:

```rust
pub(crate) fn decode_table_reloptions(raw: &[String])
    -> Result<TableStorageOptions, CatalogError>;
pub(crate) fn decode_index_reloptions(raw: &[String], method: &str)
    -> Result<IndexStorageOptions, CatalogError>;
```

`pg_class.reloptions` is `text[]` of `"key=value"` strings (PG already quotes/escapes; we parse as plain `key=value` split-on-first-`=`). Known keys populate typed fields; unknown keys land in `extra`. Numeric parse errors return `CatalogError::BadColumnType { ... }` per the v0.3.1 precedent.

### Per-family query additions

- **Tables / MVs query**: `pg_class.reloptions::text[]` is **already selected** for views; extend the tables/MVs query to also pull it (small change since the column exists on every `pg_class` row). Already partially in use.
- **Indexes query**: add `coalesce(c.reloptions, '{}'::text[]) AS reloptions` to `INDEXES_QUERY`.

### Assembler integration

After the existing per-table/per-index field decoding, decode the reloptions array and assign to `storage`. Mirror the pattern v0.3.1 Stage 5 established for `owner` + `grants`.

## Canon

In `crates/pgevolve-core/src/ir/canon/reloptions.rs` (new):

- Sort each `TableStorageOptions.extra` / `IndexStorageOptions.extra` map (BTreeMap already orders by key — Canon just enforces nothing further). Mostly a no-op since BTreeMap is already ordered.
- No type-default stripping (e.g., `autovacuum_enabled = true` is PG's default but the user explicitly set it — `Some(true)` stays).
- Wired into `Catalog::canonicalize` after existing per-family canons.

## Differ + render

Three new `Change` variants — one SET per relkind. **No `Reset*` variants** because the lenient drift policy makes them unreachable: source `None` means "skip," so the differ never produces RESET steps.

```rust
SetTableStorage { qname: QualifiedName, options: TableStorageOptions },
SetIndexStorage { qname: QualifiedName, options: IndexStorageOptions },
SetMaterializedViewStorage { qname: QualifiedName, options: TableStorageOptions },
```

Each `Set*` variant carries the **delta** — only the keys whose source value differs from the catalog. The renderer emits `ALTER TABLE/INDEX/MATERIALIZED VIEW t SET (k1 = v1, k2 = v2);` — one step per relkind per diff (PG accepts multiple options per SET, so batching is cheaper than one step per key).

All ops `Destructiveness::Safe`. SQL helpers live in `plan/rewrite/reloptions.rs`.

Three new `StepKind` variants matching the Change variants. Snake-case names: `set_table_storage`, `set_index_storage`, `set_materialized_view_storage`.

**Note for implementers:** if someone later argues for adding `Reset*` to support active clearing (e.g., source author wants to explicitly say "reset fillfactor to PG default"), that requires changing the IR to distinguish "unmanaged" from "explicitly reset" — currently both are `None`. Out-of-spec for v0.3.3.

## Lint — `unmanaged-reloption`

New rule at `crates/pgevolve-core/src/lint/rules/unmanaged_reloption.rs`:

Fires when catalog has a typed reloption set to `Some(_)` or an `extra` bag entry whose key isn't in source's typed fields or `extra` bag. Surfaces as Warning. Message format: `"table app.docs: catalog has reloption fillfactor = 80 not declared in source"`.

This is a source-tree-level rule (operates on `&Catalog`) — runs from `check_universal` (or `check_plan_time_catalog`, the bridge function added in v0.3.2 Stage 8).

Waivable per existing waiver conventions.

## Conformance fixtures (~10)

Under `crates/pgevolve-conformance/tests/cases/objects/reloptions/`:

1. `table-fillfactor/` — `CREATE TABLE … WITH (fillfactor = 80);`
2. `table-autovacuum-disabled/` — `WITH (autovacuum_enabled = false)`
3. `table-multi-set/` — multiple keys in one SET
4. `alter-table-set-after-create/` — option added via ALTER, not WITH
5. `index-fillfactor/` — `CREATE INDEX … WITH (fillfactor = 70);` (B-tree)
6. `index-gin-fastupdate/` — `WITH (fastupdate = false)` on GIN
7. `index-brin-pages-per-range/` — `WITH (pages_per_range = 32)` on BRIN
8. `mv-fillfactor/` — fillfactor on materialized view
9. `partition-inherits-reloptions/` — verify partition table gets its own reloptions (proves the "partitions are Tables in IR" claim)
10. `lint/unmanaged-reloption/` — catalog has a reloption not in source; lint warning fires
11. `extra-bag/` — unknown reloption key flows into the `extra` bag, round-trips

11 if `extra-bag` is included; 10 otherwise.

## Property test

Extend `arbitrary_table` to randomly populate `TableStorageOptions`: 0–3 typed fields set, 0–2 `extra` keys. `arb_autovacuum_options` similar. `arbitrary_index` similar with valid-per-AM keys. Use `prop_oneof![Just(None), Just(Some(value))]` for each field.

Run 10× per constitution §9.

## Documentation

- `docs/spec/objects.md:269` (table reloptions row) moves from 🟡 Partial → ✅ Supported.
- Add new rows for **Index reloptions** (currently unlisted) and **MV reloptions** (currently inherits from views row — split).
- `docs/spec/objects.md:240` note about per-partition reloptions — update to point at this sub-spec since partitions inherit automatically.
- New `docs/spec/reloptions.md` — overview of supported keys per relkind, the `None`=unmanaged semantics, the out-of-band RESET requirement, lint behavior.
- `CHANGELOG.md` — new `[0.3.3]` section.

## Release shape

v0.3.3 — third (and final) v0.3.x patch in the security/permissions push (technically reloptions isn't security, but it ships in the same series). After this, pivot to v0.4 starting with PUBLICATION/SUBSCRIPTION (per agreed roadmap).

## Non-goals reconfirmed

- No `toast.*` options. Rare; deferred.
- No RESET semantics in source. Operators clear via out-of-band ALTER, then drop from source.
- No automatic backfill from catalog when first declaring a table (e.g., importing existing fillfactor settings). Operators run `pgevolve dump` to scaffold.
- No retroactive validation that an unknown `extra` key is valid — pgevolve trusts the source; PG rejects at apply if invalid.

## Open questions resolved during brainstorming

- **Typed + bag**: yes, both. Typed fields for the well-known set, `extra: BTreeMap<String, String>` for the rest.
- **All three relkinds at once**: yes — Table, Index, MaterializedView.
- **Lenient drift**: yes — `None` always means "skip" in the differ.
- **Stricter parse-time validation**: yes — per-relkind fillfactor ranges, numeric bounds, type checking.

No remaining open questions.
