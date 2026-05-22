//! Function and procedure assembly from `pg_proc` catalog rows.
//!
//! Called from [`super::assemble`] to build [`crate::ir::function::Function`]
//! and [`crate::ir::procedure::Procedure`] IR entries.

use crate::catalog::DriftReport;
use crate::catalog::error::CatalogError;
use crate::catalog::filter::CatalogFilter;
use crate::catalog::rows::Row;
use crate::identifier::QualifiedName;
use crate::ir::function::{
    ArgMode, Function, FunctionArg, FunctionLanguage, NormalizedArgTypes, ParallelSafety,
    ReturnType, SecurityMode, TableColumn, Volatility,
};
use crate::ir::procedure::Procedure;
use crate::parse::error::SourceLocation;

use super::ident_required;

/// Build functions and procedures from `pg_proc` rows.
///
/// Rows with an unsupported language are skipped and reported in the
/// [`DriftReport::unmanaged_language_routines`] list. The SQL fragment used
/// to fetch these rows is [`crate::catalog::queries::functions::SELECT_FUNCTIONS`].
///
/// Returns `(functions, procedures)`.
#[allow(clippy::too_many_lines)]
pub(super) fn build_functions_and_procedures(
    rows: &[Row],
    filter: &CatalogFilter,
    drift: &mut DriftReport,
) -> Result<(Vec<Function>, Vec<Procedure>), CatalogError> {
    use crate::catalog::CatalogQuery as Q;

    let mut functions: Vec<Function> = Vec::new();
    let mut procedures: Vec<Procedure> = Vec::new();

    let catalog_location = SourceLocation::new(std::path::PathBuf::from("<catalog>"), 1, 1);

    for r in rows {
        let schema_name = r.get_text(Q::Functions, "schema_name")?;
        let name = r.get_text(Q::Functions, "name")?;
        let kind = r.get_text(Q::Functions, "kind")?;
        let language_str = r.get_text(Q::Functions, "language")?;
        let arg_full = r.get_text(Q::Functions, "arg_full")?;
        let return_type_str = r
            .get_opt_text(Q::Functions, "return_type")?
            .unwrap_or_default();
        let full_def = r.get_text(Q::Functions, "full_def")?;
        let comment = r.get_opt_text(Q::Functions, "comment")?;
        let security_definer = r.get_bool(Q::Functions, "security_definer")?;

        let qname = QualifiedName::new(ident_required(&schema_name)?, ident_required(&name)?);

        if !filter.allows(&qname) {
            continue;
        }

        // Determine language — skip unsupported ones.
        let language = match language_str.to_lowercase().as_str() {
            "sql" => FunctionLanguage::Sql,
            "plpgsql" => FunctionLanguage::PlPgSql,
            other => {
                drift
                    .unmanaged_language_routines
                    .push((qname, other.to_string()));
                continue;
            }
        };

        let security = if security_definer {
            SecurityMode::Definer
        } else {
            SecurityMode::Invoker
        };

        // Parse the arg list from the "full arguments" string provided by
        // pg_get_function_arguments. We synthesize a wrapper CREATE FUNCTION
        // and re-parse it to get a properly typed argument list.
        let args = parse_arg_full(&arg_full, &qname, &catalog_location)?;

        // Extract the body from full_def via dollar-quote matching.
        let body_text = extract_body_from_functiondef(&full_def, &qname)?;

        // Re-parse the body through the same pipeline as the source side.
        let (body, body_dependencies, commits_in_body) =
            crate::parse::builder::plpgsql::parse_routine_body(
                &body_text,
                language,
                &qname,
                &catalog_location,
            )
            .map_err(|e| {
                CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                    "catalog body parse for {qname}: {e}"
                )))
            })?;

        match kind.as_str() {
            "f" => {
                // Function-only columns.
                let volatility_str = r.get_text(Q::Functions, "volatility")?;
                let strict = r.get_bool(Q::Functions, "strict")?;
                let parallel_str = r.get_text(Q::Functions, "parallel")?;
                let leakproof = r.get_bool(Q::Functions, "leakproof")?;
                let cost_str = r.get_opt_text(Q::Functions, "cost")?;
                let rows_str = r.get_opt_text(Q::Functions, "rows")?;

                let volatility = match volatility_str.as_str() {
                    "i" => Volatility::Immutable,
                    "s" => Volatility::Stable,
                    _ => Volatility::Volatile,
                };
                let parallel = match parallel_str.as_str() {
                    "s" => ParallelSafety::Safe,
                    "r" => ParallelSafety::Restricted,
                    _ => ParallelSafety::Unsafe,
                };
                // Catalog returns raw cost/rows; `ir::canon::filter_pg_defaults`
                // normalizes the PG-defaults (procost=100, prorows=1000 for
                // SETOF, prorows=0 otherwise) to None on both sides.
                let cost: Option<f32> = cost_str.as_deref().and_then(|s| s.parse::<f32>().ok());
                let rows: Option<f32> = rows_str.as_deref().and_then(|s| s.parse::<f32>().ok());

                let return_type = parse_return_type_from_string(&return_type_str, &qname)?;
                let arg_types_normalized = NormalizedArgTypes::from_args(&args);

                functions.push(Function {
                    qname,
                    args,
                    arg_types_normalized,
                    return_type,
                    language,
                    body,
                    body_dependencies,
                    volatility,
                    strict,
                    security,
                    parallel,
                    leakproof,
                    cost,
                    rows,
                    comment,
                    owner: None,
                    grants: vec![],
                });
            }
            "p" => {
                procedures.push(Procedure {
                    qname,
                    args,
                    language,
                    body,
                    body_dependencies,
                    security,
                    commits_in_body,
                    comment,
                    owner: None,
                    grants: vec![],
                });
            }
            other => {
                return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                    format!("unexpected prokind {other:?} in functions query"),
                )));
            }
        }
    }

    Ok((functions, procedures))
}

