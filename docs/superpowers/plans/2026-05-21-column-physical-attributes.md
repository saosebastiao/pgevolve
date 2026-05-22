# Column Physical Attributes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land per-column TOAST `storage` (`PLAIN | EXTERNAL | EXTENDED | MAIN`) and TOAST `compression` (`pglz | lz4`) as fully-managed `pg_attribute` metadata in pgevolve's source→IR→diff→render→apply pipeline, with conformance fixtures and lint rules covering the new surface. Ships as v0.2.1.

**Architecture:** Nine sequential stages. Each stage is independently committable and leaves the workspace green. Stages 1–6 add code (IR, canon, parser, catalog, differ, render+lint); Stage 7 adds conformance fixtures; Stage 8 extends a property test; Stage 9 ships the version bump. The differ piggybacks on the existing `TableOp::SetColumn*` family; the catalog reader extends the existing `COLUMNS_QUERY`; the canon pass extends `filter_pg_defaults`. No new module boundaries; only additive changes to existing files.

**Tech Stack:** Rust 1.95+, `pg_query` 5.x, `serde`, `tokio_postgres`, `testcontainers`, `proptest`, existing pgevolve workspace.

**Source spec:** `docs/superpowers/specs/2026-05-21-column-physical-attributes-design.md`.

---

## Pre-flight

- [ ] **Step 1: Confirm clean baseline**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --tests
```

Expected: all green. If anything fails, fix before starting. The plan assumes a clean main.

- [ ] **Step 2: Re-read the spec sections relevant to each stage**

Open `docs/superpowers/specs/2026-05-21-column-physical-attributes-design.md` and skim:
- "IR" — informs Stage 1
- "Canon — type-default stripping" — informs Stage 1
- "Parser" — informs Stage 2
- "Catalog reader" — informs Stage 3
- "Differ" — informs Stage 4
- "Lint" — informs Stage 6
- "Conformance fixtures" — informs Stage 7

---

## Stage 1 — IR + canon

Adds `Column.storage` and `Column.compression` fields, two new enums, and extends the `filter_pg_defaults` canon pass to strip type-default storage.

**Files modified:** `crates/pgevolve-core/src/ir/column.rs`, `crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs`, `crates/pgevolve-core/src/ir/canon/mod.rs` (re-exports if needed).

### Task 1.1: Add `StorageKind` + `Compression` enums to `ir/column.rs`

- [ ] **Step 1: Add the two enums at the bottom of `ir/column.rs`**

Add after the existing `SequenceOptions` block:

```rust
/// Per-column TOAST storage strategy.
///
/// Mirrors Postgres `pg_attribute.attstorage` (`p|e|x|m`). The semantic
/// effective value depends on the column's data type; see
/// [`crate::ir::canon::filter_pg_defaults`] for how `None` is preserved
/// when the requested storage matches the type default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    /// `PLAIN` — inline only, never TOASTed or compressed.
    Plain,
    /// `EXTERNAL` — out-of-line allowed, compression forbidden.
    External,
    /// `EXTENDED` — out-of-line and compression allowed (default for most variable-width types).
    Extended,
    /// `MAIN` — compression allowed inline, out-of-line only as a last resort.
    Main,
}

/// Per-column TOAST compression codec.
///
/// Mirrors Postgres `pg_attribute.attcompression`. `None` (i.e. field
/// value `None` on `Column`) means "use the cluster
/// `default_toast_compression` GUC."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compression {
    /// PGLZ — Postgres's historical lz-family codec; the default for clusters
    /// shipped with `default_toast_compression = 'pglz'`.
    Pglz,
    /// LZ4 — added in PG 14; faster compression/decompression at lower ratio.
    Lz4,
}
```

- [ ] **Step 2: Verify the file still compiles**

```bash
cargo check -p pgevolve-core
```

Expected: clean compile, no warnings.

### Task 1.2: Add `storage` + `compression` fields to `Column`

- [ ] **Step 1: Extend the `Column` struct**

In `crates/pgevolve-core/src/ir/column.rs`, add two new fields just before `comment`:

```rust
    /// Optional collation.
    #[diff(via_debug)]
    pub collation: Option<QualifiedName>,
    /// Per-column TOAST storage strategy. `None` means "PG type default".
    #[diff(via_debug)]
    pub storage: Option<StorageKind>,
    /// Per-column TOAST compression codec. `None` means "cluster default".
    #[diff(via_debug)]
    pub compression: Option<Compression>,
    /// Optional comment.
    #[diff(via_debug)]
    pub comment: Option<String>,
```

- [ ] **Step 2: Update the `base()` test helper and any other constructors**

In the `tests` module at the bottom of `ir/column.rs`, extend `base()`:

```rust
    fn base() -> Column {
        Column {
            name: id("email"),
            ty: ColumnType::Text,
            nullable: false,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        }
    }
```

- [ ] **Step 3: Search for every other `Column { ... }` literal in the workspace and add the two new fields**

```bash
rg -n "Column \{" crates/ --type rust
```

For each match, add `storage: None,` and `compression: None,` in the same position they appear in the struct definition (after `collation`, before `comment`). Likely sites: `diff/columns.rs` test helpers, `catalog/assemble/tables.rs`, `parse/builder/create_stmt.rs`, conformance test fixtures (if any inline-construct `Column`), property test generators.

- [ ] **Step 4: Add diff tests for the two new fields**

In `ir/column.rs::tests`, add at the bottom of the module:

```rust
    #[test]
    fn storage_change_diffs() {
        let mut b = base();
        b.storage = Some(StorageKind::External);
        assert!(base().diff(&b).iter().any(|x| x.path == "storage"));
    }

    #[test]
    fn compression_change_diffs() {
        let mut b = base();
        b.compression = Some(Compression::Lz4);
        assert!(base().diff(&b).iter().any(|x| x.path == "compression"));
    }
```

- [ ] **Step 5: Run column tests**

```bash
cargo test -p pgevolve-core --lib ir::column
```

Expected: all green. If a `Column` literal elsewhere is missing fields, the compiler will say so — add the fields and re-run.

- [ ] **Step 6: Full workspace compile check**

```bash
cargo check --workspace --all-targets
```

Expected: clean. Any missed `Column { ... }` literal surfaces here.

- [ ] **Step 7: Commit**

```bash
git add -p crates/
git commit -m "$(cat <<'EOF'
feat(ir): add Column.storage + Column.compression

Per-column TOAST attributes from pg_attribute. Two new enums
(StorageKind: Plain/External/Extended/Main, Compression: Pglz/Lz4)
and two Option<...> fields on Column. None means "use the PG default"
for that attribute (type-derived for storage, cluster GUC for
compression).

Backfills field defaults at every Column literal across the workspace.
No behavior change yet — differ + render still ignore the new fields;
those land in subsequent stages.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.3: Extend `filter_pg_defaults` to strip type-default storage

The canon pass already handles `normalize_column_collation`. Add a `normalize_column_storage` rule using a `ColumnType → StorageKind` mapping function.

**Files modified:** `crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs`.

- [ ] **Step 1: Add the type-default storage function**

At the bottom of `filter_pg_defaults.rs`:

```rust
/// Postgres's default `attstorage` for the column's type.
///
/// Derived from `pg_type.typstorage`. The mapping is stable across all
/// supported PG versions for built-in types. Adding a new `ColumnType`
/// variant requires extending this match — the compiler will catch it.
fn type_default_storage(ty: &ColumnType) -> StorageKind {
    use ColumnType::{
        BigInt, Bool, Bytea, Date, Double, Integer, Interval, Jsonb, Json,
        Numeric, Real, SmallInt, Text, Time, TimeTz, Timestamp, TimestampTz,
        Uuid, Varchar, /* extend as needed for every variant */
    };
    match ty {
        // Fixed-width by-value → PLAIN.
        BigInt | Bool | Date | Double | Integer | Real | SmallInt
        | Time { .. } | TimeTz { .. } | Timestamp { .. } | TimestampTz { .. }
        | Uuid => StorageKind::Plain,
        // Numeric, interval → MAIN per pg_type.typstorage.
        Numeric { .. } | Interval { .. } => StorageKind::Main,
        // Variable-width toastable → EXTENDED.
        Bytea | Json | Jsonb | Text | Varchar { .. } => StorageKind::Extended,
        // Add arms for every remaining ColumnType variant. Use rustc's
        // non-exhaustive-match error to find them; default the unknown to
        // EXTENDED only if you confirm via pg_type.typstorage that it's safe.
    }
}
```

