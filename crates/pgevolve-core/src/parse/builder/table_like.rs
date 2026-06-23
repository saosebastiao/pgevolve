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

    /// Construct from raw Postgres option bits.
    #[must_use] pub const fn new(bits: u32) -> Self { Self(bits) }
    /// Whether `INCLUDING COMMENTS` is set.
    #[must_use] pub const fn comments(self) -> bool { self.0 & Self::COMMENTS != 0 }
    /// Whether `INCLUDING COMPRESSION` is set.
    #[must_use] pub const fn compression(self) -> bool { self.0 & Self::COMPRESSION != 0 }
    /// Whether `INCLUDING CONSTRAINTS` is set.
    #[must_use] pub const fn constraints(self) -> bool { self.0 & Self::CONSTRAINTS != 0 }
    /// Whether `INCLUDING DEFAULTS` is set.
    #[must_use] pub const fn defaults(self) -> bool { self.0 & Self::DEFAULTS != 0 }
    /// Whether `INCLUDING GENERATED` is set.
    #[must_use] pub const fn generated(self) -> bool { self.0 & Self::GENERATED != 0 }
    /// Whether `INCLUDING IDENTITY` is set.
    #[must_use] pub const fn identity(self) -> bool { self.0 & Self::IDENTITY != 0 }
    /// Whether `INCLUDING INDEXES` is set.
    #[must_use] pub const fn indexes(self) -> bool { self.0 & Self::INDEXES != 0 }
    /// Whether `INCLUDING STATISTICS` is set.
    #[must_use] pub const fn statistics(self) -> bool { self.0 & Self::STATISTICS != 0 }
    /// Whether `INCLUDING STORAGE` is set.
    #[must_use] pub const fn storage(self) -> bool { self.0 & Self::STORAGE != 0 }
}

/// One unresolved `LIKE` clause, captured during `process_file`.
#[derive(Debug, Clone)]
pub struct PendingLike {
    /// Schema-qualified name of the table being created (the clone).
    pub target: QualifiedName,
    /// Schema-qualified name of the source table to copy from.
    pub source: QualifiedName,
    /// `INCLUDING`/`EXCLUDING` option bitmask from the clause.
    pub options: TableLikeOptions,
    /// Number of explicitly-listed columns that preceded this clause in the
    /// `CREATE TABLE` element list — the insertion point for copied columns.
    pub explicit_cols_before: usize,
    /// Source location for error reporting.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::column_type::ColumnType;
    use std::path::PathBuf;

    fn loc() -> SourceLocation { SourceLocation::new(PathBuf::from("t.sql"), 1, 1) }
    fn id(s: &str) -> Identifier { Identifier::from_unquoted(s).unwrap() }
    fn qn(s: &str, n: &str) -> QualifiedName { QualifiedName::new(id(s), id(n)) }

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

    #[test]
    fn bare_like_copies_columns_names_types_notnull() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (id integer NOT NULL, name text);\n\
             CREATE TABLE pub.clone (LIKE pub.base);\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let clone = cat.tables.iter().find(|t| t.qname.name.as_str() == "clone").unwrap();
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
        let pend = PendingLike {
            target: qn("pub", "clone"),
            source: qn("pub", "missing"),
            options: TableLikeOptions::new(0),
            explicit_cols_before: 0,
            location: loc(),
        };
        // Build a minimal catalog with just the target table (no source).
        // We can't construct Table directly (fields may be private), but we can
        // use parse_directory_with_locations on a minimal SQL file and then
        // call apply_pending_likes on its result.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.clone (id int);\n").unwrap();
        let (mut cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        assert!(apply_pending_likes(&mut cat, vec![pend]).is_err());
    }
}
