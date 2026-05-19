//! SQL emission for function and procedure planner steps.
//!
//! Each `emit_*` function produces a single canonical SQL statement string
//! (always ending with `;`) suitable for direct embedding in `plan.sql`.
//! Output is deterministic: same input IR always produces the same bytes.

use crate::identifier::QualifiedName;
use crate::ir::function::{
    ArgMode, Function, FunctionArg, FunctionLanguage, NormalizedArgTypes, ParallelSafety,
    ReturnType, SecurityMode, Volatility,
};
use crate::ir::procedure::Procedure;

// ---------------------------------------------------------------------------
// CREATE OR REPLACE FUNCTION
// ---------------------------------------------------------------------------

/// `CREATE OR REPLACE FUNCTION qname(args) RETURNS ... LANGUAGE ... AS $pgevolve$...$pgevolve$;`
pub(crate) fn emit_create_or_replace_function(f: &Function) -> String {
    let mut sql = format!("CREATE OR REPLACE FUNCTION {}(", f.qname.render_sql());
    emit_arg_list(&f.args, &mut sql);
    sql.push(')');

    sql.push_str("\n    RETURNS ");
    emit_return_type(&f.return_type, &mut sql);

    sql.push_str("\n    LANGUAGE ");
    sql.push_str(match f.language {
        FunctionLanguage::Sql => "sql",
        FunctionLanguage::PlPgSql => "plpgsql",
    });

    // Attributes in fixed order for determinism.
    match f.volatility {
        Volatility::Immutable => sql.push_str(" IMMUTABLE"),
        Volatility::Stable => sql.push_str(" STABLE"),
        Volatility::Volatile => {}
    }
    if f.strict {
        sql.push_str(" STRICT");
    }
    if matches!(f.security, SecurityMode::Definer) {
        sql.push_str(" SECURITY DEFINER");
    }
    match f.parallel {
        ParallelSafety::Safe => sql.push_str(" PARALLEL SAFE"),
        ParallelSafety::Restricted => sql.push_str(" PARALLEL RESTRICTED"),
        ParallelSafety::Unsafe => {}
    }
    if f.leakproof {
        sql.push_str(" LEAKPROOF");
    }
    if let Some(c) = f.cost {
        sql.push_str(&format!(" COST {c}"));
    }
    if let Some(r) = f.rows {
        sql.push_str(&format!(" ROWS {r}"));
    }

    sql.push_str("\nAS $pgevolve$");
    sql.push_str(f.body.canonical_text());
    sql.push_str("$pgevolve$;");
    sql
}

// ---------------------------------------------------------------------------
// DROP FUNCTION
// ---------------------------------------------------------------------------

/// `DROP FUNCTION qname(arg_types);`
pub(crate) fn emit_drop_function(qname: &QualifiedName, args: &NormalizedArgTypes) -> String {
    format!(
        "DROP FUNCTION {}({});",
        qname.render_sql(),
        render_arg_types(args)
    )
}

// ---------------------------------------------------------------------------
// COMMENT ON FUNCTION
// ---------------------------------------------------------------------------

/// `COMMENT ON FUNCTION qname(arg_types) IS '...'|NULL;`
pub(crate) fn emit_comment_on_function(
    qname: &QualifiedName,
    args: &NormalizedArgTypes,
    comment: Option<&str>,
) -> String {
    let arg_sig = render_arg_types(args);
    match comment {
        Some(c) => format!(
            "COMMENT ON FUNCTION {}({}) IS '{}';",
            qname.render_sql(),
            arg_sig,
            c.replace('\'', "''"),
        ),
        None => format!(
            "COMMENT ON FUNCTION {}({}) IS NULL;",
            qname.render_sql(),
            arg_sig,
        ),
    }
}

// ---------------------------------------------------------------------------
// CREATE OR REPLACE PROCEDURE
// ---------------------------------------------------------------------------

/// `CREATE OR REPLACE PROCEDURE qname(args) LANGUAGE ... AS $pgevolve$...$pgevolve$;`
pub(crate) fn emit_create_or_replace_procedure(p: &Procedure) -> String {
    let mut sql = format!("CREATE OR REPLACE PROCEDURE {}(", p.qname.render_sql());
    emit_arg_list(&p.args, &mut sql);
    sql.push(')');

    sql.push_str("\n    LANGUAGE ");
    sql.push_str(match p.language {
        FunctionLanguage::Sql => "sql",
        FunctionLanguage::PlPgSql => "plpgsql",
    });

    if matches!(p.security, SecurityMode::Definer) {
        sql.push_str(" SECURITY DEFINER");
    }

    sql.push_str("\nAS $pgevolve$");
    sql.push_str(p.body.canonical_text());
    sql.push_str("$pgevolve$;");
    sql
}

