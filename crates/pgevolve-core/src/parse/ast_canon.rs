//! AST canonicalization pass for view and materialized view bodies.
//!
//! Runs after the source parser produces a provisional Catalog with
//! views/MVs whose `body_canonical` is the empty sentinel and
//! `body_dependencies` is empty. For each view and MV:
//!
//!  1. Calls [`NormalizedBody::from_sql`] on `raw_body` to fill
//!     `body_canonical`.
//!  2. Walks the body AST to extract [`DepEdge`]s with
//!     [`DepSource::AstExtracted`].
//!  3. Resolves each referenced relation against the provisional Catalog.
//!     Unresolved → [`AstCanonError::UnresolvedReference`].
//!  4. Fills `columns` from the SELECT target list when the alias list was
//!     absent (PG's column-naming algorithm: explicit alias → rightmost
//!     `ColumnRef` name → `"?column?"` fallback).
//!
//! No Postgres, no network, no Docker.

use std::collections::BTreeSet;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::column_type::ColumnType;
use crate::ir::index::IndexParent;
use crate::ir::view::ViewColumn;
use crate::parse::normalize_body::NormalizedBody;
use crate::plan::edges::{DepEdge, DepSource, NodeId};

/// Errors raised by the AST canonicalization pass.
#[derive(Debug, thiserror::Error)]
pub enum AstCanonError {
    /// `pg_query` or `NormalizedBody` failed to parse the body.
    #[error("view {view}: failed to canonicalize body: {reason}")]
    NormalizeFailed {
        /// Qualified name of the view.
        view: String,
        /// Underlying error message.
        reason: String,
    },
    /// The body AST references a relation that was not declared in source.
    #[error("view {view}: references {object} which is not declared in source")]
    UnresolvedReference {
        /// Qualified name of the view.
        view: String,
        /// Qualified name of the missing object.
        object: String,
    },
}

/// Fills `body_canonical`, `body_dependencies`, and (when needed) `columns`
/// for all views and materialized views in `catalog`. Mutates in place.
///
/// Errors on the first unresolvable reference (fail-fast, like the existing
/// `ast_resolution` pass).
pub fn canonicalize_view_bodies(catalog: &mut Catalog) -> Result<(), AstCanonError> {
    // Build the set of known relations up front. We snapshot from the catalog
    // once; the catalog is not mutated between view passes.
    let known = KnownObjects::from_catalog(catalog);

    // --- Regular views ---
    for i in 0..catalog.views.len() {
        let raw_body = catalog.views[i].raw_body.clone();
        let qname = catalog.views[i].qname.clone();
        let qname_str = qname.to_string();
        let has_explicit_columns = !catalog.views[i].columns.is_empty();

        let normalized =
            NormalizedBody::from_sql(&raw_body).map_err(|e| AstCanonError::NormalizeFailed {
                view: qname_str.clone(),
                reason: e.to_string(),
            })?;

        let (deps, derived_columns) = walk_body_ast(&raw_body, &qname, &known)?;

        catalog.views[i].body_canonical = normalized;
        catalog.views[i].body_dependencies = deps;
        // Only fill columns from the SELECT target list when no explicit alias
        // list was provided in the CREATE VIEW statement.
        if !has_explicit_columns {
            catalog.views[i].columns = derived_columns;
        }
    }

    // --- Materialized views ---
    for i in 0..catalog.materialized_views.len() {
        let raw_body = catalog.materialized_views[i].raw_body.clone();
        let qname = catalog.materialized_views[i].qname.clone();
        let has_explicit_columns = !catalog.materialized_views[i].columns.is_empty();

        let normalized =
            NormalizedBody::from_sql(&raw_body).map_err(|e| AstCanonError::NormalizeFailed {
                view: qname.to_string(),
                reason: e.to_string(),
            })?;

        let (deps, derived_columns) = walk_body_ast(&raw_body, &qname, &known)?;

        catalog.materialized_views[i].body_canonical = normalized;
        catalog.materialized_views[i].body_dependencies = deps;
        if !has_explicit_columns {
            catalog.materialized_views[i].columns = derived_columns;
        }
    }

    Ok(())
}

