//! Helpers shared by all AST → IR builders.
//!
//! - Schema-qualified name resolution (with `-- @pgevolve schema=` defaulting).
//! - Type-name stringification (so [`crate::ir::column_type::ColumnType::parse_from_pg_type_string`]
//!   can decide the canonical form).
//! - Default-expression classification (literal vs. `nextval` vs. arbitrary expr).

use pg_query::NodeEnum;
use pg_query::protobuf::{AConst, ColumnRef, DefElem, Node, RangeVar, TypeName, a_const};

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

/// Extract the string value of a bare `String` [`Node`].
///
/// Returns `Some(sval)` when `node` is a [`NodeEnum::String`], otherwise `None`.
/// Used where `pg_query` encodes a list element (publication name, column name)
/// directly as a `String` node rather than wrapping it in a `DefElem`.
pub fn node_string_value(node: &Node) -> Option<String> {
    match node.node.as_ref()? {
        NodeEnum::String(s) => Some(s.sval.clone()),
        _ => None,
    }
}

/// Extract a string value from a `DefElem.arg`.
///
/// Handles the encodings `pg_query` uses for scalar option values:
/// - [`NodeEnum::String`] — bare identifier or quoted string.
/// - [`NodeEnum::Integer`] / [`NodeEnum::Float`] — numeric option values,
///   stringified to their textual form.
/// - [`NodeEnum::List`] — dollar-quoted bodies are wrapped in a `List` whose
///   first item is the body `String` (the optional second item is the
///   dollar-quote tag); this returns that first `String`'s value.
///
/// Returns `None` for an absent arg or any other node kind.
pub fn def_elem_string(de: &DefElem) -> Option<String> {
    let arg = de.arg.as_ref()?;
    match arg.node.as_ref()? {
        NodeEnum::String(s) => Some(s.sval.clone()),
        NodeEnum::Integer(i) => Some(i.ival.to_string()),
        NodeEnum::Float(f) => Some(f.fval.clone()),
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
///
/// **Special case — `interval`**: `pg_query` encodes interval typmods as
/// `[fields_bitmask, precision?]` where the fields bitmask is the PG
/// `INTERVAL_MASK` value from `datetime.h`.  Blindly joining them produces
/// `"interval(32767,6)"` which `parse_canonical` cannot round-trip back to a
/// typed `ColumnType::Interval`.  We decode the bitmask here and emit the same
/// canonical string that `pg_catalog.format_type` produces so both paths
/// converge. (issue #41)
pub fn render_type_name_to_string(type_name: &TypeName) -> Option<String> {
    let last = type_name.names.last()?.node.as_ref()?;
    let NodeEnum::String(s) = last else {
        return None;
    };
    let bare = s.sval.as_str();

    // Interval gets special typmod handling — see module doc above.
    let mut out = if bare == "interval" && !type_name.typmods.is_empty() {
        render_interval_type_name(type_name)?
    } else {
        let mut o = bare.to_string();
        if !type_name.typmods.is_empty() {
            let mut args: Vec<String> = Vec::with_capacity(type_name.typmods.len());
            for n in &type_name.typmods {
                args.push(typmod_arg_to_string(n.node.as_ref()?)?);
            }
            o = format!("{o}({})", args.join(","));
        }
        o
    };

    for _ in 0..type_name.array_bounds.len() {
        out.push_str("[]");
    }
    Some(out)
}

/// Decode the `pg_query` interval typmod encoding into a canonical string that
/// `ColumnType::parse_from_pg_type_string` can round-trip.
///
/// `pg_query` represents interval typmods as:
/// - One arg: `[fields_bitmask]` — fields restriction only (e.g. `interval hour to minute`).
/// - Two args: `[fields_bitmask, precision]` — precision (and maybe fields).
///
/// The fields bitmask is the `INTERVAL_MASK` value from PG's `datetime.h`.
/// `INTERVAL_FULL_RANGE = 0x7FFF = 32767` means "no restriction".
///
/// Returns `None` if the typmod nodes cannot be decoded; the caller will then
/// produce `None` overall, which surfaces as a `Structural` parse error — no
/// worse than before this fix.
fn render_interval_type_name(type_name: &TypeName) -> Option<String> {
    // Extract integer values from the AConst typmod nodes.
    let mut int_args: Vec<i32> = Vec::with_capacity(2);
    for n in &type_name.typmods {
        match n.node.as_ref()? {
            NodeEnum::AConst(c) => match c.val.as_ref()? {
                a_const::Val::Ival(i) => int_args.push(i.ival),
                _ => return None, // unexpected non-integer typmod for interval
            },
            _ => return None, // unexpected node kind (e.g. ColumnRef) for interval
        }
    }

    let (fields_mask, precision): (i32, Option<u8>) = match int_args.as_slice() {
        [mask] => (*mask, None),
        [mask, prec] => {
            let p = u8::try_from(*prec).ok()?;
            (*mask, Some(p))
        }
        _ => return None,
    };

    let fields_str = interval_fields_from_mask(fields_mask);

    // Build canonical string matching `format_type` output.
    Some(match (fields_str, precision) {
        (None, None) => "interval".to_string(),
        (None, Some(p)) => format!("interval({p})"),
        (Some(f), None) => format!("interval {f}"),
        (Some(f), Some(p)) => format!("interval {f}({p})"),
    })
}

/// Map a PG `INTERVAL_MASK` bitmask to the canonical lowercase fields qualifier
/// that `pg_catalog.format_type` emits, or `None` for the full-range sentinel
/// (`INTERVAL_FULL_RANGE = 32767 = 0x7FFF`).
///
/// Bit positions from PG `src/include/utils/datetime.h`:
/// `INTERVAL_MASK(b) = 1 << b`, with YEAR=2, MONTH=1, DAY=3, HOUR=10, MINUTE=11,
/// SECOND=12.
///
/// Unrecognized bitmasks fall back to `None` so behavior is no worse than
/// the pre-fix state (type degrades to `Other` rather than panicking).
const fn interval_fields_from_mask(mask: i32) -> Option<&'static str> {
    // INTERVAL_FULL_RANGE — no fields restriction.
    if mask == 0x7FFF {
        return None;
    }
    // Individual fields and all recognized combinations, ordered to match the
    // canonical `format_type` output.
    match mask {
        // Single fields: YEAR=1<<2, MONTH=1<<1, DAY=1<<3, HOUR=1<<10, MINUTE=1<<11, SECOND=1<<12
        4 => Some("year"),
        2 => Some("month"),
        8 => Some("day"),
        1024 => Some("hour"),
        2048 => Some("minute"),
        4096 => Some("second"),
        // Ranges
        6 => Some("year to month"),       // YEAR|MONTH = 4|2
        1032 => Some("day to hour"),      // DAY|HOUR = 8|1024
        3080 => Some("day to minute"),    // DAY|HOUR|MINUTE = 8|1024|2048
        7176 => Some("day to second"),    // DAY|HOUR|MINUTE|SECOND = 8|1024|2048|4096
        3072 => Some("hour to minute"),   // HOUR|MINUTE = 1024|2048
        7168 => Some("hour to second"),   // HOUR|MINUTE|SECOND = 1024|2048|4096
        6144 => Some("minute to second"), // MINUTE|SECOND = 2048|4096
        // Unrecognized — fall back; no worse than pre-fix behaviour.
        _ => None,
    }
}

/// Stringify a single type-modifier argument node.
///
/// Type modifiers are an `expr_list`, so an argument is either a literal
/// ([`AConst`]) or a bareword. The bareword form is what `PostGIS` uses for the
/// subtype in `geometry(Point,4326)` — Postgres parses it as a single-field
/// [`ColumnRef`], not a constant.
fn typmod_arg_to_string(node: &NodeEnum) -> Option<String> {
    match node {
        NodeEnum::AConst(c) => literal_arg_to_string(c),
        NodeEnum::ColumnRef(cref) => columnref_ident(cref),
        _ => None,
    }
}

/// Convert an [`AConst`] used as a typmod argument to its canonical string form.
fn literal_arg_to_string(c: &AConst) -> Option<String> {
    match c.val.as_ref()? {
        a_const::Val::Ival(i) => Some(i.ival.to_string()),
        a_const::Val::Sval(s) => Some(s.sval.clone()),
        _ => None,
    }
}

/// Extract the identifier text of a single-field bareword [`ColumnRef`].
///
/// A multi-field ref (`a.b`) is not a valid type-modifier shape, so it is
/// rejected (returns `None`).
fn columnref_ident(cref: &ColumnRef) -> Option<String> {
    let [field] = cref.fields.as_slice() else {
        return None;
    };
    match field.node.as_ref()? {
        NodeEnum::String(s) => Some(s.sval.clone()),
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
    // Collect String nodes from names; fail fast on unexpected kinds.
    let name_strings: Vec<&str> = type_name
        .names
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

    // Two-segment, non-pg_catalog prefix → user-defined type reference.
    //
    // Special case: PostGIS `geometry`/`geography` with typmods (issue #42).
    // `public.geometry(Point,4326)` must become `Other { raw: "public.geometry(point,4326)" }`
    // so it converges with the catalog path, which emits the same schema-qualified string.
    // For all other schema-qualified types — including non-geo UDTs — keep the
    // existing `UserDefined` return (no behavior change).
    if let [schema, name] = name_strings.as_slice()
        && *schema != "pg_catalog"
    {
        let name_lower = name.to_ascii_lowercase();
        if (name_lower == "geometry" || name_lower == "geography") && !type_name.typmods.is_empty()
        {
            // Build `schema.name(arg,arg,…)` and route through the canonical
            // parse path so casing is normalised (subtype barewords are already
            // lowercased by pg_query) and `Other` equality holds.
            let mut args: Vec<String> = Vec::with_capacity(type_name.typmods.len());
            for n in &type_name.typmods {
                let arg = typmod_arg_to_string(n.node.as_ref().ok_or_else(|| {
                    ParseError::Structural {
                        location: location.clone(),
                        message: "missing node in typmod list".into(),
                    }
                })?)
                .ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: "could not stringify typmod argument".into(),
                })?;
                args.push(arg);
            }
            let s = format!("{schema}.{name_lower}({})", args.join(","));
            return ColumnType::parse_from_pg_type_string(&s).map_err(|e| ParseError::Structural {
                location: location.clone(),
                message: format!("invalid column type {s:?}: {e}"),
            });
        }

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

    /// Extract the first column's `TypeName` from a one-column `CREATE TABLE`.
    fn first_column_type_name(sql: &str) -> TypeName {
        let NodeEnum::CreateStmt(create) = parse_first(sql) else {
            panic!("expected CREATE TABLE")
        };
        let elt = create.table_elts.into_iter().next().unwrap();
        let NodeEnum::ColumnDef(col) = elt.node.unwrap() else {
            panic!("expected ColumnDef")
        };
        col.type_name.unwrap()
    }

    #[test]
    fn parameterized_postgis_types_parse() {
        // PostGIS parameterized types put the subtype (Point / MultiPolygon / …)
        // in the typmod list as a *bareword*, which pg_query parses as a ColumnRef
        // (type modifiers are an expr_list), not an AConst. The renderer must
        // stringify it; pg_query lowercases the bareword, so the canonical raw is
        // lowercase. (issue #40)
        let cases = [
            ("geometry(Point,4326)", "geometry(point,4326)"),
            (
                "geography(MultiPolygon,4326)",
                "geography(multipolygon,4326)",
            ),
            ("geometry(geometry,4326)", "geometry(geometry,4326)"),
        ];
        for (decl, expected_raw) in cases {
            let tn = first_column_type_name(&format!("CREATE TABLE t (c {decl});"));
            let ct = type_name_to_column_type(&tn, &loc())
                .unwrap_or_else(|e| panic!("{decl} should parse, got {e:?}"));
            assert!(
                matches!(&ct, ColumnType::Other { raw } if raw == expected_raw),
                "{decl} -> {ct:?}"
            );
        }
    }

    /// Convergence tests for schema-qualified `PostGIS` types (issue #42).
    ///
    /// `public.geometry(Point,4326)` must converge with the catalog path that
    /// emits `Other { raw: "public.geometry(point,4326)" }` — i.e. the subtype
    /// is lowercased and the schema prefix is preserved.
    #[test]
    fn schema_qualified_postgis_types_converge() {
        // AST (source) path: type_name_to_column_type must NOT return UserDefined.
        let cases = [
            ("public.geometry(Point,4326)", "public.geometry(point,4326)"),
            (
                "public.geography(Point,4326)",
                "public.geography(point,4326)",
            ),
            (
                "myschema.geometry(MultiPolygon,4326)",
                "myschema.geometry(multipolygon,4326)",
            ),
        ];
        for (decl, expected_raw) in cases {
            let tn = first_column_type_name(&format!("CREATE TABLE t (g {decl});"));
            let ct = type_name_to_column_type(&tn, &loc())
                .unwrap_or_else(|e| panic!("{decl} should parse, got {e:?}"));
            assert!(
                matches!(&ct, ColumnType::Other { raw } if raw == expected_raw),
                "{decl} — expected Other{{raw:{expected_raw:?}}}, got {ct:?}"
            );

            // Catalog path: parse_from_pg_type_string with TitleCase input must
            // produce the same Other so source == catalog (the actual convergence).
            let catalog_input = decl; // e.g. "public.geometry(Point,4326)"
            let from_catalog = ColumnType::parse_from_pg_type_string(catalog_input).unwrap();
            assert_eq!(
                ct, from_catalog,
                "{decl}: source path {ct:?} != catalog path {from_catalog:?}"
            );
        }
    }

    /// No-typmod schema-qualified types must remain `UserDefined` (no regression).
    #[test]
    fn schema_qualified_no_typmod_stays_user_defined() {
        // `public.geometry` without typmods → UserDefined.
        let tn = first_column_type_name("CREATE TABLE t (g public.geometry);");
        let ct = type_name_to_column_type(&tn, &loc()).expect("should parse");
        assert!(
            matches!(&ct, ColumnType::UserDefined(q) if q.to_string() == "public.geometry"),
            "expected UserDefined(public.geometry), got {ct:?}"
        );

        // `public.mytype` → UserDefined.
        let tn2 = first_column_type_name("CREATE TABLE t (x public.mytype);");
        let ct2 = type_name_to_column_type(&tn2, &loc()).expect("should parse");
        assert!(
            matches!(&ct2, ColumnType::UserDefined(q) if q.to_string() == "public.mytype"),
            "expected UserDefined(public.mytype), got {ct2:?}"
        );
    }

    #[test]
    fn heterogeneous_typmod_args_render() {
        // A type modifier list may legally mix string/int literals and barewords.
        // The renderer dispatches per node kind rather than assuming AConst.
        let tn = first_column_type_name("CREATE TABLE t (c mytype('foo',1,bar));");
        let ct = type_name_to_column_type(&tn, &loc()).expect("mixed typmods should parse");
        assert!(
            matches!(&ct, ColumnType::Other { raw } if raw == "mytype(foo,1,bar)"),
            "got {ct:?}"
        );
    }

    /// Convergence tests (issue #41): the AST source path must produce the same
    /// `ColumnType::Interval` that the catalog path (`format_type`) emits.
    ///
    /// Before the fix, `interval(6)` → typmods `[AConst(32767), AConst(6)]` →
    /// `render_type_name_to_string` produced `"interval(32767,6)"` → `parse_canonical`
    /// failed to parse the precision → fell through to `ColumnType::Other`.
    #[test]
    fn interval_precision_source_path_convergence() {
        // `interval(6)` — precision-only form.
        let tn = first_column_type_name("CREATE TABLE t (c interval(6));");
        let ct = type_name_to_column_type(&tn, &loc())
            .unwrap_or_else(|e| panic!("interval(6) should parse, got {e:?}"));
        assert_eq!(
            ct,
            ColumnType::Interval {
                fields: None,
                precision: Some(6),
            },
            "interval(6) source path should yield Interval{{fields:None, precision:Some(6)}}, got {ct:?}"
        );
    }

    #[test]
    fn interval_fields_source_path_convergence() {
        // `interval hour to minute` — fields-only form.
        let tn = first_column_type_name("CREATE TABLE t (c interval hour to minute);");
        let ct = type_name_to_column_type(&tn, &loc())
            .unwrap_or_else(|e| panic!("interval hour to minute should parse, got {e:?}"));
        assert_eq!(
            ct,
            ColumnType::Interval {
                fields: Some("hour to minute".to_string()),
                precision: None,
            },
            "interval hour to minute source path should yield Interval{{fields:Some(\"hour to minute\"), precision:None}}, got {ct:?}"
        );
    }

    #[test]
    fn interval_fields_and_precision_converges_from_ast() {
        // Combined `interval <fields>(p)` form: typmods `[7176, 3]` (DAY|HOUR|
        // MINUTE|SECOND = 7176, precision 3) → render "interval day to second(3)"
        // → parse. Must converge to a typed Interval, not Other.
        let tn = first_column_type_name("CREATE TABLE t (c interval day to second(3));");
        let ct = type_name_to_column_type(&tn, &loc())
            .unwrap_or_else(|e| panic!("interval day to second(3) should parse, got {e:?}"));
        assert_eq!(
            ct,
            ColumnType::Interval {
                fields: Some("day to second".to_string()),
                precision: Some(3),
            },
            "interval day to second(3) source path should yield typed Interval, got {ct:?}"
        );
    }

    #[test]
    fn interval_bare_source_path() {
        // `interval` with no modifiers — must not regress.
        let tn = first_column_type_name("CREATE TABLE t (c interval);");
        let ct = type_name_to_column_type(&tn, &loc())
            .unwrap_or_else(|e| panic!("interval should parse, got {e:?}"));
        assert_eq!(
            ct,
            ColumnType::Interval {
                fields: None,
                precision: None,
            },
        );
    }
}