Open `crates/pgevolve-core/src/ir/column_type.rs` to enumerate every variant. The match must be exhaustive — no wildcard arm. If pgevolve adds a new ColumnType variant later, the compiler will require this match to cover it.

- [ ] **Step 2: Add the `normalize_column_storage` rule**

Add below the function defined in Step 1:

```rust
/// If `col.storage` equals the type default, strip it to `None`.
fn normalize_column_storage(col: &mut crate::ir::column::Column) {
    if let Some(s) = col.storage
        && s == type_default_storage(&col.ty)
    {
        col.storage = None;
    }
}
```

- [ ] **Step 3: Call the rule from `run`**

Extend the existing `run` function:

```rust
pub fn run(cat: &mut Catalog) {
    for seq in &mut cat.sequences {
        normalize_sequence_defaults(seq);
    }
    for table in &mut cat.tables {
        for col in &mut table.columns {
            normalize_column_collation(col);
            normalize_column_storage(col);
        }
    }
    for f in &mut cat.functions {
        normalize_function_defaults(f);
    }
}
```

Note: compression has no IR-side normalization; the catalog reader emits `None` when `attcompression = '\0'` and the source parser only emits `Some(_)` when the author wrote a clause. So compression stays untouched here.

- [ ] **Step 4: Import `StorageKind` at the top of the file**

```rust
use crate::ir::column::{Column, StorageKind};
```

(The existing top-of-file imports already include `ColumnType` and others; just add the two new names.)

- [ ] **Step 5: Add canon tests**

At the bottom of `filter_pg_defaults.rs::tests` (or create the module if absent):

```rust
    use crate::ir::column::{Column, Compression, StorageKind};
    use crate::ir::column_type::ColumnType;
    use crate::identifier::Identifier;

    fn col(name: &str, ty: ColumnType) -> Column {
        Column {
            name: Identifier::from_unquoted(name).unwrap(),
            ty,
            nullable: true,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        }
    }

    #[test]
    fn type_default_storage_stripped() {
        let mut c = col("body", ColumnType::Text);
        c.storage = Some(StorageKind::Extended); // text default
        normalize_column_storage(&mut c);
        assert_eq!(c.storage, None, "EXTENDED on text should normalize to None");
    }

    #[test]
    fn non_default_storage_preserved() {
        let mut c = col("body", ColumnType::Text);
        c.storage = Some(StorageKind::External); // text default is EXTENDED
        normalize_column_storage(&mut c);
        assert_eq!(c.storage, Some(StorageKind::External));
    }

    #[test]
    fn type_default_for_int_is_plain() {
        let mut c = col("id", ColumnType::BigInt);
        c.storage = Some(StorageKind::Plain);
        normalize_column_storage(&mut c);
        assert_eq!(c.storage, None);
    }

    #[test]
    fn compression_is_not_stripped_by_canon() {
        let mut c = col("body", ColumnType::Text);
        c.compression = Some(Compression::Pglz);
        normalize_column_storage(&mut c);
        assert_eq!(c.compression, Some(Compression::Pglz),
            "canon does not touch compression");
    }
```

- [ ] **Step 6: Run canon tests**

```bash
cargo test -p pgevolve-core --lib ir::canon::filter_pg_defaults
```

Expected: all green.

- [ ] **Step 7: Workspace check**

```bash
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: green.

- [ ] **Step 8: Commit**

```bash
git add -p crates/pgevolve-core/src/ir/canon/
git commit -m "$(cat <<'EOF'
feat(canon): strip type-default storage in filter_pg_defaults

Adds normalize_column_storage to the canon pass. Mirrors the pattern
used for collation (Some(pg_catalog.default) → None) and sequence
min/max defaults. The type→default mapping is an exhaustive
ColumnType match — compiler enforces coverage for future variants.

Compression intentionally not normalized: catalog reader already
emits None for cluster-default, and the source parser only sets
Some(_) when the author wrote an explicit clause.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 2 — Parser

Two parse paths feed the new fields: inline `CREATE TABLE` column attributes (PG 16+ syntax) and `ALTER TABLE … ALTER COLUMN SET STORAGE/COMPRESSION`.

**Files modified:** `crates/pgevolve-core/src/parse/builder/create_stmt.rs`, `crates/pgevolve-core/src/parse/builder/alter_table.rs` (path may differ — find the file containing `AT_SetNotNull` / `AT_DropDefault` handlers).

### Task 2.1: Inline column STORAGE/COMPRESSION in CREATE TABLE / ADD COLUMN

- [ ] **Step 1: Locate the `ColumnDef` fields in pg_query's Rust bindings**

```bash
cargo doc -p pg_query --no-deps --open
```

Or `rg "pub storage\|pub compression" ~/.cargo/registry/src/**/pg_query-*/src/ 2>/dev/null` to find the `ColumnDef` struct definition. Confirm two relevant fields:
- `pub storage: i32` (Postgres `ColumnDef.storage` — single char as i32: `'p'`, `'e'`, `'x'`, `'m'`, or `'\0'`)
- `pub compression: String` (PG 14+ `ColumnDef.compression` — codec name as lowercase string, empty if not set)

If pg_query's Rust binding uses different field names, adjust the parser code to match what's actually in the binding.

- [ ] **Step 2: Add a helper in `parse/builder/create_stmt.rs` (or its shared module)**

```rust
/// Decode the inline `STORAGE x` clause from a `ColumnDef.storage` char.
/// Returns `None` if no clause was written ('\0').
fn decode_inline_storage(c: i32, location: &SourceLocation) -> Result<Option<StorageKind>, ParseError> {
    match u8::try_from(c).ok().map(char::from) {
        Some('\0') | None => Ok(None),
        Some('p') => Ok(Some(StorageKind::Plain)),
        Some('e') => Ok(Some(StorageKind::External)),
        Some('x') => Ok(Some(StorageKind::Extended)),
        Some('m') => Ok(Some(StorageKind::Main)),
        Some(other) => Err(ParseError::Structural {
            location: location.clone(),
            message: format!("unknown STORAGE attribute '{other}'"),
        }),
    }
}

/// Decode `COMPRESSION codec` from `ColumnDef.compression` (empty = unset).
fn decode_inline_compression(s: &str, location: &SourceLocation) -> Result<Option<Compression>, ParseError> {
    match s.to_ascii_lowercase().as_str() {
        "" => Ok(None),
        "pglz" => Ok(Some(Compression::Pglz)),
        "lz4" => Ok(Some(Compression::Lz4)),
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!("unknown COMPRESSION codec '{other}'"),
        }),
    }
}
```

Add the `StorageKind` and `Compression` imports near the top of the file.

- [ ] **Step 3: Wire decoded values into `build_column`**

In `build_column` (around line 146 of `create_stmt.rs`), after the `collation` line, add:

```rust
    let storage = decode_inline_storage(col.storage, location)?;
    let compression = decode_inline_compression(&col.compression, location)?;
```

And populate the returned `Column`:

```rust
    Ok((
        Column {
            name,
            ty,
            nullable,
            default,
            identity,
            generated,
            collation,
            storage,
            compression,
            comment,
        },
        produced_constraints,
        pk_inline,
    ))
```

(Find the actual `Column { ... }` construction at the bottom of `build_column` and add the two fields in the right position.)

- [ ] **Step 4: Add inline parse tests**

Find the existing `create_stmt` parser tests (`crates/pgevolve-core/src/parse/builder/create_stmt.rs::tests` or `crates/pgevolve-core/tests/parse_*.rs`). Add:

```rust
#[test]
fn create_table_inline_storage_external() {
    let sql = "CREATE TABLE app.t (doc text STORAGE EXTERNAL);";
    let cat = parse_to_catalog(sql);
    let col = &cat.tables[0].columns[0];
    assert_eq!(col.storage, Some(StorageKind::External));
}

#[test]
fn create_table_inline_compression_lz4() {
    let sql = "CREATE TABLE app.t (blob bytea COMPRESSION lz4);";
    let cat = parse_to_catalog(sql);
    let col = &cat.tables[0].columns[0];
    assert_eq!(col.compression, Some(Compression::Lz4));
}

#[test]
fn create_table_inline_both() {
    let sql = "CREATE TABLE app.t (doc text STORAGE EXTERNAL COMPRESSION lz4);";
    let cat = parse_to_catalog(sql);
    let col = &cat.tables[0].columns[0];
    assert_eq!(col.storage, Some(StorageKind::External));
    assert_eq!(col.compression, Some(Compression::Lz4));
}
```

