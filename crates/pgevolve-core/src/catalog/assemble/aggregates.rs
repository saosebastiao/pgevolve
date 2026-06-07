//! Assemble `pg_aggregate` rows into `Vec<Aggregate>`.
//!
//! Each row pairs a `pg_aggregate` entry with its wrapper `pg_proc` entry (the
//! aggregate's own name + argument signature). Rows are decoded into
//! [`crate::ir::aggregate::Aggregate`] IR entries.
//!
//! Rows are **skipped** (and recorded in
//! [`crate::catalog::DriftReport::unmanaged_aggregates`]) when:
//! - the aggregate is ordered-set / hypothetical-set (`aggkind <> 'n'`), or
//! - the state function's language is not `sql` / `plpgsql`, or
//! - a final function is present and its language is not `sql` / `plpgsql`.
//!
//! Argument types are resolved through the same AST →
//! [`crate::ir::column_type::ColumnType`] path the source-side
//! `CREATE AGGREGATE` parser uses ([`crate::parse::builder::shared::type_name_to_column_type`]),
//! by re-parsing the `pg_get_function_identity_arguments` signature inside a
//! synthetic `CREATE FUNCTION` wrapper. The state type arrives as a
//! `format_type` string and is decoded via
//! [`crate::ir::column_type::ColumnType::parse_from_pg_type_string`]. This keeps
//! catalog-side `arg_types` / `state_type` identical to the source side so the
//! two canonical forms compare equal.

// `CatalogError` embeds `IrError` and `ParseError`, both large. Boxing them
// would add indirection noise without benefit — these are cold-path catalog
// reads, not hot loops.
#![allow(clippy::result_large_err)]

use pg_query::NodeEnum;

use crate::catalog::CatalogQuery;
use crate::catalog::DriftReport;
use crate::catalog::error::CatalogError;
use crate::catalog::filter::CatalogFilter;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::aggregate::Aggregate;
use crate::ir::column_type::ColumnType;
use crate::parse::error::SourceLocation;

const Q: CatalogQuery = CatalogQuery::Aggregates;

/// Languages whose routines pgevolve manages. Mirrors the function reader's
/// `sql` / `plpgsql` whitelist.
fn is_managed_language(lang: &str) -> bool {
    matches!(lang.to_ascii_lowercase().as_str(), "sql" | "plpgsql")
}

/// Decode all `pg_aggregate` rows into [`Aggregate`] IR entries.
///
/// Ordered-set / hypothetical-set aggregates and aggregates whose state/final
/// function is in an unmanaged language are skipped and recorded in
/// `drift.unmanaged_aggregates`.
pub(super) fn assemble_aggregates(
    rows: &[Row],
    filter: &CatalogFilter,
    drift: &mut DriftReport,
) -> Result<Vec<Aggregate>, CatalogError> {
    let location = SourceLocation::new(std::path::PathBuf::from("<catalog>"), 1, 1);
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(agg) = decode_aggregate_row(row, filter, drift, &location)? {
            out.push(agg);
        }
    }
    Ok(out)
}

