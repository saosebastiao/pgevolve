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
use crate::ir::constraint::{Constraint, ConstraintKind};
use crate::ir::index::{Index, IndexColumnExpr, IndexParent};
use crate::ir::statistic::{Statistic, StatisticColumn};
use crate::parse::builder::{choose_name, shared};
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

/// Copy a source column for a `LIKE` clause, gating optional attributes on the
/// corresponding `INCLUDING` option bits.  Always copies name, type, collation,
/// and not-null; everything else is controlled by `opts`.
fn copy_column(src: &Column, opts: TableLikeOptions) -> Column {
    Column {
        name: src.name.clone(),
        ty: src.ty.clone(),
        nullable: src.nullable,
        collation: src.collation.clone(),
        default:     if opts.defaults()    { src.default.clone() }    else { None },
        identity:    if opts.identity()    { src.identity.clone() }   else { None },
        generated:   if opts.generated()   { src.generated.clone() }  else { None },
        storage:     if opts.storage()     { src.storage }            else { None },
        compression: if opts.compression() { src.compression }        else { None },
        // comments are copied in a separate pass after deferred comments are applied
        // (see apply_pending_like_comments)
        comment: None,
    }
}

/// One materialized snapshot of columns, constraints, indexes, and statistics
/// to be spliced into a LIKE target.  Built during the immutable-borrow phase;
/// applied during the mutable-borrow phase, keeping the two borrows cleanly
/// separate.
struct LikeSnapshot {
    explicit_cols_before: usize,
    cols: Vec<Column>,
    constraints: Vec<Constraint>,
    /// Plain (non-constraint) indexes to append to `catalog.indexes`.
    /// Populated only when `INCLUDING INDEXES` is set.
    indexes: Vec<Index>,
    /// Extended statistics to append to `catalog.statistics`.
    /// Populated only when `INCLUDING STATISTICS` is set.
    statistics: Vec<Statistic>,
}

/// Build a [`LikeSnapshot`] for one `like` clause against `catalog`, updating
/// `taken` so that any subsequent clause for the same target doesn't reuse a
/// just-generated constraint name.
///
/// The `target_name` and `target_schema` arguments identify the clone table
/// whose schema the new constraint qnames should carry.
fn snapshot_like(
    like: &PendingLike,
    target_name: &Identifier,
    target_schema: &Identifier,
    catalog: &Catalog,
    taken: &mut choose_name::TakenNames,
) -> Result<LikeSnapshot, ParseError> {
    let src = catalog.tables.iter().find(|t| t.qname == like.source)
        .ok_or_else(|| {
            // Give a more specific diagnostic: distinguish "exists as a view/MV
            // (unsupported source kind)" from "does not exist at all".
            let is_view = catalog.views.iter().any(|v| v.qname == like.source);
            let is_mv   = catalog.materialized_views.iter().any(|v| v.qname == like.source);
            let message = if is_view {
                format!(
                    "LIKE source {} is a view; LIKE requires a base table (views are not supported as LIKE sources)",
                    like.source
                )
            } else if is_mv {
                format!(
                    "LIKE source {} is a materialized view; LIKE requires a base table (materialized views are not supported as LIKE sources)",
                    like.source
                )
            } else {
                format!(
                    "LIKE source table {} not found (must be a table declared in the schema)",
                    like.source
                )
            };
            ParseError::Structural {
                location: like.location.clone(),
                message,
            }
        })?;

    let cols: Vec<Column> = src.columns.iter().map(|c| copy_column(c, like.options)).collect();
    let mut constraints: Vec<Constraint> = Vec::new();

    // INCLUDING CONSTRAINTS → copy CHECK constraints.
    // PK/UNIQUE belong to INCLUDING INDEXES (handled below); FOREIGN KEY is
    // never copied by LIKE regardless of options.
    if like.options.constraints() {
        // pgevolve auto-names an unnamed CHECK as `{table}_check` (see
        // `constraint_qname` in `create_stmt.rs`).  When copying a CHECK
        // from a LIKE source we must distinguish:
        //   - auto-named  → name equals the source sentinel `{source}_check`
        //                  → re-derive for the clone: `{target}_check`
        //   - explicitly-named → anything else → preserve the source name
        // This ensures that `LIKE pub.base INCLUDING CONSTRAINTS` produces a
        // clone whose unnamed check is named `{clone}_check`, matching an
        // equivalent hand-written clone (fixes #45).
        //
        // NOTE: full Postgres column-check naming fidelity (`{table}_{col}_check`)
        // is a separate broader concern (see #44/#46) because pgevolve's inline
        // path also uses the simpler `{table}_check` form.
        let source_auto_name = format!("{}_check", like.source.name.as_str());
        for c in &src.constraints {
            if matches!(c.kind, ConstraintKind::Check { .. }) {
                let local_name = if c.qname.name.as_str() == source_auto_name {
                    // Auto-generated name: re-derive for the clone table.
                    Identifier::from_unquoted(&format!("{}_check", target_name.as_str()))
                        .map_err(|e| ParseError::Structural {
                            location: like.location.clone(),
                            message: format!(
                                "re-derived CHECK constraint name is not a valid identifier: {e}"
                            ),
                        })?
                } else {
                    // Explicitly-named constraint: preserve the source name.
                    c.qname.name.clone()
                };
                constraints.push(Constraint {
                    qname: QualifiedName::new(target_schema.clone(), local_name),
                    kind: c.kind.clone(),
                    deferrable: c.deferrable,
                    // comments under INCLUDING COMMENTS are handled separately;
                    // INCLUDING CONSTRAINTS does not copy comments.
                    comment: None,
                });
            }
        }
    }

    // INCLUDING INDEXES → copy PK/UNIQUE constraints (re-derived names) and
    // plain (CREATE INDEX) indexes.  EXCLUDE constraints aren't modeled in
    // ConstraintKind.  FOREIGN KEY is never copied by LIKE.
    let indexes = if like.options.indexes() {
        copy_index_constraints(
            &src.constraints,
            target_name,
            target_schema,
            &like.location,
            taken,
            &mut constraints,
        )?;
        copy_plain_indexes(
            &like.source,
            target_name,
            target_schema,
            &like.location,
            &catalog.indexes,
            taken,
        )?
    } else {
        Vec::new()
    };

    // INCLUDING STATISTICS → copy extended statistics targeting the source.
    let statistics = if like.options.statistics() {
        copy_statistics(
            &like.source,
            target_name,
            target_schema,
            &like.location,
            &catalog.statistics,
            taken,
        )?
    } else {
        Vec::new()
    };

    Ok(LikeSnapshot {
        explicit_cols_before: like.explicit_cols_before,
        cols,
        constraints,
        indexes,
        statistics,
    })
}

