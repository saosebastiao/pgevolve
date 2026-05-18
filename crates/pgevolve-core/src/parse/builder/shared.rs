//! Helpers shared by all AST → IR builders.
//!
//! - Schema-qualified name resolution (with `-- @pgevolve schema=` defaulting).
//! - Type-name stringification (so [`crate::ir::column_type::ColumnType::parse_from_pg_type_string`]
//!   can decide the canonical form).
//! - Default-expression classification (literal vs. `nextval` vs. arbitrary expr).

use pg_query::NodeEnum;
use pg_query::protobuf::{AConst, RangeVar, TypeName, a_const};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column_type::ColumnType;
use crate::ir::default_expr::{DefaultExpr, LiteralValue};
use crate::parse::error::{ParseError, SourceLocation};
use crate::parse::normalize_expr;

/// Resolve a `RangeVar` into a [`QualifiedName`], filling in `default_schema`
/// when the source did not qualify the object.
///
/// Returns [`ParseError::UnqualifiedName`] if neither the source nor the
/// directive supply a schema.
pub fn resolve_qname(
    range_var: &RangeVar,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let name = ident(&range_var.relname, location)?;
    if !range_var.schemaname.is_empty() {
        let schema = ident(&range_var.schemaname, location)?;
        return Ok(QualifiedName::new(schema, name));
    }
    default_schema.map_or_else(
        || {
            Err(ParseError::UnqualifiedName {
                location: location.clone(),
            })
        },
        |s| Ok(QualifiedName::new(s.clone(), name)),
    )
}

/// Resolve a `repeated Node` type-name list (as produced by `CreateEnumStmt`,
/// `CreateDomainStmt`, `CompositeTypeStmt`, etc.) into a [`QualifiedName`].
///
/// PG encodes the name as a list of [`pg_query::NodeEnum::String`] nodes:
/// - one element → unqualified name (requires `default_schema`).
/// - two elements → `[schema, name]`.
///
/// Returns [`ParseError::UnqualifiedName`] if neither the source nor the
/// directive supply a schema.
///
/// This helper is shared by the enum, domain, and composite builders (T2–T4).
pub fn qname_from_string_list(
    nodes: &[pg_query::protobuf::Node],
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let strings: Vec<&str> = nodes
        .iter()
        .map(|n| match n.node.as_ref() {
            Some(NodeEnum::String(s)) => Ok(s.sval.as_str()),
            other => Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "expected String node in type-name list, got {:?}",
                    other.map(std::mem::discriminant),
                ),
            }),
        })
        .collect::<Result<Vec<_>, _>>()?;
    match strings.as_slice() {
        [name] => {
            let name_id = ident(name, location)?;
            let schema = default_schema
                .cloned()
                .ok_or_else(|| ParseError::UnqualifiedName {
                    location: location.clone(),
                })?;
            Ok(QualifiedName::new(schema, name_id))
        }
        [schema, name] => {
            let schema_id = ident(schema, location)?;
            let name_id = ident(name, location)?;
            Ok(QualifiedName::new(schema_id, name_id))
        }
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "unexpected type-name list length {} (expected 1 or 2 String nodes)",
                nodes.len()
            ),
        }),
    }
}

/// Build an unquoted [`Identifier`], wrapping [`crate::ir::IrError`] into a
/// source-located [`ParseError::Ir`].
pub fn ident(s: &str, location: &SourceLocation) -> Result<Identifier, ParseError> {
    Identifier::from_unquoted(s).map_err(|e| ParseError::Ir {
        location: location.clone(),
        source: crate::ir::IrError::InvalidIdentifier(e.to_string()),
    })
}

/// Render a `pg_query::TypeName` into a string `ColumnType::parse_from_pg_type_string`
/// can consume.
///
/// Strategy: take the *last* segment of `names` (Postgres prefixes types with
/// `pg_catalog.` internally; the parser is alias-aware), append a parenthesized
/// list of typmod arguments, append `[]` for each array dimension.
pub fn render_type_name_to_string(type_name: &TypeName) -> Option<String> {
    let last = type_name.names.last()?.node.as_ref()?;
    let NodeEnum::String(s) = last else {
        return None;
    };
    let bare = s.sval.clone();
    let mut out = bare;
    if !type_name.typmods.is_empty() {
        let mut args: Vec<String> = Vec::with_capacity(type_name.typmods.len());
        for n in &type_name.typmods {
            let Some(NodeEnum::AConst(c)) = &n.node else {
                return None;
            };
            args.push(literal_arg_to_string(c)?);
        }
        out = format!("{out}({})", args.join(","));
    }
    for _ in 0..type_name.array_bounds.len() {
        out.push_str("[]");
    }
    Some(out)
}

/// Convert an [`AConst`] used as a typmod argument to its canonical string form.
fn literal_arg_to_string(c: &AConst) -> Option<String> {
    match c.val.as_ref()? {
        a_const::Val::Ival(i) => Some(i.ival.to_string()),
        a_const::Val::Sval(s) => Some(s.sval.clone()),
        _ => None,
    }
}

