//! Source-side parser for `CREATE FUNCTION` and `CREATE PROCEDURE`.
//!
//! Both statements are represented in `pg_query` by a single
//! `CreateFunctionStmt` with an `is_procedure: bool` discriminant.
//! This builder dispatches on that flag and returns either a
//! [`Routine::Function`] or a [`Routine::Procedure`].
//!
//! **Body parsing (T2 stub)**: SQL-language bodies are canonicalized via
//! [`NormalizedBody::from_sql`]; PL/pgSQL bodies fall back to
//! [`NormalizedBody::empty`] because `pg_query` cannot parse PL/pgSQL.
//! T4 replaces the fallback with real PL/pgSQL AST parsing.

use pg_query::NodeEnum;
use pg_query::protobuf::{CreateFunctionStmt, FunctionParameterMode};

use crate::identifier::Identifier;
use crate::ir::function::{
    ArgMode, Function, FunctionArg, FunctionLanguage, NormalizedArgTypes, ParallelSafety,
    ReturnType, SecurityMode, TableColumn, Volatility,
};
use crate::ir::procedure::Procedure;
use crate::parse::builder::plpgsql;
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};
use crate::parse::normalize_body::NormalizedBody;
use crate::parse::normalize_expr;

/// Dispatch output for this builder.
#[derive(Debug)]
pub enum Routine {
    /// A `CREATE FUNCTION` statement.
    Function(Function),
    /// A `CREATE PROCEDURE` statement.
    Procedure(Procedure),
}

