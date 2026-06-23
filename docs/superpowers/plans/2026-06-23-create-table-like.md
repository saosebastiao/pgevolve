# CREATE TABLE … (LIKE source) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `CREATE TABLE clone (LIKE source [INCLUDING …])` parse and expand into concrete IR (columns, constraints, indexes, statistics, comments) so it matches what a live Postgres catalog reports — closing GitHub issue #43.

**Architecture:** `build_table` stops rejecting `TableLikeClause`. `process_file` records each clause as a `PendingLike` (capturing how many explicit columns preceded it, for ordering). After all files are parsed and every table is in the catalog, a new deferred pass `apply_pending_likes` resolves each clause against the now-complete catalog: it copies the source table's columns (always) and option-gated attributes/constraints/indexes/statistics/comments, re-deriving auto-generated index/constraint/statistic names via a faithful port of Postgres's `ChooseRelationName` / `ChooseIndexName` so the generated names match the live DB exactly. This mirrors the existing deferred-pass pattern (`apply_pending_fks`, `apply_pending_owners`, …).

**Tech Stack:** Rust 2024, `pg_query` 6 (protobuf AST), the existing `pgevolve-core` IR + diff + conformance suite, ephemeral Postgres fixtures (`testcontainers`) for live-DB verification.

## Global Constraints

- License MIT OR Apache-2.0; no new dependencies (constitution). This feature adds **zero** crates.
- No `unwrap`/`expect` in production code; tests may use them.
- Workspace lints are strict: `clippy::pedantic` + `clippy::nursery` + `-D warnings`. No casual `#[allow]`.
- Newtypes/enums over stringly-typed/bool-bag fields (constitution): the LIKE option bitmask becomes a `TableLikeOptions` newtype with named accessors, not a bare `u32` threaded around.
- Postgres support matrix: 14, 15, 16, 17, 18. Any naming behavior that differs across majors must be covered by conformance fixtures on every major.
- Run tests + clippy locally before each commit; run non-deterministic / live-DB tests ≥10× before relying on them.
- Co-author trailer on every commit:
  `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Commits go directly to `main`, one coherent unit per task.

## Key facts established during investigation (do not re-derive)

- **Error site today:** `crates/pgevolve-core/src/parse/builder/create_stmt.rs:72-77` — the `other =>` arm of the `table_elts` loop rejects any non-`ColumnDef`/`Constraint` element. `node_kind_name` (line 595) already labels `TableLikeClause` as `"LIKE clause"`.
- **AST:** `TableLikeClause { relation: Option<RangeVar>, options: u32, relation_oid: u32 }`. `options` is the **raw C bitmask** (`1<<n`), NOT the protobuf enum ordinal. Verified empirically:
  - bare `LIKE` → `0`
  - `INCLUDING DEFAULTS INCLUDING CONSTRAINTS` → `0xC` (DEFAULTS=`1<<3`, CONSTRAINTS=`1<<2`)
  - `INCLUDING ALL` → `0x7fffffff` (`PG_INT32_MAX`)
  - `EXCLUDING ALL INCLUDING DEFAULTS` → `0x8`
  - bit positions: COMMENTS `1<<0`, COMPRESSION `1<<1`, CONSTRAINTS `1<<2`, DEFAULTS `1<<3`, GENERATED `1<<4`, IDENTITY `1<<5`, INDEXES `1<<6`, STATISTICS `1<<7`, STORAGE `1<<8`.
  - LIKE keeps its **position** among explicit columns: `(x int, LIKE base, y text)` yields 3 `table_elts` in order.
- **Postgres semantics:** A bare `LIKE` copies only **column names, types, and NOT NULL**. Everything else is gated by an `INCLUDING` option. The new table is fully decoupled — Postgres does **not** retain the LIKE relationship, so the live catalog reports concrete columns/constraints/indexes. The source IR must therefore expand to concrete elements.
- **Matching is by name, not structure:** `crates/pgevolve-core/src/diff/indexes.rs:27-30` and `crates/pgevolve-core/src/diff/constraints.rs:28-31` key on `QualifiedName`; `Index::structurally_eq` (`ir/index.rs:137-150`) even includes `qname`. Therefore LIKE-generated index/constraint names **must** match Postgres's auto-naming or every clone diffs (DROP+CREATE) forever.
- **EXCLUDE constraints are not modeled** — `ConstraintKind` has only `PrimaryKey | Unique | ForeignKey | Check` (`ir/constraint.rs`). `INCLUDING INDEXES` can copy an EXCLUDE constraint; this plan emits a precise error for that narrow case rather than silently dropping it (a LIKE source defined in the same schema dir cannot itself carry an EXCLUDE constraint today, so this case is near-unreachable in practice).
- **Pre-existing, OUT OF SCOPE:** unnamed inline `UNIQUE(a,b)` is auto-named `{table}_key` by `constraint_qname` (`create_stmt.rs:500-515`), whereas Postgres names it `{table}_a_b_key`. This is a latent mismatch in *existing* functionality. Do **not** change the inline path here (would re-bless many goldens and is a separate issue). The LIKE path uses the new correct `choose_name` helper; note the divergence in a code comment and file a follow-up issue.

## IR reference (exact field lists the tasks rely on)

```rust
// ir/column.rs (fields confirmed via create_stmt.rs build_column)
struct Column { name: Identifier, ty: ColumnType, nullable: bool,
    default: Option<DefaultExpr>, identity: Option<Identity>,
    generated: Option<Generated>, collation: Option<QualifiedName>,
    storage: Option<StorageKind>, compression: Option<Compression>,
    comment: Option<String> }

// ir/constraint.rs
struct Constraint { qname: QualifiedName, kind: ConstraintKind,
    deferrable: Deferrable, comment: Option<String> }
enum ConstraintKind {
    PrimaryKey { columns: Vec<Identifier>, include: Vec<Identifier> },
    Unique { columns: Vec<Identifier>, include: Vec<Identifier>, nulls_distinct: bool },
    ForeignKey(ForeignKey),
    Check { expression: NormalizedExpr, no_inherit: bool } }

// ir/index.rs
struct Index { qname: QualifiedName, on: IndexParent, method: IndexMethod,
    columns: Vec<IndexColumn>, include: Vec<Identifier>, unique: bool,
    nulls_not_distinct: bool, predicate: Option<NormalizedExpr>,
    tablespace: Option<Identifier>, comment: Option<String>,
    storage: IndexStorageOptions }
enum IndexParent { Table(QualifiedName), Mv(QualifiedName) }
enum IndexColumnExpr { Column(Identifier), Expression(NormalizedExpr) }

