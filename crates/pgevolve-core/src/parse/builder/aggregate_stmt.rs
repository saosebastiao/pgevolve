//! Parser for `CREATE AGGREGATE`, `ALTER AGGREGATE … OWNER TO`, and
//! `COMMENT ON AGGREGATE`.
//!
//! `pg_query` 6.x encodes `CREATE AGGREGATE` as a [`DefineStmt`] with
//! `kind = ObjectType::ObjectAggregate`:
//! - `defnames` is a list of `String` nodes (1–2 parts: `[name]` or `[schema, name]`).
//! - `args` is a two-element list: `args[0]` is either a `List` of
//!   `FunctionParameter` nodes (one per argument type) **or** `node: None` for
//!   the zero-argument `(*)` form; `args[1]` is an `Integer` whose `ival` is
//!   `-1` for an ordinary aggregate and `>= 0` (the number of direct arguments)
//!   for an ordered-set aggregate. A non-`-1` value means `ORDER BY` was used,
//!   which we reject.
//! - `definition` is a list of `DefElem` nodes (`sfunc`, `stype`, `finalfunc`,
//!   `initcond`, plus the many features we reject). `sfunc`/`finalfunc` args are
//!   function-name `TypeName` nodes; `stype` is a type `TypeName`; `initcond` is
//!   a bare `String`.
//!
//! v0.4.1 supports only ordinary aggregates: `SFUNC` + `STYPE` with optional
//! `FINALFUNC` / `INITCOND`. Every other `definition` option (combine/serial/
//! deserial/moving/sortop/parallel/…) and every ordered-set / hypothetical-set
//! form is rejected up front with a structured [`ParseError::Structural`].
//!
//! Aggregate identity is `(qname, arg_types)` (aggregates are overloadable), so
//! the accumulator is a `Vec<Aggregate>` and `ALTER … OWNER` / `COMMENT ON …`
//! are applied by matching that identity. `DROP AGGREGATE` and
//! `ALTER AGGREGATE … RENAME TO` in source are rejected (the latter in
//! `statement.rs`; DROP here).

use pg_query::NodeEnum;
use pg_query::protobuf::{
    AlterOwnerStmt, CommentStmt, DefElem, DefineStmt, FunctionParameter, ObjectWithArgs, TypeName,
};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::aggregate::Aggregate;
use crate::ir::column_type::ColumnType;
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// The set of `definition` options v0.4.1 understands. Anything else is rejected
/// with the "unsupported aggregate feature" message.
const SUPPORTED_OPTIONS: [&str; 4] = ["sfunc", "stype", "finalfunc", "initcond"];

/// Build an [`Aggregate`] from a `CREATE AGGREGATE` AST node and append it to the
/// accumulator.
///
/// Rejects ordered-set aggregates, unknown `definition` options, missing
/// `SFUNC`/`STYPE`, and duplicate `(qname, arg_types)` identities.
pub(crate) fn parse_create(
    stmt: &DefineStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    existing: &mut Vec<Aggregate>,
) -> Result<(), ParseError> {
    let qname = shared::qname_from_string_list(&stmt.defnames, default_schema, location)?;

    let arg_types = parse_arg_types(stmt, &qname, location)?;

    let mut sfunc: Option<QualifiedName> = None;
    let mut state_type: Option<ColumnType> = None;
    let mut finalfunc: Option<QualifiedName> = None;
    let mut initcond: Option<String> = None;

    for node in &stmt.definition {
        let Some(NodeEnum::DefElem(de)) = node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE AGGREGATE {qname}: unexpected non-DefElem in option list"),
            });
        };
        let key = de.defname.to_ascii_lowercase();
        if !SUPPORTED_OPTIONS.contains(&key.as_str()) {
            return Err(unsupported_feature(&key, location));
        }
        match key.as_str() {
            "sfunc" => sfunc = Some(funcname_from_defelem(de, "sfunc", &qname, location)?),
            "stype" => state_type = Some(typename_from_defelem(de, "stype", &qname, location)?),
            "finalfunc" => {
                finalfunc = Some(funcname_from_defelem(de, "finalfunc", &qname, location)?);
            }
            "initcond" => initcond = Some(string_from_defelem(de, "initcond", &qname, location)?),
            _ => unreachable!("guarded by SUPPORTED_OPTIONS check above"),
        }
    }

    let sfunc = sfunc.ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: format!("CREATE AGGREGATE {qname}: missing required option `sfunc`"),
    })?;
    let state_type = state_type.ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: format!("CREATE AGGREGATE {qname}: missing required option `stype`"),
    })?;

    if existing
        .iter()
        .any(|a| a.qname == qname && a.arg_types == arg_types)
    {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "duplicate aggregate {}({})",
                qname,
                render_arg_types(&arg_types)
            ),
        });
    }

    existing.push(Aggregate {
        qname,
        arg_types,
        state_type,
        sfunc,
        finalfunc,
        initcond,
        owner: None,
        comment: None,
    });
    Ok(())
}