// ---------------------------------------------------------------------------
// DROP PROCEDURE
// ---------------------------------------------------------------------------

/// `DROP PROCEDURE qname;`
pub(crate) fn emit_drop_procedure(qname: &QualifiedName) -> String {
    format!("DROP PROCEDURE {};", qname.render_sql())
}

// ---------------------------------------------------------------------------
// COMMENT ON PROCEDURE
// ---------------------------------------------------------------------------

/// `COMMENT ON PROCEDURE qname IS '...'|NULL;`
pub(crate) fn emit_comment_on_procedure(qname: &QualifiedName, comment: Option<&str>) -> String {
    match comment {
        Some(c) => format!(
            "COMMENT ON PROCEDURE {} IS '{}';",
            qname.render_sql(),
            c.replace('\'', "''"),
        ),
        None => format!("COMMENT ON PROCEDURE {} IS NULL;", qname.render_sql()),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn emit_arg_list(args: &[FunctionArg], out: &mut String) {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        match arg.mode {
            ArgMode::In => {} // default — no keyword needed
            ArgMode::Out => out.push_str("OUT "),
            ArgMode::InOut => out.push_str("INOUT "),
            ArgMode::Variadic => out.push_str("VARIADIC "),
        }
        if let Some(name) = &arg.name {
            out.push_str(&name.render_sql());
            out.push(' ');
        }
        out.push_str(&arg.ty.render_sql());
        if let Some(default) = &arg.default {
            out.push_str(" DEFAULT ");
            out.push_str(&default.canonical_text);
        }
    }
}

fn emit_return_type(rt: &ReturnType, out: &mut String) {
    match rt {
        ReturnType::Scalar { ty } => out.push_str(&ty.render_sql()),
        ReturnType::SetOf { ty } => {
            out.push_str("SETOF ");
            out.push_str(&ty.render_sql());
        }
        ReturnType::Table { columns } => {
            out.push_str("TABLE(");
            for (i, c) in columns.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&c.name.render_sql());
                out.push(' ');
                out.push_str(&c.ty.render_sql());
            }
            out.push(')');
        }
        ReturnType::Trigger => out.push_str("trigger"),
        ReturnType::EventTrigger => out.push_str("event_trigger"),
        ReturnType::Void => out.push_str("void"),
    }
}