/// Promote `IndexParent::Table(q)` to `IndexParent::Mv(q)` for every source-
/// side index whose parent qname actually belongs to a materialized view.
///
/// The source parser (`parse/builder/index_stmt.rs`) hardcodes
/// `IndexParent::Table` for all `CREATE INDEX` statements because at parse
/// time it does not yet know whether the relation name refers to a table or an
/// MV. This pass runs after both the indexes *and* the MVs are in the catalog
/// and corrects the parent variant.
///
/// Consequences of the promotion:
/// - `mv_has_unique_index` (in `plan/rewrite/refresh_mv_concurrently`) matches
///   `IndexParent::Mv` — REFRESH CONCURRENTLY now fires.
/// - `build_create_graph` routes `IndexParent::Mv(q)` to `NodeId::Mv(q)`, so
///   the dep-graph correctly orders `CREATE MATERIALIZED VIEW` before
///   `CREATE INDEX` on that MV.
/// - The `mv-no-unique-index` lint no longer false-positives for source-
///   defined MVs with source-defined unique indexes.
pub fn promote_mv_index_parents(catalog: &mut Catalog) {
    let mv_qnames: BTreeSet<QualifiedName> = catalog
        .materialized_views
        .iter()
        .map(|mv| mv.qname.clone())
        .collect();
    if mv_qnames.is_empty() {
        return;
    }
    for idx in &mut catalog.indexes {
        if let IndexParent::Table(qname) = &idx.on
            && mv_qnames.contains(qname)
        {
            idx.on = IndexParent::Mv(qname.clone());
        }
    }
}

/// Snapshot of the qualified names known in the catalog, used for reference
/// resolution during AST walking.
struct KnownObjects {
    /// All table and view qualified names (both are valid relation references).
    relations: BTreeSet<QualifiedName>,
}

impl KnownObjects {
    fn from_catalog(catalog: &Catalog) -> Self {
        let mut relations = BTreeSet::new();
        for t in &catalog.tables {
            relations.insert(t.qname.clone());
        }
        for v in &catalog.views {
            relations.insert(v.qname.clone());
        }
        for m in &catalog.materialized_views {
            relations.insert(m.qname.clone());
        }
        Self { relations }
    }

    fn has_relation(&self, qname: &QualifiedName) -> bool {
        self.relations.contains(qname)
    }
}

/// Walk the parsed body AST. Returns `(dep_edges, derived_columns)`.
///
/// `derived_columns` are derived from the top-level SELECT target list via
/// PG's column-naming algorithm; the caller decides whether to apply them.
fn walk_body_ast(
    raw_body: &str,
    view_qname: &QualifiedName,
    known: &KnownObjects,
) -> Result<(Vec<DepEdge>, Vec<ViewColumn>), AstCanonError> {
    let qname_str = view_qname.to_string();
    let parsed = pg_query::parse(raw_body).map_err(|e| AstCanonError::NormalizeFailed {
        view: qname_str.clone(),
        reason: e.to_string(),
    })?;

    let mut deps: Vec<DepEdge> = Vec::new();
    let mut columns: Vec<ViewColumn> = Vec::new();

    let Some(root) = parsed.protobuf.stmts.first() else {
        return Ok((deps, columns));
    };

    let Some(node) = &root.stmt else {
        return Ok((deps, columns));
    };

    walk_node(
        node,
        view_qname,
        known,
        &mut deps,
        &mut columns,
        /* is_top_level_select */ true,
        &qname_str,
    )?;

    // Stable dedup: sort then dedup (DepEdge derives Ord).
    deps.sort();
    deps.dedup();

    Ok((deps, columns))
}