/// Copy PK and UNIQUE constraints from `src_constraints` into `out`, assigning
/// Postgres-faithful names for the clone table (`target_name` / `target_schema`).
fn copy_index_constraints(
    src_constraints: &[Constraint],
    target_name: &Identifier,
    target_schema: &Identifier,
    location: &crate::parse::error::SourceLocation,
    taken: &mut choose_name::TakenNames,
    out: &mut Vec<Constraint>,
) -> Result<(), ParseError> {
    for c in src_constraints {
        match &c.kind {
            ConstraintKind::PrimaryKey { columns, include } => {
                let generated = choose_name::choose_index_name(
                    target_name.as_str(),
                    &[],
                    choose_name::IndexNameKind::Pkey,
                    taken,
                );
                let qname = QualifiedName::new(
                    target_schema.clone(),
                    Identifier::from_unquoted(&generated).map_err(|e| ParseError::Structural {
                        location: location.clone(),
                        message: format!(
                            "generated PK constraint name {generated:?} is not a valid identifier: {e}"
                        ),
                    })?,
                );
                out.push(Constraint {
                    qname,
                    kind: ConstraintKind::PrimaryKey {
                        columns: columns.clone(),
                        include: include.clone(),
                    },
                    deferrable: c.deferrable,
                    comment: None,
                });
            }
            ConstraintKind::Unique { columns, include, nulls_distinct } => {
                let col_opts: Vec<Option<&str>> =
                    columns.iter().map(|col| Some(col.as_str())).collect();
                let generated = choose_name::choose_index_name(
                    target_name.as_str(),
                    &col_opts,
                    choose_name::IndexNameKind::Unique,
                    taken,
                );
                let qname = QualifiedName::new(
                    target_schema.clone(),
                    Identifier::from_unquoted(&generated).map_err(|e| ParseError::Structural {
                        location: location.clone(),
                        message: format!(
                            "generated UNIQUE constraint name {generated:?} is not a valid identifier: {e}"
                        ),
                    })?,
                );
                out.push(Constraint {
                    qname,
                    kind: ConstraintKind::Unique {
                        columns: columns.clone(),
                        include: include.clone(),
                        nulls_distinct: *nulls_distinct,
                    },
                    deferrable: c.deferrable,
                    comment: None,
                });
            }
            // CHECK belongs to INCLUDING CONSTRAINTS (handled in snapshot_like),
            // FOREIGN KEY is never copied by LIKE.
            ConstraintKind::Check { .. } | ConstraintKind::ForeignKey(_) => {}
        }
    }
    Ok(())
}