/// Apply an `ALTER AGGREGATE name(args) OWNER TO role` against the accumulator.
pub(crate) fn apply_owner(
    stmt: &AlterOwnerStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    existing: &mut [Aggregate],
) -> Result<(), ParseError> {
    let owa = object_with_args(stmt.object.as_deref(), location)?;
    let (qname, arg_types) = identity_from_object(owa, default_schema, location)?;
    let new_owner = crate::parse::builder::owner_stmt::extract_new_owner(stmt, location)?;
    let agg = find_mut(existing, &qname, &arg_types, location)?;
    agg.owner = Some(new_owner);
    Ok(())
}

/// Apply a `COMMENT ON AGGREGATE name(args) IS '…'` against the accumulator.
pub(crate) fn apply_comment(
    stmt: &CommentStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    existing: &mut [Aggregate],
) -> Result<(), ParseError> {
    let owa = object_with_args(stmt.object.as_deref(), location)?;
    let (qname, arg_types) = identity_from_object(owa, default_schema, location)?;
    let comment = if stmt.comment.is_empty() {
        None
    } else {
        Some(stmt.comment.clone())
    };
    let agg = find_mut(existing, &qname, &arg_types, location)?;
    agg.comment = comment;
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build the "unsupported aggregate feature" rejection error.
fn unsupported_feature(feature: &str, location: &SourceLocation) -> ParseError {
    ParseError::Structural {
        location: location.clone(),
        message: format!(
            "unsupported aggregate feature `{feature}` — v0.4.1 supports ordinary aggregates only"
        ),
    }
}

/// Parse `args` into the ordered list of argument [`ColumnType`]s, rejecting
/// ordered-set aggregates (`args[1].ival != -1`).
fn parse_arg_types(
    stmt: &DefineStmt,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<Vec<ColumnType>, ParseError> {
    // `args[1]` is an Integer: -1 for ordinary, >= 0 (direct-arg count) for
    // ordered-set. Treat any non-(-1) value as ORDER BY → reject.
    if let Some(second) = stmt.args.get(1)
        && let Some(NodeEnum::Integer(i)) = second.node.as_ref()
        && i.ival != -1
    {
        return Err(unsupported_feature("ORDER BY", location));
    }

    // `args[0]` is either a List of FunctionParameter (one per arg type) or
    // `node: None` for the zero-argument `(*)` form.
    let Some(first) = stmt.args.first() else {
        return Ok(Vec::new());
    };
    let Some(node) = first.node.as_ref() else {
        // `(*)` — no argument types.
        return Ok(Vec::new());
    };
    let NodeEnum::List(list) = node else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "CREATE AGGREGATE {qname}: expected argument-type list, got {:?}",
                std::mem::discriminant(node)
            ),
        });
    };

    let mut types = Vec::with_capacity(list.items.len());
    for item in &list.items {
        let param = match item.node.as_ref() {
            Some(NodeEnum::FunctionParameter(p)) => p.as_ref(),
            other => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!(
                        "CREATE AGGREGATE {qname}: expected FunctionParameter in argument list, \
                         got {:?}",
                        other.map(std::mem::discriminant)
                    ),
                });
            }
        };
        types.push(function_parameter_type(param, qname, location)?);
    }
    Ok(types)
}