fn render_arg_types(args: &NormalizedArgTypes) -> String {
    args.types
        .iter()
        .map(crate::ir::column_type::ColumnType::render_sql)
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::column_type::ColumnType;
    use crate::ir::function::{FunctionArg, TableColumn};
    use crate::parse::normalize_body::NormalizedBody;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn arg_types(types: Vec<ColumnType>) -> NormalizedArgTypes {
        let canonical_string = types
            .iter()
            .map(ColumnType::render_sql)
            .collect::<Vec<_>>()
            .join(",");
        let canonical_hash = blake3::hash(canonical_string.as_bytes()).into();
        NormalizedArgTypes {
            types,
            canonical_hash,
        }
    }

    fn minimal_function() -> Function {
        let args = vec![];
        let arg_types_normalized = NormalizedArgTypes::from_args(&args);
        Function {
            qname: qn("app", "noop"),
            args,
            arg_types_normalized,
            return_type: ReturnType::Void,
            language: FunctionLanguage::Sql,
            body: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            volatility: Volatility::Volatile,
            strict: false,
            security: SecurityMode::Invoker,
            parallel: ParallelSafety::Unsafe,
            leakproof: false,
            cost: None,
            rows: None,
            comment: None,
        }
    }

    fn minimal_procedure() -> Procedure {
        Procedure {
            qname: qn("app", "do_thing"),
            args: vec![],
            language: FunctionLanguage::PlPgSql,
            body: NormalizedBody::empty(),
            body_dependencies: vec![],
            security: SecurityMode::Invoker,
            commits_in_body: false,
            comment: None,
        }
    }

    // --- emit_drop_function ---

    #[test]
    fn emit_drop_function_basic() {
        let sql = emit_drop_function(&qn("app", "noop"), &arg_types(vec![]));
        assert_eq!(sql, "DROP FUNCTION app.noop();");
    }

    #[test]
    fn emit_drop_function_with_overload_args() {
        let sql = emit_drop_function(
            &qn("app", "compute"),
            &arg_types(vec![ColumnType::Integer, ColumnType::Text]),
        );
        assert_eq!(sql, "DROP FUNCTION app.compute(integer, text);");
    }

    // --- emit_create_or_replace_function ---

    #[test]
    fn emit_create_or_replace_function_sql_minimal() {
        let f = minimal_function();
        let sql = emit_create_or_replace_function(&f);
        assert!(
            sql.starts_with("CREATE OR REPLACE FUNCTION app.noop()"),
            "got: {sql}"
        );
        assert!(sql.contains("LANGUAGE sql"), "got: {sql}");
        assert!(sql.contains("RETURNS void"), "got: {sql}");
        assert!(sql.contains("$pgevolve$"), "got: {sql}");
    }

    #[test]
    fn emit_create_or_replace_function_plpgsql_with_all_attrs() {
        let args = vec![FunctionArg {
            name: Some(id("n")),
            mode: ArgMode::In,
            ty: ColumnType::Integer,
            default: None,
        }];
        let arg_types_normalized = NormalizedArgTypes::from_args(&args);
        let f = Function {
            qname: qn("app", "double"),
            args,
            arg_types_normalized,
            return_type: ReturnType::Scalar {
                ty: ColumnType::Integer,
            },
            language: FunctionLanguage::PlPgSql,
            body: NormalizedBody::from_raw_canonical("BEGIN RETURN n * 2; END".to_string()),
            body_dependencies: vec![],
            volatility: Volatility::Immutable,
            strict: true,
            security: SecurityMode::Definer,
            parallel: ParallelSafety::Safe,
            leakproof: true,
            cost: Some(100.0),
            rows: None,
            comment: None,
        };
        let sql = emit_create_or_replace_function(&f);
        assert!(sql.contains("LANGUAGE plpgsql"), "got: {sql}");
        assert!(sql.contains("IMMUTABLE"), "got: {sql}");
        assert!(sql.contains("STRICT"), "got: {sql}");
        assert!(sql.contains("SECURITY DEFINER"), "got: {sql}");
        assert!(sql.contains("PARALLEL SAFE"), "got: {sql}");
        assert!(sql.contains("LEAKPROOF"), "got: {sql}");
        assert!(sql.contains("COST"), "got: {sql}");
        assert!(sql.contains("RETURNS integer"), "got: {sql}");
        assert!(sql.contains("n integer"), "got: {sql}");
    }

    #[test]
    fn emit_create_or_replace_function_stable_not_written_for_volatile() {
        // VOLATILE is the default and should produce no keyword.
        let f = minimal_function();
        let sql = emit_create_or_replace_function(&f);
        assert!(!sql.contains("VOLATILE"), "got: {sql}");
        assert!(!sql.contains("STABLE"), "got: {sql}");
        assert!(!sql.contains("IMMUTABLE"), "got: {sql}");
    }

    #[test]
    fn emit_create_or_replace_function_stable_keyword() {
        let mut f = minimal_function();
        f.volatility = Volatility::Stable;
        let sql = emit_create_or_replace_function(&f);
        assert!(sql.contains("STABLE"), "got: {sql}");
    }

    // --- emit_comment_on_function ---

    #[test]
    fn emit_comment_on_function_with_args() {
        let sql = emit_comment_on_function(
            &qn("app", "compute"),
            &arg_types(vec![ColumnType::Integer]),
            Some("computes a value"),
        );
        assert_eq!(
            sql,
            "COMMENT ON FUNCTION app.compute(integer) IS 'computes a value';"
        );
    }

    #[test]
    fn emit_comment_on_function_null_clears() {
        let sql = emit_comment_on_function(&qn("app", "noop"), &arg_types(vec![]), None);
        assert_eq!(sql, "COMMENT ON FUNCTION app.noop() IS NULL;");
    }

    #[test]
    fn emit_comment_on_function_escapes_single_quotes() {
        let sql = emit_comment_on_function(&qn("app", "f"), &arg_types(vec![]), Some("it's cool"));
        assert!(sql.contains("'it''s cool'"), "got: {sql}");
    }

    // --- emit_setof_return_type ---

    #[test]
    fn emit_setof_return_type() {
        let mut f = minimal_function();
        f.return_type = ReturnType::SetOf {
            ty: ColumnType::Integer,
        };
        let sql = emit_create_or_replace_function(&f);
        assert!(sql.contains("RETURNS SETOF integer"), "got: {sql}");
    }

    // --- emit_table_return_type ---

    #[test]
    fn emit_table_return_type() {
        let mut f = minimal_function();
        f.return_type = ReturnType::Table {
            columns: vec![
                TableColumn {
                    name: id("id"),
                    ty: ColumnType::BigInt,
                },
                TableColumn {
                    name: id("name"),
                    ty: ColumnType::Text,
                },
            ],
        };
        let sql = emit_create_or_replace_function(&f);
        assert!(
            sql.contains("RETURNS TABLE(id bigint, name text)"),
            "got: {sql}"
        );
    }

    // --- emit_create_or_replace_procedure ---

    #[test]
    fn emit_create_or_replace_procedure_minimal() {
        let p = minimal_procedure();
        let sql = emit_create_or_replace_procedure(&p);
        assert!(
            sql.starts_with("CREATE OR REPLACE PROCEDURE app.do_thing()"),
            "got: {sql}"
        );
        assert!(sql.contains("LANGUAGE plpgsql"), "got: {sql}");
        assert!(sql.contains("$pgevolve$"), "got: {sql}");
    }

    #[test]
    fn emit_create_or_replace_procedure_sql_language() {
        let mut p = minimal_procedure();
        p.language = FunctionLanguage::Sql;
        let sql = emit_create_or_replace_procedure(&p);
        assert!(sql.contains("LANGUAGE sql"), "got: {sql}");
    }

    #[test]
    fn emit_create_or_replace_procedure_security_definer() {
        let mut p = minimal_procedure();
        p.security = SecurityMode::Definer;
        let sql = emit_create_or_replace_procedure(&p);
        assert!(sql.contains("SECURITY DEFINER"), "got: {sql}");
    }

    // --- emit_drop_procedure ---

    #[test]
    fn emit_drop_procedure_basic() {
        let sql = emit_drop_procedure(&qn("app", "do_thing"));
        assert_eq!(sql, "DROP PROCEDURE app.do_thing;");
    }

    // --- emit_comment_on_procedure ---

    #[test]
    fn emit_comment_on_procedure_some() {
        let sql = emit_comment_on_procedure(&qn("app", "do_thing"), Some("does the thing"));
        assert_eq!(
            sql,
            "COMMENT ON PROCEDURE app.do_thing IS 'does the thing';"
        );
    }

    #[test]
    fn emit_comment_on_procedure_null() {
        let sql = emit_comment_on_procedure(&qn("app", "do_thing"), None);
        assert_eq!(sql, "COMMENT ON PROCEDURE app.do_thing IS NULL;");
    }

    #[test]
    fn emit_comment_on_procedure_escapes_quotes() {
        let sql = emit_comment_on_procedure(&qn("app", "p"), Some("it's a proc"));
        assert!(sql.contains("'it''s a proc'"), "got: {sql}");
    }

    // --- arg modes ---

    #[test]
    fn emit_arg_list_with_out_and_inout_modes() {
        let args = vec![
            FunctionArg {
                name: Some(id("x")),
                mode: ArgMode::In,
                ty: ColumnType::Integer,
                default: None,
            },
            FunctionArg {
                name: Some(id("y")),
                mode: ArgMode::Out,
                ty: ColumnType::Text,
                default: None,
            },
            FunctionArg {
                name: Some(id("z")),
                mode: ArgMode::InOut,
                ty: ColumnType::BigInt,
                default: None,
            },
        ];
        let mut out = String::new();
        emit_arg_list(&args, &mut out);
        assert_eq!(out, "x integer, OUT y text, INOUT z bigint");
    }

    #[test]
    fn emit_arg_list_variadic() {
        let args = vec![FunctionArg {
            name: Some(id("items")),
            mode: ArgMode::Variadic,
            ty: ColumnType::Text,
            default: None,
        }];
        let mut out = String::new();
        emit_arg_list(&args, &mut out);
        assert_eq!(out, "VARIADIC items text");
    }
}