/// Convert a `TypeName` into a [`ColumnType`], propagating parser errors.
///
/// When `TypeName.names` contains two String nodes and the first is not
/// `pg_catalog`, the type is schema-qualified by the user (e.g. `app.order_status`).
/// In that case we emit `ColumnType::UserDefined(QualifiedName)` so that the
/// AST resolution pass can validate that the referenced type is declared in source.
///
/// Unqualified single-segment names are handled by the usual string path; if they
/// don't match any built-in they fall through to `ColumnType::Other`, which is
/// intentional — unqualified user types must be resolved via `default_schema` at
/// domain/composite parse time rather than here.
pub fn type_name_to_column_type(
    type_name: &TypeName,
    location: &SourceLocation,
) -> Result<ColumnType, ParseError> {
    // Collect String nodes from names.
    let name_strings: Vec<&str> = type_name
        .names
        .iter()
        .filter_map(|n| {
            if let Some(NodeEnum::String(s)) = &n.node {
                Some(s.sval.as_str())
            } else {
                None
            }
        })
        .collect();

    // Two-segment, non-pg_catalog prefix → user-defined type reference.
    if let [schema, name] = name_strings.as_slice()
        && *schema != "pg_catalog"
    {
        let schema_id = ident(schema, location)?;
        let name_id = ident(name, location)?;
        return Ok(ColumnType::UserDefined(QualifiedName::new(
            schema_id, name_id,
        )));
    }

    let s = render_type_name_to_string(type_name).ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: "could not stringify type name".into(),
    })?;
    ColumnType::parse_from_pg_type_string(&s).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("invalid column type {s:?}: {e}"),
    })
}