/// Extract the [`ColumnType`] from a `FunctionParameter.arg_type`.
fn function_parameter_type(
    param: &FunctionParameter,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<ColumnType, ParseError> {
    let type_name = param
        .arg_type
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("CREATE AGGREGATE {qname}: argument is missing a type"),
        })?;
    shared::type_name_to_column_type(type_name, location)
}

/// Decode a `DefElem.arg` that is a function-name `TypeName` (`sfunc`/`finalfunc`)
/// into a [`QualifiedName`]. The function name is resolved against the file's
/// default schema when unqualified.
fn funcname_from_defelem(
    de: &DefElem,
    option: &str,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let type_name = type_name_arg(de, option, qname, location)?;
    // The function name arrives as `TypeName.names` (a list of String nodes);
    // reuse the same name-list resolver used for the aggregate name itself, but
    // resolve unqualified names against the aggregate's own schema.
    let default = Some(qname.schema.clone());
    shared::qname_from_string_list(&type_name.names, default.as_ref(), location)
}

/// Decode a `DefElem.arg` that is a type `TypeName` (`stype`) into a [`ColumnType`].
fn typename_from_defelem(
    de: &DefElem,
    option: &str,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<ColumnType, ParseError> {
    let type_name = type_name_arg(de, option, qname, location)?;
    shared::type_name_to_column_type(type_name, location)
}

/// Decode a `DefElem.arg` that is a bare `String` (`initcond`).
fn string_from_defelem(
    de: &DefElem,
    option: &str,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<String, ParseError> {
    let arg = de
        .arg
        .as_ref()
        .and_then(|n| n.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("CREATE AGGREGATE {qname}: option `{option}` has no value"),
        })?;
    match arg {
        NodeEnum::String(s) => Ok(s.sval.clone()),
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "CREATE AGGREGATE {qname}: option `{option}` must be a string, got {:?}",
                std::mem::discriminant(other)
            ),
        }),
    }
}

/// Borrow a `DefElem.arg` as a `TypeName`, erroring if it is missing or some
/// other node kind.
fn type_name_arg<'a>(
    de: &'a DefElem,
    option: &str,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<&'a TypeName, ParseError> {
    let arg = de
        .arg
        .as_ref()
        .and_then(|n| n.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("CREATE AGGREGATE {qname}: option `{option}` has no value"),
        })?;
    match arg {
        NodeEnum::TypeName(tn) => Ok(tn),
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "CREATE AGGREGATE {qname}: option `{option}` has unexpected value kind {:?}",
                std::mem::discriminant(other)
            ),
        }),
    }
}

/// Borrow the `ObjectWithArgs` from an `ALTER`/`COMMENT` object reference.
fn object_with_args<'a>(
    object: Option<&'a pg_query::protobuf::Node>,
    location: &SourceLocation,
) -> Result<&'a ObjectWithArgs, ParseError> {
    match object.and_then(|o| o.node.as_ref()) {
        Some(NodeEnum::ObjectWithArgs(owa)) => Ok(owa),
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: "AGGREGATE reference missing ObjectWithArgs".into(),
        }),
    }
}

/// Decode an `ObjectWithArgs` (used by `ALTER`/`COMMENT`/`DROP`) into the
/// `(qname, arg_types)` identity. `objargs` is a list of `TypeName` nodes; an
/// empty list is the zero-argument `(*)` form.
fn identity_from_object(
    owa: &ObjectWithArgs,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<(QualifiedName, Vec<ColumnType>), ParseError> {
    let qname = shared::qname_from_string_list(&owa.objname, default_schema, location)?;
    let mut arg_types = Vec::with_capacity(owa.objargs.len());
    for node in &owa.objargs {
        match node.node.as_ref() {
            Some(NodeEnum::TypeName(tn)) => {
                arg_types.push(shared::type_name_to_column_type(tn, location)?);
            }
            other => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!(
                        "AGGREGATE {qname}: expected TypeName in argument list, got {:?}",
                        other.map(std::mem::discriminant)
                    ),
                });
            }
        }
    }
    Ok((qname, arg_types))
}

