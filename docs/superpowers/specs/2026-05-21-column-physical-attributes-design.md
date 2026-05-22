# Column physical attributes — TOAST storage and compression

**Status:** Design accepted 2026-05-21. Ships as v0.2.1.
**Closes:** Issue #6 (TOAST column storage strategy). Bundles compression — see Scope.
**Spec line touched:** `docs/spec/objects.md:270` (will move from 📋 Planned → ✅ Supported).

## Summary

Add per-column TOAST storage strategy (`PLAIN | EXTERNAL | EXTENDED | MAIN`) and TOAST compression codec (`pglz | lz4`) to the managed-column surface. Both are `pg_attribute` metadata-only attributes, both parse from two source forms (inline column clause and `ALTER COLUMN`), and both diff as non-destructive metadata changes. Lint rules surface the non-retroactive nature of downgrades.

## Scope

The GitHub issue scopes storage only. This spec **bundles compression** because:

- Both attributes live on `pg_attribute` (`attstorage` byte + `attcompression` byte).
- Both have the same parser shape, the same differ shape, and the same "metadata-only, not retroactive" semantics.
- Modeling them together is materially cheaper than two near-identical follow-up PRs.

User accepted the bundling during 2026-05-21 brainstorming.

**Explicitly out of scope (separate work):**

- Per-table `storage_parameters` — `fillfactor`, `autovacuum_*`, etc. (`objects.md:269` row "table reloptions").
- Composite-type attribute storage (`ALTER TYPE … ALTER ATTRIBUTE … SET STORAGE`).
- Table access methods (`USING heap/zheap/columnar`) — `objects.md:268`, marked 🔮 Future.

## IR — `crates/pgevolve-core/src/ir/column.rs`

Extend `Column` with two new optional fields:

```rust
pub struct Column {
    // ... existing fields ...
    /// Per-column TOAST storage strategy. `None` means "use the type's default"
    /// (e.g. `text` defaults to EXTENDED, `int4` defaults to PLAIN).
    pub storage: Option<StorageKind>,
    /// Per-column TOAST compression codec. `None` means "use the cluster
    /// `default_toast_compression` GUC".
    pub compression: Option<Compression>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind { Plain, External, Extended, Main }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compression { Pglz, Lz4 }
```

Both new fields use `#[diff(via_debug)]` consistent with `default`, `identity`, `generated`, `collation`, `comment`.

**Why two enums over a packed struct:** Storage and compression evolve independently in Postgres (compression codecs may grow; storage strategies are fixed). Separate enums avoid cross-coupling in the differ.

**Why `Compression::Pglz` is explicit (not folded into `None`):** Author intent matters. `default_toast_compression` is a GUC that can be set to `lz4` cluster-wide; an author who explicitly writes `COMPRESSION pglz` is overriding that, not accepting the default.

## Canon — type-default stripping

Extend `ir::canon::filter_pg_defaults` (the existing pass that strips PG-injected defaults from catalog-read IR). When the catalog reader produces `storage = Some(Extended)` for a column whose type's default storage is `EXTENDED`, strip it to `None`.

The mapping (Postgres type → default storage) is small and stable; encode it as a function over `ColumnType`:

| Type family | Default |
|---|---|
| Fixed-width scalars (`int*`, `bool`, `float*`, `timestamp*`, `uuid`, `date`, `time*`, `oid`) | `PLAIN` |
| Variable-width toastable (`text`, `varchar`, `bytea`, `json`, `jsonb`, `xml`, arrays, ranges, composite, hstore) | `EXTENDED` |
| `numeric`, `tsvector`, `tsquery`, large-object geometry (`path`, `polygon`) | `MAIN` |

The full mapping is sourced from `typstorage` in `pg_type`; the table above shows the dominant cases. The implementation reads `typstorage` for the column's underlying type to determine the default rather than enumerating every type by hand.