/// Recursive AST walker over a [`pg_query::protobuf::Node`].
///
/// `is_top_level_select` is `true` only for the outermost `SelectStmt` —
/// this is the one whose target list produces the view's column names.
fn walk_node(
    node: &pg_query::protobuf::Node,
    view_qname: &QualifiedName,
    known: &KnownObjects,
    deps: &mut Vec<DepEdge>,
    columns: &mut Vec<ViewColumn>,
    is_top_level_select: bool,
    qname_str: &str,
) -> Result<(), AstCanonError> {
    use pg_query::NodeEnum as N;
    let Some(inner) = &node.node else {
        return Ok(());
    };

    match inner {
        // ------------------------------------------------------------------ //
        // SELECT: capture target list when top-level, then recurse.
        // ------------------------------------------------------------------ //
        N::SelectStmt(sel) => {
            if is_top_level_select {
                for target in &sel.target_list {
                    if let Some(col) = derive_column_name(target) {
                        columns.push(col);
                    }
                }
            }
            // Recurse into FROM clauses (tables, subqueries, joins).
            for from in &sel.from_clause {
                walk_node(from, view_qname, known, deps, columns, false, qname_str)?;
            }
            // WHERE clause may reference functions that in turn reference relations
            // (not extracted in v0.2; just recurse for structural completeness).
            if let Some(wc) = &sel.where_clause {
                walk_node(wc, view_qname, known, deps, columns, false, qname_str)?;
            }
            // UNION / INTERSECT / EXCEPT: recurse into both branches.
            if let Some(larg) = &sel.larg {
                walk_select(larg, view_qname, known, deps, qname_str)?;
            }
            if let Some(rarg) = &sel.rarg {
                walk_select(rarg, view_qname, known, deps, qname_str)?;
            }
            // WITH (CTE) clause.
            if let Some(with) = &sel.with_clause {
                for cte in &with.ctes {
                    walk_node(cte, view_qname, known, deps, columns, false, qname_str)?;
                }
            }
        }

        // ------------------------------------------------------------------ //
        // Table/view reference: resolve and emit a DepEdge.
        // ------------------------------------------------------------------ //
        N::RangeVar(rv) => {
            // Only schema-qualified names are checked against the catalog.
            // Unqualified references require schema-search-path knowledge
            // which is out of scope for v0.2 (file directives don't propagate
            // to query body resolution yet).
            // If identifier construction fails (e.g., overlong name),
            // skip silently — pg_query already validated it's parseable.
            if !rv.schemaname.is_empty()
                && !rv.relname.is_empty()
                && let Ok(s) = Identifier::from_unquoted(&rv.schemaname)
                    .or_else(|_| Identifier::from_quoted(&rv.schemaname))
                && let Ok(n) = Identifier::from_unquoted(&rv.relname)
                    .or_else(|_| Identifier::from_quoted(&rv.relname))
            {
                let ref_qname = QualifiedName::new(s, n);
                if !known.has_relation(&ref_qname) {
                    return Err(AstCanonError::UnresolvedReference {
                        view: qname_str.to_string(),
                        object: ref_qname.to_string(),
                    });
                }
                deps.push(DepEdge {
                    from: NodeId::Table(view_qname.clone()),
                    to: NodeId::Table(ref_qname),
                    source: DepSource::AstExtracted,
                });
            }
        }

        // ------------------------------------------------------------------ //
        // JOIN: recurse into left and right args.
        // ------------------------------------------------------------------ //
        N::JoinExpr(j) => {
            if let Some(larg) = &j.larg {
                walk_node(larg, view_qname, known, deps, columns, false, qname_str)?;
            }
            if let Some(rarg) = &j.rarg {
                walk_node(rarg, view_qname, known, deps, columns, false, qname_str)?;
            }
        }

        // ------------------------------------------------------------------ //
        // Subquery in FROM: `SELECT ... FROM (SELECT ...) sub`
        // ------------------------------------------------------------------ //
        N::RangeSubselect(sub) => {
            if let Some(subquery) = &sub.subquery {
                walk_node(subquery, view_qname, known, deps, columns, false, qname_str)?;
            }
        }

        // ------------------------------------------------------------------ //
        // CommonTableExpr (CTE): walk the query inside.
        // ------------------------------------------------------------------ //
        N::CommonTableExpr(cte) => {
            if let Some(q) = &cte.ctequery {
                walk_node(q, view_qname, known, deps, columns, false, qname_str)?;
            }
        }

        // Other node types (expressions, literals, function calls, etc.) do
        // not contain relation references that need resolving in v0.2.
        _ => {}
    }

    Ok(())
}