/// Decode a single row. Returns `Ok(None)` when the row is filtered out (not a
/// managed schema) or skipped (ordered-set / unmanaged-language).
fn decode_aggregate_row(
    row: &Row,
    filter: &CatalogFilter,
    drift: &mut DriftReport,
    location: &SourceLocation,
) -> Result<Option<Aggregate>, CatalogError> {
    let schema_name = row.get_text(Q, "schema_name")?;
    let name = row.get_text(Q, "name")?;
    let qname = QualifiedName::new(ident_required(&schema_name)?, ident_required(&name)?);

    if !filter.allows(&qname) {
        return Ok(None);
    }

    // Skip ordered-set / hypothetical-set aggregates (`aggkind <> 'n'`).
    let aggkind = row.get_text(Q, "aggkind")?;
    if aggkind != "n" {
        drift.unmanaged_aggregates.push(qname);
        return Ok(None);
    }

    // Skip aggregates whose state function is in an unmanaged language.
    let sfunc_lang = row.get_text(Q, "sfunc_lang")?;
    if !is_managed_language(&sfunc_lang) {
        drift.unmanaged_aggregates.push(qname);
        return Ok(None);
    }

    // A final function is optional. When present, it must also be managed.
    let finalfunc_name = row.get_opt_text(Q, "finalfunc_name")?;
    let finalfunc = if let Some(ff_name) = finalfunc_name {
        let ff_lang = row.get_opt_text(Q, "finalfunc_lang")?.unwrap_or_default();
        if !is_managed_language(&ff_lang) {
            drift.unmanaged_aggregates.push(qname);
            return Ok(None);
        }
        let ff_schema = row.get_text(Q, "finalfunc_schema")?;
        Some(QualifiedName::new(
            ident_required(&ff_schema)?,
            ident_required(&ff_name)?,
        ))
    } else {
        None
    };

    let arg_signature = row.get_text(Q, "arg_signature")?;
    let arg_types = parse_arg_types(&arg_signature, &qname, location)?;

    let state_type_str = row.get_text(Q, "state_type")?;
    let state_type = ColumnType::parse_from_pg_type_string(&state_type_str).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidColumnType(format!(
            "aggregate {qname} state type {state_type_str:?}: {e}"
        )))
    })?;

    let sfunc_schema = row.get_text(Q, "sfunc_schema")?;
    let sfunc_name = row.get_text(Q, "sfunc_name")?;
    let sfunc = QualifiedName::new(ident_required(&sfunc_schema)?, ident_required(&sfunc_name)?);

    // `agginitval` is text, NULL when there is no INITCOND.
    let initcond = row.get_opt_text(Q, "initcond")?;

    let owner_str = row.get_text(Q, "owner")?;
    let owner = if owner_str.is_empty() {
        None
    } else {
        Some(ident_required(&owner_str)?)
    };

    let comment = match row.get_opt_text(Q, "comment")? {
        Some(s) if !s.is_empty() => Some(s),
        _ => None,
    };

    Ok(Some(Aggregate {
        qname,
        arg_types,
        state_type,
        sfunc,
        finalfunc,
        initcond,
        owner,
        comment,
    }))
}

/// Parse a `pg_get_function_identity_arguments` signature (e.g. `"integer, text"`)
/// into the ordered list of argument [`ColumnType`]s.
///
/// The signature is wrapped in a synthetic `CREATE FUNCTION` and re-parsed via
/// `pg_query`, walking the resulting `CreateFunctionStmt.parameters`. Each
/// parameter's type goes through the same
/// [`crate::parse::builder::shared::type_name_to_column_type`] path the
/// source-side `CREATE AGGREGATE` parser uses, so the two `arg_types` lists
/// compare equal.
fn parse_arg_types(
    arg_signature: &str,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<Vec<ColumnType>, CatalogError> {
    if arg_signature.trim().is_empty() {
        return Ok(Vec::new());
    }

    let wrapper = format!(
        "CREATE FUNCTION pgevolve_temp({arg_signature}) RETURNS void LANGUAGE sql AS $$ SELECT NULL $$;"
    );
    let parsed = pg_query::parse(&wrapper).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "catalog aggregate arg parse for {qname} ({arg_signature:?}): {e}"
        )))
    })?;
    let stmt = parsed
        .protobuf
        .stmts
        .into_iter()
        .next()
        .and_then(|r| r.stmt)
        .and_then(|n| n.node)
        .ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "catalog aggregate arg parse for {qname}: no statement"
            )))
        })?;
    let NodeEnum::CreateFunctionStmt(stmt) = stmt else {
        return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            format!("catalog aggregate arg parse for {qname}: unexpected stmt kind"),
        )));
    };

    let mut types = Vec::with_capacity(stmt.parameters.len());
    for param_node in &stmt.parameters {
        let Some(NodeEnum::FunctionParameter(p)) = param_node.node.as_ref() else {
            continue;
        };
        let Some(tn) = p.arg_type.as_ref() else {
            continue;
        };
        let ty =
            crate::parse::builder::shared::type_name_to_column_type(tn, location).map_err(|e| {
                CatalogError::Ir(crate::ir::IrError::InvalidColumnType(format!(
                    "aggregate {qname} arg type: {e}"
                )))
            })?;
        types.push(ty);
    }
    Ok(types)
}