/// Convert a column-default expression node into a [`DefaultExpr`].
///
/// Recognizes:
/// - Bare [`AConst`] integer/float/string/bool/null literals → [`DefaultExpr::Literal`].
/// - `nextval('seq')` and `nextval('schema.seq')` → [`DefaultExpr::Sequence`].
/// - Anything else → [`DefaultExpr::Expr`] via the normalizer.
///
/// `target_type` enables redundant-cast stripping in the `Expr` arm.
pub fn build_default_expr(
    node: &NodeEnum,
    target_type: Option<&ColumnType>,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<DefaultExpr, ParseError> {
    if let Some(lit) = literal_from_node(node) {
        return Ok(DefaultExpr::Literal(lit));
    }
    if let Some(seq) = nextval_target(node, default_schema, location)? {
        return Ok(DefaultExpr::Sequence(seq));
    }
    let normalized = normalize_expr::from_pg_node(node, target_type, location)?;
    Ok(DefaultExpr::Expr(normalized))
}

/// If `node` is a literal `AConst` (possibly wrapped in a `TypeCast` whose target
/// matches the column type), return the equivalent [`LiteralValue`].
fn literal_from_node(node: &NodeEnum) -> Option<LiteralValue> {
    let inner = unwrap_typecast(node);
    match inner {
        NodeEnum::AConst(c) => aconst_to_literal(c),
        _ => None,
    }
}

/// Strip a single layer of `TypeCast` wrapping, if any.
fn unwrap_typecast(node: &NodeEnum) -> &NodeEnum {
    if let NodeEnum::TypeCast(cast) = node
        && let Some(arg) = cast.arg.as_ref()
        && let Some(inner) = arg.node.as_ref()
    {
        return inner;
    }
    node
}

fn aconst_to_literal(c: &AConst) -> Option<LiteralValue> {
    if c.isnull {
        return Some(LiteralValue::Null);
    }
    match c.val.as_ref()? {
        a_const::Val::Ival(i) => Some(LiteralValue::Integer(i64::from(i.ival))),
        a_const::Val::Fval(f) => f.fval.parse::<f64>().ok().map(LiteralValue::Float),
        a_const::Val::Boolval(b) => Some(LiteralValue::Bool(b.boolval)),
        a_const::Val::Sval(s) => Some(LiteralValue::Text(s.sval.clone())),
        a_const::Val::Bsval(_) => None,
    }
}

/// If `node` is `nextval('seq')` (with optional `::regclass` cast on the arg),
/// return the referenced sequence's [`QualifiedName`].
fn nextval_target(
    node: &NodeEnum,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Option<QualifiedName>, ParseError> {
    let inner = unwrap_typecast(node);
    let NodeEnum::FuncCall(fc) = inner else {
        return Ok(None);
    };
    let func = match fc.funcname.last().and_then(|n| n.node.as_ref()) {
        Some(NodeEnum::String(s)) => s.sval.as_str(),
        _ => return Ok(None),
    };
    if !func.eq_ignore_ascii_case("nextval") {
        return Ok(None);
    }
    let arg = fc.args.first().and_then(|n| n.node.as_ref());
    let Some(arg_node) = arg else {
        return Ok(None);
    };
    // The argument to nextval is normally a string literal cast to regclass:
    // `nextval('app.seq1'::regclass)`. Strip the cast and look for an AConst Sval.
    let inner_arg = unwrap_typecast(arg_node);
    let NodeEnum::AConst(c) = inner_arg else {
        return Ok(None);
    };
    let Some(a_const::Val::Sval(s)) = c.val.as_ref() else {
        return Ok(None);
    };
    Ok(Some(parse_qualified_seq_name(
        &s.sval,
        default_schema,
        location,
    )?))
}

/// Parse a possibly-qualified sequence reference like `schema.seq` or `seq`.
fn parse_qualified_seq_name(
    s: &str,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let parts: Vec<&str> = s.split('.').collect();
    match parts.len() {
        1 => {
            let name = ident(parts[0], location)?;
            let schema = default_schema
                .cloned()
                .ok_or_else(|| ParseError::UnqualifiedName {
                    location: location.clone(),
                })?;
            Ok(QualifiedName::new(schema, name))
        }
        2 => {
            let schema = ident(parts[0], location)?;
            let name = ident(parts[1], location)?;
            Ok(QualifiedName::new(schema, name))
        }
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: format!("unsupported qualified sequence reference {s:?}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn parse_first(sql: &str) -> NodeEnum {
        let parsed = pg_query::parse(sql).expect("parses");
        parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("at least one statement")
    }

    fn parse_select_expr(expr: &str) -> NodeEnum {
        let n = parse_first(&format!("SELECT {expr};"));
        let NodeEnum::SelectStmt(s) = n else { panic!() };
        let target = s.target_list.into_iter().next().unwrap().node.unwrap();
        let NodeEnum::ResTarget(rt) = target else {
            panic!()
        };
        rt.val.unwrap().node.unwrap()
    }

    #[test]
    fn literal_integer_default() {
        let n = parse_select_expr("42");
        let d = build_default_expr(&n, Some(&ColumnType::Integer), None, &loc()).unwrap();
        assert!(matches!(d, DefaultExpr::Literal(LiteralValue::Integer(42))));
    }

    #[test]
    fn literal_text_default() {
        let n = parse_select_expr("'hello'");
        let d = build_default_expr(&n, Some(&ColumnType::Text), None, &loc()).unwrap();
        assert!(matches!(d, DefaultExpr::Literal(LiteralValue::Text(s)) if s == "hello"));
    }

    #[test]
    fn null_default() {
        let n = parse_select_expr("NULL");
        let d = build_default_expr(&n, None, None, &loc()).unwrap();
        assert!(matches!(d, DefaultExpr::Literal(LiteralValue::Null)));
    }

    #[test]
    fn bool_default() {
        let n = parse_select_expr("true");
        let d = build_default_expr(&n, Some(&ColumnType::Boolean), None, &loc()).unwrap();
        // Booleans in pg_query may parse as a function call `'t'::boolean` form
        // — accept either Bool literal or Expr containing "true".
        match d {
            DefaultExpr::Literal(LiteralValue::Bool(true)) => {}
            DefaultExpr::Expr(e) => assert!(e.canonical_text.contains("true")),
            other => panic!("unexpected default: {other:?}"),
        }
    }

    #[test]
    fn nextval_qualified_default() {
        let n = parse_select_expr("nextval('app.seq1'::regclass)");
        let d = build_default_expr(&n, None, None, &loc()).unwrap();
        match d {
            DefaultExpr::Sequence(q) => assert_eq!(q.to_string(), "app.seq1"),
            other => panic!("expected Sequence, got {other:?}"),
        }
    }

    #[test]
    fn nextval_unqualified_uses_default_schema() {
        let n = parse_select_expr("nextval('seq1'::regclass)");
        let app = Identifier::from_unquoted("app").unwrap();
        let d = build_default_expr(&n, None, Some(&app), &loc()).unwrap();
        match d {
            DefaultExpr::Sequence(q) => assert_eq!(q.to_string(), "app.seq1"),
            other => panic!("expected Sequence, got {other:?}"),
        }
    }

    #[test]
    fn func_call_other_than_nextval_is_expr() {
        let n = parse_select_expr("now()");
        let d = build_default_expr(&n, None, None, &loc()).unwrap();
        assert!(matches!(d, DefaultExpr::Expr(_)));
    }

    #[test]
    fn cast_to_target_strips_in_expr_arm() {
        // `'a' || 'b'` is an expression — cast stripping has nothing to strip
        // here. This test just checks the Expr arm runs.
        let n = parse_select_expr("'a' || 'b'");
        let d = build_default_expr(&n, Some(&ColumnType::Text), None, &loc()).unwrap();
        assert!(matches!(d, DefaultExpr::Expr(_)));
    }

    #[test]
    fn type_name_renders_with_typmod() {
        // Parse a CREATE TABLE to get a real TypeName.
        let stmt = parse_first("CREATE TABLE t (c varchar(50));");
        let NodeEnum::CreateStmt(create) = stmt else {
            panic!()
        };
        let elt = create.table_elts.into_iter().next().unwrap();
        let NodeEnum::ColumnDef(col) = elt.node.unwrap() else {
            panic!()
        };
        let tn = col.type_name.unwrap();
        let s = render_type_name_to_string(&tn).unwrap();
        let ct = ColumnType::parse_from_pg_type_string(&s).unwrap();
        assert_eq!(ct, ColumnType::Varchar { len: Some(50) });
    }
}
