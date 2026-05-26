//! Convert raw catalog rows into [`Catalog`] IR.
//!
//! This is the heart of the catalog reader: it stitches together rows from
//! [`crate::catalog::CatalogQuery`] into the same IR shape the source-side
//! parser produces.
//!
//! The strategy:
//! - Schemas, tables, sequences, columns: direct field-for-field translation.
//! - Indexes: re-parse `pg_get_indexdef` text via `pg_query` and reuse
//!   [`crate::parse::builder::index_stmt::build_index`].
//! - Constraints: build PK/UNIQUE/FK from row fields; for CHECK, extract the
//!   expression from `pg_get_constraintdef` text.
//! - Default expressions: parse the `pg_get_expr` text and run it through
//!   the same default-expr builder the source parser uses.
//! - SERIAL/IDENTITY ownership: walk the dependencies rows and populate
//!   `Sequence.owned_by` and the column-side `Identity`/`Default::Sequence`
//!   linkage so source-IR and catalog-IR converge on the same shape.

pub(super) mod default_privileges;
mod functions;
mod partitions;
pub(super) mod policies;
pub(super) mod publications;
mod tables;
mod triggers;
mod user_types;
mod views;

use std::collections::HashMap;
use std::path::PathBuf;

use pg_query::NodeEnum;

use crate::catalog::CatalogQuery;
use crate::catalog::DriftReport;
use crate::catalog::error::CatalogError;
use crate::catalog::filter::CatalogFilter;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::constraint::{FkMatchType, ReferentialAction};
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::extension::Extension;
use crate::ir::sequence::Sequence;

/// Bundle of rows passed to [`assemble`].
pub struct RawRows {
    pub version: crate::catalog::version::PgVersion,
    pub schemas: Vec<Row>,
    pub tables: Vec<Row>,
    pub columns: Vec<Row>,
    pub constraints: Vec<Row>,
    pub indexes: Vec<Row>,
    pub sequences: Vec<Row>,
    pub dependencies: Vec<Row>,
    pub views_and_mvs: Vec<Row>,
    pub view_columns: Vec<Row>,
    pub user_types: Vec<Row>,
    pub enum_values: Vec<Row>,
    pub domain_details: Vec<Row>,
    pub domain_checks: Vec<Row>,
    pub composite_attributes: Vec<Row>,
    pub functions: Vec<Row>,
    pub extensions: Vec<Row>,
    pub triggers: Vec<Row>,
    pub partitioned_tables: Vec<Row>,
    pub partitions: Vec<Row>,
    pub default_privileges: Vec<Row>,
    pub policies: Vec<Row>,
    /// `pg_publication` rows.
    pub publications: Vec<Row>,
    /// `pg_publication_rel` rows.
    pub publication_rels: Vec<Row>,
    /// `pg_publication_namespace` rows (PG 15+; empty on PG 14).
    pub publication_namespaces: Vec<Row>,
    /// Column-attnum resolver rows from `pg_attribute` joined to
    /// `pg_publication_rel`.
    pub publication_attributes: Vec<Row>,
}