The catalog reader stays "raw" — it emits whatever `attstorage` says. Canon handles the normalization. This matches the established convention from views/MVs sentinel-column handling and the `filter_pg_defaults` pass.

For `compression`: the catalog reader produces `None` when `attcompression = '\0'` (cluster default) and `Some(Pglz)` / `Some(Lz4)` otherwise. No canon pass needed — what the catalog says is what the author asked for.

## Parser

Accept both Postgres surface forms:

1. **Inline column attributes** (CREATE TABLE / ALTER TABLE … ADD COLUMN):
   ```sql
   CREATE TABLE t (
       doc  text  STORAGE EXTERNAL  COMPRESSION lz4,
       blob bytea STORAGE MAIN
   );
   ```
   The `STORAGE` inline clause is Postgres 16+ syntax. The parser accepts it on all versions (pg_query supports it); apply against PG 14/15 will fail at execution time, which is correct PG behavior. We do not warn at parse time — that's a `pgevolve apply` concern, not a source-canonicalization concern.

2. **ALTER COLUMN** (any PG version):
   ```sql
   ALTER TABLE t ALTER COLUMN doc SET STORAGE EXTERNAL;
   ALTER TABLE t ALTER COLUMN doc SET COMPRESSION lz4;
   ```

Both parse paths populate the same `Column.storage` / `Column.compression` fields. Source authors choose whichever form reads better.

Touchpoints:

- `crates/pgevolve-core/src/parse/builder/create_stmt.rs` — column clause loop already handles `Constraint` (NOT NULL), `CollClause`, etc.; add `ColumnDef.storage` / `ColumnDef.compression` parsing.
- `crates/pgevolve-core/src/parse/builder/alter_table.rs` (or wherever ALTER subcommands live) — add `AT_SetStorage` and `AT_SetCompression` subcommand handlers.

## Catalog reader — `pg_attribute` extensions

Extend the shared catalog query (`crates/pgevolve-core/src/catalog/queries/shared.rs`):

```sql
-- add to the existing pg_attribute selection
SELECT ...
       a.attstorage,                       -- char: 'p'|'e'|'x'|'m'
       a.attcompression,                   -- char: '\0'|'p'|'l'
       ...
FROM pg_attribute a ...
```

Decode in `catalog/rows.rs`:

```rust
fn decode_storage(c: char) -> StorageKind {
    match c { 'p' => Plain, 'e' => External, 'x' => Extended, 'm' => Main, ... }
}
fn decode_compression(c: char) -> Option<Compression> {
    match c { '\0' => None, 'p' => Some(Pglz), 'l' => Some(Lz4), ... }
}
```

Per-version queries (`pg14.rs`–`pg17.rs`) need adjustment only if a column is missing in older versions. `attstorage` exists since PG ≤9. `attcompression` was added in **PG 14**. Since pgevolve's MSRV is PG 14, no version split is needed; the column is present everywhere we care about.

Catalog-assemble (`catalog/assemble/tables.rs` after Stage 5 split) populates the new `Column` fields directly from the decoded values. Canon strips type-default `storage` to `None` downstream.

## Differ — new column-level changes

Two new variants on `ColumnChange` (in `crates/pgevolve-core/src/diff/changeset.rs`):

```rust
ColumnChange::SetStorage {
    table: QualifiedName,
    column: Identifier,
    from: StorageKind,    // resolved type default if source was None
    to:   StorageKind,    // resolved type default if target was None
},
ColumnChange::SetCompression {
    table: QualifiedName,
    column: Identifier,
    from: Option<Compression>,
    to:   Option<Compression>,
},
```

**Why resolve `None` to the type default in `SetStorage` but not `SetCompression`:** A storage change is meaningful only relative to what the column *actually* used; the type default is the effective value. A compression change is meaningful relative to the cluster GUC, which we can't observe at diff time — so we preserve `None` and emit `SET COMPRESSION default`.