/// Parse a `pg_get_function_arguments` string (e.g. `"x integer, y text DEFAULT 'a'"`)
/// into a `Vec<FunctionArg>`.
///
/// The strategy: synthesize a wrapper `CREATE FUNCTION` and re-parse via
/// `pg_query`, then walk the resulting `CreateFunctionStmt.parameters`.
fn parse_arg_full(
    arg_full: &str,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<Vec<FunctionArg>, CatalogError> {
    use pg_query::NodeEnum;
    use pg_query::protobuf::FunctionParameterMode;

    // Empty argument list.
    if arg_full.trim().is_empty() {
        return Ok(vec![]);
    }

    let wrapper = format!(
        "CREATE FUNCTION pgevolve_temp({arg_full}) RETURNS void LANGUAGE sql AS $$ SELECT NULL $$;"
    );
    let parsed = pg_query::parse(&wrapper).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "catalog arg parse for {qname} ({arg_full:?}): {e}"
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
                "catalog arg parse for {qname}: no statement"
            )))
        })?;
    let NodeEnum::CreateFunctionStmt(stmt) = stmt else {
        return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            format!("catalog arg parse for {qname}: unexpected stmt kind"),
        )));
    };

    let mut args: Vec<FunctionArg> = Vec::new();

    for param_node in &stmt.parameters {
        let Some(NodeEnum::FunctionParameter(p)) = param_node.node.as_ref() else {
            continue;
        };
        let raw_mode =
            FunctionParameterMode::try_from(p.mode).unwrap_or(FunctionParameterMode::Undefined);

        // TABLE mode params are OUT columns in the argument string — treat as Out.
        let mode = match raw_mode {
            FunctionParameterMode::FuncParamIn
            | FunctionParameterMode::FuncParamDefault
            | FunctionParameterMode::Undefined => ArgMode::In,
            FunctionParameterMode::FuncParamOut | FunctionParameterMode::FuncParamTable => {
                ArgMode::Out
            }
            FunctionParameterMode::FuncParamInout => ArgMode::InOut,
            FunctionParameterMode::FuncParamVariadic => ArgMode::Variadic,
        };

        let Some(tn) = p.arg_type.as_ref() else {
            continue;
        };
        let ty =
            crate::parse::builder::shared::type_name_to_column_type(tn, location).map_err(|e| {
                CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                    "catalog arg type for {qname}: {e}"
                )))
            })?;
        let arg_name = if p.name.is_empty() {
            None
        } else {
            Some(ident_required(&p.name)?)
        };

        let default = p
            .defexpr
            .as_ref()
            .and_then(|defexpr| defexpr.node.as_ref())
            .and_then(|node_enum| {
                crate::parse::normalize_expr::from_pg_node(node_enum, Some(&ty), location).ok()
            });

        args.push(FunctionArg {
            name: arg_name,
            mode,
            ty,
            default,
        });
    }

    Ok(args)
}