/// Build a [`Routine`] from a `CreateFunctionStmt` AST node.
///
/// * `default_schema` — used when the statement has no schema prefix and a
///   `-- @pgevolve schema=` directive is in effect.
/// * Unsupported languages (anything other than `sql` / `plpgsql`) are
///   rejected with a [`ParseError::Structural`] naming the bad language.
/// * Procedure-specific constraints: `VOLATILE/STABLE/IMMUTABLE`, `STRICT`,
///   `PARALLEL`, `LEAKPROOF`, `COST`, `ROWS`, and a return-type clause are
///   all rejected on procedures.
#[allow(clippy::too_many_lines)] // exhaustive walk of `CREATE FUNCTION`/`CREATE PROCEDURE` options; one arm per option kind.
pub(crate) fn build_function_or_procedure(
    stmt: &CreateFunctionStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Routine, ParseError> {
    let qname = shared::qname_from_string_list(&stmt.funcname, default_schema, location)?;
    let is_procedure = stmt.is_procedure;

    // ── Parameters ──────────────────────────────────────────────────────────
    let mut args: Vec<FunctionArg> = Vec::new();
    let mut table_columns: Vec<TableColumn> = Vec::new();

    for param_node in &stmt.parameters {
        let Some(NodeEnum::FunctionParameter(p)) = param_node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE FUNCTION/PROCEDURE {qname}: unexpected parameter node"),
            });
        };

        let raw_mode =
            FunctionParameterMode::try_from(p.mode).unwrap_or(FunctionParameterMode::Undefined);

        // TABLE mode parameters are collected separately for RETURNS TABLE.
        if raw_mode == FunctionParameterMode::FuncParamTable {
            let tn = p.arg_type.as_ref().ok_or_else(|| ParseError::Structural {
                location: location.clone(),
                message: format!("{qname}: TABLE column missing type"),
            })?;
            let ty = shared::type_name_to_column_type(tn, location)?;
            let name = if p.name.is_empty() {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!("{qname}: TABLE column missing name"),
                });
            } else {
                shared::ident(&p.name, location)?
            };
            table_columns.push(TableColumn { name, ty });
            continue;
        }

        let mode = match raw_mode {
            FunctionParameterMode::FuncParamIn
            | FunctionParameterMode::FuncParamDefault
            | FunctionParameterMode::Undefined => ArgMode::In,
            FunctionParameterMode::FuncParamOut => ArgMode::Out,
            FunctionParameterMode::FuncParamInout => ArgMode::InOut,
            FunctionParameterMode::FuncParamVariadic => ArgMode::Variadic,
            FunctionParameterMode::FuncParamTable => unreachable!("handled above"),
        };

        let tn = p.arg_type.as_ref().ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("{qname}: parameter missing type"),
        })?;
        let ty = shared::type_name_to_column_type(tn, location)?;

        let name = if p.name.is_empty() {
            None
        } else {
            Some(shared::ident(&p.name, location)?)
        };

        // Default expression (stubbed normalize via from_pg_node)
        let default = if let Some(defexpr_boxed) = p.defexpr.as_ref() {
            let node_enum = defexpr_boxed
                .node
                .as_ref()
                .ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: format!("{qname}: argument default expression is empty"),
                })?;
            Some(normalize_expr::from_pg_node(
                node_enum,
                Some(&ty),
                location,
            )?)
        } else {
            None
        };

        args.push(FunctionArg {
            name,
            mode,
            ty,
            default,
        });
    }

    // ── Options ──────────────────────────────────────────────────────────────
    let mut language: Option<FunctionLanguage> = None;
    let mut volatility: Option<Volatility> = None;
    let mut strict = false;
    let mut security: Option<SecurityMode> = None;
    let mut parallel: Option<ParallelSafety> = None;
    let mut leakproof = false;
    let mut cost: Option<f32> = None;
    let mut rows: Option<f32> = None;
    let mut body_text: Option<String> = None;

    for opt_node in &stmt.options {
        let Some(NodeEnum::DefElem(de)) = opt_node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("{qname}: unexpected option node"),
            });
        };

        match de.defname.as_str() {
            "language" => {
                let lang_str = string_from_def_elem(de).ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: format!("{qname}: LANGUAGE option missing value"),
                })?;
                language = Some(match lang_str.to_lowercase().as_str() {
                    "sql" => FunctionLanguage::Sql,
                    "plpgsql" => FunctionLanguage::PlPgSql,
                    other => {
                        return Err(ParseError::Structural {
                            location: location.clone(),
                            message: format!(
                                "{qname}: unsupported language {other:?} \
                                 (pgevolve v0.2 supports sql and plpgsql)"
                            ),
                        });
                    }
                });
            }
            "volatility" => {
                if is_procedure {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!(
                            "{qname}: VOLATILE/STABLE/IMMUTABLE is not valid on procedures"
                        ),
                    });
                }
                let v_str = string_from_def_elem(de).ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: format!("{qname}: volatility option missing value"),
                })?;
                volatility = Some(match v_str.to_lowercase().as_str() {
                    "immutable" => Volatility::Immutable,
                    "stable" => Volatility::Stable,
                    "volatile" => Volatility::Volatile,
                    other => {
                        return Err(ParseError::Structural {
                            location: location.clone(),
                            message: format!("{qname}: unknown volatility {other:?}"),
                        });
                    }
                });
            }
            "strict" => {
                if is_procedure {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!("{qname}: STRICT is not valid on procedures"),
                    });
                }
                strict = bool_from_def_elem(de);
            }
            "security" => {
                // PG encodes SECURITY DEFINER as defname="security" with
                // arg = Boolean(true); SECURITY INVOKER as Boolean(false).
                // Also accept Integer/String values for robustness.
                security = Some(match de.arg.as_ref().and_then(|a| a.node.as_ref()) {
                    Some(NodeEnum::Boolean(b)) => {
                        if b.boolval {
                            SecurityMode::Definer
                        } else {
                            SecurityMode::Invoker
                        }
                    }
                    Some(NodeEnum::Integer(i)) => {
                        if i.ival != 0 {
                            SecurityMode::Definer
                        } else {
                            SecurityMode::Invoker
                        }
                    }
                    Some(NodeEnum::String(s)) => match s.sval.to_lowercase().as_str() {
                        "invoker" => SecurityMode::Invoker,
                        "definer" => SecurityMode::Definer,
                        other => {
                            return Err(ParseError::Structural {
                                location: location.clone(),
                                message: format!("{qname}: unknown security mode {other:?}"),
                            });
                        }
                    },
                    None => SecurityMode::Invoker, // bare "security" without qualifier
                    _ => {
                        return Err(ParseError::Structural {
                            location: location.clone(),
                            message: format!("{qname}: SECURITY option has unexpected value"),
                        });
                    }
                });
            }
            "parallel" => {
                if is_procedure {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!("{qname}: PARALLEL is not valid on procedures"),
                    });
                }
                let p_str = string_from_def_elem(de).ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: format!("{qname}: PARALLEL option missing value"),
                })?;
                parallel = Some(match p_str.to_lowercase().as_str() {
                    "safe" => ParallelSafety::Safe,
                    "restricted" => ParallelSafety::Restricted,
                    "unsafe" => ParallelSafety::Unsafe,
                    other => {
                        return Err(ParseError::Structural {
                            location: location.clone(),
                            message: format!("{qname}: unknown parallel safety {other:?}"),
                        });
                    }
                });
            }
            "leakproof" => {
                if is_procedure {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!("{qname}: LEAKPROOF is not valid on procedures"),
                    });
                }
                leakproof = bool_from_def_elem(de);
            }
            "cost" => {
                if is_procedure {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!("{qname}: COST is not valid on procedures"),
                    });
                }
                // Raw parse: `ir::canon::filter_pg_defaults` strips the
                // PG-default value of 100 to None on both sides.
                cost = float_from_def_elem(de);
            }
            "rows" => {
                if is_procedure {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!("{qname}: ROWS is not valid on procedures"),
                    });
                }
                // Raw parse: `ir::canon::filter_pg_defaults` strips the
                // PG-default (1000 for SETOF, 0 otherwise) to None.
                rows = float_from_def_elem(de);
            }
            "as" => {
                // The body is the first element (the $$ string $$).
                // For sql_body-style functions, body comes via stmt.sql_body instead.
                body_text = Some(string_from_def_elem(de).unwrap_or_default());
            }
            other => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!("{qname}: unsupported function option {other:?}"),
                });
            }
        }
    }

    let lang = language.unwrap_or(FunctionLanguage::Sql);

    // ── Body parsing ─────────────────────────────────────────────────────────
    // Delegate to the T4 PL/pgSQL body parser which handles both SQL and
    // PL/pgSQL languages, extracts dep edges, and detects COMMIT/ROLLBACK.
    let raw_body = body_text.as_deref().unwrap_or("").trim().to_string();
    let (body, body_deps, commits_in_body) = if raw_body.is_empty() {
        (NormalizedBody::empty(), vec![], false)
    } else {
        plpgsql::parse_routine_body(&raw_body, lang, &qname, location)?
    };

    if is_procedure {
        // Procedure: no return type clause allowed.
        if stmt.return_type.is_some() {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("{qname}: procedures cannot have a RETURNS clause"),
            });
        }

        let security = security.unwrap_or(SecurityMode::Invoker);

        return Ok(Routine::Procedure(Procedure {
            qname,
            args,
            language: lang,
            body,
            body_dependencies: body_deps,
            security,
            commits_in_body,
            comment: None,
            owner: None,
            grants: vec![],
        }));
    }

    // ── Return type ──────────────────────────────────────────────────────────
    let return_type = if !table_columns.is_empty() {
        // RETURNS TABLE(...) — collected from FuncParamTable parameters.
        ReturnType::Table {
            columns: table_columns,
        }
    } else if let Some(tn) = stmt.return_type.as_ref() {
        let type_str =
            shared::render_type_name_to_string(tn).ok_or_else(|| ParseError::Structural {
                location: location.clone(),
                message: format!("{qname}: could not stringify return type"),
            })?;
        match type_str.to_lowercase().as_str() {
            "trigger" => ReturnType::Trigger,
            "event_trigger" => ReturnType::EventTrigger,
            "void" => ReturnType::Void,
            _ => {
                let ty = shared::type_name_to_column_type(tn, location)?;
                if tn.setof {
                    ReturnType::SetOf { ty }
                } else {
                    ReturnType::Scalar { ty }
                }
            }
        }
    } else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("{qname}: function is missing a RETURNS clause"),
        });
    };

    let arg_types_normalized = NormalizedArgTypes::from_args(&args);

    Ok(Routine::Function(Function {
        qname,
        args,
        arg_types_normalized,
        return_type,
        language: lang,
        body,
        body_dependencies: body_deps,
        volatility: volatility.unwrap_or(Volatility::Volatile),
        strict,
        security: security.unwrap_or(SecurityMode::Invoker),
        parallel: parallel.unwrap_or(ParallelSafety::Unsafe),
        leakproof,
        cost,
        rows,
        comment: None,
        owner: None,
        grants: vec![],
    }))
}