Use whatever existing `parse_to_catalog` (or equivalent) test helper the parser tests already use — grep for prior tests in the same file to mirror their setup.

- [ ] **Step 5: Run parser tests**

```bash
cargo test -p pgevolve-core --lib parse::builder::create_stmt
cargo test -p pgevolve-core --tests parse
```

Expected: green. If pg_query's field name differs from `storage`/`compression`, fix Step 1's identification and rerun.

### Task 2.2: ALTER TABLE … ALTER COLUMN SET STORAGE / SET COMPRESSION

- [ ] **Step 1: Locate the alter-table subcommand handler**

```bash
rg -l "AT_SetNotNull\|AlterTableType::AtSetNotNull\|SetColumnNullable" crates/pgevolve-core/src/parse/
```

Find the file/function that converts `pg_query::AlterTableCmd` into pgevolve's IR/parser model (likely `parse/builder/alter_table.rs` or a sibling). Open it and locate the `match cmd.subtype` arm.

- [ ] **Step 2: Add two subtype handlers**

The pg_query enum names are `AT_SetStorage` and `AT_SetCompression`. Add arms:

```rust
AlterTableType::AtSetStorage => {
    let storage_name = cmd.name.as_deref().unwrap_or("");
    // pg_query gives us the keyword as a string in `cmd.name`
    // (e.g. "external", "extended", "main", "plain").
    let storage = match storage_name.to_ascii_lowercase().as_str() {
        "plain" => StorageKind::Plain,
        "external" => StorageKind::External,
        "extended" => StorageKind::Extended,
        "main" => StorageKind::Main,
        other => return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("unknown STORAGE attribute '{other}'"),
        }),
    };
    // pgevolve's existing alter-table parser produces a high-level
    // intent; emit one with kind ColumnStorage. Match the surrounding
    // pattern used by AtSetNotNull. If alter-table commands are
    // accumulated into a Vec<AlterAction> or similar, append the
    // new action here.
    out.push(AlterAction::SetColumnStorage {
        column: column_ident(&cmd, location)?,
        storage,
    });
}
AlterTableType::AtSetCompression => {
    let codec = cmd.name.as_deref().unwrap_or("");
    let compression = match codec.to_ascii_lowercase().as_str() {
        "default" => None,
        "pglz" => Some(Compression::Pglz),
        "lz4" => Some(Compression::Lz4),
        other => return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("unknown COMPRESSION codec '{other}'"),
        }),
    };
    out.push(AlterAction::SetColumnCompression {
        column: column_ident(&cmd, location)?,
        compression,
    });
}
```

The exact `AlterAction` variant names and the surrounding `out.push` API depend on whatever the existing alter-table parser uses (it might be `IrPatch::*`, `RawAlterAction::*`, or directly mutating the table IR). Match the existing pattern verbatim. The variant names below are placeholders; rename as needed.

**Important:** the source-side alter-table parser typically applies the change directly to the source `Catalog` rather than emitting an intermediate action. If that's the case here, the handler should:
1. Look up the table by `cmd.relation`.
2. Find the column by `cmd.name` (the column name).
3. Mutate `column.storage` / `column.compression` in place.

Read the existing `AT_SetNotNull` handler to confirm which model is used; replicate it.

- [ ] **Step 3: Add `AlterAction` variants if the parser uses an intermediate action enum**

If the parser uses an action enum, extend it (in the same file or a sibling):

```rust
pub enum AlterAction {
    // ... existing variants ...
    SetColumnStorage { column: Identifier, storage: StorageKind },
    SetColumnCompression { column: Identifier, compression: Option<Compression> },
}
```

And handle them in the action-applier (the function that mutates the source catalog given an `AlterAction`):

```rust
AlterAction::SetColumnStorage { column, storage } => {
    let c = table.columns.iter_mut().find(|c| &c.name == &column)
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("ALTER COLUMN SET STORAGE references unknown column {column}"),
        })?;
    c.storage = Some(storage);
}
AlterAction::SetColumnCompression { column, compression } => {
    let c = table.columns.iter_mut().find(|c| &c.name == &column)
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("ALTER COLUMN SET COMPRESSION references unknown column {column}"),
        })?;
    c.compression = compression;
}
```

Skip this step entirely if the parser is direct-mutate (no intermediate enum).

- [ ] **Step 4: Add ALTER tests**

In the alter-table parser test module:

```rust
#[test]
fn alter_column_set_storage_external() {
    let sql = "
        CREATE TABLE app.t (doc text);
        ALTER TABLE app.t ALTER COLUMN doc SET STORAGE EXTERNAL;
    ";
    let cat = parse_to_catalog(sql);
    let col = &cat.tables[0].columns[0];
    assert_eq!(col.storage, Some(StorageKind::External));
}

#[test]
fn alter_column_set_compression_lz4() {
    let sql = "
        CREATE TABLE app.t (blob bytea);
        ALTER TABLE app.t ALTER COLUMN blob SET COMPRESSION lz4;
    ";
    let cat = parse_to_catalog(sql);
    let col = &cat.tables[0].columns[0];
    assert_eq!(col.compression, Some(Compression::Lz4));
}

#[test]
fn alter_column_set_compression_default() {
    let sql = "
        CREATE TABLE app.t (blob bytea COMPRESSION lz4);
        ALTER TABLE app.t ALTER COLUMN blob SET COMPRESSION DEFAULT;
    ";
    let cat = parse_to_catalog(sql);
    let col = &cat.tables[0].columns[0];
    assert_eq!(col.compression, None, "DEFAULT means cluster GUC, modeled as None");
}

#[test]
fn alter_column_set_storage_unknown_errors() {
    let sql = "
        CREATE TABLE app.t (doc text);
        ALTER TABLE app.t ALTER COLUMN doc SET STORAGE BOGUS;
    ";
    let err = parse_to_catalog_err(sql);
    assert!(err.to_string().contains("STORAGE"));
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p pgevolve-core --lib parse
cargo test -p pgevolve-core --tests
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add -p crates/pgevolve-core/src/parse/
git commit -m "$(cat <<'EOF'
feat(parse): STORAGE and COMPRESSION column attributes

Source parser accepts both surface forms:

  CREATE TABLE t (doc text STORAGE EXTERNAL COMPRESSION lz4);
  ALTER TABLE t ALTER COLUMN doc SET STORAGE EXTERNAL;
  ALTER TABLE t ALTER COLUMN doc SET COMPRESSION lz4;
  ALTER TABLE t ALTER COLUMN doc SET COMPRESSION DEFAULT;

Decode helpers reject unknown storage strategies / codecs with a
structural ParseError; the AST-side validation in pg_query catches
keyword typos earlier, but the explicit error is more useful than
"missing variant" in the IR.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 3 — Catalog reader

Extend `COLUMNS_QUERY` with `attstorage` + `attcompression`; decode the chars in `catalog/rows.rs`; populate the IR fields in `catalog/assemble/tables.rs`.

**Files modified:** `crates/pgevolve-core/src/catalog/queries/shared.rs`, `crates/pgevolve-core/src/catalog/rows.rs`, `crates/pgevolve-core/src/catalog/assemble/tables.rs`.

### Task 3.1: Extend `COLUMNS_QUERY`

- [ ] **Step 1: Add two columns to the `SELECT` list**

In `crates/pgevolve-core/src/catalog/queries/shared.rs`, in `COLUMNS_QUERY` (currently around lines 64–110), add after `comment`:

```sql
  a.attstorage::text                                AS attstorage,
  a.attcompression::text                            AS attcompression,