Destructiveness classification (`destructiveness.rs`): **non-destructive metadata-only**. Both changes execute as catalog updates without rewriting heap pages; they apply instantly on tables of any size.

Rendering (`plan/rewrite/sql.rs` or its split successor): always emit as `ALTER TABLE … ALTER COLUMN … SET STORAGE/COMPRESSION …` regardless of how the author wrote the source. This form works uniformly on PG 14+ and avoids per-version branching.

## Lint — two new universal rules

In `crates/pgevolve-core/src/lint/rules/` (post-Stage-5.2 split):

### `storage-downgrade-not-retroactive` (warning, waivable)

Fires when a `SetStorage` change goes `EXTERNAL → MAIN/PLAIN/EXTENDED` or `EXTENDED → MAIN/PLAIN` for a column that might already hold TOASTed values. The strategy change takes effect immediately at the catalog level, but **existing rows keep their current storage** until the next UPDATE rewrites them. Authors who expect retroactive recompaction usually want `VACUUM FULL` or a table rewrite, neither of which pgevolve emits.

Severity: `Warning`. Waiver path: standard `@pgevolve waive` directive.

### `compression-change-not-retroactive` (warning, waivable)

Fires on any `SetCompression` change for a column with TOAST-eligible storage (`EXTENDED` or `MAIN` or `EXTERNAL`). Same rationale: existing toasted values keep their old codec; only new/updated rows get the new codec.

Severity: `Warning`. Waiver path: standard.

**Why not fold into one rule:** The two phenomena are semantically distinct (one is about heap-vs-toast placement; the other is about codec inside toast). Operators reading the lint output benefit from the specific name. Cost is two ~40-line rule files vs. one ~70-line file.

## Conformance fixtures

Add to `crates/pgevolve-conformance/tests/cases/objects/columns/`:

1. `set-storage-external/` — text column, `STORAGE EXTERNAL` change. Verifies storage round-trips through parse → diff → render → apply, no lint.
2. `set-storage-plain-warning/` — text column with prior `EXTERNAL`, downgrade to `PLAIN`. Verifies the lint fires.
3. `set-compression-lz4/` — bytea column, switch from `pglz` to `lz4`. Verifies the lint fires and the SQL renders.
4. `create-table-with-storage/` — fresh table with inline `STORAGE EXTERNAL COMPRESSION lz4`. Verifies the PG-16 inline form parses and round-trips.
5. `set-storage-type-default-noop/` — author writes explicit `STORAGE EXTENDED` on a `text` column; canon strips to `None`; no diff vs. the implicit form. Verifies the canon pass.

All five follow the existing `before.sql + after.sql + fixture.toml + expected/` layout. Bless via `cargo xtask bless --conformance`.

## Property test addition

Extend the existing column-diff property test to include `storage` and `compression` in the arbitrary `Column` generator, with type-aware shrinking (don't generate `STORAGE MAIN` on a fixed-width scalar — PG accepts it but it's meaningless).

## Documentation updates

- `docs/spec/objects.md:270` — move row from 📋 Planned → ✅ Supported, link this spec.
- `CHANGELOG.md` — new `[0.2.1]` section: "Added: per-column TOAST storage strategy and compression codec".
- Bump `[workspace.package].version` to `0.2.1`. Per the release runbook the CHANGELOG-version sync CI gate from Task 2.2 verifies these stay in lockstep.

## Non-goals reconfirmed

- No automatic `VACUUM FULL` emission. The lint warns; the operator decides.
- No table-rewrite emission for retroactive recompression. Same reason.
- No support for setting `default_toast_compression` GUC from pgevolve. That's cluster-wide config, outside pgevolve's per-database scope.
- No modeling of `pg_class.reloptions` (per-table autovacuum_*, fillfactor, etc.) — separate planned row.

## Open questions

None remaining as of design acceptance. The compression-vs-storage bundling, the `None`-as-default convention, the two-rule lint split, and the always-emit-via-ALTER rendering choice were all settled during brainstorming.