/// Convert raw rows into a [`Catalog`] and a [`DriftReport`]. Caller is
/// responsible for canonicalization.
pub fn assemble(
    raw: RawRows,
    filter: &CatalogFilter,
) -> Result<(Catalog, DriftReport), CatalogError> {
    let RawRows {
        version: _version,
        schemas,
        tables,
        columns,
        constraints,
        indexes,
        sequences,
        dependencies,
        views_and_mvs,
        view_columns,
        user_types,
        enum_values,
        domain_details,
        domain_checks,
        composite_attributes,
        functions,
        extensions,
        triggers,
        partitioned_tables,
        partitions,
        default_privileges,
        policies,
        publications: pub_rows,
        publication_rels,
        publication_namespaces,
        publication_attributes,
    } = raw;

    let mut catalog = Catalog::empty();
    let mut drift = DriftReport::default();

    // Build table family: schemas, tables, columns, constraints, indexes,
    // sequences, and SERIAL/IDENTITY ownership wiring.
    catalog.schemas = tables::build_schemas(&schemas, filter)?;

    // Attach constraints to their tables. Also collect drift from NOT VALID constraints.
    let mut tables_mut = tables::build_tables(tables, &columns, filter)?;
    tables::apply_constraints(&mut tables_mut, &constraints, filter, &mut drift)?;

    // Build indexes (re-parsing pg_get_indexdef). Also collect drift from INVALID indexes.
    catalog.indexes = tables::build_indexes(&indexes, filter, &mut drift)?;

    // Build sequences.
    let mut sequence_by_qname: HashMap<String, Sequence> = HashMap::new();
    for r in &sequences {
        if let Some(s) = tables::build_sequence(r, filter)? {
            sequence_by_qname.insert(s.qname.to_string(), s);
        }
    }

    // Wire SERIAL/IDENTITY ownership.
    tables::apply_dependencies(&dependencies, &mut tables_mut, &mut sequence_by_qname)?;

    catalog.tables = tables_mut.into_values().collect();
    catalog.sequences = sequence_by_qname.into_values().collect();

    // Attach RLS policies to their tables. Must run after `catalog.tables` is set.
    policies::attach_policies(&policies, &mut catalog.tables)?;

    // Build views and materialized views.
    let (views, materialized_views) =
        views::build_views_and_mvs(&views_and_mvs, &view_columns, filter)?;
    catalog.views = views;
    catalog.materialized_views = materialized_views;

    // Build user-defined types (enums, domains, composites).
    catalog.types = user_types::build_user_types(
        &user_types,
        &enum_values,
        &domain_details,
        &domain_checks,
        &composite_attributes,
        filter,
    )?;

    // Build functions and procedures from pg_proc.
    let (fns, procs) = functions::build_functions_and_procedures(&functions, filter, &mut drift)?;
    catalog.functions = fns;
    catalog.procedures = procs;

    // Build extensions from pg_extension.
    catalog.extensions = build_extensions(&extensions)?;

    // Build triggers from pg_trigger (re-parses pg_get_triggerdef output).
    catalog.triggers = triggers::build_triggers(&triggers)?;

    // Merge partition metadata: re-parse pg_get_partkeydef / pg_get_expr(relpartbound).
    partitions::merge_partition_metadata(&mut catalog, &partitioned_tables, &partitions)?;

    // Build ALTER DEFAULT PRIVILEGES rules from pg_default_acl.
    catalog.default_privileges = default_privileges::build_default_privileges(&default_privileges)?;

    // Build publications from pg_publication + pg_publication_rel + pg_publication_namespace.
    catalog.publications = publications::assemble_publications(
        &pub_rows,
        &publication_rels,
        &publication_namespaces,
        &publication_attributes,
    )?;

    Ok((catalog, drift))
}

// ---- shared helpers used by multiple sub-modules ----

/// Build a qualified name from two row fields.
pub(super) fn qname_from(
    r: &Row,
    q: CatalogQuery,
    schema_key: &str,
    name_key: &str,
) -> Result<QualifiedName, CatalogError> {
    let schema = ident_required(&r.get_text(q, schema_key)?)?;
    let name = ident_required(&r.get_text(q, name_key)?)?;
    Ok(QualifiedName::new(schema, name))
}

/// Parse a raw string as an unquoted identifier, mapping the error to [`CatalogError`].
pub(super) fn ident_required(s: &str) -> Result<Identifier, CatalogError> {
    Identifier::from_unquoted(s)
        .map_err(|e| CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(e.to_string())))
}

/// Build a [`QualifiedName`] from two raw string slices, emitting a
/// [`CatalogError::Ir`] on invalid identifier.
pub(super) fn qname_from_strings(schema: &str, name: &str) -> Result<QualifiedName, CatalogError> {
    let s = Identifier::from_unquoted(schema).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "bad schema identifier {schema:?}: {e}"
        )))
    })?;
    let n = Identifier::from_unquoted(name).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "bad name identifier {name:?}: {e}"
        )))
    })?;
    Ok(QualifiedName::new(s, n))
}