```

Cast both to `text` for stable row-decoding regardless of how the `char` type is represented on the wire.

- [ ] **Step 2: Verify no per-version branch is needed**

`attstorage` has been in `pg_attribute` since PG ≤9. `attcompression` was added in PG 14 — pgevolve's MSRV. Confirm `crates/pgevolve-core/src/catalog/queries/{pg14,pg15,pg16,pg17}.rs` either share `COLUMNS_QUERY` or each have an analogous query that also needs the additions:

```bash
grep -n "attstorage\|attcompression\|COLUMNS_QUERY" crates/pgevolve-core/src/catalog/queries/pg14.rs crates/pgevolve-core/src/catalog/queries/pg15.rs crates/pgevolve-core/src/catalog/queries/pg16.rs crates/pgevolve-core/src/catalog/queries/pg17.rs
```

If those files reference `shared::COLUMNS_QUERY` directly, no further changes. If they have their own column SELECTs, repeat the two new lines in each.

### Task 3.2: Decode `attstorage` / `attcompression` in `rows.rs`

- [ ] **Step 1: Add decoder helpers**

In `crates/pgevolve-core/src/catalog/rows.rs`, near where other char-decode helpers live (search for `attidentity` decoding to find the right neighborhood):

```rust
fn decode_attstorage(raw: &str) -> Result<StorageKind, CatalogError> {
    match raw {
        "p" => Ok(StorageKind::Plain),
        "e" => Ok(StorageKind::External),
        "x" => Ok(StorageKind::Extended),
        "m" => Ok(StorageKind::Main),
        other => Err(CatalogError::Structural(format!(
            "unknown attstorage value {other:?}"
        ))),
    }
}

fn decode_attcompression(raw: &str) -> Option<Compression> {
    match raw {
        "p" => Some(Compression::Pglz),
        "l" => Some(Compression::Lz4),
        // "" or "\0" → cluster default → None
        _ => None,
    }
}
```

Adjust `CatalogError` variant naming to match whatever exists in `catalog/error.rs` (the project recently removed unwrap/expect, so there's a typed error to use).

- [ ] **Step 2: Add the two new columns to the row struct used for `COLUMNS_QUERY`**

Find the struct that decodes `COLUMNS_QUERY` rows in `rows.rs` (it'll have fields like `attidentity`, `attgenerated`, `collation_name`). Add:

```rust
pub attstorage: String,
pub attcompression: String,
```

And in the `From<&Row>` / `FromRow` impl, map them from the SQL columns:

```rust
attstorage: r.try_get("attstorage")?,
attcompression: r.try_get("attcompression")?,
```

Use whatever access pattern surrounding fields use (`try_get`, `get`, `decode_text`, etc.).

- [ ] **Step 3: Run catalog-reader tests**

```bash
cargo test -p pgevolve-core --lib catalog
```

Expected: green. If decoder errors trip on existing fixtures, double-check the cast in the query is producing the expected single-char string.

### Task 3.3: Populate `Column.storage` and `Column.compression` in `assemble/tables.rs`

- [ ] **Step 1: Wire decoded fields into the `Column` constructor**

In `crates/pgevolve-core/src/catalog/assemble/tables.rs`, find the function that constructs `Column` from a column-row (likely `build_column` or similar, near `build_tables`). Where it currently sets `collation` / `comment`, add:

```rust
    storage: Some(decode_attstorage(&row.attstorage)?),
    compression: decode_attcompression(&row.attcompression),
```

Use `Some(...)` for storage because the canon pass will strip type-defaults to `None` later. For compression, the decoder already returns `Option<Compression>` (None for cluster default).

- [ ] **Step 2: Add an integration-ish test**

In `crates/pgevolve-core/tests/` (or wherever catalog-read tests live), add a Docker-gated test similar to existing ones:

```rust
#[tokio::test]
#[cfg(feature = "docker")]
async fn catalog_reader_decodes_storage_and_compression() {
    let pg = ephemeral_pg().await;
    pg.exec("CREATE SCHEMA app").await;
    pg.exec("CREATE TABLE app.t (
        plain_int  int,
        ext_text   text STORAGE EXTERNAL,
        lz4_bytea  bytea COMPRESSION lz4
    )").await;
    let cat = read_catalog(&pg, &["app"]).await;
    let cols = &cat.tables[0].columns;

    // After canon: int's PLAIN default is stripped.
    assert_eq!(cols[0].storage, None);
    // EXTERNAL on text != EXTENDED default → preserved.
    assert_eq!(cols[1].storage, Some(StorageKind::External));
    // EXTENDED on bytea = default → stripped.
    assert_eq!(cols[2].storage, None);
    // lz4 compression preserved.
    assert_eq!(cols[2].compression, Some(Compression::Lz4));
}
```

Use the existing ephemeral-PG test helper (it's in `pgevolve-testkit`). Mirror the setup of any nearby catalog-reader Docker test.

- [ ] **Step 3: Run catalog tests (with Docker if available, otherwise just lib)**

```bash
cargo test -p pgevolve-core --lib catalog
# If Docker available:
cargo test -p pgevolve-core --tests --features docker catalog_reader_decodes_storage
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add -p crates/pgevolve-core/src/catalog/
git commit -m "$(cat <<'EOF'
feat(catalog): read attstorage + attcompression

Extends COLUMNS_QUERY with two pg_attribute fields and populates the
new IR fields on Column. Decoder maps single-char pg values to the
StorageKind / Compression enums; canon strips type-default storage
downstream.

attcompression was added in PG 14 (our MSRV), so no per-version
query split is needed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 4 — Differ

Add `TableOp::SetColumnStorage` / `SetColumnCompression`, extend `diff_column` to emit them, add matching `StepKind` variants.

**Files modified:** `crates/pgevolve-core/src/diff/table_op.rs`, `crates/pgevolve-core/src/diff/columns.rs`, `crates/pgevolve-core/src/plan/raw_step.rs`.

### Task 4.1: Add `TableOp` variants

- [ ] **Step 1: Extend the `TableOp` enum**

In `crates/pgevolve-core/src/diff/table_op.rs` (around line 27 the enum starts), add after `SetColumnComment`:

```rust
    /// Change a column's TOAST storage strategy.
    SetColumnStorage {
        name: Identifier,
        /// Previous storage. Caller resolves `Column.storage = None` to
        /// the type default before emitting, so both sides are explicit.
        /// Carried so the lint rule (Stage 6) can detect downgrades
        /// without needing to re-derive the previous state.
        from: StorageKind,
        /// New storage. Same resolution rule as `from`.
        to: StorageKind,
    },
    /// Change a column's TOAST compression codec.
    SetColumnCompression {
        name: Identifier,
        /// New compression. `None` means "use cluster default" — emits
        /// `SET COMPRESSION DEFAULT` at render time.
        compression: Option<Compression>,
    },
```

Import `StorageKind` and `Compression` at the top of the file.

### Task 4.2: Add `StepKind` variants

- [ ] **Step 1: Extend the `StepKind` enum**

In `crates/pgevolve-core/src/plan/raw_step.rs` (around line 33), add after `SetColumnGenerated`:

```rust
    SetColumnStorage,
    SetColumnCompression,
```

The enum has a round-trip serialization test around line 209-242; extend the list of variants tested to include the two new ones.

### Task 4.3: Emit `SetColumnStorage` / `SetColumnCompression` from `diff_column`

- [ ] **Step 1: Extend `diff_column`**

In `crates/pgevolve-core/src/diff/columns.rs`, after the `comment` block (around line 156-164), add:

```rust
    // Storage: resolve None to the type default on both sides so the
    // emitted op is always explicit. Carries both `from` and `to` so the
    // lint rule can detect downgrades.
    {
        let from = target.storage.unwrap_or_else(|| {
            crate::ir::canon::filter_pg_defaults::type_default_storage(&target.ty)
        });
        let to = source.storage.unwrap_or_else(|| {
            crate::ir::canon::filter_pg_defaults::type_default_storage(&source.ty)
        });
        if from != to {
            out.push(TableOpEntry {
                op: TableOp::SetColumnStorage {
                    name: target.name.clone(),
                    from,
                    to,
                },
                destructiveness: Destructiveness::Safe,
            });
        }
    }

    if target.compression != source.compression {
        out.push(TableOpEntry {
            op: TableOp::SetColumnCompression {
                name: target.name.clone(),
                compression: source.compression,
            },
            destructiveness: Destructiveness::Safe,
        });
    }
```

