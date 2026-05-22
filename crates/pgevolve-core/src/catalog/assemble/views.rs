//! View and materialized-view assembly from catalog rows.
//!
//! Called from [`super::assemble`] to build [`crate::ir::view::View`] and
//! [`crate::ir::view::MaterializedView`] IR entries.

use std::collections::HashMap;

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::catalog::filter::CatalogFilter;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column_type::ColumnType;
use crate::ir::view::{MaterializedView, View, ViewColumn};
use crate::parse::normalize_body::NormalizedBody;

use super::{ident_required, qname_from};

/// Parse a `reloptions` text array (`["security_barrier=true", ...]`) and
/// extract a named boolean option. Returns `None` if the option is absent.
fn parse_bool_reloption(reloptions: &[String], key: &str) -> Option<bool> {
    let prefix = format!("{key}=");
    for opt in reloptions {
        if let Some(val) = opt.strip_prefix(&prefix) {
            return Some(val.eq_ignore_ascii_case("true") || val == "1" || val == "on");
        }
    }
    None
}

/// Build a [`NormalizedBody`] from a `body_text` returned by
/// `pg_get_viewdef`. Returns an error if the text cannot be parsed.
fn build_body(body_text: &str, qname: &QualifiedName) -> Result<NormalizedBody, CatalogError> {
    NormalizedBody::from_sql(body_text).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "could not canonicalize body of view {qname}: {e}"
        )))
    })
}

/// Build all views and materialized views from catalog rows.
///
/// Returns `(views, materialized_views)`.
#[allow(clippy::too_many_lines)]
pub(super) fn build_views_and_mvs(
    view_rows: &[Row],
    column_rows: &[Row],
    filter: &CatalogFilter,
) -> Result<(Vec<View>, Vec<MaterializedView>), CatalogError> {
    // Group view-column rows by (schema_name, view_name) preserving attnum order.
    // Also collect per-column ACL grants keyed by the same key.
    let mut columns_by_key: HashMap<(String, String), Vec<ViewColumn>> = HashMap::new();
    // col_grants_by_key accumulates Grant entries derived from attacl per column.
    let mut col_grants_by_key: HashMap<(String, String), Vec<crate::ir::grant::Grant>> =
        HashMap::new();
    for cr in column_rows {
        let q = CatalogQuery::ViewColumns;
        let schema = cr.get_text(q, "schema_name")?;
        let view_name = cr.get_text(q, "view_name")?;
        let col_name_str = cr.get_text(q, "column_name")?;
        let col_type_str = cr.get_text(q, "column_type")?;
        let comment = cr.get_opt_text(q, "column_comment")?;
        let name = ident_required(&col_name_str)?;
        let column_type = ColumnType::parse_from_pg_type_string(&col_type_str).map_err(|e| {
            CatalogError::Ir(crate::ir::IrError::InvalidColumnType(format!(
                "view column {col_name_str} type {col_type_str:?}: {e}"
            )))
        })?;
        // Decode column-level ACL entries.
        let col_acl_strings = cr.get_text_array(q, "attacl")?;
        if !col_acl_strings.is_empty() {
            let col_grants = crate::catalog::grants::decode_aclitem_array(&col_acl_strings)?;
            let entry = col_grants_by_key
                .entry((schema.clone(), view_name.clone()))
                .or_default();
            for mut g in col_grants {
                g.columns = Some(vec![name.clone()]);
                entry.push(g);
            }
        }
        columns_by_key
            .entry((schema, view_name))
            .or_default()
            .push(ViewColumn {
                name,
                column_type,
                comment,
            });
    }

    let mut views: Vec<View> = Vec::new();
    let mut materialized_views: Vec<MaterializedView> = Vec::new();

    for r in view_rows {
        let q = CatalogQuery::ViewsAndMvs;
        let qname = qname_from(r, q, "schema_name", "name")?;
        if !filter.allows(&qname) {
            continue;
        }
        let relkind = r.get_text(q, "relkind")?;
        let body_text = r.get_text(q, "body_text")?;
        let reloptions = r.get_text_array(q, "reloptions").unwrap_or_default();
        let comment = r.get_opt_text(q, "comment")?;

        let owner_str = r.get_text(q, "owner")?;
        let owner_ident =
            Identifier::from_unquoted(&owner_str).map_err(|e| CatalogError::BadColumnType {
                query: q,
                column: "owner".to_string(),
                message: format!("invalid owner {owner_str:?}: {e}"),
            })?;
        let acl_strings = r.get_text_array(q, "acl")?;
        let raw_grants = crate::catalog::grants::decode_aclitem_array(&acl_strings)?;
        let mut grants = crate::catalog::grants::strip_owner_self_grants(raw_grants, &owner_ident);
        let owner = Some(owner_ident);

        let body_canonical = build_body(&body_text, &qname)?;

        let col_key = (
            qname.schema.as_str().to_string(),
            qname.name.as_str().to_string(),
        );
        let columns = columns_by_key.remove(&col_key).unwrap_or_default();

        // Append column-level ACL grants collected above.
        // Strip owner self-grants from attacl for the same reason as relacl.
        if let Some(col_grants) = col_grants_by_key.remove(&col_key) {
            let filtered = crate::catalog::grants::strip_owner_self_grants(
                col_grants,
                owner.as_ref().expect("owner was just set above"),
            );
            grants.extend(filtered);
        }

        // Extract dependencies by walking the body AST. On the catalog side we
        // don't need to resolve against a KnownObjects set — the DB is the ground
        // truth. We call the internal walker without the resolution guard by
        // building an unrestricted KnownObjects that always returns true.
        let body_dependencies = extract_deps_from_body(&body_text, &qname);

        match relkind.as_str() {
            "v" => {
                let security_barrier = parse_bool_reloption(&reloptions, "security_barrier");
                let security_invoker = parse_bool_reloption(&reloptions, "security_invoker");
                views.push(View {
                    qname,
                    columns,
                    body_canonical,
                    body_dependencies,
                    security_barrier,
                    security_invoker,
                    comment: comment.clone(),
                    raw_body: String::new(),
                    owner,
                    grants,
                });
            }
            "m" => {
                materialized_views.push(MaterializedView {
                    qname,
                    columns,
                    body_canonical,
                    body_dependencies,
                    comment: comment.clone(),
                    raw_body: String::new(),
                    owner,
                    grants,
                });
            }
            other => {
                return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                    format!("unexpected relkind {other:?} in view catalog query"),
                )));
            }
        }
    }

    Ok((views, materialized_views))
}