/// Extract the body text from the output of `pg_get_functiondef`.
///
/// The format is:
/// ```text
/// CREATE OR REPLACE FUNCTION schema.name(args) ...
///     AS $<tag>$
/// body text
/// $<tag>$
///   LANGUAGE ...;
/// ```
///
/// Or for SQL functions with `BEGIN ATOMIC` syntax (PG 14+):
/// ```text
/// CREATE OR REPLACE FUNCTION ... BEGIN ATOMIC ...  END
/// ```
///
/// We locate `AS $<tag>$`, then find the matching `$<tag>$` close.
fn extract_body_from_functiondef(
    full_def: &str,
    qname: &QualifiedName,
) -> Result<String, CatalogError> {
    // Find the dollar-quote open: "AS $tag$" or "AS $$".
    // We scan case-insensitively for "AS " then try to find a dollar-quote tag.
    let upper = full_def.to_ascii_uppercase();

    // Find " AS $" (dollar-quote style body).
    if let Some(as_pos) = find_as_dollar_quote_pos(&upper) {
        let rest = &full_def[as_pos..];
        // Find the tag: everything between the first $ and the next $.
        let tag_end = rest[1..].find('$').ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "catalog body for {qname}: malformed dollar-quote open"
            )))
        })?;
        // tag is the text between the two dollar signs (possibly empty for $$).
        let tag = &rest[1..=tag_end]; // e.g. "function" or ""
        let open = format!("${tag}$");
        let body_start = as_pos + open.len();
        let body_text_after_open = &full_def[body_start..];
        // Find the closing $tag$.
        let close_pos = body_text_after_open.find(open.as_str()).ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "catalog body for {qname}: closing dollar-quote {open:?} not found"
            )))
        })?;
        let body = &body_text_after_open[..close_pos];
        return Ok(body.to_string());
    }

    // SQL functions with BEGIN ATOMIC style (no dollar-quote).
    // pg_get_functiondef omits dollar-quotes for `BEGIN ATOMIC` SQL functions.
    // In this case the body is between "BEGIN ATOMIC" and the final "END".
    if let Some(begin_pos) = upper.find("BEGIN ATOMIC") {
        let body_start = begin_pos; // include BEGIN ... END
        // Find the matching final END before the semicolon.
        let tail = &full_def[body_start..];
        // strip trailing semicolon and find last END
        let tail_upper = tail.to_ascii_uppercase();
        let end_pos = tail_upper.rfind("END").ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "catalog body for {qname}: BEGIN ATOMIC without END"
            )))
        })?;
        let body = &tail[..end_pos + "END".len()];
        return Ok(body.to_string());
    }

    Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
        format!("catalog body for {qname}: could not extract body from pg_get_functiondef"),
    )))
}

/// Locate the position in `upper` (an uppercase version of the `full_def`) of
/// the start of the dollar-quote opening tag (i.e. at the `$`), scanning for
/// `" AS $"`.
///
/// Returns the byte offset in `upper` pointing at the first `$` of the
/// dollar-quote open tag.
fn find_as_dollar_quote_pos(upper: &str) -> Option<usize> {
    // We want "AS $tag$" where tag may be empty.
    // Scan for occurrences of " AS $" or "\nAS $" or "\tAS $".
    let bytes = upper.as_bytes();
    for i in 0..bytes.len().saturating_sub(3) {
        // Look for 'A' 'S' then whitespace then '$'
        if bytes[i] == b'A' && bytes[i + 1] == b'S' {
            // Check there's whitespace before (or it's at start)
            if i > 0 && !bytes[i - 1].is_ascii_whitespace() {
                continue;
            }
            // Skip whitespace after "AS"
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'$' {
                return Some(j);
            }
        }
    }
    None
}