This requires making `type_default_storage` (defined in Stage 1.3) public to its crate. Edit `filter_pg_defaults.rs` to change `fn type_default_storage(...)` → `pub(crate) fn type_default_storage(...)`. Re-export from `canon/mod.rs` if needed.

- [ ] **Step 2: Add unit tests in `diff/columns.rs::tests`**

Extend the existing `col()` helper to accept storage/compression (or add a new helper). Then:

```rust
#[test]
fn storage_change_emits_safe_op() {
    let mut from_col = col("doc", ColumnType::Text, true);
    from_col.storage = Some(StorageKind::Extended); // text default → canon would strip
    let mut to_col = col("doc", ColumnType::Text, true);
    to_col.storage = Some(StorageKind::External);
    let target = tbl(vec![from_col]);
    let source = tbl(vec![to_col]);
    let ops = diff_one(&target, &source);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0].op,
        TableOp::SetColumnStorage {
            from: StorageKind::Extended,
            to: StorageKind::External,
            ..
        }));
    assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
}

#[test]
fn storage_none_vs_type_default_is_noop() {
    let from = col("doc", ColumnType::Text, true);
    let mut to = col("doc", ColumnType::Text, true);
    to.storage = Some(StorageKind::Extended); // text default
    let target = tbl(vec![from]);
    let source = tbl(vec![to]);
    let ops = diff_one(&target, &source);
    assert!(ops.is_empty(),
        "None and Some(type_default) must collapse to the same effective storage");
}

#[test]
fn compression_change_emits_safe_op() {
    let mut from = col("blob", ColumnType::Bytea, true);
    from.compression = Some(Compression::Pglz);
    let mut to = col("blob", ColumnType::Bytea, true);
    to.compression = Some(Compression::Lz4);
    let target = tbl(vec![from]);
    let source = tbl(vec![to]);
    let ops = diff_one(&target, &source);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0].op,
        TableOp::SetColumnCompression { compression: Some(Compression::Lz4), .. }));
}

#[test]
fn compression_to_cluster_default_emits_none() {
    let mut from = col("blob", ColumnType::Bytea, true);
    from.compression = Some(Compression::Lz4);
    let to = col("blob", ColumnType::Bytea, true);
    let target = tbl(vec![from]);
    let source = tbl(vec![to]);
    let ops = diff_one(&target, &source);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0].op,
        TableOp::SetColumnCompression { compression: None, .. }));
}
```

Update the `col()` helper to set `storage: None, compression: None` by default and other call sites accordingly.

- [ ] **Step 3: Run differ tests**

```bash
cargo test -p pgevolve-core --lib diff
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add -p crates/pgevolve-core/src/diff/ crates/pgevolve-core/src/plan/raw_step.rs crates/pgevolve-core/src/ir/canon/
git commit -m "$(cat <<'EOF'
feat(diff): TableOp::SetColumnStorage + SetColumnCompression

Two new safe TableOps. Storage diff resolves None on either side to
the type default before comparing, so an author writing the explicit
default still gets a no-op diff. Compression diff preserves None
(cluster GUC) and renders as `SET COMPRESSION DEFAULT`.

Both ops are non-destructive: PG executes them as catalog updates
with no heap rewrite.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 5 — Render / emit

Add the SQL helpers and emit handlers, plus inline rendering in `render_table` (the source-side render path used by shadow-validate).

**Files modified:** `crates/pgevolve-core/src/plan/rewrite/sql.rs`, `crates/pgevolve-core/src/plan/rewrite/emit/table.rs`, `crates/pgevolve-core/src/render/table.rs` (or wherever `render_table` lives).

### Task 5.1: Add SQL helper functions

- [ ] **Step 1: Add `alter_column_set_storage` and `alter_column_set_compression`**

In `crates/pgevolve-core/src/plan/rewrite/sql.rs`, add near the other `alter_column_*` helpers (around line 240):

```rust
pub fn alter_column_set_storage(
    qname: &QualifiedName,
    column: &Identifier,
    storage: StorageKind,
) -> String {
    let keyword = match storage {
        StorageKind::Plain => "PLAIN",
        StorageKind::External => "EXTERNAL",
        StorageKind::Extended => "EXTENDED",
        StorageKind::Main => "MAIN",
    };
    format!(
        "ALTER TABLE {} ALTER COLUMN {} SET STORAGE {};",
        qname.render_sql(),
        column.render_sql(),
        keyword,
    )
}

pub fn alter_column_set_compression(
    qname: &QualifiedName,
    column: &Identifier,
    compression: Option<Compression>,
) -> String {
    let keyword = match compression {
        None => "DEFAULT",
        Some(Compression::Pglz) => "pglz",
        Some(Compression::Lz4) => "lz4",
    };
    format!(
        "ALTER TABLE {} ALTER COLUMN {} SET COMPRESSION {};",
        qname.render_sql(),
        column.render_sql(),
        keyword,
    )
}
```

Import `StorageKind` and `Compression` at the top.

- [ ] **Step 2: Add SQL helper tests**

In `sql.rs::tests` (or wherever the existing `alter_column_*` helpers are tested):

```rust
#[test]
fn renders_set_storage_external() {
    let s = alter_column_set_storage(&qn("app", "t"), &id("c"), StorageKind::External);
    assert_eq!(s, "ALTER TABLE app.t ALTER COLUMN c SET STORAGE EXTERNAL;");
}

#[test]
fn renders_set_compression_lz4() {
    let s = alter_column_set_compression(&qn("app", "t"), &id("c"), Some(Compression::Lz4));
    assert_eq!(s, "ALTER TABLE app.t ALTER COLUMN c SET COMPRESSION lz4;");
}

#[test]
fn renders_set_compression_default() {
    let s = alter_column_set_compression(&qn("app", "t"), &id("c"), None);
    assert_eq!(s, "ALTER TABLE app.t ALTER COLUMN c SET COMPRESSION DEFAULT;");
}
```

### Task 5.2: Wire emit handlers in `emit/table.rs`

- [ ] **Step 1: Add two `match` arms**

In `crates/pgevolve-core/src/plan/rewrite/emit/table.rs`, after the `SetColumnComment` arm (around line 204), add:

```rust
        TableOp::SetColumnStorage { name, from: _, to } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnStorage,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_set_storage(qname, &name, to),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetColumnCompression { name, compression } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnCompression,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_set_compression(qname, &name, compression),
            transactional: TransactionConstraint::InTransaction,
        }),
```

### Task 5.3: Render inline `STORAGE`/`COMPRESSION` in `render_table`

The source-side render path (used by shadow-validate's `cross_check`) emits CREATE TABLE statements from the IR. Add inline rendering for the new fields.

- [ ] **Step 1: Locate the column-render function**

```bash
rg -n "fn render_column\|fn column_def\|fn render_table" crates/pgevolve-core/src/render/
```

Open the file containing the column-render helper (likely `crates/pgevolve-core/src/render/table.rs` or `render/column.rs`).

- [ ] **Step 2: Emit inline clauses after collation, before COMMENT**

In the column-render helper, after collation:

```rust
    if let Some(storage) = col.storage {
        write!(out, " STORAGE {}", match storage {
            StorageKind::Plain => "PLAIN",
            StorageKind::External => "EXTERNAL",
            StorageKind::Extended => "EXTENDED",
            StorageKind::Main => "MAIN",
        })?;
    }
    if let Some(compression) = col.compression {
        write!(out, " COMPRESSION {}", match compression {
            Compression::Pglz => "pglz",
            Compression::Lz4 => "lz4",
        })?;
    }
```

**Important:** `STORAGE` inline in `CREATE TABLE` column requires PG 16+. The render path is used by `cross_check` against an ephemeral PG (which is whatever version the test container uses). If the ephemeral PG is PG 14/15, the render must use the ALTER TABLE form instead. Two ways to handle:

(a) **Always inline.** Shadow-validate runs against the user's target PG; if it's PG 14/15 and the IR has explicit `storage`, the shadow apply will fail with a clear PG error message — which is the correct semantics (the user wrote unsupported syntax for their target). Recommended; mirrors how pgevolve handles other PG-16-only features.

(b) **Conditional render.** Pass the target PG version into the render function and choose inline vs. trailing ALTER. More work, no real benefit.

Go with (a). Document the choice in a one-line comment.

- [ ] **Step 3: Add render tests**

In `render/table.rs::tests` (mirror the pattern of existing column-render tests):

```rust
#[test]
fn renders_inline_storage_external() {
    let mut c = col("doc", ColumnType::Text);
    c.storage = Some(StorageKind::External);
    let s = render_column(&c);
    assert!(s.contains("STORAGE EXTERNAL"), "got: {s}");
}