/// Parse a referential-action single-character code into [`ReferentialAction`].
pub(super) fn parse_referential_action(s: &str) -> ReferentialAction {
    match s {
        "r" => ReferentialAction::Restrict,
        "c" => ReferentialAction::Cascade,
        "n" => ReferentialAction::SetNull(vec![]),
        "d" => ReferentialAction::SetDefault(vec![]),
        // `a` (default) or empty/space.
        _ => ReferentialAction::NoAction,
    }
}

/// Parse a match-type single-character code into [`FkMatchType`].
pub(super) const fn parse_match_type(s: &str) -> FkMatchType {
    let b = s.as_bytes();
    if b.len() == 1 && (b[0] == b'f' || b[0] == b'F') {
        FkMatchType::Full
    } else {
        FkMatchType::Simple
    }
}

/// Extract the referenced-column list from a `pg_get_constraintdef` FK body.
///
/// Wraps the constraint body in a synthetic `CREATE TABLE` statement so that
/// `pg_query` can produce a typed AST, then reads the `pk_attrs` field of the
/// resulting [`pg_query::protobuf::Constraint`] node — those are the columns on
/// the *referenced* (primary-key) side of the FK.
///
/// Returns `None` when the body cannot be parsed or yields no constraint node,
/// so callers can fall back to a placeholder list.
///
/// Constitution §5: parsing is not reimplemented — all SQL decomposition goes
/// through `pg_query`.
pub(super) fn parse_fk_referenced_columns(def: &str) -> Option<Vec<Identifier>> {
    // Wrap in a synthetic CREATE TABLE so pg_query sees a full statement.
    let synthetic =
        format!("CREATE TABLE _pgevolve_synth (_pgevolve_dummy int, CONSTRAINT _c {def});");
    let parsed = pg_query::parse(&synthetic).ok()?;

    // Dig into: RawStmt → CreateStmt → table_elts → Constraint
    let stmt_node = parsed
        .protobuf
        .stmts
        .into_iter()
        .next()
        .and_then(|raw| raw.stmt)
        .and_then(|n| n.node)?;
    let NodeEnum::CreateStmt(create) = stmt_node else {
        return None;
    };

    let constraint = create.table_elts.into_iter().find_map(|n| match n.node {
        Some(NodeEnum::Constraint(c)) => Some(c),
        _ => None,
    })?;

    // pk_attrs holds the referenced (right-hand) column names; fk_attrs holds
    // the local (left-hand) column names.
    let columns: Vec<Identifier> = constraint
        .pk_attrs
        .into_iter()
        .filter_map(|n| match n.node {
            Some(NodeEnum::String(s)) => Identifier::from_unquoted(&s.sval).ok(),
            _ => None,
        })
        .collect();

    if columns.is_empty() {
        None
    } else {
        Some(columns)
    }
}

/// Strip the outer `CHECK (` / `)` from a `pg_get_constraintdef` payload and
/// reparse the inner predicate.
pub(super) fn parse_check_expression(def: &str) -> Result<NormalizedExpr, CatalogError> {
    let s = def.trim();
    let inner = s
        .strip_prefix("CHECK")
        .or_else(|| s.strip_prefix("check"))
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(s);
    reparse_expression_text(inner)
}