/// Recurse into a nested `SelectStmt` (from UNION / INTERSECT / EXCEPT).
fn walk_select(
    sel: &pg_query::protobuf::SelectStmt,
    view_qname: &QualifiedName,
    known: &KnownObjects,
    deps: &mut Vec<DepEdge>,
    qname_str: &str,
) -> Result<(), AstCanonError> {
    // Wrap the SelectStmt in a Node for the generic walker.
    let node = pg_query::protobuf::Node {
        node: Some(pg_query::NodeEnum::SelectStmt(Box::new(sel.clone()))),
    };
    // Pass empty columns vec; set-operation branches don't contribute column names.
    walk_node(
        &node,
        view_qname,
        known,
        deps,
        &mut Vec::new(),
        false,
        qname_str,
    )
}

/// Derive a view column name from a SELECT target (`ResTarget`).
///
/// Uses PG's column-naming algorithm:
///  1. `ResTarget.name` (explicit `AS alias`) wins.
///  2. Otherwise the rightmost field name of a `ColumnRef`
///     (`schema.table.col` → `"col"`).
///  3. Otherwise `"?column?"` (PG's fallback for expressions with no name).
///
/// The `column_type` is set to `ColumnType::Other { raw: "expression" }` as
/// a sentinel for the v0.2 source-side path; the live-catalog path (T5)
/// populates the real type from `format_type(a.atttypid, a.atttypmod)`.
/// The OR-REPLACE compatibility predicate in `diff::views` uses catalog-side
/// types (target) vs. catalog-side types (source), so expression-typed columns
/// from the source side will compare as unequal to typed columns in the
/// catalog, conservatively declaring a replace incompatible — which is correct.
fn derive_column_name(target: &pg_query::protobuf::Node) -> Option<ViewColumn> {
    use pg_query::NodeEnum as N;
    let Some(N::ResTarget(rt)) = &target.node else {
        return None;
    };

    // 1. Explicit alias.
    if !rt.name.is_empty() {
        let name = Identifier::from_unquoted(&rt.name)
            .or_else(|_| Identifier::from_quoted(&rt.name))
            .ok()?;
        return Some(ViewColumn {
            name,
            column_type: ColumnType::Other {
                raw: "expression".to_string(),
            },
            comment: None,
        });
    }

    // 2. Try to extract the rightmost ColumnRef field name.
    if let Some(val_box) = &rt.val
        && let Some(col_name) = extract_column_ref_name(val_box)
    {
        return Some(ViewColumn {
            name: col_name,
            column_type: ColumnType::Other {
                raw: "expression".to_string(),
            },
            comment: None,
        });
    }

    // 3. PG fallback.
    Identifier::from_quoted("?column?")
        .ok()
        .map(|name| ViewColumn {
            name,
            column_type: ColumnType::Other {
                raw: "expression".to_string(),
            },
            comment: None,
        })
}