#[test]
fn renders_inline_compression_lz4() {
    let mut c = col("blob", ColumnType::Bytea);
    c.compression = Some(Compression::Lz4);
    let s = render_column(&c);
    assert!(s.contains("COMPRESSION lz4"), "got: {s}");
}

#[test]
fn renders_no_clauses_when_none() {
    let c = col("plain", ColumnType::Text);
    let s = render_column(&c);
    assert!(!s.contains("STORAGE"));
    assert!(!s.contains("COMPRESSION"));
}
```

### Task 5.4: Run render + emit tests; commit

- [ ] **Step 1: Test**

```bash
cargo test -p pgevolve-core --lib plan::rewrite
cargo test -p pgevolve-core --lib render
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all green.

- [ ] **Step 2: Commit**

```bash
git add -p crates/pgevolve-core/src/plan/rewrite/ crates/pgevolve-core/src/render/
git commit -m "$(cat <<'EOF'
feat(render): emit STORAGE + COMPRESSION

Plan emit handlers turn TableOp::SetColumnStorage / SetColumnCompression
into ALTER TABLE … ALTER COLUMN SET STORAGE/COMPRESSION steps —
non-destructive, InTransaction.

render_column also emits inline STORAGE/COMPRESSION clauses for the
source-render path (shadow-validate cross_check). Inline STORAGE is
PG 16+ syntax; on older targets the shadow apply will surface a
clear PG error, which is the correct semantics — the user wrote
unsupported syntax for their target.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 6 — Lint rules

Two new universal lint rules, each in its own file under `lint/rules/`.

**Files created:** `crates/pgevolve-core/src/lint/rules/storage_downgrade_not_retroactive.rs`, `crates/pgevolve-core/src/lint/rules/compression_change_not_retroactive.rs`. **Modified:** `crates/pgevolve-core/src/lint/rules/mod.rs`, `crates/pgevolve-core/src/lint/universal.rs` (the dispatcher).

### Task 6.1: `storage-downgrade-not-retroactive`

- [ ] **Step 1: Read an existing rule for the pattern**

```bash
cat crates/pgevolve-core/src/lint/rules/column_position_drift.rs
```

Note the signature, the `Finding` shape, and the test pattern.

- [ ] **Step 2: Create the new rule file**

`crates/pgevolve-core/src/lint/rules/storage_downgrade_not_retroactive.rs`:

```rust
//! Warns when a SET STORAGE change reduces toastability. PG accepts the
//! change but existing rows keep their current placement until the next
//! UPDATE; authors expecting retroactive compaction usually want
//! VACUUM FULL or a table rewrite, neither of which pgevolve emits.

use crate::diff::changeset::Changeset;
use crate::diff::table_op::TableOp;
use crate::ir::column::StorageKind;
use crate::lint::finding::{Finding, Severity};

/// Rule ID emitted on the finding; matches the file name.
pub const RULE_ID: &str = "storage-downgrade-not-retroactive";

pub(crate) fn check(changeset: &Changeset) -> Vec<Finding> {
    let mut findings = Vec::new();
    for change in changeset.iter_table_ops() {
        if let TableOp::SetColumnStorage { name, from, to } = change.op()
            && is_downgrade(*from, *to)
        {
            findings.push(Finding {
                rule_id: RULE_ID.into(),
                severity: Severity::Warning,
                message: format!(
                    "column {} STORAGE {} → {} is not retroactive; existing TOASTed values \
                     remain in their current placement until rewritten by UPDATE or VACUUM FULL",
                    name,
                    storage_name(*from),
                    storage_name(*to),
                ),
                target: change.qname().clone(),
            });
        }
    }
    findings
}

fn is_downgrade(from: StorageKind, to: StorageKind) -> bool {
    // Order from most-toastable to least.
    fn rank(s: StorageKind) -> u8 {
        match s {
            StorageKind::External => 3, // out-of-line, no compress
            StorageKind::Extended => 2, // out-of-line + compress
            StorageKind::Main => 1,     // compress inline, out-of-line as last resort
            StorageKind::Plain => 0,    // inline only
        }
    }
    rank(to) < rank(from)
}

fn storage_name(s: StorageKind) -> &'static str {
    match s {
        StorageKind::Plain => "PLAIN",
        StorageKind::External => "EXTERNAL",
        StorageKind::Extended => "EXTENDED",
        StorageKind::Main => "MAIN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Construct a minimal Changeset with one SetColumnStorage op and assert
    // the rule's behavior. Mirror the test pattern in
    // column_position_drift.rs.

    #[test]
    fn external_to_main_fires() {
        let cs = changeset_with_storage_change(StorageKind::External, StorageKind::Main);
        let f = check(&cs);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule_id, RULE_ID);
        assert_eq!(f[0].severity, Severity::Warning);
    }

    #[test]
    fn plain_to_extended_does_not_fire() {
        let cs = changeset_with_storage_change(StorageKind::Plain, StorageKind::Extended);
        assert!(check(&cs).is_empty());
    }

    #[test]
    fn no_storage_change_no_finding() {
        let cs = changeset_with_no_changes();
        assert!(check(&cs).is_empty());
    }

    // Helpers — implement using whatever Changeset constructor the
    // existing lint-rule tests use. If there's a `Changeset::new_for_test`
    // or builder pattern, use it; otherwise construct a Changeset value
    // directly.
}
```

**Note:** the rule reads `from` directly off the `TableOp::SetColumnStorage` variant — that field is defined in Stage 4 Task 4.1 precisely so this rule (and any future ones) can detect transitions without extra plumbing.

- [ ] **Step 3: Register the rule in `mod.rs`**

In `crates/pgevolve-core/src/lint/rules/mod.rs`:

```rust
pub mod storage_downgrade_not_retroactive;
```

In `crates/pgevolve-core/src/lint/universal.rs` (the dispatcher), find the list of rule calls and add:

```rust
findings.extend(rules::storage_downgrade_not_retroactive::check(changeset));
```

Mirror the form of the surrounding `findings.extend(...)` calls — if the dispatcher passes `(catalog, changeset)` or some other tuple, match it.

- [ ] **Step 4: Run lint tests**

```bash
cargo test -p pgevolve-core --lib lint::rules::storage_downgrade_not_retroactive
```

Expected: green.

### Task 6.2: `compression-change-not-retroactive`

- [ ] **Step 1: Create the rule file**

`crates/pgevolve-core/src/lint/rules/compression_change_not_retroactive.rs`:

```rust
//! Warns on any SET COMPRESSION change. Existing TOASTed values keep
//! their original codec; only new/updated rows get the new codec.

use crate::diff::changeset::Changeset;
use crate::diff::table_op::TableOp;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "compression-change-not-retroactive";