/// Parse `text` as a SQL expression by wrapping it in `SELECT (...) AS x` and
/// extracting the resulting expression node, then normalize it.
pub(super) fn reparse_expression_text(text: &str) -> Result<NormalizedExpr, CatalogError> {
    let sql = format!("SELECT ({text}) AS __pgevolve_expr__");
    let parsed = pg_query::parse(&sql).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "could not reparse expression {text:?}: {e}"
        )))
    })?;
    let stmt = parsed
        .protobuf
        .stmts
        .into_iter()
        .next()
        .and_then(|raw| raw.stmt)
        .and_then(|n| n.node)
        .ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "reparsed expression had no statement".into(),
            ))
        })?;
    let NodeEnum::SelectStmt(s) = stmt else {
        return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            "expression scaffold did not yield SelectStmt".into(),
        )));
    };
    let target = s
        .target_list
        .into_iter()
        .next()
        .and_then(|n| n.node)
        .ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "expression scaffold had no target".into(),
            ))
        })?;
    let NodeEnum::ResTarget(rt) = target else {
        return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            "expression scaffold target was not a ResTarget".into(),
        )));
    };
    let inner = rt.val.and_then(|n| n.node).ok_or_else(|| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            "expression scaffold ResTarget missing value".into(),
        ))
    })?;
    let location = crate::parse::error::SourceLocation::new(PathBuf::from("<catalog>"), 1, 1);
    crate::parse::normalize_expr::from_pg_node(&inner, None, &location).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "could not normalize expression: {e}"
        )))
    })
}

/// Strip the outer `CHECK (…)` wrapper that `pg_get_constraintdef` prepends.
///
/// Handles both `CHECK (x)` and `CHECK ((x))` forms. The resulting slice
/// points into the original string (no allocation).
pub(super) fn strip_check_wrapper(text: &str) -> &str {
    let t = text.trim();
    let t = t.strip_prefix("CHECK").unwrap_or(t).trim_start();
    let t = t.strip_prefix('(').unwrap_or(t);
    let t = t.strip_suffix(')').unwrap_or(t);
    t.trim()
}

// ---- extensions (standalone, no sub-module needed) ----

fn build_extensions(rows: &[Row]) -> Result<Vec<Extension>, CatalogError> {
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let q = CatalogQuery::Extensions;
        let name_str = r.get_text(q, "name")?;
        let name = Identifier::from_unquoted(&name_str)
            .map_err(|e| CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(e.to_string())))?;
        let schema_str = r.get_text(q, "schema")?;
        let schema = Identifier::from_unquoted(&schema_str)
            .map_err(|e| CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(e.to_string())))?;
        let version = r.get_text(q, "version")?;
        let comment = r.get_opt_text(q, "comment")?;
        out.push(Extension {
            name,
            schema: Some(schema),
            version: Some(version),
            comment,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fk_referenced_columns_parsed() {
        let def = "FOREIGN KEY (org_id) REFERENCES app.orgs(id) ON DELETE CASCADE";
        let cols = parse_fk_referenced_columns(def).unwrap();
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].as_str(), "id");
    }

    #[test]
    fn fk_referenced_columns_multi() {
        let def = "FOREIGN KEY (a, b) REFERENCES app.t(x, y)";
        let cols = parse_fk_referenced_columns(def).unwrap();
        assert_eq!(cols.len(), 2);
    }

    #[test]
    fn check_expression_strips_outer_check() {
        let e = parse_check_expression("CHECK ((n > 0))").unwrap();
        assert!(e.canonical_text.contains('n') || e.canonical_text.contains('>'));
    }

    #[test]
    fn strip_check_wrapper_unwraps_single_paren_form() {
        assert_eq!(strip_check_wrapper("CHECK (VALUE > 0)"), "VALUE > 0");
    }

    #[test]
    fn strip_check_wrapper_unwraps_double_paren_form() {
        // pg_get_constraintdef sometimes emits `CHECK ((expr))`; we strip
        // exactly one layer, leaving inner parens for the parser to handle.
        assert_eq!(strip_check_wrapper("CHECK ((VALUE > 0))"), "(VALUE > 0)");
    }

    #[test]
    fn strip_check_wrapper_preserves_inner_function_parens() {
        // Stripping a single trailing `)` must not eat the closing paren
        // of an inner function call.
        assert_eq!(
            strip_check_wrapper("CHECK (length(x) > 0)"),
            "length(x) > 0",
        );
    }
}