/// Walk a single AST node, collecting schema-qualified `RangeVar` references.
///
/// Used by [`extract_deps_from_body`] to avoid nesting a `fn` after statements
/// (which Clippy forbids with `items_after_statements`).
fn walk_node_for_deps(
    node: &pg_query::protobuf::Node,
    view_qname: &QualifiedName,
    deps: &mut Vec<crate::plan::edges::DepEdge>,
) {
    use crate::plan::edges::{DepEdge, DepSource, NodeId};
    use pg_query::NodeEnum as N;

    let Some(inner) = &node.node else { return };
    match inner {
        N::SelectStmt(sel) => {
            for from in &sel.from_clause {
                walk_node_for_deps(from, view_qname, deps);
            }
            if let Some(wc) = &sel.where_clause {
                walk_node_for_deps(wc, view_qname, deps);
            }
            if let Some(larg) = &sel.larg {
                let n = pg_query::protobuf::Node {
                    node: Some(N::SelectStmt(Box::new(larg.as_ref().clone()))),
                };
                walk_node_for_deps(&n, view_qname, deps);
            }
            if let Some(rarg) = &sel.rarg {
                let n = pg_query::protobuf::Node {
                    node: Some(N::SelectStmt(Box::new(rarg.as_ref().clone()))),
                };
                walk_node_for_deps(&n, view_qname, deps);
            }
            if let Some(with) = &sel.with_clause {
                for cte in &with.ctes {
                    walk_node_for_deps(cte, view_qname, deps);
                }
            }
        }
        N::RangeVar(rv) if !rv.schemaname.is_empty() && !rv.relname.is_empty() => {
            if let (Ok(s), Ok(n)) = (
                Identifier::from_unquoted(&rv.schemaname)
                    .or_else(|_| Identifier::from_quoted(&rv.schemaname)),
                Identifier::from_unquoted(&rv.relname)
                    .or_else(|_| Identifier::from_quoted(&rv.relname)),
            ) {
                let ref_qname = QualifiedName::new(s, n);
                deps.push(DepEdge {
                    from: NodeId::Table(view_qname.clone()),
                    to: NodeId::Table(ref_qname),
                    source: DepSource::AstExtracted,
                });
            }
        }
        N::JoinExpr(j) => {
            if let Some(l) = &j.larg {
                walk_node_for_deps(l, view_qname, deps);
            }
            if let Some(r) = &j.rarg {
                walk_node_for_deps(r, view_qname, deps);
            }
        }
        N::RangeSubselect(sub) => {
            if let Some(sq) = &sub.subquery {
                walk_node_for_deps(sq, view_qname, deps);
            }
        }
        N::CommonTableExpr(cte) => {
            if let Some(q) = &cte.ctequery {
                walk_node_for_deps(q, view_qname, deps);
            }
        }
        _ => {}
    }
}

/// Extract [`crate::plan::edges::DepEdge`]s from a view body on the catalog side.
///
/// On the catalog side we are the ground truth — there is no "unknown object"
/// error. We perform a best-effort extraction: any schema-qualified `RangeVar`
/// nodes become dep edges. Unresolvable or unqualified references are silently
/// skipped.
fn extract_deps_from_body(
    body_text: &str,
    view_qname: &QualifiedName,
) -> Vec<crate::plan::edges::DepEdge> {
    use crate::plan::edges::DepEdge;

    let Ok(parsed) = pg_query::parse(body_text) else {
        return vec![];
    };

    let mut deps: Vec<DepEdge> = Vec::new();
    for raw_stmt in &parsed.protobuf.stmts {
        if let Some(node) = &raw_stmt.stmt {
            walk_node_for_deps(node, view_qname, &mut deps);
        }
    }

    deps.sort();
    deps.dedup();
    deps
}