/// Extract a string value from a `DefElem.arg` (String node).
///
/// Dollar-quoted function bodies are wrapped in a `List` by `pg_query`;
/// this function unwraps the outer `List` and returns the first `String`
/// element's value (which is the body text without the dollar-quote
/// delimiters).
fn string_from_def_elem(de: &pg_query::protobuf::DefElem) -> Option<String> {
    let arg = de.arg.as_ref()?;
    match arg.node.as_ref()? {
        NodeEnum::String(s) => Some(s.sval.clone()),
        NodeEnum::Integer(i) => Some(i.ival.to_string()),
        NodeEnum::Float(f) => Some(f.fval.clone()),
        // Dollar-quoted bodies: the arg is a List whose first item is the
        // body String (the second item, when present, is the dollar-quote tag).
        NodeEnum::List(list) => list
            .items
            .first()
            .and_then(|n| n.node.as_ref())
            .and_then(|n| {
                if let NodeEnum::String(s) = n {
                    Some(s.sval.clone())
                } else {
                    None
                }
            }),
        _ => None,
    }
}

/// Extract a boolean value from a `DefElem.arg`, defaulting to `true` if
/// the arg is absent (bare keyword like `STRICT` or `LEAKPROOF`).
fn bool_from_def_elem(de: &pg_query::protobuf::DefElem) -> bool {
    let Some(arg) = de.arg.as_ref() else {
        return true; // bare keyword = true
    };
    match arg.node.as_ref() {
        Some(NodeEnum::Integer(i)) => i.ival != 0,
        _ => true,
    }
}