// ir/statistic.rs
struct Statistic { qname: QualifiedName, target: QualifiedName,
    kinds: StatisticKinds, columns: Vec<StatisticColumn>,
    statistics_target: Option<i32>, owner: Option<Identifier>,
    comment: Option<String> }

// ir/catalog.rs
struct Catalog { tables: Vec<Table>, indexes: Vec<Index>,
    statistics: Vec<Statistic>, /* … */ }
struct Table { qname, columns: Vec<Column>, constraints: Vec<Constraint>, /* … */ }
```

---

### Task 1: Plumbing — `TableLikeOptions`, `PendingLike`, stop rejecting the clause, copy columns end-to-end

**Files:**
- Create: `crates/pgevolve-core/src/parse/builder/table_like.rs`
- Modify: `crates/pgevolve-core/src/parse/builder/mod.rs` (add `pub mod table_like;`)
- Modify: `crates/pgevolve-core/src/parse/builder/create_stmt.rs:72-77` (replace the `other =>` arm so `TableLikeClause` is skipped, not rejected)
- Modify: `crates/pgevolve-core/src/parse/mod.rs` (add `pending_likes` to `ParseContext` at line 65; extract in the `Statement::CreateTable` arm ~line 431; run `apply_pending_likes` as the **first** finalization step ~line 155, before deferred comments)

**Interfaces:**
- Produces:
  - `pub struct TableLikeOptions(u32)` with `pub const fn new(bits: u32) -> Self` and `pub const fn {comments,compression,constraints,defaults,generated,identity,indexes,statistics,storage}(self) -> bool`.
  - `pub struct PendingLike { pub target: QualifiedName, pub source: QualifiedName, pub options: TableLikeOptions, pub explicit_cols_before: usize, pub location: SourceLocation }`
  - `pub fn extract_pending_likes(create: &CreateStmt, target: &QualifiedName, default_schema: Option<&Identifier>, location: &SourceLocation) -> Result<Vec<PendingLike>, ParseError>`
  - `pub fn apply_pending_likes(catalog: &mut Catalog, pending: Vec<PendingLike>) -> Result<(), ParseError>`
- Consumes: `shared::resolve_qname` for the source `RangeVar`.

- [ ] **Step 1: Write the failing test** (in `table_like.rs` `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::table::Table;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::parse::error::SourceLocation;
    use std::path::PathBuf;

    fn loc() -> SourceLocation { SourceLocation::new(PathBuf::from("t.sql"), 1, 1) }
    fn id(s: &str) -> Identifier { Identifier::from_unquoted(s).unwrap() }
    fn qn(s: &str, n: &str) -> QualifiedName { QualifiedName::new(id(s), id(n)) }

    fn plain_col(name: &str, ty: ColumnType, nullable: bool) -> Column {
        Column { name: id(name), ty, nullable, default: None, identity: None,
            generated: None, collation: None, storage: None, compression: None, comment: None }
    }
    fn empty_table(schema: &str, name: &str) -> Table {
        // Mirror create_stmt build output for an empty table.
        Table { qname: qn(schema, name), columns: vec![], constraints: vec![],
            partition_by: None, partition_of: None, comment: None, owner: None,
            grants: vec![], rls_enabled: false, rls_forced: false, policies: vec![],
            storage: Default::default(), access_method: None, tablespace: None }
    }

    #[test]
    fn bare_like_copies_columns_names_types_notnull() {
        let mut base = empty_table("pub", "base");
        base.columns = vec![
            plain_col("id", ColumnType::Integer, false),   // PK-derived NOT NULL
            plain_col("name", ColumnType::Text, true),
        ];
        let clone = empty_table("pub", "clone");
        let mut cat = Catalog::default();
        cat.tables = vec![base, clone];
        let pend = PendingLike {
            target: qn("pub", "clone"), source: qn("pub", "base"),
            options: TableLikeOptions::new(0), explicit_cols_before: 0, location: loc(),
        };
        apply_pending_likes(&mut cat, vec![pend]).unwrap();
        let clone = cat.tables.iter().find(|t| t.qname == qn("pub", "clone")).unwrap();
        let got: Vec<_> = clone.columns.iter()
            .map(|c| (c.name.as_str().to_string(), c.ty.clone(), c.nullable)).collect();
        assert_eq!(got, vec![
            ("id".into(), ColumnType::Integer, false),
            ("name".into(), ColumnType::Text, true),
        ]);
        assert!(clone.columns.iter().all(|c| c.default.is_none()), "bare LIKE copies no defaults");
    }

    #[test]
    fn like_unknown_source_errors() {
        let mut cat = Catalog::default();
        cat.tables = vec![empty_table("pub", "clone")];
        let pend = PendingLike { target: qn("pub", "clone"), source: qn("pub", "missing"),
            options: TableLikeOptions::new(0), explicit_cols_before: 0, location: loc() };
        assert!(apply_pending_likes(&mut cat, vec![pend]).is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p pgevolve-core --lib table_like`
Expected: FAIL — `table_like` module / `apply_pending_likes` not defined.

- [ ] **Step 3: Write minimal implementation** (`table_like.rs`)

```rust
//! `CREATE TABLE … (LIKE source [INCLUDING …])` resolution.
//!
//! `build_table` skips `TableLikeClause` elements; `process_file` records one
//! [`PendingLike`] per clause. After every table is in the catalog,
//! [`apply_pending_likes`] expands each clause against the source table — the
//! clone is fully decoupled in Postgres, so we must materialize concrete IR.

use pg_query::NodeEnum;
use pg_query::protobuf::CreateStmt;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::column::Column;
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// The `INCLUDING`/`EXCLUDING` option bitmask from a `TableLikeClause`.
/// Stores Postgres's raw `1<<n` bits; `INCLUDING ALL` sets all of them.
#[derive(Debug, Clone, Copy)]
pub struct TableLikeOptions(u32);

impl TableLikeOptions {
    const COMMENTS: u32 = 1 << 0;
    const COMPRESSION: u32 = 1 << 1;
    const CONSTRAINTS: u32 = 1 << 2;
    const DEFAULTS: u32 = 1 << 3;
    const GENERATED: u32 = 1 << 4;
    const IDENTITY: u32 = 1 << 5;
    const INDEXES: u32 = 1 << 6;
    const STATISTICS: u32 = 1 << 7;
    const STORAGE: u32 = 1 << 8;

    #[must_use] pub const fn new(bits: u32) -> Self { Self(bits) }
    #[must_use] pub const fn comments(self) -> bool { self.0 & Self::COMMENTS != 0 }
    #[must_use] pub const fn compression(self) -> bool { self.0 & Self::COMPRESSION != 0 }
    #[must_use] pub const fn constraints(self) -> bool { self.0 & Self::CONSTRAINTS != 0 }
    #[must_use] pub const fn defaults(self) -> bool { self.0 & Self::DEFAULTS != 0 }
    #[must_use] pub const fn generated(self) -> bool { self.0 & Self::GENERATED != 0 }
    #[must_use] pub const fn identity(self) -> bool { self.0 & Self::IDENTITY != 0 }
    #[must_use] pub const fn indexes(self) -> bool { self.0 & Self::INDEXES != 0 }
    #[must_use] pub const fn statistics(self) -> bool { self.0 & Self::STATISTICS != 0 }
    #[must_use] pub const fn storage(self) -> bool { self.0 & Self::STORAGE != 0 }
}

/// One unresolved `LIKE` clause, captured during `process_file`.
#[derive(Debug, Clone)]
pub struct PendingLike {
    pub target: QualifiedName,
    pub source: QualifiedName,
    pub options: TableLikeOptions,
    /// Number of explicitly-listed columns that preceded this clause in the
    /// `CREATE TABLE` element list — the insertion point for copied columns.
    pub explicit_cols_before: usize,
    pub location: SourceLocation,
}

/// Scan a `CREATE TABLE`'s element list for `LIKE` clauses, recording each
/// with the count of explicit columns that preceded it (for ordering).
pub fn extract_pending_likes(
    create: &CreateStmt,
    target: &QualifiedName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Vec<PendingLike>, ParseError> {
    let mut out = Vec::new();
    let mut explicit_cols = 0usize;
    for elt in &create.table_elts {
        match elt.node.as_ref() {
            Some(NodeEnum::ColumnDef(_)) => explicit_cols += 1,
            Some(NodeEnum::TableLikeClause(like)) => {
                let rv = like.relation.as_ref().ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: "LIKE clause missing source relation".into(),
                })?;
                let source = shared::resolve_qname(rv, default_schema, location)?;
                out.push(PendingLike {
                    target: target.clone(),
                    source,
                    options: TableLikeOptions::new(like.options),
                    explicit_cols_before: explicit_cols,
                    location: location.clone(),
                });
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Copy a source column for a bare `LIKE` (Task 1 gates everything off; later
/// tasks add option-driven attributes). Always copies name, type, collation,
/// not-null.
fn copy_column_bare(src: &Column) -> Column {
    Column {
        name: src.name.clone(),
        ty: src.ty.clone(),
        nullable: src.nullable,
        collation: src.collation.clone(),
        default: None,
        identity: None,
        generated: None,
        storage: None,
        compression: None,
        comment: None,
    }
}

/// Resolve every pending `LIKE` against the assembled catalog.
pub fn apply_pending_likes(
    catalog: &mut Catalog,
    pending: Vec<PendingLike>,
) -> Result<(), ParseError> {
    // Group by target so multiple LIKE clauses on one table share an
    // insertion-offset accumulator and a deterministic processing order.
    let mut by_target: std::collections::BTreeMap<QualifiedName, Vec<PendingLike>> =
        std::collections::BTreeMap::new();
    for p in pending {
        by_target.entry(p.target.clone()).or_default().push(p);
    }

    for (target, mut likes) in by_target {
        likes.sort_by_key(|p| p.explicit_cols_before);
        let mut inserted = 0usize;
        for like in likes {
            // Snapshot the source's columns before borrowing the target mutably.
            let src_cols: Vec<Column> = {
                let src = catalog.tables.iter().find(|t| t.qname == like.source)
                    .ok_or_else(|| ParseError::Structural {
                        location: like.location.clone(),
                        message: format!(
                            "LIKE source table {} not found (must be a table declared in the schema)",
                            like.source
                        ),
                    })?;
                src.columns.iter().map(copy_column_bare).collect()
            };
            let n = src_cols.len();
            let tgt = catalog.tables.iter_mut().find(|t| t.qname == target)
                .ok_or_else(|| ParseError::Structural {
                    location: like.location.clone(),
                    message: format!("LIKE target table {target} vanished"),
                })?;
            let at = (like.explicit_cols_before + inserted).min(tgt.columns.len());
            tgt.columns.splice(at..at, src_cols);
            inserted += n;
        }
    }
    Ok(())
}
```

Then edit `create_stmt.rs:72-77`, replacing the catch-all `other =>` arm:

```rust
            // LIKE clauses are recorded separately by `process_file` and
            // resolved in the `apply_pending_likes` deferred pass, because the
            // source table may be declared in a later file.
            NodeEnum::TableLikeClause(_) => {}
            other => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!("unsupported table element: {}", node_kind_name(other)),
                });
            }
```

Edit `builder/mod.rs`: add `pub mod table_like;`.

Edit `parse/mod.rs`:
- Add field to `ParseContext` (line ~73): `pending_likes: Vec<builder::table_like::PendingLike>,`
- In both destructurings of `ParseContext` (lines ~135 and ~390) add `pending_likes,`.
- In the `Statement::CreateTable` arm, right after `build_table` and before `desugar_serials`, extract:
  ```rust
  pending_likes.extend(builder::table_like::extract_pending_likes(
      &s, &table.qname, directives.schema.as_ref(), &location)?);
  ```
- As the **first** finalization step (before the `deferred_comments` loop, ~line 155):
  ```rust
  // Expand CREATE TABLE … (LIKE …) before any pass that references the
  // clone's columns (comments, FKs, resolution).
  builder::table_like::apply_pending_likes(&mut catalog, pending_likes)?;
  ```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p pgevolve-core --lib table_like`
Expected: PASS (both tests).

- [ ] **Step 5: Add an end-to-end parse test** (in `table_like.rs` tests) using a temp dir, proving the parse pipeline no longer errors and orders columns:

```rust
#[test]
fn end_to_end_bare_like_via_parse_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("pub")).unwrap();
    std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
    std::fs::write(dir.path().join("pub/t.sql"),
        "CREATE TABLE pub.base (id int PRIMARY KEY, name text);\n\
         CREATE TABLE pub.clone (LIKE pub.base);\n").unwrap();
    let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
    let clone = cat.tables.iter().find(|t| t.qname.name.as_str() == "clone").unwrap();
    assert_eq!(clone.columns.iter().map(|c| c.name.as_str().to_string()).collect::<Vec<_>>(),
        vec!["id".to_string(), "name".to_string()]);
}
```

Run: `cargo test -p pgevolve-core --lib table_like` → PASS.
(If `Catalog`/`Table` lack `Default`/public constructors, build the test fixtures via `parse_directory_with_locations` on a temp dir instead — adjust the unit tests accordingly; do not add new public constructors just for tests.)

- [ ] **Step 6: Lint + commit**

```bash
cargo clippy -p pgevolve-core --all-targets 2>&1 | tail -5
git add crates/pgevolve-core/src/parse/builder/table_like.rs \
        crates/pgevolve-core/src/parse/builder/mod.rs \
        crates/pgevolve-core/src/parse/builder/create_stmt.rs \
        crates/pgevolve-core/src/parse/mod.rs
git commit -m "feat(parse): expand CREATE TABLE (LIKE source) columns (#43)"
```

---

### Task 2: Column ordering with interleaved explicit columns and multiple LIKE clauses

**Files:**
- Test: `crates/pgevolve-core/src/parse/builder/table_like.rs` (tests)
- Modify: only if Task 1's splice logic proves insufficient (it should already handle this — this task is a verification gate that locks the behavior with tests).

**Interfaces:** unchanged from Task 1.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn like_preserves_position_among_explicit_columns() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("pub")).unwrap();
    std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
    std::fs::write(dir.path().join("pub/t.sql"),
        "CREATE TABLE pub.base (a int, b int);\n\
         CREATE TABLE pub.c (x int, LIKE pub.base, y text);\n").unwrap();
    let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
    let c = cat.tables.iter().find(|t| t.qname.name.as_str() == "c").unwrap();
    assert_eq!(c.columns.iter().map(|c| c.name.as_str().to_string()).collect::<Vec<_>>(),
        vec!["x", "a", "b", "y"]);
}

#[test]
fn multiple_like_clauses_expand_in_order() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("pub")).unwrap();
    std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
    std::fs::write(dir.path().join("pub/t.sql"),
        "CREATE TABLE pub.l (a int);\nCREATE TABLE pub.r (b int);\n\
         CREATE TABLE pub.c (LIKE pub.l, mid int, LIKE pub.r);\n").unwrap();
    let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
    let c = cat.tables.iter().find(|t| t.qname.name.as_str() == "c").unwrap();
    assert_eq!(c.columns.iter().map(|c| c.name.as_str().to_string()).collect::<Vec<_>>(),
        vec!["a", "mid", "b"]);
}
```

- [ ] **Step 2: Run to verify** — Run: `cargo test -p pgevolve-core --lib table_like`. Expected: PASS if Task 1's offset accumulator is correct; FAIL signals an ordering bug to fix in `apply_pending_likes`.
- [ ] **Step 3: Fix if needed** — if the second LIKE lands in the wrong slot, the bug is the `inserted` accumulator vs `explicit_cols_before`; ensure `explicit_cols_before` counts only `ColumnDef` elements (it does) and `at = explicit_cols_before + inserted`.
- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/parse/builder/table_like.rs
git commit -m "test(parse): lock LIKE column ordering with explicit cols + multiple LIKE (#43)"
```