/// Find the aggregate matching `(qname, arg_types)` for ALTER/COMMENT, or error.
fn find_mut<'a>(
    existing: &'a mut [Aggregate],
    qname: &QualifiedName,
    arg_types: &[ColumnType],
    location: &SourceLocation,
) -> Result<&'a mut Aggregate, ParseError> {
    existing
        .iter_mut()
        .find(|a| a.qname == *qname && a.arg_types == arg_types)
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!(
                "aggregate {}({}) referenced before it is created in source",
                qname,
                render_arg_types(arg_types)
            ),
        })
}

/// Render an argument-type list for diagnostics (e.g. `Integer, Text`).
fn render_arg_types(arg_types: &[ColumnType]) -> String {
    arg_types
        .iter()
        .map(|t| format!("{t:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;

    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::parse::parse_directory;

    fn write(dir: &Path, rel: &str, contents: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    /// Parse SQL through the full `parse_directory` entry point (the same path
    /// production uses) and return the resulting canonical catalog.
    fn parse_source(sql: &str) -> Result<Catalog, ParseError> {
        let tmp = tempdir().expect("tempdir");
        write(tmp.path(), "schema.sql", sql);
        parse_directory(tmp.path(), &[])
    }

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    // The aggregate's referenced functions/types need not exist for the parser
    // itself; we declare them so `parse_directory`'s resolution pass is happy.
    const PRELUDE: &str = "CREATE SCHEMA app;\n\
         CREATE FUNCTION app.sf(bigint, integer) RETURNS bigint \
            AS $$ SELECT $1 $$ LANGUAGE sql;\n\
         CREATE FUNCTION app.ff(bigint) RETURNS bigint \
            AS $$ SELECT $1 $$ LANGUAGE sql;\n";

    #[test]
    fn create_simple() {
        let sql =
            format!("{PRELUDE}CREATE AGGREGATE app.s(integer) (SFUNC = app.sf, STYPE = bigint);");
        let cat = parse_source(&sql).expect("parses");
        assert_eq!(cat.aggregates.len(), 1);
        let a = &cat.aggregates[0];
        assert_eq!(a.qname.to_string(), "app.s");
        assert_eq!(a.arg_types, vec![ColumnType::Integer]);
        assert_eq!(a.state_type, ColumnType::BigInt);
        assert_eq!(a.sfunc.to_string(), "app.sf");
        assert!(a.finalfunc.is_none());
        assert!(a.initcond.is_none());
        assert!(a.owner.is_none());
        assert!(a.comment.is_none());
    }

    #[test]
    fn create_with_finalfunc() {
        let sql = format!(
            "{PRELUDE}CREATE AGGREGATE app.s(integer) \
             (SFUNC = app.sf, STYPE = bigint, FINALFUNC = app.ff);"
        );
        let cat = parse_source(&sql).expect("parses");
        let a = &cat.aggregates[0];
        assert_eq!(
            a.finalfunc.as_ref().map(ToString::to_string).as_deref(),
            Some("app.ff")
        );
    }

    #[test]
    fn create_with_initcond() {
        let sql = format!(
            "{PRELUDE}CREATE AGGREGATE app.s(integer) \
             (SFUNC = app.sf, STYPE = bigint, INITCOND = '0');"
        );
        let cat = parse_source(&sql).expect("parses");
        assert_eq!(cat.aggregates[0].initcond.as_deref(), Some("0"));
    }

    #[test]
    fn create_zero_arg_star() {
        let sql = format!("{PRELUDE}CREATE AGGREGATE app.s(*) (SFUNC = app.sf, STYPE = bigint);");
        let cat = parse_source(&sql).expect("parses");
        assert!(cat.aggregates[0].arg_types.is_empty());
    }

    #[test]
    fn alter_owner_applies() {
        let sql = format!(
            "{PRELUDE}CREATE AGGREGATE app.s(integer) (SFUNC = app.sf, STYPE = bigint);\n\
             ALTER AGGREGATE app.s(integer) OWNER TO app_owner;"
        );
        let cat = parse_source(&sql).expect("parses");
        assert_eq!(cat.aggregates[0].owner, Some(id("app_owner")));
    }

    #[test]
    fn comment_applies() {
        let sql = format!(
            "{PRELUDE}CREATE AGGREGATE app.s(integer) (SFUNC = app.sf, STYPE = bigint);\n\
             COMMENT ON AGGREGATE app.s(integer) IS 'x';"
        );
        let cat = parse_source(&sql).expect("parses");
        assert_eq!(cat.aggregates[0].comment.as_deref(), Some("x"));
    }

    #[test]
    fn rejects_combinefunc() {
        let sql = format!(
            "{PRELUDE}CREATE AGGREGATE app.s(integer) \
             (SFUNC = app.sf, STYPE = bigint, COMBINEFUNC = app.sf);"
        );
        let err = parse_source(&sql).expect_err("should reject");
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(
            msg.contains("combinefunc") && msg.contains("ordinary aggregates only"),
            "msg: {msg}"
        );
    }

    #[test]
    fn rejects_ordered_set() {
        let sql = format!(
            "{PRELUDE}CREATE AGGREGATE app.os(integer ORDER BY integer) \
             (SFUNC = app.sf, STYPE = bigint);"
        );
        let err = parse_source(&sql).expect_err("should reject");
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(
            msg.contains("ORDER BY") && msg.contains("ordinary aggregates only"),
            "msg: {msg}"
        );
    }

    #[test]
    fn rejects_drop_in_source() {
        let sql = format!(
            "{PRELUDE}CREATE AGGREGATE app.s(integer) (SFUNC = app.sf, STYPE = bigint);\n\
             DROP AGGREGATE app.s(integer);"
        );
        let err = parse_source(&sql).expect_err("should reject");
        assert!(matches!(err, ParseError::Structural { .. }), "got: {err:?}");
    }

    #[test]
    fn rejects_rename_in_source() {
        let sql = format!(
            "{PRELUDE}CREATE AGGREGATE app.s(integer) (SFUNC = app.sf, STYPE = bigint);\n\
             ALTER AGGREGATE app.s(integer) RENAME TO t;"
        );
        let err = parse_source(&sql).expect_err("should reject");
        assert!(matches!(err, ParseError::Structural { .. }), "got: {err:?}");
    }

    #[test]
    fn rejects_duplicate_identity() {
        let sql = format!(
            "{PRELUDE}CREATE AGGREGATE app.s(integer) (SFUNC = app.sf, STYPE = bigint);\n\
             CREATE AGGREGATE app.s(integer) (SFUNC = app.sf, STYPE = bigint);"
        );
        let err = parse_source(&sql).expect_err("should reject");
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("duplicate aggregate"), "msg: {msg}");
    }

    /// Two aggregates sharing a name but differing in argument types are
    /// distinct identities and must both be accepted.
    #[test]
    fn overloaded_aggregates_are_distinct() {
        let sql = format!(
            "{PRELUDE}\
             CREATE FUNCTION app.sf2(bigint, text) RETURNS bigint \
                AS $$ SELECT $1 $$ LANGUAGE sql;\n\
             CREATE AGGREGATE app.s(integer) (SFUNC = app.sf, STYPE = bigint);\n\
             CREATE AGGREGATE app.s(text) (SFUNC = app.sf2, STYPE = bigint);"
        );
        let cat = parse_source(&sql).expect("parses");
        assert_eq!(cat.aggregates.len(), 2);
    }

    /// `parse_create` builds against an accumulator directly (unit-level check
    /// independent of `parse_directory`'s resolution pass).
    #[test]
    fn parse_create_unit_appends() {
        let parsed =
            pg_query::parse("CREATE AGGREGATE app.s(integer) (SFUNC = app.sf, STYPE = bigint);")
                .unwrap();
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .unwrap();
        let NodeEnum::DefineStmt(stmt) = node else {
            panic!("expected DefineStmt");
        };
        let mut acc: Vec<Aggregate> = Vec::new();
        parse_create(&stmt, None, &loc(), &mut acc).expect("ok");
        assert_eq!(acc.len(), 1);
        assert_eq!(acc[0].sfunc.to_string(), "app.sf");
    }
}