/// Attempt to extract a column name from a node that may be a `ColumnRef`.
///
/// Returns `None` for expressions that are not simple column references.
fn extract_column_ref_name(node: &pg_query::protobuf::Node) -> Option<Identifier> {
    use pg_query::NodeEnum as N;
    match &node.node {
        Some(N::ColumnRef(cr)) => {
            // Take the rightmost `String` field.
            for field in cr.fields.iter().rev() {
                if let Some(N::String(s)) = &field.node
                    && !s.sval.is_empty()
                {
                    return Identifier::from_unquoted(&s.sval)
                        .or_else(|_| Identifier::from_quoted(&s.sval))
                        .ok();
                }
            }
            None
        }
        // TypeCast: `expr::type` — the name comes from the inner expression.
        Some(N::TypeCast(tc)) => tc.arg.as_deref().and_then(extract_column_ref_name),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::index::{
        Index, IndexColumn, IndexColumnExpr, IndexMethod, NullsOrder, SortOrder,
    };
    use crate::ir::view::MaterializedView;
    use crate::parse::normalize_body::NormalizedBody;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn simple_index(name: &str, on: IndexParent, unique: bool) -> Index {
        Index {
            qname: qn("app", name),
            on,
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("k")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
        }
    }

    fn simple_mv(schema: &str, name: &str) -> MaterializedView {
        MaterializedView {
            qname: qn(schema, name),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
        }
    }

    // ── promote_mv_index_parents ─────────────────────────────────────────────

    /// A source-SQL unique index on an MV (`CREATE UNIQUE INDEX ... ON app.mv`)
    /// starts life as `IndexParent::Table` and must be promoted to
    /// `IndexParent::Mv` by the pass.
    #[test]
    fn unique_index_on_mv_is_promoted_to_mv_parent() {
        let mv_qname = qn("app", "mv");
        let mut catalog = Catalog::empty();
        catalog.materialized_views.push(simple_mv("app", "mv"));
        // Initially parsed as Table (as the source parser always does).
        catalog.indexes.push(simple_index(
            "mv_uk",
            IndexParent::Table(mv_qname.clone()),
            true,
        ));

        promote_mv_index_parents(&mut catalog);

        assert!(
            matches!(&catalog.indexes[0].on, IndexParent::Mv(q) if q == &mv_qname),
            "expected IndexParent::Mv after promotion, got {:?}",
            catalog.indexes[0].on,
        );
    }

    /// An index on a *table* must not be promoted, even if a same-named MV
    /// does not exist.
    #[test]
    fn index_on_table_is_not_promoted() {
        let table_qname = qn("app", "users");
        let mut catalog = Catalog::empty();
        // No MVs in the catalog.
        catalog.indexes.push(simple_index(
            "users_idx",
            IndexParent::Table(table_qname.clone()),
            false,
        ));

        promote_mv_index_parents(&mut catalog);

        assert!(
            matches!(&catalog.indexes[0].on, IndexParent::Table(q) if q == &table_qname),
            "Table index must not be promoted: {:?}",
            catalog.indexes[0].on,
        );
    }

    /// When the catalog contains no MVs the pass is a no-op (early return).
    #[test]
    fn no_mvs_no_promotion() {
        let mut catalog = Catalog::empty();
        catalog.indexes.push(simple_index(
            "some_idx",
            IndexParent::Table(qn("app", "t")),
            false,
        ));
        promote_mv_index_parents(&mut catalog);
        assert!(matches!(&catalog.indexes[0].on, IndexParent::Table(_)));
    }

    /// An index that already has `IndexParent::Mv` (e.g., from the live-catalog
    /// reader) must be left unchanged.
    #[test]
    fn already_mv_parent_is_unchanged() {
        let mv_qname = qn("app", "mv");
        let mut catalog = Catalog::empty();
        catalog.materialized_views.push(simple_mv("app", "mv"));
        catalog.indexes.push(simple_index(
            "mv_idx",
            IndexParent::Mv(mv_qname.clone()),
            false,
        ));

        promote_mv_index_parents(&mut catalog);

        assert!(
            matches!(&catalog.indexes[0].on, IndexParent::Mv(q) if q == &mv_qname),
            "already-Mv parent must remain Mv: {:?}",
            catalog.indexes[0].on,
        );
    }
}