/// Extract a float cost/rows value from a `DefElem.arg`.
fn float_from_def_elem(de: &pg_query::protobuf::DefElem) -> Option<f32> {
    let arg = de.arg.as_ref()?;
    match arg.node.as_ref()? {
        NodeEnum::Float(f) => f.fval.parse::<f32>().ok(),
        #[allow(clippy::cast_precision_loss)]
        NodeEnum::Integer(i) => Some(i.ival as f32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use pg_query::NodeEnum as PgNodeEnum;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn parse_function(sql: &str) -> CreateFunctionStmt {
        let parsed = pg_query::parse(sql).expect("parses");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let PgNodeEnum::CreateFunctionStmt(boxed) = node else {
            panic!("expected CreateFunctionStmt, got: {node:?}");
        };
        *boxed
    }

    fn build_fn(sql: &str) -> Function {
        let stmt = parse_function(sql);
        match build_function_or_procedure(&stmt, None, &loc()).expect("build") {
            Routine::Function(f) => f,
            Routine::Procedure(_) => panic!("expected Function"),
        }
    }

    fn build_proc(sql: &str) -> Procedure {
        let stmt = parse_function(sql);
        match build_function_or_procedure(&stmt, None, &loc()).expect("build") {
            Routine::Procedure(p) => p,
            Routine::Function(_) => panic!("expected Procedure"),
        }
    }

    #[test]
    fn simple_sql_function() {
        let f = build_fn(
            "CREATE FUNCTION app.double(x integer) RETURNS integer \
             LANGUAGE sql IMMUTABLE STRICT AS $$ SELECT x * 2 $$;",
        );
        assert_eq!(f.qname.to_string(), "app.double");
        assert_eq!(f.args.len(), 1);
        assert_eq!(
            f.args[0]
                .name
                .as_ref()
                .map(crate::identifier::Identifier::as_str),
            Some("x")
        );
        assert_eq!(f.args[0].mode, ArgMode::In);
        assert!(matches!(
            f.return_type,
            ReturnType::Scalar {
                ty: crate::ir::column_type::ColumnType::Integer
            }
        ));
        assert_eq!(f.language, FunctionLanguage::Sql);
        assert!(matches!(f.volatility, Volatility::Immutable));
        assert!(f.strict);
    }

    #[test]
    fn plpgsql_function_body_parsed() {
        let f = build_fn(
            "CREATE FUNCTION app.greet(name text) RETURNS text \
             LANGUAGE plpgsql AS $$ BEGIN RETURN 'hello'; END $$;",
        );
        assert_eq!(f.qname.to_string(), "app.greet");
        assert_eq!(f.language, FunctionLanguage::PlPgSql);
        // T4: body is now parsed; canonical text should be non-empty.
        assert!(
            !f.body.canonical_text().is_empty(),
            "PL/pgSQL body should be canonicalized, got empty string"
        );
    }

    #[test]
    fn unqualified_uses_default_schema() {
        let stmt = parse_function(
            "CREATE FUNCTION double(x integer) RETURNS integer \
             LANGUAGE sql AS $$ SELECT x $$;",
        );
        let app = Identifier::from_unquoted("app").unwrap();
        let Routine::Function(f) = build_function_or_procedure(&stmt, Some(&app), &loc()).unwrap()
        else {
            panic!()
        };
        assert_eq!(f.qname.to_string(), "app.double");
    }

    #[test]
    fn unsupported_language_rejected() {
        let stmt = parse_function("CREATE FUNCTION app.f() RETURNS void LANGUAGE plperl AS $$ $$;");
        let err = build_function_or_procedure(&stmt, None, &loc()).unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("plperl"), "should name bad language: {msg}");
    }

    #[test]
    fn procedure_builds() {
        let p = build_proc(
            "CREATE PROCEDURE app.do_work() \
             LANGUAGE plpgsql AS $$ BEGIN NULL; END $$;",
        );
        assert_eq!(p.qname.to_string(), "app.do_work");
        assert!(p.args.is_empty());
        assert_eq!(p.language, FunctionLanguage::PlPgSql);
    }

    #[test]
    fn procedure_rejects_volatility() {
        let stmt = parse_function(
            "CREATE PROCEDURE app.p() LANGUAGE plpgsql VOLATILE AS $$ BEGIN NULL; END $$;",
        );
        let err = build_function_or_procedure(&stmt, None, &loc()).unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(
            msg.contains("VOLATILE") || msg.contains("STABLE") || msg.contains("IMMUTABLE"),
            "msg: {msg}"
        );
    }

    #[test]
    fn procedure_rejects_return_type() {
        // pg_query rejects CREATE PROCEDURE ... RETURNS at the grammar level,
        // so the builder's defensive check is unreachable via parsed SQL. We
        // construct a synthetic CreateFunctionStmt with is_procedure=true and
        // return_type=Some(...) to exercise the rejection path directly.
        use pg_query::NodeEnum;
        use pg_query::protobuf::{CreateFunctionStmt, Node, TypeName};

        // Build the type-name list for "app.proc".
        let funcname = vec![
            Node {
                node: Some(NodeEnum::String(pg_query::protobuf::String {
                    sval: "app".into(),
                })),
            },
            Node {
                node: Some(NodeEnum::String(pg_query::protobuf::String {
                    sval: "proc".into(),
                })),
            },
        ];
        // Build a return-type TypeName for "integer".
        let return_type = TypeName {
            names: vec![Node {
                node: Some(NodeEnum::String(pg_query::protobuf::String {
                    sval: "integer".into(),
                })),
            }],
            type_oid: 0,
            setof: false,
            pct_type: false,
            typmods: vec![],
            typemod: -1,
            array_bounds: vec![],
            location: 0,
        };

        let stmt = CreateFunctionStmt {
            is_procedure: true,
            replace: false,
            funcname,
            parameters: vec![],
            return_type: Some(return_type),
            options: vec![],
            sql_body: None,
        };

        let err = build_function_or_procedure(&stmt, None, &loc()).unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        let lower = msg.to_lowercase();
        assert!(
            lower.contains("return") || lower.contains("returns"),
            "expected a 'returns/return-type' rejection message; got: {msg}",
        );
    }

    #[test]
    fn function_with_default_arg() {
        let f = build_fn(
            "CREATE FUNCTION app.greet(name text DEFAULT 'world') RETURNS text \
             LANGUAGE sql AS $$ SELECT 'hello ' || name $$;",
        );
        assert_eq!(f.args.len(), 1);
        assert!(f.args[0].default.is_some(), "should have a default");
    }

    #[test]
    fn security_definer() {
        let f = build_fn(
            "CREATE FUNCTION app.f() RETURNS void LANGUAGE sql SECURITY DEFINER \
             AS $$ SELECT NULL $$;",
        );
        assert!(matches!(f.security, SecurityMode::Definer));
    }
}
