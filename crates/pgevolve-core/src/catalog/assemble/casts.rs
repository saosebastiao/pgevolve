//! Assemble `pg_cast` rows into `Vec<Cast>`.
//!
//! Each row represents one user-defined cast (`c.oid >= 16384`), with
//! extension-owned casts already excluded by the SQL `WHERE NOT EXISTS (…)`
//! clause. Rows are decoded into [`crate::ir::cast::Cast`] IR entries.
//!
//! Rows are **skipped** (and recorded in
//! [`crate::catalog::DriftReport::unmanaged_casts`]) when:
//! - `castmethod = 'f'` (WITH FUNCTION) **and** the function language is not
//!   `sql` / `plpgsql`.
//!
//! `Inout` and `Binary` casts are always representable and are never skipped.
//!
//! The cast function's argument types are decoded through the same
//! `pg_get_function_identity_arguments` → synthetic `CREATE FUNCTION` →
//! [`crate::ir::column_type::ColumnType`] path that the aggregate reader uses
//! (see `crate::parse::builder::shared::type_name_to_column_type`), so the
//! catalog-side `arg_types` compares equal to the source-side value produced
//! by the `CREATE CAST … WITH FUNCTION` parser.
//!
//! Source and target types arrive as `(nspname, typname)` pairs and are
//! assembled directly into [`crate::identifier::QualifiedName`] values.  For
//! built-in types Postgres stores them in `pg_catalog`, so the reader produces
//! e.g. `pg_catalog.int4` — matching the parser which also qualifies built-ins
//! to `pg_catalog`.

// `CatalogError` embeds `IrError` and `ParseError`, both large. Boxing them
// would add indirection noise without benefit — these are cold-path catalog
// reads, not hot loops.
#![allow(clippy::result_large_err)]

use pg_query::NodeEnum;

use crate::catalog::CatalogQuery;
use crate::catalog::DriftReport;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::cast::{Cast, CastContext, CastMethod};
use crate::ir::column_type::ColumnType;
use crate::parse::error::SourceLocation;

const Q: CatalogQuery = CatalogQuery::Casts;

/// Languages whose routines pgevolve manages. Mirrors the function and
/// aggregate readers' `sql` / `plpgsql` whitelist.
fn is_managed_language(lang: &str) -> bool {
    matches!(lang.to_ascii_lowercase().as_str(), "sql" | "plpgsql")
}

/// Decode all `pg_cast` rows into [`Cast`] IR entries.
///
/// Casts whose `castmethod = 'f'` and whose conversion function is written in
/// an unmanaged language are skipped and recorded in `drift.unmanaged_casts`.
pub(in crate::catalog) fn assemble_casts(
    rows: &[Row],
    drift: &mut DriftReport,
) -> Result<Vec<Cast>, CatalogError> {
    let location = SourceLocation::new(std::path::PathBuf::from("<catalog>"), 1, 1);
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(cast) = decode_cast_row(row, drift, &location)? {
            out.push(cast);
        }
    }
    Ok(out)
}

