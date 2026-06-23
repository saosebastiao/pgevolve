//! `CREATE TABLE … (LIKE source [INCLUDING …])` resolution.
//!
//! `build_table` skips `TableLikeClause` elements; `process_file` records one
//! [`PendingLike`] per clause. After every table is in the catalog,
//! [`apply_pending_likes`] expands each clause against the source table — the
//! clone is fully decoupled in Postgres, so we must materialize concrete IR.

use std::collections::{BTreeMap, BTreeSet};

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
///
/// LIKE clauses can chain (`x (LIKE z)` where `z (LIKE a)`), so we cannot
/// simply iterate targets in name order — a dependent must be expanded only
/// after every table it copies from is fully materialized. We resolve in
/// rounds: each round fully resolves any target whose sources are all already
/// materialized (i.e. not themselves still-pending targets). If a round makes
/// no progress while targets remain, the remaining set is a cycle or
/// self-reference and we error.
pub fn apply_pending_likes(
    catalog: &mut Catalog,
    pending: Vec<PendingLike>,
) -> Result<(), ParseError> {
    // Group by target so multiple LIKE clauses on one table share an
    // insertion-offset accumulator and a deterministic processing order.
    let mut by_target: BTreeMap<QualifiedName, Vec<PendingLike>> = BTreeMap::new();
    for p in pending {
        by_target.entry(p.target.clone()).or_default().push(p);
    }

    // The set of targets that still need expanding. A target is "ready" once
    // none of its sources are themselves still-unresolved targets.
    let mut unresolved: BTreeSet<QualifiedName> = by_target.keys().cloned().collect();

    while !unresolved.is_empty() {
        // Pick targets whose every source is already materialized this round.
        let ready: Vec<QualifiedName> = unresolved
            .iter()
            .filter(|target| {
                by_target[*target]
                    .iter()
                    .all(|like| !unresolved.contains(&like.source))
            })
            .cloned()
            .collect();

        if ready.is_empty() {
            // No progress with targets remaining → cycle (includes self-reference).
            let involved = unresolved
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            // Use any remaining target's location for the diagnostic.
            let location = unresolved
                .iter()
                .next()
                .and_then(|t| by_target.get(t))
                .and_then(|likes| likes.first())
                .map_or_else(
                    || SourceLocation::new(std::path::PathBuf::new(), 0, 0),
                    |like| like.location.clone(),
                );
            return Err(ParseError::Structural {
                location,
                message: format!("LIKE forms a cycle involving {involved}"),
            });
        }

        for target in ready {
            // SAFETY: `target` came from `unresolved` which is keyed off `by_target`.
            let mut likes = by_target.remove(&target).unwrap_or_default();
            likes.sort_by_key(|p| p.explicit_cols_before); // stable; preserves clause order on ties
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
            unresolved.remove(&target);
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
        // Assemble a real catalog via the parse pipeline (`Table` has no
        // `Default` and a literal would be verbose), then feed it a pending
        // LIKE whose source table does not exist.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.clone (id int);\n").unwrap();
        let (mut cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        assert!(apply_pending_likes(&mut cat, vec![pend]).is_err());
    }

    /// A chain `z (LIKE a)` then `x (LIKE z)` must resolve fully regardless of
    /// the order the targets sort in. Here the dependent (`x`) sorts AFTER its
    /// source (`z`), so qname order happens to be correct.
    #[test]
    fn chained_like_resolves_dependent_after_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.a (id int PRIMARY KEY, name text);\n\
             CREATE TABLE pub.z (LIKE pub.a);\n\
             CREATE TABLE pub.x (LIKE pub.z);\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        for tname in ["a", "z", "x"] {
            let t = cat.tables.iter().find(|t| t.qname.name.as_str() == tname).unwrap();
            assert_eq!(
                t.columns.iter().map(|c| c.name.as_str().to_string()).collect::<Vec<_>>(),
                vec!["id".to_string(), "name".to_string()],
                "table {tname} should have both copied columns",
            );
        }
    }

    /// Same chain, but names chosen so the dependent (`aaa (LIKE zzz)`) sorts
    /// BEFORE its source (`zzz (LIKE base)`). A naive qname-sorted pass would
    /// resolve `aaa` before `zzz` is materialized and leave it empty; the
    /// dependency-ordered pass must still fully populate both.
    #[test]
    fn chained_like_resolves_when_dependent_sorts_before_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (id int PRIMARY KEY, name text);\n\
             CREATE TABLE pub.aaa (LIKE pub.zzz);\n\
             CREATE TABLE pub.zzz (LIKE pub.base);\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        for tname in ["base", "aaa", "zzz"] {
            let t = cat.tables.iter().find(|t| t.qname.name.as_str() == tname).unwrap();
            assert_eq!(
                t.columns.iter().map(|c| c.name.as_str().to_string()).collect::<Vec<_>>(),
                vec!["id".to_string(), "name".to_string()],
                "table {tname} should have both copied columns",
            );
        }
    }

    /// A self-referential LIKE (`c (LIKE c)`) is a cycle and must error rather
    /// than silently leave `c` with no columns.
    #[test]
    fn self_referential_like_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.c (LIKE pub.c);\n").unwrap();
        let err = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap_err();
        assert!(matches!(err, crate::parse::ParseError::Structural { .. }), "got {err:?}");
    }

    /// A 2-cycle (`p (LIKE q)`, `q (LIKE p)`) must also error.
    #[test]
    fn two_cycle_like_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.p (LIKE pub.q);\n\
             CREATE TABLE pub.q (LIKE pub.p);\n").unwrap();
        let err = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap_err();
        assert!(matches!(err, crate::parse::ParseError::Structural { .. }), "got {err:?}");
    }

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
}
