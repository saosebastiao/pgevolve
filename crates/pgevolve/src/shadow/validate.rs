//! Cross-check the source IR against a live shadow Postgres.
//!
//! For each view/MV the source declares, the shadow's `pg_get_viewdef` is
//! used to round-trip the body through Postgres and compare the
//! catalog-canonical form against the source-side [`NormalizedBody`].
//! `pg_depend` is then queried to cross-check `AstExtracted`
//! `body_dependencies` edges.
//!
//! Mismatches warn by default; `--shadow-strict` promotes them to errors.

use std::collections::BTreeSet;

use anyhow::Result;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::parse::normalize_body::NormalizedBody;
use pgevolve_core::plan::edges::{DepEdge, DepSource, NodeId, build_create_graph};
use pgevolve_core::render::render_catalog;

use crate::shadow::ShadowBackend;

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

/// Summary of a cross-check run.
#[derive(Debug, Default)]
pub struct CrossCheckReport {
    /// Number of Structural edges examined.
    pub structural_edges_checked: usize,
    /// Body text mismatches: source canonical ≠ catalog canonical.
    pub canonical_mismatches: Vec<CanonicalMismatch>,
    /// Edges present in `pg_depend` but absent from source `body_dependencies`
    /// (filtered to `AstExtracted` — `AstDeclared` covers dynamic SQL
    /// intentionally).
    pub missing_ast_edges: Vec<MissingEdge>,
    /// Edges present in source `body_dependencies` (`AstExtracted`) but absent
    /// from `pg_depend`.
    pub extra_ast_edges: Vec<ExtraEdge>,
}

/// A view/MV whose body canonical text differs between source and shadow.
#[derive(Debug)]
pub struct CanonicalMismatch {
    /// Schema-qualified view name, e.g. `app.active_users`.
    pub view_qname: String,
    /// Canonical body text as recorded in the source IR.
    pub source_canonical: String,
    /// Canonical body text produced by round-tripping through `pg_get_viewdef`.
    pub catalog_canonical: String,
}

/// An edge present in `pg_depend` but absent from AST-extracted
/// `body_dependencies`.
#[derive(Debug)]
pub struct MissingEdge {
    /// Schema-qualified view name, e.g. `app.active_users`.
    pub view_qname: String,
    /// Schema of the referenced object reported by `pg_depend`.
    pub ref_schema: String,
    /// Name of the referenced object reported by `pg_depend`.
    pub ref_name: String,
}