/// Build retargeted copies of every plain (non-constraint) index on `source`,
/// assigning Postgres-faithful names via `choose_index_name`.
///
/// ## Why no double-copy risk
/// `catalog.indexes` is populated exclusively from `CREATE INDEX` statements.
/// PK and UNIQUE *constraint*-backing indexes are stored as table constraints
/// (`Constraint::PrimaryKey` / `Constraint::Unique`), never as `Index`
/// entries.  Therefore every `catalog.indexes` entry whose `on` targets the
/// source table is a plain index, with no overlap with the constraint path.
fn copy_plain_indexes(
    source: &QualifiedName,
    target_name: &Identifier,
    target_schema: &Identifier,
    location: &crate::parse::error::SourceLocation,
    catalog_indexes: &[Index],
    taken: &mut choose_name::TakenNames,
) -> Result<Vec<Index>, ParseError> {
    let mut out = Vec::new();
    let source_parent = IndexParent::Table(source.clone());
    for idx in catalog_indexes.iter().filter(|i| i.on == source_parent) {
        let col_opts: Vec<Option<&str>> = idx.columns.iter().map(|col| match &col.expr {
            IndexColumnExpr::Column(id) => Some(id.as_str()),
            IndexColumnExpr::Expression(_) => None,
        }).collect();
        // Plain `CREATE [UNIQUE] INDEX` always uses `_idx` suffix (Postgres
        // `ChooseIndexName` with `isconstraint=false`), regardless of
        // uniqueness.  The `_key` suffix is only for UNIQUE *constraints*.
        let generated = choose_name::choose_index_name(
            target_name.as_str(),
            &col_opts,
            choose_name::IndexNameKind::Plain,
            taken,
        );
        let new_qname = QualifiedName::new(
            target_schema.clone(),
            Identifier::from_unquoted(&generated).map_err(|e| ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "generated index name {generated:?} is not a valid identifier: {e}"
                ),
            })?,
        );
        out.push(Index {
            qname: new_qname,
            on: IndexParent::Table(QualifiedName::new(
                target_schema.clone(),
                target_name.clone(),
            )),
            method: idx.method,
            columns: idx.columns.clone(),
            include: idx.include.clone(),
            unique: idx.unique,
            nulls_not_distinct: idx.nulls_not_distinct,
            predicate: idx.predicate.clone(),
            tablespace: idx.tablespace.clone(),
            // Index comments are out of scope for INCLUDING INDEXES;
            // they are not propagated (similar to INCLUDING COMMENTS
            // which is a separate option).
            comment: None,
            storage: idx.storage.clone(),
        });
    }
    Ok(out)
}

/// Build retargeted copies of every extended statistic on `source`,
/// assigning Postgres-faithful names via `choose_index_name` with
/// [`choose_name::IndexNameKind::Stat`].
///
/// Column name fragments follow the same convention as index names:
/// `StatisticColumn::Column` contributes the column name, and
/// `StatisticColumn::Expression` contributes `None` (mapped to `"expr"`
/// by `name_addition`).
fn copy_statistics(
    source: &QualifiedName,
    target_name: &Identifier,
    target_schema: &Identifier,
    location: &crate::parse::error::SourceLocation,
    catalog_statistics: &[Statistic],
    taken: &mut choose_name::TakenNames,
) -> Result<Vec<Statistic>, ParseError> {
    let clone_qname = QualifiedName::new(target_schema.clone(), target_name.clone());
    let mut out = Vec::new();
    for stat in catalog_statistics.iter().filter(|s| &s.target == source) {
        // TODO(#43 Task 11): verify extended-statistics naming vs live PG
        let col_opts: Vec<Option<&str>> = stat.columns.iter().map(|col| match col {
            StatisticColumn::Column(id) => Some(id.as_str()),
            StatisticColumn::Expression(_) => None,
        }).collect();
        let generated = choose_name::choose_index_name(
            target_name.as_str(),
            &col_opts,
            choose_name::IndexNameKind::Stat,
            taken,
        );
        let new_qname = QualifiedName::new(
            target_schema.clone(),
            Identifier::from_unquoted(&generated).map_err(|e| ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "generated statistics name {generated:?} is not a valid identifier: {e}"
                ),
            })?,
        );
        out.push(Statistic {
            qname: new_qname,
            target: clone_qname.clone(),
            kinds: stat.kinds,
            columns: stat.columns.clone(),
            statistics_target: stat.statistics_target,
            owner: None,
            comment: None,
        });
    }
    Ok(out)
}