---

### Task 3: Column-attribute options — DEFAULTS, IDENTITY, GENERATED, STORAGE, COMPRESSION

**Files:**
- Modify: `crates/pgevolve-core/src/parse/builder/table_like.rs` (`copy_column_bare` → `copy_column`, taking `TableLikeOptions`)

**Interfaces:**
- Produces: `fn copy_column(src: &Column, opts: TableLikeOptions) -> Column` (replaces `copy_column_bare`; update the call site in `apply_pending_likes` to pass `like.options`).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn including_defaults_and_storage_gate_attributes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("pub")).unwrap();
    std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
    std::fs::write(dir.path().join("pub/t.sql"),
        "CREATE TABLE pub.base (id int DEFAULT 7, doc text STORAGE EXTERNAL);\n\
         CREATE TABLE pub.bare (LIKE pub.base);\n\
         CREATE TABLE pub.full (LIKE pub.base INCLUDING DEFAULTS INCLUDING STORAGE);\n").unwrap();
    let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
    let bare = cat.tables.iter().find(|t| t.qname.name.as_str() == "bare").unwrap();
    assert!(bare.columns[0].default.is_none());
    assert!(bare.columns[1].storage.is_none());
    let full = cat.tables.iter().find(|t| t.qname.name.as_str() == "full").unwrap();
    assert!(full.columns[0].default.is_some(), "INCLUDING DEFAULTS copies default");
    assert!(full.columns[1].storage.is_some(), "INCLUDING STORAGE copies storage");
}
```

- [ ] **Step 2: Run to verify it fails** — Run: `cargo test -p pgevolve-core --lib including_defaults`. Expected: FAIL (defaults not copied yet).
- [ ] **Step 3: Implement `copy_column`**

```rust
fn copy_column(src: &Column, opts: TableLikeOptions) -> Column {
    Column {
        name: src.name.clone(),
        ty: src.ty.clone(),
        nullable: src.nullable,
        collation: src.collation.clone(),
        default:     if opts.defaults()    { src.default.clone() }     else { None },
        identity:    if opts.identity()    { src.identity.clone() }    else { None },
        generated:   if opts.generated()   { src.generated.clone() }   else { None },
        storage:     if opts.storage()     { src.storage }             else { None },
        compression: if opts.compression() { src.compression }         else { None },
        comment:     if opts.comments()    { src.comment.clone() }     else { None },
    }
}
```

Update the call site: `src.columns.iter().map(|c| copy_column(c, like.options)).collect()`. Delete `copy_column_bare`.

- [ ] **Step 4: Run to verify it passes** — Run: `cargo test -p pgevolve-core --lib table_like`. Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/parse/builder/table_like.rs
git commit -m "feat(parse): LIKE INCLUDING DEFAULTS/IDENTITY/GENERATED/STORAGE/COMPRESSION (#43)"
```