/// An AST-extracted edge present in source `body_dependencies` but absent
/// from `pg_depend`.
#[derive(Debug)]
pub struct ExtraEdge {
    /// Schema-qualified view name, e.g. `app.active_users`.
    pub view_qname: String,
    /// `schema.name` key of the declared dependency.
    pub dep_node: String,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the cross-check against a shadow backend.
///
/// If the source catalog contains no views or materialized views, this is a
/// fast no-op (counts structural edges only — identical to v0.1 behaviour).
///
/// When views/MVs are present:
/// 1. Boots shadow, applies source via `render_catalog`.
/// 2. Queries `pg_get_viewdef` for each view/MV; re-canonicalizes; compares.
/// 3. Queries `pg_depend` for each view/MV; cross-checks `AstExtracted` edges.
/// 4. Populates `canonical_mismatches`, `extra_ast_edges`, `missing_ast_edges`.
///
/// Under `strict`, returns `Err` if any mismatch list is non-empty.
pub async fn cross_check(
    backend: &dyn ShadowBackend,
    source: &Catalog,
    pg_major: u32,
    strict: bool,
) -> Result<CrossCheckReport> {
    let mut report = CrossCheckReport::default();

    // Always count structural edges (v0.1 compatibility).
    let graph = build_create_graph(source);
    for edge in graph.dep_edges() {
        if matches!(edge.source, DepSource::Structural) {
            report.structural_edges_checked += 1;
        }
    }

    // Only run view cross-checks when views or MVs are present.
    if source.views.is_empty() && source.materialized_views.is_empty() {
        return Ok(report);
    }

    // Boot shadow, apply source.
    let guard = backend.checkout(pg_major).await?;
    apply_source_to_shadow(guard.url(), source).await?;

    let (client, conn) = tokio_postgres::connect(guard.url(), tokio_postgres::NoTls).await?;
    tokio::spawn(conn);

    check_views(&client, source, &mut report).await?;
    check_materialized_views(&client, source, &mut report).await?;

    if strict
        && (!report.canonical_mismatches.is_empty()
            || !report.extra_ast_edges.is_empty()
            || !report.missing_ast_edges.is_empty())
    {
        let n_canon = report.canonical_mismatches.len();
        let n_extra = report.extra_ast_edges.len();
        let n_missing = report.missing_ast_edges.len();
        anyhow::bail!(
            "shadow-strict: {n_canon} canonical mismatch(es), \
             {n_extra} extra AST edge(s), \
             {n_missing} missing AST edge(s)"
        );
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// Per-object-type check helpers
// ---------------------------------------------------------------------------

async fn check_views(
    client: &tokio_postgres::Client,
    source: &Catalog,
    report: &mut CrossCheckReport,
) -> Result<()> {
    for view in &source.views {
        let qname = view.qname.to_string();
        let qname_sql = view.qname.render_sql();
        check_body_canonical(client, &qname, &qname_sql, &view.body_canonical, report).await?;
        check_dep_edges(client, &qname, &qname_sql, &view.body_dependencies, report).await?;
    }
    Ok(())
}

async fn check_materialized_views(
    client: &tokio_postgres::Client,
    source: &Catalog,
    report: &mut CrossCheckReport,
) -> Result<()> {
    for mv in &source.materialized_views {
        let qname = mv.qname.to_string();
        let qname_sql = mv.qname.render_sql();
        check_body_canonical(client, &qname, &qname_sql, &mv.body_canonical, report).await?;
        check_dep_edges(client, &qname, &qname_sql, &mv.body_dependencies, report).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Core check helpers
// ---------------------------------------------------------------------------

/// Apply the source catalog to the shadow DB by executing the output of
/// `render_catalog` via `batch_execute`.
async fn apply_source_to_shadow(url: &str, source: &Catalog) -> Result<()> {
    let sql = render_catalog(source);
    if sql.trim().is_empty() {
        return Ok(());
    }
    let (client, conn) = tokio_postgres::connect(url, tokio_postgres::NoTls).await?;
    tokio::spawn(conn);
    client
        .batch_execute(&sql)
        .await
        .map_err(|e| anyhow::anyhow!("apply_source_to_shadow failed: {e}\nSQL:\n{sql}"))?;
    Ok(())
}

/// Round-trip the view/MV body through `pg_get_viewdef` and compare canonical text.
async fn check_body_canonical(
    client: &tokio_postgres::Client,
    qname: &str,
    qname_sql: &str,
    source_body: &NormalizedBody,
    report: &mut CrossCheckReport,
) -> Result<()> {
    let pg_body: String = client
        .query_one(
            &format!("SELECT pg_get_viewdef('{qname_sql}'::regclass, true)"),
            &[],
        )
        .await
        .map(|row| row.get::<_, String>(0))
        .map_err(|e| anyhow::anyhow!("pg_get_viewdef failed for {qname}: {e}"))?;

    match NormalizedBody::from_sql(&pg_body) {
        Ok(catalog_canonical) => {
            if catalog_canonical.canonical_text() != source_body.canonical_text() {
                report.canonical_mismatches.push(CanonicalMismatch {
                    view_qname: qname.to_string(),
                    source_canonical: source_body.canonical_text().to_string(),
                    catalog_canonical: catalog_canonical.canonical_text().to_string(),
                });
            }
        }
        Err(e) => {
            report.canonical_mismatches.push(CanonicalMismatch {
                view_qname: qname.to_string(),
                source_canonical: source_body.canonical_text().to_string(),
                catalog_canonical: format!("<parse error: {e}>"),
            });
        }
    }
    Ok(())
}

/// Cross-check `body_dependencies` for a single view/MV against `pg_depend`.
///
/// Filters `body_dependencies` to `AstExtracted` edges only — `AstDeclared`
/// edges cover dynamic SQL that `pg_depend` will never see, so they are
/// tolerated without comparison.
async fn check_dep_edges(
    client: &tokio_postgres::Client,
    qname: &str,
    qname_sql: &str,
    body_dependencies: &[DepEdge],
    report: &mut CrossCheckReport,
) -> Result<()> {
    // Build AST-extracted set from source IR.
    let ast_extracted: BTreeSet<String> = body_dependencies
        .iter()
        .filter(|e| e.source == DepSource::AstExtracted)
        .map(|e| node_id_to_key(&e.to))
        .collect();

    if ast_extracted.is_empty() {
        // No AstExtracted edges to verify — skip pg_depend query.
        return Ok(());
    }

    // Postgres records view→table dependencies via pg_rewrite rules:
    //   pg_depend(classid=pg_rewrite, objid=rule.oid) → (refclassid=pg_class, refobjid=table.oid)
    // We use a CTE to resolve the view OID and the pg_rewrite rule for it,
    // then find all referenced pg_class objects excluding the view itself.
    let rows = client
        .query(
            &format!(
                "WITH view_info AS ( \
                     SELECT c.oid AS view_oid \
                     FROM pg_class c \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname || '.' || c.relname = '{qname_sql}' \
                 ) \
                 SELECT DISTINCT ref_n.nspname AS ref_schema, ref_c.relname AS ref_name \
                 FROM view_info vi \
                 JOIN pg_rewrite rw ON rw.ev_class = vi.view_oid \
                 JOIN pg_depend d ON d.objid = rw.oid \
                                 AND d.classid = 'pg_rewrite'::regclass \
                                 AND d.refclassid = 'pg_class'::regclass \
                 JOIN pg_class ref_c ON ref_c.oid = d.refobjid \
                 JOIN pg_namespace ref_n ON ref_n.oid = ref_c.relnamespace \
                 WHERE d.refobjid <> vi.view_oid"
            ),
            &[],
        )
        .await
        .map_err(|e| anyhow::anyhow!("pg_depend/pg_rewrite query failed for {qname}: {e}"))?;

    let pg_deps: BTreeSet<String> = rows
        .iter()
        .map(|row| {
            let schema: String = row.get("ref_schema");
            let name: String = row.get("ref_name");
            format!("{schema}.{name}")
        })
        .collect();

    // extra_ast_edges: in AST set, not in pg_depend.
    for key in &ast_extracted {
        if !pg_deps.contains(key) {
            report.extra_ast_edges.push(ExtraEdge {
                view_qname: qname.to_string(),
                dep_node: key.clone(),
            });
        }
    }

    // missing_ast_edges: in pg_depend, not in AST set.
    for key in &pg_deps {
        if !ast_extracted.contains(key) {
            report.missing_ast_edges.push(MissingEdge {
                view_qname: qname.to_string(),
                ref_schema: key.split('.').next().unwrap_or("").to_string(),
                ref_name: key.split('.').nth(1).unwrap_or("").to_string(),
            });
        }
    }

    Ok(())
}

/// Convert a `NodeId` to a `schema.name` key string for set-membership tests.
fn node_id_to_key(node: &NodeId) -> String {
    match node {
        NodeId::Table(q)
        | NodeId::View(q)
        | NodeId::Mv(q)
        | NodeId::Index(q)
        | NodeId::Sequence(q)
        | NodeId::Type(q)
        | NodeId::Trigger(q)
        | NodeId::Procedure(q)
        | NodeId::Function(q, _) => q.to_string(),
        NodeId::Schema(id) | NodeId::Extension(id) => id.to_string(),
        NodeId::Constraint { table, name } => format!("{table}.{name}"),
    }
}