/// Return the pending-LIKE targets in dependency order: every target whose
/// LIKE sources are themselves targets comes after those sources.
///
/// A stall with remaining targets means a cycle or self-reference; the error
/// names all involved tables.
fn resolve_order(
    by_target: &BTreeMap<QualifiedName, Vec<&PendingLike>>,
) -> Result<Vec<QualifiedName>, ParseError> {
    let mut unresolved: BTreeSet<QualifiedName> = by_target.keys().cloned().collect();
    let mut ordered: Vec<QualifiedName> = Vec::with_capacity(by_target.len());

    while !unresolved.is_empty() {
        // Pick targets whose every source is already outside the unresolved set.
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
            unresolved.remove(&target);
            ordered.push(target);
        }
    }

    Ok(ordered)
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
///
/// Note: takes `&[PendingLike]` so the caller retains the slice for a
/// subsequent [`apply_pending_like_comments`] pass.
pub fn apply_pending_likes(
    catalog: &mut Catalog,
    pending: &[PendingLike],
) -> Result<(), ParseError> {
    // Group by target so multiple LIKE clauses on one table share an
    // insertion-offset accumulator and a deterministic processing order.
    let mut by_target: BTreeMap<QualifiedName, Vec<&PendingLike>> = BTreeMap::new();
    for p in pending {
        by_target.entry(p.target.clone()).or_default().push(p);
    }

    for target in resolve_order(&by_target)? {
        // target is always a by_target key; or_default is a defensive no-op.
        let mut likes = by_target.remove(&target).unwrap_or_default();
        likes.sort_by_key(|p| p.explicit_cols_before); // stable; preserves clause order on ties

            // Build a name-collision tracker seeded with every name already live in
            // the target's schema.  One `TakenNames` per target so that all LIKE
            // clauses for the same clone share a single counter state — meaning
            // two different LIKE clauses on the same clone won't independently
            // produce colliding names.
            //
            // `from_schema` borrows `catalog` immutably; we finish the borrow
            // completely before we need the mutable borrow below.
            let mut taken = choose_name::TakenNames::from_schema(catalog, &target.schema);

            // Snapshot pass (immutable borrow): gather columns + constraints for
            // every clause, driving `taken` forward so names stay globally unique.
            let mut snapshots: Vec<LikeSnapshot> = Vec::with_capacity(likes.len());
            for like in &likes {
                snapshots.push(snapshot_like(like, &target.name, &target.schema, catalog, &mut taken)?);
            }

            // Mutation pass: apply all snapshots to the (now mutably-borrowed) target.
            // Column and constraint mutations go to `catalog.tables`; index
            // mutations go to `catalog.indexes`.  Both happen after ALL
            // snapshotting above, so the immutable borrows are fully released.
            let mut inserted = 0usize;
            for snap in snapshots {
                let n = snap.cols.len();
                let tgt = catalog.tables.iter_mut().find(|t| t.qname == target)
                    .ok_or_else(|| ParseError::Structural {
                        location: likes.first().map_or_else(
                            || SourceLocation::new(std::path::PathBuf::new(), 0, 0),
                            |l| l.location.clone(),
                        ),
                        message: format!("LIKE target table {target} vanished"),
                    })?;
                let at = (snap.explicit_cols_before + inserted).min(tgt.columns.len());
                tgt.columns.splice(at..at, snap.cols);
                inserted += n;
                tgt.constraints.extend(snap.constraints);
                // Push retargeted plain indexes after table mutations so that
                // the immutable borrow of `catalog.indexes` (in snapshot_like)
                // is already complete when we mutate here.
                catalog.indexes.extend(snap.indexes);
                // Push retargeted statistics after table mutations so that
                // the immutable borrow of `catalog.statistics` (in snapshot_like)
                // is already complete when we mutate here.
                catalog.statistics.extend(snap.statistics);
            }
        }
    Ok(())
}