> **Note for the verifier:** `COMMENTS` on columns is handled here via `copy_column`. Table-level comment copying (`INCLUDING COMMENTS`) is added in Task 4. `IDENTITY`/`GENERATED` correctness against the live DB is verified in the conformance task (Task 11), since a copied identity column's sequence options must match.

---

### Task 4: INCLUDING COMMENTS — table-level comment

**Files:**
- Modify: `crates/pgevolve-core/src/parse/builder/table_like.rs` (`apply_pending_likes`: copy `src.comment` to target when `opts.comments()`)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn including_comments_copies_table_comment() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("pub")).unwrap();
    std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
    std::fs::write(dir.path().join("pub/t.sql"),
        "CREATE TABLE pub.base (id int);\nCOMMENT ON TABLE pub.base IS 'hi';\n\
         CREATE TABLE pub.c (LIKE pub.base INCLUDING COMMENTS);\n").unwrap();
    let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
    let c = cat.tables.iter().find(|t| t.qname.name.as_str() == "c").unwrap();
    assert_eq!(c.comment.as_deref(), Some("hi"));
}
```

- [ ] **Step 2: Run to verify it fails.** Expected: FAIL (`c.comment` is `None`). **Note:** the source's `COMMENT ON TABLE` is applied in the `deferred_comments` pass, which Task 1 placed *after* `apply_pending_likes`. So at LIKE-expansion time `src.comment` is still `None`. **This task must move the `deferred_comments` loop to run BEFORE `apply_pending_likes`** so the source comment exists when copied — but the clone's own comments (set by COMMENT statements targeting the clone) must still apply. Resolution: run `apply_pending_likes` *between* deferred comments on sources and... simplest correct order: run deferred comments first, then `apply_pending_likes`. A `COMMENT ON TABLE pub.clone` would then run in the same pre-pass and target a table whose columns may not yet exist — but table-level comments don't depend on columns, so this is safe. Move `apply_pending_likes` to run **immediately after** the `deferred_comments` loop (still before `pending_fks`). Update Task 1's placement note accordingly.
- [ ] **Step 3: Implement** — reorder in `parse/mod.rs` (deferred_comments loop, then `apply_pending_likes`); in `apply_pending_likes`, after splicing columns, set `if like.options.comments() { tgt.comment = src_comment.clone(); }` where `src_comment` is snapshotted alongside `src_cols`.
- [ ] **Step 4: Run to verify it passes.** Run: `cargo test -p pgevolve-core --lib table_like`. Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/parse/builder/table_like.rs crates/pgevolve-core/src/parse/mod.rs
git commit -m "feat(parse): LIKE INCLUDING COMMENTS copies table comment (#43)"
```