/// Decode a single `pg_cast` row.
///
/// Returns `Ok(None)` when the row is skipped (unmanaged cast-function
/// language). The `(source, target)` pair is recorded in `drift.unmanaged_casts`.
fn decode_cast_row(
    row: &Row,
    drift: &mut DriftReport,
    location: &SourceLocation,
) -> Result<Option<Cast>, CatalogError> {
    let source_schema = row.get_text(Q, "source_schema")?;
    let source_name = row.get_text(Q, "source_name")?;
    let source = QualifiedName::new(
        ident_required(&source_schema)?,
        ident_required(&source_name)?,
    );

    let target_schema = row.get_text(Q, "target_schema")?;
    let target_name = row.get_text(Q, "target_name")?;
    let target = QualifiedName::new(
        ident_required(&target_schema)?,
        ident_required(&target_name)?,
    );

    let castmethod = row.get_text(Q, "castmethod")?;

    let method = match castmethod.as_str() {
        "f" => {
            // WITH FUNCTION — check the function language.
            let lang = row.get_opt_text(Q, "func_lang")?.unwrap_or_default();
            if !is_managed_language(&lang) {
                drift.unmanaged_casts.push((source, target));
                return Ok(None);
            }

            let func_schema = row.get_opt_text(Q, "func_schema")?.unwrap_or_default();
            let func_name_str = row.get_opt_text(Q, "func_name")?.unwrap_or_default();
            let func = QualifiedName::new(
                ident_required(&func_schema)?,
                ident_required(&func_name_str)?,
            );

            let sig = row
                .get_opt_text(Q, "func_arg_signature")?
                .unwrap_or_default();
            let arg_types = parse_arg_types(&sig, &func, location)?;

            CastMethod::Function {
                name: func,
                arg_types,
            }
        }
        "i" => CastMethod::Inout,
        "b" => CastMethod::Binary,
        other => {
            return Err(CatalogError::BadColumnType {
                query: Q,
                column: "castmethod".to_string(),
                message: format!("unknown castmethod {other:?}"),
            });
        }
    };

    let castcontext = row.get_text(Q, "castcontext")?;
    let context = match castcontext.as_str() {
        "e" => CastContext::Explicit,
        "a" => CastContext::Assignment,
        "i" => CastContext::Implicit,
        other => {
            return Err(CatalogError::BadColumnType {
                query: Q,
                column: "castcontext".to_string(),
                message: format!("unknown castcontext {other:?}"),
            });
        }
    };

    let comment = match row.get_opt_text(Q, "comment")? {
        Some(s) if !s.is_empty() => Some(s),
        _ => None,
    };

    Ok(Some(Cast {
        source,
        target,
        method,
        context,
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
/// source-side `CREATE CAST … WITH FUNCTION` parser uses, so the two
/// `arg_types` lists compare equal.
fn parse_arg_types(
    arg_signature: &str,
    func_qname: &QualifiedName,
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
            "catalog cast arg parse for {func_qname} ({arg_signature:?}): {e}"
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
                "catalog cast arg parse for {func_qname}: no statement"
            )))
        })?;
    let NodeEnum::CreateFunctionStmt(stmt) = stmt else {
        return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            format!("catalog cast arg parse for {func_qname}: unexpected stmt kind"),
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
                    "cast {func_qname} arg type: {e}"
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

    /// A minimal cast row using WITH FUNCTION (sql language, explicit context).
    fn cast_fn_row() -> Row {
        Row::new()
            .with("source_schema", Value::Text("app".to_string()))
            .with("source_name", Value::Text("my_type".to_string()))
            .with("target_schema", Value::Text("pg_catalog".to_string()))
            .with("target_name", Value::Text("text".to_string()))
            .with("castmethod", Value::Text("f".to_string()))
            .with("castcontext", Value::Text("e".to_string()))
            .with("func_schema", Value::Text("app".to_string()))
            .with("func_name", Value::Text("my_type_to_text".to_string()))
            .with("func_arg_signature", Value::Text("integer".to_string()))
            .with("func_lang", Value::Text("sql".to_string()))
            .with("comment", Value::Null)
    }

    #[test]
    fn decode_with_function_sql() {
        let mut drift = DriftReport::default();
        let casts = assemble_casts(&[cast_fn_row()], &mut drift).unwrap();
        assert_eq!(casts.len(), 1);
        let c = &casts[0];
        assert_eq!(c.source.schema.as_str(), "app");
        assert_eq!(c.source.name.as_str(), "my_type");
        assert_eq!(c.target.schema.as_str(), "pg_catalog");
        assert_eq!(c.target.name.as_str(), "text");
        assert_eq!(c.context, CastContext::Explicit);
        let CastMethod::Function { name, arg_types } = &c.method else {
            panic!("expected Function method");
        };
        assert_eq!(name.schema.as_str(), "app");
        assert_eq!(name.name.as_str(), "my_type_to_text");
        assert_eq!(arg_types, &[ColumnType::Integer]);
        assert!(c.comment.is_none());
        assert!(drift.unmanaged_casts.is_empty());
    }

    #[test]
    fn decode_with_function_plpgsql() {
        let mut r = cast_fn_row();
        r.insert("func_lang", Value::Text("plpgsql".to_string()));
        let mut drift = DriftReport::default();
        let casts = assemble_casts(&[r], &mut drift).unwrap();
        assert_eq!(casts.len(), 1);
        assert!(drift.unmanaged_casts.is_empty());
    }

    #[test]
    fn decode_without_function_binary() {
        let row = Row::new()
            .with("source_schema", Value::Text("app".to_string()))
            .with("source_name", Value::Text("type_x".to_string()))
            .with("target_schema", Value::Text("app".to_string()))
            .with("target_name", Value::Text("type_y".to_string()))
            .with("castmethod", Value::Text("b".to_string()))
            .with("castcontext", Value::Text("i".to_string()))
            .with("func_schema", Value::Null)
            .with("func_name", Value::Null)
            .with("func_arg_signature", Value::Null)
            .with("func_lang", Value::Null)
            .with("comment", Value::Null);
        let mut drift = DriftReport::default();
        let casts = assemble_casts(&[row], &mut drift).unwrap();
        assert_eq!(casts.len(), 1);
        assert_eq!(casts[0].method, CastMethod::Binary);
        assert_eq!(casts[0].context, CastContext::Implicit);
        assert!(drift.unmanaged_casts.is_empty());
    }

    #[test]
    fn decode_with_inout() {
        let row = Row::new()
            .with("source_schema", Value::Text("app".to_string()))
            .with("source_name", Value::Text("domain_a".to_string()))
            .with("target_schema", Value::Text("app".to_string()))
            .with("target_name", Value::Text("domain_b".to_string()))
            .with("castmethod", Value::Text("i".to_string()))
            .with("castcontext", Value::Text("a".to_string()))
            .with("func_schema", Value::Null)
            .with("func_name", Value::Null)
            .with("func_arg_signature", Value::Null)
            .with("func_lang", Value::Null)
            .with("comment", Value::Null);
        let mut drift = DriftReport::default();
        let casts = assemble_casts(&[row], &mut drift).unwrap();
        assert_eq!(casts.len(), 1);
        assert_eq!(casts[0].method, CastMethod::Inout);
        assert_eq!(casts[0].context, CastContext::Assignment);
        assert!(drift.unmanaged_casts.is_empty());
    }

    #[test]
    fn each_context_decodes() {
        for (code, expected) in [
            ("e", CastContext::Explicit),
            ("a", CastContext::Assignment),
            ("i", CastContext::Implicit),
        ] {
            let r = Row::new()
                .with("source_schema", Value::Text("app".to_string()))
                .with("source_name", Value::Text("t".to_string()))
                .with("target_schema", Value::Text("app".to_string()))
                .with("target_name", Value::Text("u".to_string()))
                .with("castmethod", Value::Text("b".to_string()))
                .with("castcontext", Value::Text(code.to_string()))
                .with("func_schema", Value::Null)
                .with("func_name", Value::Null)
                .with("func_arg_signature", Value::Null)
                .with("func_lang", Value::Null)
                .with("comment", Value::Null);
            let mut drift = DriftReport::default();
            let casts = assemble_casts(&[r], &mut drift).unwrap();
            assert_eq!(casts[0].context, expected, "code {code}");
        }
    }

    #[test]
    fn skip_unmanaged_function_language_records_drift() {
        let mut r = cast_fn_row();
        r.insert("func_lang", Value::Text("c".to_string()));
        let mut drift = DriftReport::default();
        let casts = assemble_casts(&[r], &mut drift).unwrap();
        assert!(casts.is_empty());
        assert_eq!(drift.unmanaged_casts.len(), 1);
        assert_eq!(drift.unmanaged_casts[0].0.to_string(), "app.my_type");
        assert_eq!(drift.unmanaged_casts[0].1.to_string(), "pg_catalog.text");
    }

    #[test]
    fn inout_unmanaged_lang_not_skipped() {
        // Inout casts never need a function — they are always representable.
        let row = Row::new()
            .with("source_schema", Value::Text("app".to_string()))
            .with("source_name", Value::Text("a".to_string()))
            .with("target_schema", Value::Text("app".to_string()))
            .with("target_name", Value::Text("b".to_string()))
            .with("castmethod", Value::Text("i".to_string()))
            .with("castcontext", Value::Text("e".to_string()))
            .with("func_schema", Value::Null)
            .with("func_name", Value::Null)
            .with("func_arg_signature", Value::Null)
            .with("func_lang", Value::Null)
            .with("comment", Value::Null);
        let mut drift = DriftReport::default();
        let casts = assemble_casts(&[row], &mut drift).unwrap();
        assert_eq!(casts.len(), 1);
        assert!(drift.unmanaged_casts.is_empty());
    }

    #[test]
    fn comment_is_some_when_non_empty() {
        let mut r = cast_fn_row();
        r.insert(
            "comment",
            Value::Text("converts my_type to text".to_string()),
        );
        let mut drift = DriftReport::default();
        let casts = assemble_casts(&[r], &mut drift).unwrap();
        assert_eq!(
            casts[0].comment.as_deref(),
            Some("converts my_type to text")
        );
    }

    #[test]
    fn empty_comment_is_none() {
        let mut r = cast_fn_row();
        r.insert("comment", Value::Text(String::new()));
        let mut drift = DriftReport::default();
        let casts = assemble_casts(&[r], &mut drift).unwrap();
        assert!(casts[0].comment.is_none());
    }

    #[test]
    fn empty_rows_returns_empty_vec() {
        let mut drift = DriftReport::default();
        assert!(assemble_casts(&[], &mut drift).unwrap().is_empty());
    }

    #[test]
    fn unknown_castmethod_errors() {
        let mut r = cast_fn_row();
        r.insert("castmethod", Value::Text("x".to_string()));
        let mut drift = DriftReport::default();
        let err = assemble_casts(&[r], &mut drift).unwrap_err();
        assert!(matches!(err, CatalogError::BadColumnType { .. }));
    }

    #[test]
    fn unknown_castcontext_errors() {
        let mut r = cast_fn_row();
        r.insert("castcontext", Value::Text("z".to_string()));
        let mut drift = DriftReport::default();
        let err = assemble_casts(&[r], &mut drift).unwrap_err();
        assert!(matches!(err, CatalogError::BadColumnType { .. }));
    }
}