/// Parse a raw string as an unquoted identifier, mapping the error to [`CatalogError`].
fn ident_required(s: &str) -> Result<Identifier, CatalogError> {
    Identifier::from_unquoted(s)
        .map_err(|e| CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(e.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::Value;

    fn filter() -> CatalogFilter {
        CatalogFilter::new(vec![Identifier::from_unquoted("app").unwrap()], vec![]).unwrap()
    }

    /// A minimal ordinary-aggregate row: one integer arg, plpgsql sfunc, no
    /// finalfunc, no initcond.
    fn agg_row() -> Row {
        Row::new()
            .with("schema_name", Value::Text("app".to_string()))
            .with("name", Value::Text("my_sum".to_string()))
            .with("arg_signature", Value::Text("integer".to_string()))
            .with("aggkind", Value::Text("n".to_string()))
            .with("sfunc_schema", Value::Text("app".to_string()))
            .with("sfunc_name", Value::Text("my_sfunc".to_string()))
            .with("sfunc_lang", Value::Text("plpgsql".to_string()))
            .with("state_type", Value::Text("bigint".to_string()))
            .with("finalfunc_schema", Value::Null)
            .with("finalfunc_name", Value::Null)
            .with("finalfunc_lang", Value::Null)
            .with("initcond", Value::Null)
            .with("owner", Value::Text("app_owner".to_string()))
            .with("comment", Value::Null)
    }

    #[test]
    fn decode_simple() {
        let mut drift = DriftReport::default();
        let aggs = assemble_aggregates(&[agg_row()], &filter(), &mut drift).unwrap();
        assert_eq!(aggs.len(), 1);
        let a = &aggs[0];
        assert_eq!(a.qname.to_string(), "app.my_sum");
        assert_eq!(a.arg_types, vec![ColumnType::Integer]);
        assert_eq!(a.state_type, ColumnType::BigInt);
        assert_eq!(a.sfunc.to_string(), "app.my_sfunc");
        assert!(a.finalfunc.is_none());
        assert!(a.initcond.is_none());
        assert_eq!(a.owner.as_ref().unwrap().as_str(), "app_owner");
        assert!(a.comment.is_none());
        assert!(drift.unmanaged_aggregates.is_empty());
    }

    #[test]
    fn decode_with_finalfunc_and_initcond() {
        let mut r = agg_row();
        r.insert("finalfunc_schema", Value::Text("app".to_string()));
        r.insert("finalfunc_name", Value::Text("my_final".to_string()));
        r.insert("finalfunc_lang", Value::Text("sql".to_string()));
        r.insert("initcond", Value::Text("0".to_string()));
        let mut drift = DriftReport::default();
        let aggs = assemble_aggregates(&[r], &filter(), &mut drift).unwrap();
        assert_eq!(aggs.len(), 1);
        let a = &aggs[0];
        assert_eq!(
            a.finalfunc.as_ref().map(ToString::to_string).as_deref(),
            Some("app.my_final")
        );
        assert_eq!(a.initcond.as_deref(), Some("0"));
    }

    #[test]
    fn decode_multiple_arg_types() {
        let mut r = agg_row();
        r.insert(
            "arg_signature",
            Value::Text("integer, text, numeric(10,2)".to_string()),
        );
        let mut drift = DriftReport::default();
        let aggs = assemble_aggregates(&[r], &filter(), &mut drift).unwrap();
        assert_eq!(aggs[0].arg_types.len(), 3);
        assert_eq!(aggs[0].arg_types[0], ColumnType::Integer);
        assert_eq!(aggs[0].arg_types[1], ColumnType::Text);
    }

    #[test]
    fn decode_zero_arg_aggregate() {
        let mut r = agg_row();
        r.insert("arg_signature", Value::Text(String::new()));
        let mut drift = DriftReport::default();
        let aggs = assemble_aggregates(&[r], &filter(), &mut drift).unwrap();
        assert!(aggs[0].arg_types.is_empty());
    }

    #[test]
    fn skip_ordered_set_aggregate() {
        let mut r = agg_row();
        r.insert("aggkind", Value::Text("o".to_string()));
        let mut drift = DriftReport::default();
        let aggs = assemble_aggregates(&[r], &filter(), &mut drift).unwrap();
        assert!(aggs.is_empty());
        assert_eq!(drift.unmanaged_aggregates.len(), 1);
        assert_eq!(drift.unmanaged_aggregates[0].to_string(), "app.my_sum");
    }

    #[test]
    fn skip_hypothetical_set_aggregate() {
        let mut r = agg_row();
        r.insert("aggkind", Value::Text("h".to_string()));
        let mut drift = DriftReport::default();
        let aggs = assemble_aggregates(&[r], &filter(), &mut drift).unwrap();
        assert!(aggs.is_empty());
        assert_eq!(drift.unmanaged_aggregates.len(), 1);
    }

    #[test]
    fn skip_unmanaged_sfunc_language() {
        let mut r = agg_row();
        r.insert("sfunc_lang", Value::Text("c".to_string()));
        let mut drift = DriftReport::default();
        let aggs = assemble_aggregates(&[r], &filter(), &mut drift).unwrap();
        assert!(aggs.is_empty());
        assert_eq!(drift.unmanaged_aggregates.len(), 1);
        assert_eq!(drift.unmanaged_aggregates[0].to_string(), "app.my_sum");
    }

    #[test]
    fn skip_unmanaged_finalfunc_language() {
        let mut r = agg_row();
        r.insert("finalfunc_schema", Value::Text("app".to_string()));
        r.insert("finalfunc_name", Value::Text("my_final".to_string()));
        r.insert("finalfunc_lang", Value::Text("c".to_string()));
        let mut drift = DriftReport::default();
        let aggs = assemble_aggregates(&[r], &filter(), &mut drift).unwrap();
        assert!(aggs.is_empty());
        assert_eq!(drift.unmanaged_aggregates.len(), 1);
    }

    #[test]
    fn ignore_glob_filtered_out_silently() {
        // Schema scoping is done at the SQL layer; the assembler's `allows`
        // check only applies ignore-globs. An ignored qname is dropped silently
        // (not recorded as drift).
        let f = CatalogFilter::new(
            vec![Identifier::from_unquoted("app").unwrap()],
            vec!["app.my_*".to_string()],
        )
        .unwrap();
        let mut drift = DriftReport::default();
        let aggs = assemble_aggregates(&[agg_row()], &f, &mut drift).unwrap();
        assert!(aggs.is_empty());
        assert!(drift.unmanaged_aggregates.is_empty());
    }

    #[test]
    fn empty_comment_is_none() {
        let mut r = agg_row();
        r.insert("comment", Value::Text(String::new()));
        let mut drift = DriftReport::default();
        let aggs = assemble_aggregates(&[r], &filter(), &mut drift).unwrap();
        assert!(aggs[0].comment.is_none());
    }

    #[test]
    fn non_empty_comment_is_some() {
        let mut r = agg_row();
        r.insert("comment", Value::Text("a custom sum".to_string()));
        let mut drift = DriftReport::default();
        let aggs = assemble_aggregates(&[r], &filter(), &mut drift).unwrap();
        assert_eq!(aggs[0].comment.as_deref(), Some("a custom sum"));
    }

    #[test]
    fn empty_rows_returns_empty_vec() {
        let mut drift = DriftReport::default();
        assert!(
            assemble_aggregates(&[], &filter(), &mut drift)
                .unwrap()
                .is_empty()
        );
    }
}