---

### Task 5: INCLUDING CONSTRAINTS — CHECK constraints

**Files:**
- Modify: `crates/pgevolve-core/src/parse/builder/table_like.rs`

**Design:** Bare LIKE always copies NOT NULL (already on columns via Task 1). `INCLUDING CONSTRAINTS` additionally copies **CHECK** constraints (and NOT NULL, already handled). For each source `Constraint` whose `kind` is `Check`, clone it with a re-derived qname for the clone. **Postgres re-derives the check name from the new table** — verify the exact form in Task 11 against a live DB; the working assumption is `constraint_qname(target, "", "check")`-style (`{table}_check`, with `_check1` … on collision). Use the `choose_name` helper from Task 6 once it exists; for this task, emit the name via a local helper and refine in Task 7 if Task 11 shows a mismatch.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn including_constraints_copies_check() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("pub")).unwrap();
    std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
    std::fs::write(dir.path().join("pub/t.sql"),
        "CREATE TABLE pub.base (n int, CONSTRAINT n_pos CHECK (n > 0));\n\
         CREATE TABLE pub.bare (LIKE pub.base);\n\
         CREATE TABLE pub.c (LIKE pub.base INCLUDING CONSTRAINTS);\n").unwrap();
    let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
    let bare = cat.tables.iter().find(|t| t.qname.name.as_str() == "bare").unwrap();
    assert!(bare.constraints.iter().all(|c| !matches!(c.kind, crate::ir::constraint::ConstraintKind::Check{..})));
    let c = cat.tables.iter().find(|t| t.qname.name.as_str() == "c").unwrap();
    assert_eq!(c.constraints.iter()
        .filter(|c| matches!(c.kind, crate::ir::constraint::ConstraintKind::Check{..})).count(), 1);
}
```

- [ ] **Step 2: Run to verify it fails.** Expected: FAIL (`c` has no check).
- [ ] **Step 3: Implement** — in `apply_pending_likes`, when `like.options.constraints()`, snapshot the source's `Check` constraints, re-derive each one's qname for `target`'s schema/name (preserving the user-given name if the source constraint was explicitly named — Postgres keeps explicit CHECK names; verify in Task 11), and push onto `tgt.constraints`. Skip PK/UNIQUE/FK here (PK/UNIQUE belong to INCLUDING INDEXES; FK is never copied by LIKE).
- [ ] **Step 4: Run to verify it passes.** Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/parse/builder/table_like.rs
git commit -m "feat(parse): LIKE INCLUDING CONSTRAINTS copies CHECK constraints (#43)"
```

---

### Task 6: `choose_name` — port of Postgres `ChooseRelationName` / `ChooseIndexName`

**Files:**
- Create: `crates/pgevolve-core/src/parse/builder/choose_name.rs`
- Modify: `crates/pgevolve-core/src/parse/builder/mod.rs` (`pub(crate) mod choose_name;`)

**Design (from `src/backend/commands/indexcmds.c`):**
- `ChooseIndexNameAddition(colnames)`: concatenate column names with `_`, appending names while the running buffer stays within a budget (Postgres uses a 39-byte buffer, stopping before it would exceed; first column is always included). Expression columns contribute `"expr"` (and `"exprN"` for the Nth expression). Reproduce the budget precisely — verify against live DB in Task 11.
- `ChooseRelationName(name1, name2, label, taken)`: build `name1[_name2]_label`; if the result is already in `taken`, append a decimal counter (`label1`, `label2`, …) — Postgres truncates each component to keep the whole ≤ 63 bytes (NAMEDATALEN-1). Truncation is on **bytes**, on `char_boundary`-safe positions for our purposes (ASCII identifiers dominate; multibyte handled by truncating at a char boundary).
- `IndexNameKind { Pkey, Unique, Plain, Exclude }` → label `pkey | key | idx | excl`. For `Pkey`, Postgres passes no column addition.

**Interfaces:**
- Produces:
  - `pub(crate) enum IndexNameKind { Pkey, Unique, Plain, Exclude }`
  - `pub(crate) struct TakenNames(std::collections::BTreeSet<String>)` with `pub fn from_schema(catalog: &Catalog, schema: &Identifier) -> Self` (seeds with all relation/index/constraint names in that schema) and `pub fn insert(&mut self, name: &str)`.
  - `pub(crate) fn choose_index_name(table: &str, col_names: &[Option<&str>], kind: IndexNameKind, taken: &mut TakenNames) -> String` (inserts the chosen name into `taken` before returning, so successive calls collide correctly).