pub(crate) fn check(changeset: &Changeset) -> Vec<Finding> {
    let mut findings = Vec::new();
    for change in changeset.iter_table_ops() {
        if let TableOp::SetColumnCompression { name, .. } = change.op() {
            findings.push(Finding {
                rule_id: RULE_ID.into(),
                severity: Severity::Warning,
                message: format!(
                    "column {} compression change is not retroactive; existing TOASTed values \
                     keep their original codec until rewritten by UPDATE or VACUUM FULL",
                    name,
                ),
                target: change.qname().clone(),
            });
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::column::Compression;

    #[test]
    fn any_compression_change_fires() {
        let cs = changeset_with_compression_change(Some(Compression::Pglz), Some(Compression::Lz4));
        let f = check(&cs);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule_id, RULE_ID);
    }

    #[test]
    fn set_to_default_also_fires() {
        let cs = changeset_with_compression_change(Some(Compression::Lz4), None);
        assert_eq!(check(&cs).len(), 1);
    }

    #[test]
    fn no_change_no_finding() {
        let cs = changeset_with_no_changes();
        assert!(check(&cs).is_empty());
    }
}
```

- [ ] **Step 2: Register in `mod.rs` and dispatcher**

```rust
// lint/rules/mod.rs:
pub mod compression_change_not_retroactive;

// lint/universal.rs:
findings.extend(rules::compression_change_not_retroactive::check(changeset));
```

- [ ] **Step 3: Run**

```bash
cargo test -p pgevolve-core --lib lint
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/lint/
git commit -m "$(cat <<'EOF'
feat(lint): storage + compression non-retroactive warnings

Two new universal rules:

  storage-downgrade-not-retroactive — fires when SetColumnStorage
  reduces toastability (e.g. EXTERNAL → MAIN). Existing TOASTed
  values aren't rewritten until UPDATE or VACUUM FULL.

  compression-change-not-retroactive — fires on any SetColumnCompression
  change for the same reason; existing TOASTed values keep their
  original codec.

Both are waivable warnings. Mirrors the per-rule-file layout
established by the Stage 5.2 split.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 7 — Conformance fixtures

Five fixtures under `crates/pgevolve-conformance/tests/cases/objects/columns/`.

### Task 7.1: Create the five fixture directories

For each fixture below, create:
- `before.sql` — the existing schema (CREATE TABLE statements).
- `after.sql` — the target schema (CREATE TABLE statements expressing the desired state).
- `fixture.toml` — metadata.
- `expected/` — empty directory; bless populates it.

Mirror the file layout of an existing fixture for any unclear field. Open `crates/pgevolve-conformance/tests/cases/objects/columns/set-default/` as a reference.

- [ ] **Step 1: `set-storage-external/`**

`before.sql`:
```sql
CREATE SCHEMA app;
CREATE TABLE app.docs (
    id   bigint PRIMARY KEY,
    body text
);
```

`after.sql`:
```sql
CREATE SCHEMA app;
CREATE TABLE app.docs (
    id   bigint PRIMARY KEY,
    body text STORAGE EXTERNAL
);
```

`fixture.toml`:
```toml
[meta]
title     = "ALTER COLUMN SET STORAGE EXTERNAL — text body forced out-of-line, no compression"
authoring = "objects"
spec_refs = ["objects.column.storage"]

[pg]
min = 14
max = 17

[expect.diff]
contains = [
  "app.docs.columns.body",
]

[expect.plan]
steps = 1
```

- [ ] **Step 2: `set-storage-plain-warning/`**

`before.sql`:
```sql
CREATE SCHEMA app;
CREATE TABLE app.docs (
    id   bigint PRIMARY KEY,
    body text STORAGE EXTERNAL
);
```

`after.sql`:
```sql
CREATE SCHEMA app;
CREATE TABLE app.docs (
    id   bigint PRIMARY KEY,
    body text STORAGE PLAIN
);
```

`fixture.toml`:
```toml
[meta]
title     = "STORAGE downgrade fires storage-downgrade-not-retroactive lint"
authoring = "objects"
spec_refs = ["objects.column.storage", "lint.storage-downgrade"]

[pg]
min = 14
max = 17

[expect.diff]
contains = ["app.docs.columns.body"]

[expect.plan]
steps = 1

[expect.lint]
warnings_contain = ["storage-downgrade-not-retroactive"]
```

If the existing `fixture.toml` schema uses a different key name for expected lint findings, mirror it. Grep `crates/pgevolve-conformance/tests/cases/` for `warnings_contain` or `lint` to find the right key.

- [ ] **Step 3: `set-compression-lz4/`**

`before.sql`:
```sql
CREATE SCHEMA app;
CREATE TABLE app.docs (
    id   bigint PRIMARY KEY,
    body text
);
```

`after.sql`:
```sql
CREATE SCHEMA app;
CREATE TABLE app.docs (
    id   bigint PRIMARY KEY,
    body text COMPRESSION lz4
);
```

`fixture.toml`:
```toml
[meta]
title     = "ALTER COLUMN SET COMPRESSION lz4 — fires non-retroactive warning"
authoring = "objects"
spec_refs = ["objects.column.compression", "lint.compression-change"]

[pg]
min = 14
max = 17

[expect.diff]
contains = ["app.docs.columns.body"]

[expect.plan]
steps = 1

[expect.lint]
warnings_contain = ["compression-change-not-retroactive"]
```

- [ ] **Step 4: `create-table-with-storage/`**

`before.sql`:
```sql
CREATE SCHEMA app;
```

`after.sql`:
```sql
CREATE SCHEMA app;
CREATE TABLE app.docs (
    id   bigint PRIMARY KEY,
    body text STORAGE EXTERNAL COMPRESSION lz4
);
```

`fixture.toml`:
```toml
[meta]
title     = "CREATE TABLE with inline STORAGE + COMPRESSION (PG 16+ syntax for inline storage)"
authoring = "objects"
spec_refs = ["objects.column.storage", "objects.column.compression"]

[pg]
min = 16
max = 17

[expect.diff]
contains = ["app.docs"]

[expect.plan]
steps = 1
```

PG 14/15 don't accept `STORAGE` in inline CREATE TABLE column form. Restrict the fixture to PG 16+; let the conformance harness skip on earlier versions.

- [ ] **Step 5: `set-storage-type-default-noop/`**

`before.sql`:
```sql
CREATE SCHEMA app;
CREATE TABLE app.docs (
    id   bigint PRIMARY KEY,
    body text
);
```

`after.sql`:
```sql
CREATE SCHEMA app;
CREATE TABLE app.docs (
    id   bigint PRIMARY KEY,
    body text STORAGE EXTENDED
);
```

`fixture.toml`:
```toml
[meta]
title     = "Explicit STORAGE EXTENDED on text is the type default → canon strips → no diff"
authoring = "objects"
spec_refs = ["objects.column.storage", "canon.filter_pg_defaults.storage"]

[pg]
min = 14
max = 17

[expect.diff]
empty = true

[expect.plan]
steps = 0
```

If the conformance schema's "no-op" assertion uses a different key (e.g. `[expect.diff] count = 0`), match it.

### Task 7.2: Bless and verify

- [ ] **Step 1: Run bless**

```bash
cargo xtask bless --conformance
```

Expected: populates `expected/` directories for each new fixture.

- [ ] **Step 2: Inspect the blessed output**

For each new fixture, open `expected/` and verify the diff/plan/lint output matches what the spec promises. Specifically:
- `set-storage-external/expected/plan/` should contain one `ALTER TABLE app.docs ALTER COLUMN body SET STORAGE EXTERNAL;` step.
- `set-storage-plain-warning/expected/lint/` should include the new rule ID.
- `set-storage-type-default-noop/expected/plan/` should be empty (or contain the "no changes" sentinel).

If anything's off, fix the implementation (not the bless output).

- [ ] **Step 3: Run the conformance suite**

```bash
cargo test -p pgevolve-conformance
```

Expected: all green. Pre-existing fixtures should not regress.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-conformance/tests/cases/objects/columns/
git commit -m "$(cat <<'EOF'
test(conformance): storage + compression fixtures

Five new fixtures under objects/columns/:

  set-storage-external — happy path, ALTER SET STORAGE EXTERNAL.
  set-storage-plain-warning — downgrade fires lint warning.
  set-compression-lz4 — codec change fires non-retroactive warning.
  create-table-with-storage — inline PG-16+ form, round-trips.
  set-storage-type-default-noop — canon strips type-default, zero diff.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 8 — Property test extension

Extend the existing column-diff property test so generated `Column` values include `storage` and `compression`, with type-aware shrinking.

**Files modified:** wherever `arb_column` (or the equivalent column generator) lives. Likely `crates/pgevolve-testkit/src/arbitrary.rs` or `crates/pgevolve-core/src/diff/columns.rs::tests` (search for `proptest!` or `Arbitrary for Column`).

### Task 8.1: Extend the `Column` generator

- [ ] **Step 1: Locate the generator**

```bash
rg -n "fn arb_column\|Arbitrary for Column\|prop_oneof.*Column" crates/
```

Open the file.

- [ ] **Step 2: Add storage + compression strategies**

```rust
fn arb_storage(ty: &ColumnType) -> impl Strategy<Value = Option<StorageKind>> {
    use crate::ir::canon::filter_pg_defaults::type_default_storage;
    // PG silently accepts STORAGE PLAIN on a fixed-width type but it's
    // meaningless; restrict the generator to storage values that the
    // type actually permits. Toastable types permit all four; non-
    // toastable types only permit PLAIN.
    let is_toastable = matches!(
        type_default_storage(ty),
        StorageKind::Extended | StorageKind::External | StorageKind::Main,
    );
    if is_toastable {
        prop_oneof![
            Just(None),
            Just(Some(StorageKind::Plain)),
            Just(Some(StorageKind::External)),
            Just(Some(StorageKind::Extended)),
            Just(Some(StorageKind::Main)),
        ].boxed()
    } else {
        prop_oneof![Just(None), Just(Some(StorageKind::Plain))].boxed()
    }
}

fn arb_compression() -> impl Strategy<Value = Option<Compression>> {
    prop_oneof![
        Just(None),
        Just(Some(Compression::Pglz)),
        Just(Some(Compression::Lz4)),
    ]
}
```

- [ ] **Step 3: Plumb them into `arb_column`**

In `arb_column`, after the existing strategies for `nullable`, `default`, etc.:

```rust
    (
        arb_identifier(),
        arb_column_type(),
        any::<bool>(),       // nullable
        // ... existing strategies ...
    ).prop_flat_map(|(name, ty, nullable, /* ... */)| {
        (arb_storage(&ty), arb_compression()).prop_map(move |(storage, compression)| {
            Column {
                name: name.clone(),
                ty: ty.clone(),
                nullable,
                default: None,
                identity: None,
                generated: None,
                collation: None,
                storage,
                compression,
                comment: None,
            }
        })
    })
```

Adapt to the existing generator's exact tuple shape.

- [ ] **Step 4: Run the property tests at least 10x per the constitution**

```bash
for i in 1 2 3 4 5 6 7 8 9 10; do
    PROPTEST_CASES=1024 cargo test -p pgevolve-core --release \
        --lib diff::columns::tests::roundtrip 2>&1 | tail -3
done
```

(Substitute the actual property test name if different.) Expected: all 10 runs green.

- [ ] **Step 5: Commit**

```bash
git add -p crates/
git commit -m "$(cat <<'EOF'
test(proptest): include storage + compression in arb_column

Type-aware storage generator: toastable types pick from all four
StorageKind variants; fixed-width types only pick PLAIN. Compression
generator picks from {None, Pglz, Lz4} unconditionally.

Verifies diff round-trip invariants hold across all combinations.
Ran 10× per the constitution's non-determinism rule; all green.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 9 — Docs + version bump

Spec table update, CHANGELOG, version bump, release runbook check.

### Task 9.1: Update `docs/spec/objects.md`

- [ ] **Step 1: Move the row from 📋 Planned to ✅ Supported**

Open `docs/spec/objects.md`. Find line 270 (the "Toast options" row in the "Storage and physical layout" table). Replace:

```markdown
| Toast options (`STORAGE EXTERNAL` / `EXTENDED` / `PLAIN` / `MAIN`) | 📋 Planned, v0.2 | Per-column toast strategy lands with extended `[storage]` modeling. |
```

with:

```markdown
| Toast options (`STORAGE EXTERNAL` / `EXTENDED` / `PLAIN` / `MAIN`) | ✅ Supported | Per-column TOAST storage; canon strips type-default. change_kinds: [alter] |
| TOAST compression (`COMPRESSION pglz` / `lz4`) | ✅ Supported | Per-column codec; canon preserves `None` (cluster `default_toast_compression` GUC). change_kinds: [alter] |
```

### Task 9.2: Update `CHANGELOG.md`

- [ ] **Step 1: Add a `[0.2.1]` section**

In `CHANGELOG.md`, replace:

```markdown
## [Unreleased]

## [0.2.0] — 2026-05-21
```

with:

```markdown
## [Unreleased]

## [0.2.1] — 2026-05-21

### Added

- **Per-column TOAST storage** — `STORAGE { PLAIN | EXTERNAL | EXTENDED | MAIN }` is now a managed `Column` attribute. Source parser accepts both inline (`col text STORAGE EXTERNAL`, PG 16+ syntax) and `ALTER COLUMN SET STORAGE` forms. Differ emits non-destructive `SET STORAGE` steps; canon strips type-default values so explicit and implicit defaults are equivalent.
- **Per-column TOAST compression** — `COMPRESSION { pglz | lz4 }` is now a managed attribute. `None` preserves the cluster `default_toast_compression` GUC; explicit `pglz` or `lz4` overrides it. `SET COMPRESSION DEFAULT` round-trips through the parser as `None`.
- **Two new lint rules:**
  - `storage-downgrade-not-retroactive` — warns when a SET STORAGE change reduces toastability (e.g. `EXTERNAL → MAIN`), since existing TOASTed values aren't rewritten until UPDATE or VACUUM FULL.
  - `compression-change-not-retroactive` — warns on any compression change for the same reason.

### Catalog reader

- `COLUMNS_QUERY` now selects `attstorage` and `attcompression` from `pg_attribute`. No per-version split; both columns are present in PG 14+ (the project MSRV).

### Conformance

- Five new fixtures under `objects/columns/`: `set-storage-external`, `set-storage-plain-warning`, `set-compression-lz4`, `create-table-with-storage`, `set-storage-type-default-noop`.

## [0.2.0] — 2026-05-21
```

### Task 9.3: Bump version

- [ ] **Step 1: Update `[workspace.package].version` in root `Cargo.toml`**

Change `version = "0.2.0"` → `version = "0.2.1"`.

- [ ] **Step 2: Update `crates/pgevolve-core-macros/Cargo.toml` to match**

The macros crate has its own version; bump it in lockstep to `0.2.1`.

- [ ] **Step 3: Refresh `Cargo.lock`**

```bash
cargo build --workspace
```

- [ ] **Step 4: Verify the CHANGELOG version-sync CI gate is satisfied**

```bash
v=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
echo "Cargo.toml version: $v"
grep -q "^## \[$v\] — " CHANGELOG.md && echo "OK" || echo "MISMATCH"
```

Expected: `OK`. If `MISMATCH`, the CHANGELOG header text doesn't match the format the CI gate expects — adjust either the gate's regex (see `2026-05-21-constitution-cleanup.md` Task 2.2) or the CHANGELOG header.

### Task 9.4: Final verification + commit

- [ ] **Step 1: Full workspace verification per constitution §9**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo doc --workspace --no-deps 2>&1 | grep -cE "^warning"   # expect 0
cargo deny check
```

Expected: all green.

- [ ] **Step 2: Run the property test loop one more time (constitution §9 non-determinism rule)**

```bash
for i in 1 2 3 4 5 6 7 8 9 10; do
    PROPTEST_CASES=512 cargo test --workspace --release 2>&1 | tail -3
done
```

Expected: all 10 green.

- [ ] **Step 3: Commit the docs + version bump**

```bash
git add docs/spec/objects.md CHANGELOG.md Cargo.toml Cargo.lock crates/pgevolve-core-macros/Cargo.toml
git commit -m "$(cat <<'EOF'
release: v0.2.1 — column physical attributes

Per-column TOAST storage (PLAIN/EXTERNAL/EXTENDED/MAIN) and
compression (pglz/lz4) are now fully managed. Closes issue #6 and
the long-standing 📋 Planned row in objects.md:270; bundles
COMPRESSION as a sibling attribute for symmetry.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 4: Tag the release (signed, per constitution §9)**

```bash
git tag -s v0.2.1 -m "pgevolve v0.2.1 — column physical attributes"
git push origin main
git push origin v0.2.1
```

- [ ] **Step 5: Close issue #6**

```bash
gh issue close 6 --comment "Shipped in v0.2.1: per-column STORAGE + COMPRESSION are now fully managed (parsed, diffed, rendered, applied, linted, fixture-covered). See CHANGELOG."
```

---

## Done.

After Stage 9, v0.2.1 is shipped end-to-end:
- Two new IR fields, one canon rule, two parser surface forms.
- Catalog reader handles `attstorage` + `attcompression`.
- Differ emits two new safe `TableOp` variants; planner renders them; emitter executes them in-transaction.
- Two new lint warnings cover the non-retroactive footgun.
- Five conformance fixtures, extended proptest coverage, updated docs.
- Signed release tag, closed GitHub issue.

Next plan target (per the established roadmap): the **roles → grants → RLS** v0.3 chain, starting with brainstorming a sub-spec for ROLE / CREATE USER (issue #2).