/// Second pass: copy `INCLUDING COMMENTS` from each source to its clone(s),
/// respecting the same dependency ordering as [`apply_pending_likes`].
///
/// This pass runs AFTER the `deferred_comments` loop so that every table and
/// column already has its own comments applied before we propagate them via
/// LIKE.  Only clauses with `opts.comments()` do anything; clones whose
/// comment is already set (via an explicit `COMMENT ON TABLE/COLUMN`) keep
/// theirs — the explicit comment wins.
pub fn apply_pending_like_comments(
    catalog: &mut Catalog,
    pending: &[PendingLike],
) -> Result<(), ParseError> {
    // Respect the same dependency ordering as apply_pending_likes: process a
    // source before any clone that LIKEs it, so chained LIKE WITH COMMENTS
    // propagates through the whole chain.
    let mut by_target: BTreeMap<QualifiedName, Vec<&PendingLike>> = BTreeMap::new();
    for p in pending {
        if p.options.comments() {
            by_target.entry(p.target.clone()).or_default().push(p);
        }
    }

    if by_target.is_empty() {
        return Ok(());
    }

    for target in resolve_order(&by_target)? {
        let likes = by_target.remove(&target).unwrap_or_default();
        for like in likes {
            // Snapshot source table comment and per-column comments before
            // borrowing the target mutably.
            let (src_table_comment, src_col_comments): (Option<String>, Vec<(Identifier, Option<String>)>) = {
                let src = catalog.tables.iter().find(|t| t.qname == like.source)
                    .ok_or_else(|| ParseError::Structural {
                        location: like.location.clone(),
                        message: format!(
                            "LIKE source table {} not found during comment copy",
                            like.source
                        ),
                    })?;
                let table_comment = src.comment.clone();
                let col_comments = src.columns.iter()
                    .map(|c| (c.name.clone(), c.comment.clone()))
                    .collect();
                (table_comment, col_comments)
            };

            let tgt = catalog.tables.iter_mut().find(|t| t.qname == target)
                .ok_or_else(|| ParseError::Structural {
                    location: like.location.clone(),
                    message: format!("LIKE target table {target} vanished during comment copy"),
                })?;

            // Only propagate if the clone has no explicit comment (explicit wins).
            if tgt.comment.is_none() {
                tgt.comment = src_table_comment;
            }

            // Propagate column comments: match by name, explicit clone comment wins.
            for (col_name, col_comment) in src_col_comments {
                if let Some(tgt_col) = tgt.columns.iter_mut().find(|c| c.name == col_name)
                    && tgt_col.comment.is_none()
                {
                    tgt_col.comment = col_comment;
                }
            }
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
        assert!(apply_pending_likes(&mut cat, &[pend]).is_err());
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

    // ── INCLUDING COMMENTS tests ──────────────────────────────────────────────

    #[test]
    fn including_comments_copies_table_comment() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (id int);\n\
             COMMENT ON TABLE pub.base IS 'hi';\n\
             CREATE TABLE pub.c (LIKE pub.base INCLUDING COMMENTS);\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let c = cat.tables.iter().find(|t| t.qname.name.as_str() == "c").unwrap();
        assert_eq!(c.comment.as_deref(), Some("hi"),
            "INCLUDING COMMENTS should propagate the source table comment to the clone");
    }

    #[test]
    fn including_comments_copies_column_comment() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (id int);\n\
             COMMENT ON COLUMN pub.base.id IS 'col';\n\
             CREATE TABLE pub.clone (LIKE pub.base INCLUDING COMMENTS);\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let clone = cat.tables.iter().find(|t| t.qname.name.as_str() == "clone").unwrap();
        let id_col = clone.columns.iter().find(|c| c.name.as_str() == "id").unwrap();
        assert_eq!(id_col.comment.as_deref(), Some("col"),
            "INCLUDING COMMENTS should propagate the source column comment to the clone");
    }

    #[test]
    fn bare_like_does_not_copy_comments() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (id int);\n\
             COMMENT ON TABLE pub.base IS 'tbl';\n\
             COMMENT ON COLUMN pub.base.id IS 'col';\n\
             CREATE TABLE pub.clone (LIKE pub.base);\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let clone = cat.tables.iter().find(|t| t.qname.name.as_str() == "clone").unwrap();
        assert!(clone.comment.is_none(), "bare LIKE must not copy table comment");
        assert!(clone.columns.iter().all(|c| c.comment.is_none()),
            "bare LIKE must not copy column comments");
    }

    /// Regression guard: COMMENT ON COLUMN targeting a LIKE-derived column must
    /// succeed. The brief's suggested reorder (`deferred_comments` before
    /// `apply_pending_likes`) would have broken this case because `apply_comment`
    /// would error when the clone's columns don't yet exist.
    #[test]
    fn comment_on_clone_like_derived_column_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (id int);\n\
             CREATE TABLE pub.clone (LIKE pub.base);\n\
             COMMENT ON COLUMN pub.clone.id IS 'x';\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let clone = cat.tables.iter().find(|t| t.qname.name.as_str() == "clone").unwrap();
        let id_col = clone.columns.iter().find(|c| c.name.as_str() == "id").unwrap();
        assert_eq!(id_col.comment.as_deref(), Some("x"),
            "COMMENT ON COLUMN clone.id must apply to the LIKE-derived column");
    }

    #[test]
    fn explicit_clone_comment_wins_over_including_comments() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (id int);\n\
             COMMENT ON TABLE pub.base IS 'from_base';\n\
             CREATE TABLE pub.clone (LIKE pub.base INCLUDING COMMENTS);\n\
             COMMENT ON TABLE pub.clone IS 'explicit';\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let clone = cat.tables.iter().find(|t| t.qname.name.as_str() == "clone").unwrap();
        assert_eq!(clone.comment.as_deref(), Some("explicit"),
            "explicit COMMENT ON TABLE clone must win over INCLUDING COMMENTS propagation");
    }

    // ── INCLUDING CONSTRAINTS tests ───────────────────────────────────────────

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
        assert!(bare.constraints.iter().all(|c| !matches!(c.kind, crate::ir::constraint::ConstraintKind::Check{..})),
            "bare LIKE must not copy CHECK constraints");
        let c = cat.tables.iter().find(|t| t.qname.name.as_str() == "c").unwrap();
        assert_eq!(c.constraints.iter()
            .filter(|c| matches!(c.kind, crate::ir::constraint::ConstraintKind::Check{..})).count(), 1,
            "INCLUDING CONSTRAINTS should copy exactly one CHECK constraint");
    }

    #[test]
    fn including_constraints_does_not_copy_pk_or_unique() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (id int PRIMARY KEY, e text UNIQUE, n int CHECK (n>0));\n\
             CREATE TABLE pub.c (LIKE pub.base INCLUDING CONSTRAINTS);\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let c = cat.tables.iter().find(|t| t.qname.name.as_str() == "c").unwrap();
        assert_eq!(
            c.constraints.iter().filter(|c| matches!(c.kind, crate::ir::constraint::ConstraintKind::Check{..})).count(),
            1,
            "INCLUDING CONSTRAINTS should copy the CHECK constraint"
        );
        assert!(
            c.constraints.iter().all(|c| !matches!(c.kind, crate::ir::constraint::ConstraintKind::PrimaryKey{..})),
            "INCLUDING CONSTRAINTS must not copy PrimaryKey (belongs to INCLUDING INDEXES)"
        );
        assert!(
            c.constraints.iter().all(|c| !matches!(c.kind, crate::ir::constraint::ConstraintKind::Unique{..})),
            "INCLUDING CONSTRAINTS must not copy Unique (belongs to INCLUDING INDEXES)"
        );
    }

    /// An UNNAMED source CHECK (`CHECK (n > 0)`) is auto-named `base_check` by
    /// pgevolve.  When copied via `LIKE … INCLUDING CONSTRAINTS` the clone must
    /// receive a re-derived name (`clone_check`), NOT the source name (`base_check`).
    /// This matches what an equivalent hand-written clone would produce and
    /// prevents a spurious diff (fixes #45).
    #[test]
    fn like_constraints_rederives_unnamed_check_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        // Unnamed CHECK → pgevolve auto-names it `base_check`.
        std::fs::write(
            dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (n int, CHECK (n > 0));\n\
             CREATE TABLE pub.clone (LIKE pub.base INCLUDING CONSTRAINTS);\n",
        )
        .unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let clone = cat.tables.iter().find(|t| t.qname.name.as_str() == "clone").unwrap();
        let check = clone
            .constraints
            .iter()
            .find(|c| matches!(c.kind, crate::ir::constraint::ConstraintKind::Check { .. }))
            .expect("clone must have a copied CHECK constraint");
        assert_eq!(
            check.qname.name.as_str(), "clone_check",
            "unnamed source CHECK must be re-derived to clone_check, got {:?}",
            check.qname.name.as_str(),
        );
        assert_eq!(
            check.qname.schema.as_str(), "pub",
            "copied CHECK must be in clone's schema"
        );
    }

    /// An EXPLICITLY-NAMED source CHECK (`CONSTRAINT n_pos CHECK …`) must keep
    /// its source name when copied via `LIKE … INCLUDING CONSTRAINTS`; only the
    /// schema is updated to the clone's schema.
    #[test]
    fn like_constraints_preserves_explicit_check_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        // Explicitly-named CHECK → name must be preserved verbatim in the clone.
        std::fs::write(
            dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (n int, CONSTRAINT n_pos CHECK (n > 0));\n\
             CREATE TABLE pub.clone (LIKE pub.base INCLUDING CONSTRAINTS);\n",
        )
        .unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let clone = cat.tables.iter().find(|t| t.qname.name.as_str() == "clone").unwrap();
        let check = clone
            .constraints
            .iter()
            .find(|c| matches!(c.kind, crate::ir::constraint::ConstraintKind::Check { .. }))
            .expect("clone must have a copied CHECK constraint");
        assert_eq!(
            check.qname.name.as_str(), "n_pos",
            "explicitly-named source CHECK must be preserved as n_pos, got {:?}",
            check.qname.name.as_str(),
        );
        assert_eq!(
            check.qname.schema.as_str(), "pub",
            "copied CHECK must be in clone's schema"
        );
    }

    // ── INCLUDING INDEXES tests ───────────────────────────────────────────────

    #[test]
    fn including_indexes_copies_pk_and_unique_with_pg_names() {
        use crate::ir::constraint::ConstraintKind::{PrimaryKey, Unique};
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (id int PRIMARY KEY, email text UNIQUE);\n\
             CREATE TABLE pub.c (LIKE pub.base INCLUDING INDEXES);\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let c = cat.tables.iter().find(|t| t.qname.name.as_str() == "c").unwrap();
        let names: Vec<_> = c.constraints.iter().map(|k| k.qname.name.as_str().to_string()).collect();
        assert!(names.contains(&"c_pkey".to_string()), "got {names:?}");
        assert!(names.contains(&"c_email_key".to_string()), "got {names:?}");
        assert_eq!(c.constraints.iter().filter(|k| matches!(k.kind, PrimaryKey{..})).count(), 1);
        assert_eq!(c.constraints.iter().filter(|k| matches!(k.kind, Unique{..})).count(), 1);
    }

    #[test]
    fn bare_like_copies_no_pk_or_unique() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (id int PRIMARY KEY, email text UNIQUE);\n\
             CREATE TABLE pub.d (LIKE pub.base);\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let d = cat.tables.iter().find(|t| t.qname.name.as_str() == "d").unwrap();
        assert!(
            d.constraints.iter().all(|c| !matches!(c.kind, crate::ir::constraint::ConstraintKind::PrimaryKey{..})),
            "bare LIKE must not copy PK"
        );
        assert!(
            d.constraints.iter().all(|c| !matches!(c.kind, crate::ir::constraint::ConstraintKind::Unique{..})),
            "bare LIKE must not copy UNIQUE"
        );
    }

    // ── INCLUDING INDEXES: plain index tests ─────────────────────────────────

    /// `INCLUDING INDEXES` copies a plain `CREATE INDEX` to the clone, with a
    /// Postgres-faithful re-derived name (`<target>_<cols>_idx`).
    #[test]
    fn including_indexes_copies_plain_index() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (a int, b int);\n\
             CREATE INDEX ON pub.base (a, b);\n\
             CREATE TABLE pub.c (LIKE pub.base INCLUDING INDEXES);\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let idx: Vec<_> = cat.indexes.iter()
            .filter(|i| i.on.qname().name.as_str() == "c")
            .map(|i| i.qname.name.as_str().to_string())
            .collect();
        assert_eq!(idx, vec!["c_a_b_idx".to_string()], "got {idx:?}");
    }

    /// `CREATE UNIQUE INDEX` copied via `INCLUDING INDEXES` keeps the `_idx`
    /// suffix (not `_key`), because Postgres uses `_idx` for plain indexes
    /// regardless of uniqueness; `_key` is only for UNIQUE *constraints*.
    #[test]
    fn unique_index_keeps_idx_suffix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (a int);\n\
             CREATE UNIQUE INDEX ON pub.base (a);\n\
             CREATE TABLE pub.c (LIKE pub.base INCLUDING INDEXES);\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let copied: Vec<_> = cat.indexes.iter()
            .filter(|i| i.on.qname().name.as_str() == "c")
            .collect();
        assert_eq!(copied.len(), 1, "expected exactly one copied index, got {copied:?}");
        assert_eq!(copied[0].qname.name.as_str(), "c_a_idx",
            "unique plain index must use _idx suffix, not _key");
        assert!(copied[0].unique, "copied index must preserve unique flag");
    }

    /// A bare `LIKE` (no `INCLUDING INDEXES`) must not copy any plain indexes.
    #[test]
    fn bare_like_copies_no_indexes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (a int, b int);\n\
             CREATE INDEX ON pub.base (a, b);\n\
             CREATE TABLE pub.d (LIKE pub.base);\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let targeting_d: Vec<_> = cat.indexes.iter()
            .filter(|i| i.on.qname().name.as_str() == "d")
            .collect();
        assert!(targeting_d.is_empty(),
            "bare LIKE must not copy plain indexes, got {targeting_d:?}");
    }

    // ── INCLUDING STATISTICS tests ────────────────────────────────────────────

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
        assert_eq!(
            copied[0].qname.name.as_str(), "c_a_b_stat",
            "generated statistic qname should be c_a_b_stat, got {:?}",
            copied[0].qname.name.as_str(),
        );
    }

    #[test]
    fn bare_like_copies_no_stats() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (a int, b int);\n\
             CREATE STATISTICS pub.base_stat (ndistinct) ON a, b FROM pub.base;\n\
             CREATE TABLE pub.d (LIKE pub.base);\n").unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let targeting_d: Vec<_> = cat.statistics.iter()
            .filter(|s| s.target.name.as_str() == "d")
            .collect();
        assert!(targeting_d.is_empty(),
            "bare LIKE must not copy statistics, got {targeting_d:?}");
    }

    // ── INCLUDING ALL integration tests ──────────────────────────────────────

    /// `INCLUDING ALL` copies defaults, PK+UNIQUE constraints (via INDEXES),
    /// CHECK constraints (via CONSTRAINTS), and plain indexes.
    #[test]
    fn including_all_copies_everything() {
        use crate::ir::constraint::ConstraintKind::{Check, PrimaryKey};
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(
            dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (id int PRIMARY KEY DEFAULT 1, n int CHECK (n > 0));\n\
             CREATE INDEX ON pub.base (n);\n\
             CREATE TABLE pub.c (LIKE pub.base INCLUDING ALL);\n",
        )
        .unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let c = cat.tables.iter().find(|t| t.qname.name.as_str() == "c").unwrap();
        // DEFAULTS: id column should carry the default
        assert!(
            c.columns[0].default.is_some(),
            "INCLUDING ALL must copy DEFAULT (DEFAULTS bit)"
        );
        // INDEXES: PK constraint copied
        assert!(
            c.constraints.iter().any(|k| matches!(k.kind, PrimaryKey { .. })),
            "INCLUDING ALL must copy PrimaryKey constraint (INDEXES bit)"
        );
        // CONSTRAINTS: CHECK constraint copied
        assert!(
            c.constraints.iter().any(|k| matches!(k.kind, Check { .. })),
            "INCLUDING ALL must copy Check constraint (CONSTRAINTS bit)"
        );
        // INDEXES (plain): a plain index targeting the clone must exist
        assert!(
            cat.indexes.iter().any(|i| i.on.qname().name.as_str() == "c"),
            "INCLUDING ALL must copy plain index (INDEXES bit)"
        );
    }

    /// LIKE of a VIEW source must produce a clear error mentioning "LIKE source".
    #[test]
    fn like_non_table_source_errors_clearly() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(
            dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (id int);\n\
             CREATE VIEW pub.v AS SELECT id FROM pub.base;\n\
             CREATE TABLE pub.c (LIKE pub.v);\n",
        )
        .unwrap();
        let err = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap_err();
        assert!(
            format!("{err}").contains("LIKE source"),
            "error should mention 'LIKE source', got: {err}"
        );
    }

    /// `INCLUDING ALL` on a table with CHECK + PK + UNIQUE must not double-copy
    /// any constraint kind.  Each kind must appear exactly once.
    #[test]
    fn including_all_no_double_copy_of_check() {
        use crate::ir::constraint::ConstraintKind::{Check, PrimaryKey, Unique};
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(
            dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (n int CHECK (n > 0), id int PRIMARY KEY, e text UNIQUE);\n\
             CREATE TABLE pub.c (LIKE pub.base INCLUDING ALL);\n",
        )
        .unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let c = cat.tables.iter().find(|t| t.qname.name.as_str() == "c").unwrap();
        let check_count = c.constraints.iter().filter(|k| matches!(k.kind, Check { .. })).count();
        let pk_count    = c.constraints.iter().filter(|k| matches!(k.kind, PrimaryKey { .. })).count();
        let uq_count    = c.constraints.iter().filter(|k| matches!(k.kind, Unique { .. })).count();
        assert_eq!(check_count, 1, "expected exactly 1 Check, got {check_count}");
        assert_eq!(pk_count,    1, "expected exactly 1 PrimaryKey, got {pk_count}");
        assert_eq!(uq_count,    1, "expected exactly 1 Unique, got {uq_count}");
    }

    /// A UNIQUE constraint-backing index and a plain `CREATE INDEX` on the same
    /// column must receive distinct names; they must not collide even though they
    /// share the same `taken` namespace.
    #[test]
    fn constraint_and_index_share_name_namespace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(
            dir.path().join("pub/t.sql"),
            // UNIQUE constraint on `a` → backing name base_a_key;
            // plain index on `a`        → name base_a_idx.
            "CREATE TABLE pub.base (a int UNIQUE);\n\
             CREATE INDEX ON pub.base (a);\n\
             CREATE TABLE pub.c (LIKE pub.base INCLUDING ALL);\n",
        )
        .unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let c = cat.tables.iter().find(|t| t.qname.name.as_str() == "c").unwrap();
        // The UNIQUE constraint should be named c_a_key.
        let uq_name = c
            .constraints
            .iter()
            .find(|k| matches!(k.kind, crate::ir::constraint::ConstraintKind::Unique { .. }))
            .map(|k| k.qname.name.as_str().to_string())
            .expect("expected a Unique constraint on c");
        // The plain index should be named c_a_idx.
        let idx_name = cat
            .indexes
            .iter()
            .find(|i| i.on.qname().name.as_str() == "c")
            .map(|i| i.qname.name.as_str().to_string())
            .expect("expected a plain index on c");
        // They must not collide.
        assert_ne!(
            uq_name, idx_name,
            "Unique constraint name and plain index name must not collide: both are {uq_name:?}"
        );
        // They should follow the Postgres naming conventions.
        assert_eq!(uq_name,  "c_a_key", "Unique constraint should be c_a_key, got {uq_name:?}");
        assert_eq!(idx_name, "c_a_idx", "Plain index should be c_a_idx, got {idx_name:?}");
    }

    /// Multi-hop LIKE: `leaf (LIKE mid INCLUDING ALL)` where `mid (LIKE base
    /// INCLUDING ALL)`.  The leaf must carry the PK from the base through the
    /// intermediate clone.
    #[test]
    fn chained_like_including_all_propagates() {
        use crate::ir::constraint::ConstraintKind::PrimaryKey;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pub")).unwrap();
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n").unwrap();
        std::fs::write(
            dir.path().join("pub/t.sql"),
            "CREATE TABLE pub.base (id int PRIMARY KEY);\n\
             CREATE TABLE pub.mid  (LIKE pub.base INCLUDING ALL);\n\
             CREATE TABLE pub.leaf (LIKE pub.mid  INCLUDING ALL);\n",
        )
        .unwrap();
        let (cat, _) = crate::parse::parse_directory_with_locations(dir.path(), &[]).unwrap();
        let leaf = cat.tables.iter().find(|t| t.qname.name.as_str() == "leaf").unwrap();
        // The `id` column must propagate all the way to leaf.
        assert!(
            leaf.columns.iter().any(|c| c.name.as_str() == "id"),
            "leaf must have the id column propagated via mid"
        );
        // leaf must have a PrimaryKey constraint named leaf_pkey.
        let pk = leaf
            .constraints
            .iter()
            .find(|k| matches!(k.kind, PrimaryKey { .. }))
            .expect("leaf must have a PrimaryKey constraint");
        assert_eq!(
            pk.qname.name.as_str(), "leaf_pkey",
            "leaf PK should be named leaf_pkey, got {:?}",
            pk.qname.name.as_str()
        );
    }
}