- [ ] **Step 1: Write failing unit tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn taken(names: &[&str]) -> TakenNames {
        let mut t = TakenNames::default();
        for n in names { t.insert(n); }
        t
    }
    #[test]
    fn pkey_name() {
        let mut t = TakenNames::default();
        assert_eq!(choose_index_name("clone", &[], IndexNameKind::Pkey, &mut t), "clone_pkey");
    }
    #[test]
    fn unique_two_columns() {
        let mut t = TakenNames::default();
        assert_eq!(choose_index_name("clone", &[Some("a"), Some("b")], IndexNameKind::Unique, &mut t),
            "clone_a_b_key");
    }
    #[test]
    fn plain_index_suffix_idx() {
        let mut t = TakenNames::default();
        assert_eq!(choose_index_name("clone", &[Some("a")], IndexNameKind::Plain, &mut t), "clone_a_idx");
    }
    #[test]
    fn collision_appends_counter() {
        let mut t = taken(&["clone_a_key"]);
        assert_eq!(choose_index_name("clone", &[Some("a")], IndexNameKind::Unique, &mut t), "clone_a_key1");
    }
    #[test]
    fn expression_columns_use_expr() {
        let mut t = TakenNames::default();
        assert_eq!(choose_index_name("clone", &[None, Some("a")], IndexNameKind::Plain, &mut t),
            "clone_expr_a_idx");
    }
}
```

- [ ] **Step 2: Run to verify they fail.** Run: `cargo test -p pgevolve-core --lib choose_name`. Expected: FAIL (module absent).
- [ ] **Step 3: Implement `choose_name.rs`** — port the two functions. Sketch:

```rust
use std::collections::BTreeSet;
use crate::identifier::Identifier;
use crate::ir::catalog::Catalog;

#[derive(Clone, Copy)]
pub(crate) enum IndexNameKind { Pkey, Unique, Plain, Exclude }
impl IndexNameKind {
    const fn label(self) -> &'static str {
        match self { Self::Pkey => "pkey", Self::Unique => "key", Self::Plain => "idx", Self::Exclude => "excl" }
    }
}

#[derive(Default)]
pub(crate) struct TakenNames(BTreeSet<String>);
impl TakenNames {
    pub fn from_schema(catalog: &Catalog, schema: &Identifier) -> Self {
        let mut s = BTreeSet::new();
        for t in &catalog.tables { if &t.qname.schema == schema {
            s.insert(t.qname.name.as_str().to_string());
            for c in &t.constraints { if &c.qname.schema == schema { s.insert(c.qname.name.as_str().to_string()); } }
        }}
        for i in &catalog.indexes { if &i.qname.schema == schema { s.insert(i.qname.name.as_str().to_string()); } }
        // statistics share the relation namespace too:
        for st in &catalog.statistics { if &st.qname.schema == schema { s.insert(st.qname.name.as_str().to_string()); } }
        Self(s)
    }
    pub fn insert(&mut self, name: &str) { self.0.insert(name.to_string()); }
    fn contains(&self, name: &str) -> bool { self.0.contains(name) }
}

const NAMEDATALEN: usize = 63; // NAMEDATALEN-1

fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max { return s; }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) { end -= 1; }
    &s[..end]
}

// ChooseIndexNameAddition: join column names with '_' within a budget.
fn name_addition(col_names: &[Option<&str>]) -> String {
    const BUF: usize = 39; // matches PG's NameData-sized accumulator budget
    let mut out = String::new();
    let mut expr_n = 0u32;
    for c in col_names {
        let part = match c {
            Some(name) => (*name).to_string(),
            None => { expr_n += 1; if expr_n == 1 { "expr".into() } else { format!("expr{expr_n}") } }
        };
        if out.is_empty() { out = part; }
        else if out.len() + 1 + part.len() <= BUF { out.push('_'); out.push_str(&part); }
        else { break; }
    }
    out
}

fn choose_relation_name(name1: &str, name2: &str, label: &str, taken: &TakenNames) -> String {
    // base = name1[_name2]_label, truncated so the whole fits NAMEDATALEN.
    let build = |suffix: &str| {
        let label_full = format!("{label}{suffix}");
        // reserve room for separators + label
        let overhead = 1 + (if name2.is_empty() {0} else {name2.len()+1}) + label_full.len();
        let budget1 = NAMEDATALEN.saturating_sub(overhead);
        let n1 = truncate_bytes(name1, budget1);
        if name2.is_empty() { format!("{n1}_{label_full}") }
        else { format!("{n1}_{name2}_{label_full}") }
    };
    let mut candidate = build("");
    let mut i = 0u32;
    while taken.contains(&candidate) {
        i += 1;
        candidate = build(&i.to_string());
    }
    candidate
}

pub(crate) fn choose_index_name(
    table: &str, col_names: &[Option<&str>], kind: IndexNameKind, taken: &mut TakenNames,
) -> String {
    let name = match kind {
        IndexNameKind::Pkey => choose_relation_name(table, "", kind.label(), taken),
        _ => choose_relation_name(table, &name_addition(col_names), kind.label(), taken),
    };
    taken.insert(&name);
    name
}
```

- [ ] **Step 4: Run to verify they pass.** Run: `cargo test -p pgevolve-core --lib choose_name`. Expected: PASS. (Truncation/budget edge cases are pinned against live PG in Task 11; adjust `BUF`/`NAMEDATALEN` constants there if a long-identifier fixture disagrees.)
- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/parse/builder/choose_name.rs crates/pgevolve-core/src/parse/builder/mod.rs
git commit -m "feat(parse): port Postgres ChooseRelationName/ChooseIndexName for LIKE (#43)"
```

---

### Task 7: INCLUDING INDEXES — PRIMARY KEY and UNIQUE constraints; error on EXCLUDE

**Files:**
- Modify: `crates/pgevolve-core/src/parse/builder/table_like.rs`

**Design:** With `INCLUDING INDEXES`, copy the source's PK and UNIQUE *constraints* to the clone, re-deriving names via `choose_index_name` seeded from a `TakenNames::from_schema(catalog, &target.schema)` (so counters match the live DB). PK → `IndexNameKind::Pkey`; UNIQUE → `IndexNameKind::Unique` keyed on its columns. If the source carries an EXCLUDE constraint, return a precise `ParseError::Structural` ("LIKE INCLUDING INDEXES: source X has an EXCLUDE constraint, which pgevolve does not model yet"). (EXCLUDE is not in `ConstraintKind`, so a source defined in the schema dir cannot currently have one — this guard is for catalog-sourced LIKE and future-proofing.)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn including_indexes_copies_pk_and_unique_with_pg_names() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("pub")).unwrap();
    std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
    std::fs::write(dir.path().join("pub/t.sql"),
        "CREATE TABLE pub.base (id int PRIMARY KEY, email text UNIQUE);\n\
         CREATE TABLE pub.c (LIKE pub.base INCLUDING INDEXES);\n").unwrap();
    let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
    let c = cat.tables.iter().find(|t| t.qname.name.as_str() == "c").unwrap();
    use crate::ir::constraint::ConstraintKind::*;
    let names: Vec<_> = c.constraints.iter().map(|k| k.qname.name.as_str().to_string()).collect();
    assert!(names.contains(&"c_pkey".to_string()), "got {names:?}");
    assert!(names.contains(&"c_email_key".to_string()), "got {names:?}");
    assert_eq!(c.constraints.iter().filter(|k| matches!(k.kind, PrimaryKey{..})).count(), 1);
    assert_eq!(c.constraints.iter().filter(|k| matches!(k.kind, Unique{..})).count(), 1);
}
```

- [ ] **Step 2: Run to verify it fails.** Expected: FAIL.
- [ ] **Step 3: Implement** — in `apply_pending_likes`, when `like.options.indexes()`: snapshot the source's PK/UNIQUE constraints; build `TakenNames::from_schema(catalog, &target.schema)` once per target before the loop (and `insert` each generated name); for each, derive columns → `Vec<Option<&str>>` (PK/UNIQUE columns are all `Some`), call `choose_index_name`, clone the `ConstraintKind` with the new qname, push onto target. Error on any EXCLUDE.
- [ ] **Step 4: Run to verify it passes.** Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/parse/builder/table_like.rs
git commit -m "feat(parse): LIKE INCLUDING INDEXES copies PK/UNIQUE constraints (#43)"
```