/// Parse a return-type string from `pg_get_function_result` into a
/// [`ReturnType`].
///
/// The string is one of:
/// - `"trigger"` or `"event_trigger"`
/// - `"void"`
/// - `"SETOF <type>"`
/// - `"TABLE(<col> <type>, ...)"`
/// - a scalar type string
fn parse_return_type_from_string(
    s: &str,
    qname: &QualifiedName,
) -> Result<ReturnType, CatalogError> {
    let trimmed = s.trim();
    let lower = trimmed.to_ascii_lowercase();

    if lower == "trigger" {
        return Ok(ReturnType::Trigger);
    }
    if lower == "event_trigger" {
        return Ok(ReturnType::EventTrigger);
    }
    if lower == "void" {
        return Ok(ReturnType::Void);
    }

    if let Some(inner) = lower.strip_prefix("setof ") {
        let ty = parse_return_scalar_type(inner, trimmed, qname)?;
        return Ok(ReturnType::SetOf { ty });
    }

    if lower.starts_with("table(") && lower.ends_with(')') {
        let inner = &trimmed[6..trimmed.len() - 1]; // strip "TABLE(" and ")"
        let columns = parse_table_return_columns(inner, qname)?;
        return Ok(ReturnType::Table { columns });
    }

    let ty = parse_return_scalar_type(&lower, trimmed, qname)?;
    Ok(ReturnType::Scalar { ty })
}

/// Parse a scalar type string, using the original (non-lowercased) string for
/// user-defined type names to preserve case.
fn parse_return_scalar_type(
    lower: &str,
    original: &str,
    _qname: &QualifiedName,
) -> Result<crate::ir::column_type::ColumnType, CatalogError> {
    crate::ir::column_type::ColumnType::parse_from_pg_type_string(lower)
        .or_else(|_| crate::ir::column_type::ColumnType::parse_from_pg_type_string(original))
        .map_err(|e| {
            CatalogError::Ir(crate::ir::IrError::InvalidColumnType(format!(
                "return type {original:?}: {e}"
            )))
        })
}

/// Parse `"col1 type1, col2 type2, ..."` into `Vec<TableColumn>`.
fn parse_table_return_columns(
    inner: &str,
    qname: &QualifiedName,
) -> Result<Vec<TableColumn>, CatalogError> {
    use pg_query::protobuf::FunctionParameterMode;

    // We synthesize a CREATE FUNCTION with a RETURNS TABLE clause and re-parse.
    let wrapper = format!(
        "CREATE FUNCTION pgevolve_temp() RETURNS TABLE({inner}) LANGUAGE sql AS $$ SELECT NULL $$;"
    );
    let parsed = pg_query::parse(&wrapper).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "RETURNS TABLE parse for {qname} ({inner:?}): {e}"
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
                "RETURNS TABLE parse for {qname}: no stmt"
            )))
        })?;
    let pg_query::NodeEnum::CreateFunctionStmt(stmt) = stmt else {
        return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            format!("RETURNS TABLE parse for {qname}: unexpected stmt kind"),
        )));
    };

    let catalog_location = SourceLocation::new(std::path::PathBuf::from("<catalog>"), 1, 1);
    let mut columns: Vec<TableColumn> = Vec::new();
    for param_node in &stmt.parameters {
        let Some(pg_query::NodeEnum::FunctionParameter(p)) = param_node.node.as_ref() else {
            continue;
        };
        let raw_mode =
            FunctionParameterMode::try_from(p.mode).unwrap_or(FunctionParameterMode::Undefined);
        if raw_mode != FunctionParameterMode::FuncParamTable {
            continue;
        }
        let Some(tn) = p.arg_type.as_ref() else {
            continue;
        };
        let ty = crate::parse::builder::shared::type_name_to_column_type(tn, &catalog_location)
            .map_err(|e| {
                CatalogError::Ir(crate::ir::IrError::InvalidColumnType(format!(
                    "RETURNS TABLE col type for {qname}: {e}"
                )))
            })?;
        let nm = ident_required(&p.name)?;
        columns.push(TableColumn { name: nm, ty });
    }

    Ok(columns)
}