---

### Task 8: INCLUDING INDEXES — plain (non-constraint) indexes

**Files:**
- Modify: `crates/pgevolve-core/src/parse/builder/table_like.rs`

**Design:** Copy every `Index` in `catalog.indexes` whose `on == IndexParent::Table(source)` and that is **not** backing a PK/UNIQUE constraint (those came via Task 7). Re-derive each index name via `choose_index_name(target.name, &col_names, IndexNameKind::Plain | Unique, …)` (use `Unique` label when `index.unique`), retarget `on` to `IndexParent::Table(target)`, push onto `catalog.indexes`. Column-name extraction: `IndexColumnExpr::Column(id) => Some(id)`, `Expression(_) => None`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn including_indexes_copies_plain_index() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("pub")).unwrap();
    std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
    std::fs::write(dir.path().join("pub/t.sql"),
        "CREATE TABLE pub.base (a int, b int);\nCREATE INDEX ON pub.base (a, b);\n\
         CREATE TABLE pub.c (LIKE pub.base INCLUDING INDEXES);\n").unwrap();
    let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
    let idx: Vec<_> = cat.indexes.iter()
        .filter(|i| i.on.qname().name.as_str() == "c")
        .map(|i| i.qname.name.as_str().to_string()).collect();
    assert_eq!(idx, vec!["c_a_b_idx".to_string()], "got {idx:?}");
}
```

- [ ] **Step 2: Run to verify it fails.** Expected: FAIL (no index on `c`).
- [ ] **Step 3: Implement** — extend `apply_pending_likes`. Snapshot matching indexes before mutating `catalog.indexes`; build them with retargeted `on`/`qname`; share the same `TakenNames` instance as Task 7 so PK/unique/index names don't collide. Append after the per-target column/constraint work.
- [ ] **Step 4: Run to verify it passes.** Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/parse/builder/table_like.rs
git commit -m "feat(parse): LIKE INCLUDING INDEXES copies plain indexes (#43)"
```

---

### Task 9: INCLUDING STATISTICS — extended statistics

**Files:**
- Modify: `crates/pgevolve-core/src/parse/builder/table_like.rs`

**Design:** Copy every `Statistic` whose `target == source` to the clone with `target = clone` and a re-derived `qname`. Postgres auto-names extended stats via `ChooseRelationName(tabname, <col-addition>, "stat", namespace)` → `{table}_{cols}_stat`. Add `IndexNameKind::Stat` (label `"stat"`) to `choose_name.rs`, or a dedicated `choose_statistics_name` wrapper. Verify the exact form in Task 11.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn including_statistics_copies_stats() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("pub")).unwrap();
    std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
    std::fs::write(dir.path().join("pub/t.sql"),
        "CREATE TABLE pub.base (a int, b int);\n\
         CREATE STATISTICS pub.base_stat (ndistinct) ON a, b FROM pub.base;\n\
         CREATE TABLE pub.c (LIKE pub.base INCLUDING STATISTICS);\n").unwrap();
    let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
    let copied: Vec<_> = cat.statistics.iter()
        .filter(|s| s.target.name.as_str() == "c").collect();
    assert_eq!(copied.len(), 1, "expected one copied statistic");
}
```

- [ ] **Step 2: Run to verify it fails.** Expected: FAIL.
- [ ] **Step 3: Implement** — add the `Stat` name kind + copy loop. Re-derive the qname; retarget `target`; push onto `catalog.statistics`.
- [ ] **Step 4: Run to verify it passes.** Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/parse/builder/table_like.rs crates/pgevolve-core/src/parse/builder/choose_name.rs
git commit -m "feat(parse): LIKE INCLUDING STATISTICS copies extended statistics (#43)"
```

---

### Task 10: INCLUDING ALL integration + precise errors (EXCLUDE, non-table source)

**Files:**
- Modify: `crates/pgevolve-core/src/parse/builder/table_like.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn including_all_copies_everything() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("pub")).unwrap();
    std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
    std::fs::write(dir.path().join("pub/t.sql"),
        "CREATE TABLE pub.base (id int PRIMARY KEY DEFAULT 1, n int CHECK (n > 0));\n\
         CREATE INDEX ON pub.base (n);\n\
         CREATE TABLE pub.c (LIKE pub.base INCLUDING ALL);\n").unwrap();
    let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
    let c = cat.tables.iter().find(|t| t.qname.name.as_str() == "c").unwrap();
    assert!(c.columns[0].default.is_some());                       // DEFAULTS
    use crate::ir::constraint::ConstraintKind::*;
    assert!(c.constraints.iter().any(|k| matches!(k.kind, PrimaryKey{..})));  // INDEXES
    assert!(c.constraints.iter().any(|k| matches!(k.kind, Check{..})));       // CONSTRAINTS
    assert!(cat.indexes.iter().any(|i| i.on.qname().name.as_str() == "c"));   // INDEXES (plain)
}

#[test]
fn like_non_table_source_errors_clearly() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("pub")).unwrap();
    std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
    std::fs::write(dir.path().join("pub/t.sql"),
        "CREATE TABLE pub.base (id int);\nCREATE VIEW pub.v AS SELECT id FROM pub.base;\n\
         CREATE TABLE pub.c (LIKE pub.v);\n").unwrap();
    let err = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap_err();
    assert!(format!("{err}").contains("LIKE source"), "got {err}");
}
```

- [ ] **Step 2: Run to verify.** Expected: `including_all_copies_everything` likely PASSES already (all options flow through). `like_non_table_source_errors_clearly` — confirm the not-found branch produces a clear message; a view is not in `catalog.tables`, so the existing "not found" error fires. Refine the message to mention it may be a view/MV (unsupported source kind) if helpful.
- [ ] **Step 3: Implement refinements** if either test fails (e.g. distinguish "exists as a view" from "does not exist").
- [ ] **Step 4: Run full crate tests.** Run: `cargo test -p pgevolve-core`. Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/parse/builder/table_like.rs
git commit -m "feat(parse): LIKE INCLUDING ALL + precise source-kind/EXCLUDE errors (#43)"
```

---

### Task 11: Conformance fixtures — verify LIKE expansion matches live Postgres 14–18

**Files:**
- Create: conformance cases under `crates/pgevolve-conformance/tests/cases/` (follow the existing tier layout; mirror a nearby case directory's structure — `input` schema SQL + `expected/plan.sql`).
- Use: `cargo xtask bless --conformance` to generate goldens, then inspect the diff.

**This is the task that validates every naming assumption** (CHECK names, `ChooseIndexName` truncation/collision, statistics names, identity sequence copying) against a real server. The conformance harness round-trips: declare the source + the LIKE clone in the schema, apply to an ephemeral PG, introspect, and confirm pgevolve's expanded IR produces an **empty diff** against the live catalog (i.e. no spurious DROP/CREATE for the clone's columns/constraints/indexes).

- [ ] **Step 1: Add a Tier case** exercising bare LIKE (the 58-file idiom): `CREATE TABLE etl.app (LIKE etl.lic);` with `etl.lic` having a PK, a unique, a check, a default, and a plain index. Set `PGEVOLVE_TEST_PG_VERSION` per the conformance tips (default 17).
- [ ] **Step 2: Run the conformance suite** for this case on PG 17:

Run: `PGEVOLVE_TEST_PG_VERSION=17 cargo test -p pgevolve-conformance <case_name>`
Expected: the clone produces **no diff** against the introspected catalog. If a constraint/index name mismatches, the diff shows a DROP+CREATE — **that is the signal to fix `choose_name`** (Phase 6) or the CHECK-name derivation (Task 5), not to bless the mismatch.

- [ ] **Step 3: Iterate `choose_name` against reality** — for any mismatch, capture the live name via `psql` against the ephemeral DB (`\d etl.app`) and adjust the port. Re-run until the diff is empty.
- [ ] **Step 4: Repeat on PG 14, 15, 16, 18** (naming has been stable, but the matrix is a hard gate):

Run: `for v in 14 15 16 18; do PGEVOLVE_TEST_PG_VERSION=$v cargo test -p pgevolve-conformance <case_name> || break; done`
Expected: PASS on every major.

- [ ] **Step 5: Add INCLUDING ALL + interleaved-column + multiple-LIKE conformance cases**, bless, verify empty diffs across all majors.
- [ ] **Step 6: Bless and commit**

```bash
cargo xtask bless --conformance
git add crates/pgevolve-conformance/tests/cases/
git commit -m "test(conformance): CREATE TABLE (LIKE …) verified against PG 14–18 (#43)"
```

---

### Task 12: Docs, spec, CHANGELOG, follow-up issue

**Files:**
- Modify: `docs/spec/` (the capability catalogue entry for CREATE TABLE — add the LIKE clause and the supported INCLUDING options; note EXCLUDE-source and non-table-source are rejected).
- Modify: `CHANGELOG.md` (`[Unreleased]` → Added: "`CREATE TABLE … (LIKE source [INCLUDING …])` is now expanded into concrete IR (#43)").
- File: a GitHub follow-up issue for the pre-existing inline-`UNIQUE` `{table}_key` vs `{table}_{col}_key` naming mismatch (see Global facts).

- [ ] **Step 1: Update the spec** entry; list the gated options and the two error cases.
- [ ] **Step 2: Update CHANGELOG.md** under `[Unreleased]`.
- [ ] **Step 3: Run the doc gate**

Run: `RUSTDOCFLAGS="-D warnings" cargo doc -p pgevolve-core --no-deps`
Expected: clean (no broken intra-doc links).

- [ ] **Step 4: File the follow-up issue**

```bash
gh issue create --repo saosebastiao/pgevolve \
  --title "Inline unnamed UNIQUE auto-named {table}_key, not Postgres's {table}_{col}_key" \
  --body "Discovered during #43. constraint_qname (create_stmt.rs) names unnamed UNIQUE constraints '{table}_key'; Postgres uses '{table}_{col}_key'. The LIKE path (now) uses the correct ChooseIndexName port; the inline path still uses the simplified suffix, which can cause a spurious diff against the live catalog for tables with unnamed multi-unique constraints. Reconcile by routing the inline path through choose_name (will re-bless goldens)."
```

- [ ] **Step 5: Final full gate + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets 2>&1 | tail -3
git add docs/ CHANGELOG.md
git commit -m "docs(parse): document CREATE TABLE (LIKE …) support (#43)"
```

---

## Self-Review

**Spec coverage** (issue #43 "Expected": resolve referenced columns honoring INCLUDING/EXCLUDING):
- Columns + NOT NULL → Task 1 ✓ ; ordering/multiple LIKE → Task 2 ✓
- DEFAULTS/IDENTITY/GENERATED/STORAGE/COMPRESSION → Task 3 ✓ ; COMMENTS → Tasks 3 (cols) + 4 (table) ✓
- CONSTRAINTS (CHECK) → Task 5 ✓
- INDEXES (PK/UNIQUE) → Task 7 ✓ ; INDEXES (plain) → Task 8 ✓
- STATISTICS → Task 9 ✓ ; ALL → Task 10 ✓
- EXCLUDE-not-modeled + non-table source → precise errors, Tasks 7 & 10 ✓
- Name-match-by-qname risk → `choose_name` port (Task 6) + live-DB conformance (Task 11) ✓
- Parse-before-filter (whole-tree failure) is *resolved by construction*: the clause now parses, so the table no longer blocks the tree; `ignore_objects` still works as a fallback.

**Placeholder scan:** every code step contains concrete Rust/commands. The two genuinely empirical unknowns (exact CHECK-constraint name form; `ChooseIndexNameAddition` byte budget) are explicitly routed to live-DB verification in Task 11 with a stated fix path, not left as silent TODOs.

**Type consistency:** `TableLikeOptions`, `PendingLike`, `apply_pending_likes`, `extract_pending_likes`, `copy_column`, `IndexNameKind`, `TakenNames`, `choose_index_name` are named identically across all tasks. `IndexParent::Table`, `IndexColumnExpr::Column/Expression`, `ConstraintKind::{PrimaryKey,Unique,Check}` match the IR reference block.

**Risk callouts:** (1) `Catalog`/`Table` may not implement `Default` or expose public field-init outside the crate — tasks note the tempdir-parse fallback for fixtures. (2) `choose_name` truncation constants are the highest-risk guess; Task 11 is the gate that pins them to real PG output across the support matrix.
