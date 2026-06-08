//! Parser for `CREATE CAST` and `COMMENT ON CAST`.
//!
//! `pg_query` 6.x encodes `CREATE CAST` as a [`CreateCastStmt`]:
//! - `sourcetype` / `targettype`: [`TypeName`] nodes for the source and target types.
//! - `func`: `Option<ObjectWithArgs>` — present for `WITH FUNCTION`, absent for
//!   `WITHOUT FUNCTION` (Binary) or `WITH INOUT` (distinguished by `inout: true`).
//! - `context`: a [`CoercionContext`] integer: `CoercionImplicit` (1), `CoercionAssignment`
//!   (2), `CoercionPlpgsql` (3 — rejected), `CoercionExplicit` (4).
//! - `inout`: `true` when the source has `WITH INOUT`.
//!
//! Cast identity is `(source, target)` — unlike aggregates, casts are not
//! overloadable; two `CREATE CAST` statements with the same `(source, target)`
//! pair are rejected as duplicates.
//!
//! `COMMENT ON CAST (src AS tgt) IS '…'` arrives as a `CommentStmt` with
//! `objtype = ObjectCast` and `object` = a `List` of two [`TypeName`] nodes
//! (source first, target second). This is handled by `apply_comment`.
//!
//! `DROP CAST` in source is rejected (drops are produced by the diff).
//!
//! ## `TypeName` → `QualifiedName` convention
//!
//! A cast's source and target types are always schema-qualified by `pg_query` when
//! the user writes a SQL-keyword type alias (e.g. `integer` → `pg_catalog.int4`),
//! but unqualified user-type names like `text` arrive as a single String node.
//! To ensure round-trip consistency with the future reader, we apply the
//! following rule:
//! - Two String nodes in `TypeName.names` → use them directly as `(schema, name)`.
//! - One String node → prefix with `pg_catalog` (the implicit catalog for all
//!   built-in types; user types must always be written as `schema.typename` in
//!   `CREATE CAST`).
//!
//! This is the same resolution the reader will apply: it reads `pg_catalog.text`,
//! `pg_catalog.int4`, etc.

use pg_query::NodeEnum;
use pg_query::protobuf::{CoercionContext, CommentStmt, CreateCastStmt, TypeName};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::cast::{Cast, CastContext, CastMethod};
use crate::ir::column_type::ColumnType;
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Build a [`Cast`] from a `CREATE CAST` AST node and append it to the
/// accumulator.
///
/// Rejects `CoercionPlpgsql` context, duplicate `(source, target)` identities,
/// and missing source/target type nodes.
pub(crate) fn parse_create(
    stmt: &CreateCastStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    existing: &mut Vec<Cast>,
) -> Result<(), ParseError> {
    let source_tn = stmt
        .sourcetype
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "CREATE CAST: missing source type".into(),
        })?;
    let target_tn = stmt
        .targettype
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "CREATE CAST: missing target type".into(),
        })?;

    let source = type_name_to_qname(source_tn, default_schema, location)?;
    let target = type_name_to_qname(target_tn, default_schema, location)?;

    let context = coercion_context(stmt.context, location)?;

    let method = if let Some(func) = stmt.func.as_ref() {
        let name = shared::qname_from_string_list(&func.objname, default_schema, location)?;
        let mut arg_types: Vec<ColumnType> = Vec::with_capacity(func.objargs.len());
        for node in &func.objargs {
            match node.node.as_ref() {
                Some(NodeEnum::TypeName(tn)) => {
                    arg_types.push(shared::type_name_to_column_type(tn, location)?);
                }
                other => {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!(
                            "CREATE CAST ({source} AS {target}): expected TypeName in function \
                             argument list, got {:?}",
                            other.map(std::mem::discriminant)
                        ),
                    });
                }
            }
        }
        CastMethod::Function { name, arg_types }
    } else if stmt.inout {
        CastMethod::Inout
    } else {
        CastMethod::Binary
    };

    // Reject duplicate (source, target) identity.
    if existing
        .iter()
        .any(|c| c.source == source && c.target == target)
    {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("duplicate cast ({source} AS {target})"),
        });
    }

    existing.push(Cast {
        source,
        target,
        method,
        context,
        comment: None,
    });
    Ok(())
}

/// Apply a `COMMENT ON CAST (src AS tgt) IS '…'` against the accumulator.
///
/// `pg_query` encodes the cast reference as `object = List[TypeName(src), TypeName(tgt)]`.
pub(crate) fn apply_comment(
    stmt: &CommentStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    existing: &mut [Cast],
) -> Result<(), ParseError> {
    let (source, target) = identity_from_comment(stmt, default_schema, location)?;
    let comment = if stmt.comment.is_empty() {
        None
    } else {
        Some(stmt.comment.clone())
    };
    let cast = find_mut(existing, &source, &target, location)?;
    cast.comment = comment;
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract `(source, target)` `QualifiedName`s from a `COMMENT ON CAST` object
/// reference. The object is a `List` of exactly two `TypeName` nodes.
fn identity_from_comment(
    stmt: &CommentStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<(QualifiedName, QualifiedName), ParseError> {
    let obj = stmt
        .object
        .as_ref()
        .and_then(|o| o.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "COMMENT ON CAST: missing object reference".into(),
        })?;
    let NodeEnum::List(list) = obj else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "COMMENT ON CAST: expected a List node, got {:?}",
                std::mem::discriminant(obj)
            ),
        });
    };
    if list.items.len() != 2 {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "COMMENT ON CAST: expected exactly 2 TypeName items, got {}",
                list.items.len()
            ),
        });
    }
    let src_tn = extract_type_name(&list.items[0], location)?;
    let tgt_tn = extract_type_name(&list.items[1], location)?;
    let source = type_name_to_qname(src_tn, default_schema, location)?;
    let target = type_name_to_qname(tgt_tn, default_schema, location)?;
    Ok((source, target))
}

/// Borrow a `TypeName` from a list item node.
fn extract_type_name<'a>(
    node: &'a pg_query::protobuf::Node,
    location: &SourceLocation,
) -> Result<&'a TypeName, ParseError> {
    match node.node.as_ref() {
        Some(NodeEnum::TypeName(tn)) => Ok(tn),
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "COMMENT ON CAST: expected TypeName in cast identity list, got {:?}",
                other.map(std::mem::discriminant)
            ),
        }),
    }
}

/// Convert a cast [`TypeName`] to a [`QualifiedName`].
///
/// - Two String nodes → `(schema, name)` directly.
/// - One String node  → `(pg_catalog, name)` (unqualified built-in type).
///
/// This matches the representation the reader (Task 7) will produce: both sides
/// of a built-in cast like `(text AS integer)` will be stored as
/// `pg_catalog.text` / `pg_catalog.int4`.
fn type_name_to_qname(
    type_name: &TypeName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let strings: Vec<&str> = type_name
        .names
        .iter()
        .map(|n| match n.node.as_ref() {
            Some(NodeEnum::String(s)) => Ok(s.sval.as_str()),
            other => Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "CREATE CAST: expected String node in type name, got {:?}",
                    other.map(std::mem::discriminant)
                ),
            }),
        })
        .collect::<Result<Vec<_>, _>>()?;

    match strings.as_slice() {
        [schema, name] => {
            let schema_id = shared::ident(schema, location)?;
            let name_id = shared::ident(name, location)?;
            Ok(QualifiedName::new(schema_id, name_id))
        }
        [name] => {
            // Single-segment: check if there is a user-supplied default schema.
            // If so, use that (user-defined type written unqualified is unusual
            // but accepted if a `-- @pgevolve schema=` directive is present).
            // Otherwise, fall back to `pg_catalog` which is the implicit home
            // for all built-in type aliases (e.g. `text`, `bool`).
            let name_id = shared::ident(name, location)?;
            let schema_id = default_schema.cloned().unwrap_or_else(|| {
                Identifier::from_unquoted("pg_catalog").expect("static identifier")
            });
            Ok(QualifiedName::new(schema_id, name_id))
        }
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "CREATE CAST: type name has {} components (expected 1 or 2)",
                strings.len()
            ),
        }),
    }
}

/// Map a raw [`CoercionContext`] integer to a [`CastContext`], rejecting
/// `CoercionPlpgsql` which cannot appear in a `CREATE CAST` source statement.
fn coercion_context(raw: i32, location: &SourceLocation) -> Result<CastContext, ParseError> {
    match CoercionContext::try_from(raw) {
        Ok(CoercionContext::CoercionImplicit) => Ok(CastContext::Implicit),
        Ok(CoercionContext::CoercionAssignment) => Ok(CastContext::Assignment),
        Ok(CoercionContext::CoercionExplicit) => Ok(CastContext::Explicit),
        Ok(CoercionContext::CoercionPlpgsql) => Err(ParseError::Structural {
            location: location.clone(),
            message: "CoercionPlpgsql context is not valid in a CREATE CAST source statement"
                .into(),
        }),
        Ok(CoercionContext::Undefined) | Err(_) => Err(ParseError::Structural {
            location: location.clone(),
            message: format!("CREATE CAST: unrecognised coercion context value {raw}"),
        }),
    }
}

/// Find the cast matching `(source, target)` for COMMENT, or error.
fn find_mut<'a>(
    existing: &'a mut [Cast],
    source: &QualifiedName,
    target: &QualifiedName,
    location: &SourceLocation,
) -> Result<&'a mut Cast, ParseError> {
    existing
        .iter_mut()
        .find(|c| c.source == *source && c.target == *target)
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!(
                "cast ({source} AS {target}) referenced before it is created in source"
            ),
        })
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

    /// Parse SQL through the full `parse_directory` entry point and return the
    /// resulting canonical catalog.
    fn parse_source(sql: &str) -> Result<Catalog, ParseError> {
        let tmp = tempdir().expect("tempdir");
        write(tmp.path(), "schema.sql", sql);
        parse_directory(tmp.path(), &[])
    }

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    /// Minimal prelude that satisfies `parse_directory`'s resolution pass.
    /// Declares two user-defined types so we can write casts on them.
    const PRELUDE: &str = "\
        CREATE SCHEMA app;\n\
        CREATE TYPE app.a AS ENUM ('x');\n\
        CREATE TYPE app.b AS ENUM ('y');\n\
        CREATE FUNCTION app.conv(app.a) RETURNS app.b \
            AS $$ SELECT 'y'::app.b $$ LANGUAGE sql;\n";

    // ── WITH FUNCTION variants ───────────────────────────────────────────────

    #[test]
    fn create_with_function_explicit() {
        let sql = format!("{PRELUDE}CREATE CAST (app.a AS app.b) WITH FUNCTION app.conv(app.a);");
        let cat = parse_source(&sql).expect("parses");
        assert_eq!(cat.casts.len(), 1);
        let c = &cat.casts[0];
        assert_eq!(c.source.to_string(), "app.a");
        assert_eq!(c.target.to_string(), "app.b");
        assert_eq!(c.context, CastContext::Explicit);
        assert!(matches!(c.method, CastMethod::Function { .. }));
        if let CastMethod::Function { name, arg_types } = &c.method {
            assert_eq!(name.to_string(), "app.conv");
            assert_eq!(arg_types.len(), 1);
        }
        assert!(c.comment.is_none());
    }

    #[test]
    fn create_with_function_assignment() {
        let sql = format!(
            "{PRELUDE}CREATE CAST (app.a AS app.b) WITH FUNCTION app.conv(app.a) AS ASSIGNMENT;"
        );
        let cat = parse_source(&sql).expect("parses");
        assert_eq!(cat.casts[0].context, CastContext::Assignment);
        assert!(matches!(cat.casts[0].method, CastMethod::Function { .. }));
    }

    #[test]
    fn create_with_function_implicit() {
        let sql = format!(
            "{PRELUDE}CREATE CAST (app.a AS app.b) WITH FUNCTION app.conv(app.a) AS IMPLICIT;"
        );
        let cat = parse_source(&sql).expect("parses");
        assert_eq!(cat.casts[0].context, CastContext::Implicit);
        assert!(matches!(cat.casts[0].method, CastMethod::Function { .. }));
    }

    // ── WITHOUT FUNCTION ────────────────────────────────────────────────────

    #[test]
    fn create_without_function() {
        let sql = format!("{PRELUDE}CREATE CAST (app.a AS app.b) WITHOUT FUNCTION;");
        let cat = parse_source(&sql).expect("parses");
        let c = &cat.casts[0];
        assert_eq!(c.method, CastMethod::Binary);
        assert_eq!(c.context, CastContext::Explicit);
    }

    // ── WITH INOUT ──────────────────────────────────────────────────────────

    #[test]
    fn create_with_inout() {
        let sql = format!("{PRELUDE}CREATE CAST (app.a AS app.b) WITH INOUT;");
        let cat = parse_source(&sql).expect("parses");
        let c = &cat.casts[0];
        assert_eq!(c.method, CastMethod::Inout);
        assert_eq!(c.context, CastContext::Explicit);
    }

    // ── COMMENT ON CAST ─────────────────────────────────────────────────────

    #[test]
    fn comment_on_cast_sets_comment() {
        let sql = format!(
            "{PRELUDE}CREATE CAST (app.a AS app.b) WITH FUNCTION app.conv(app.a);\n\
             COMMENT ON CAST (app.a AS app.b) IS 'converts a to b';"
        );
        let cat = parse_source(&sql).expect("parses");
        assert_eq!(cat.casts[0].comment.as_deref(), Some("converts a to b"));
    }

    // ── Error cases ─────────────────────────────────────────────────────────

    #[test]
    fn drop_cast_in_source_is_rejected() {
        let sql = format!(
            "{PRELUDE}CREATE CAST (app.a AS app.b) WITH FUNCTION app.conv(app.a);\n\
             DROP CAST (app.a AS app.b);"
        );
        let err = parse_source(&sql).expect_err("should reject DROP in source");
        assert!(matches!(err, ParseError::Structural { .. }), "got: {err:?}");
    }

    #[test]
    fn duplicate_identity_is_rejected() {
        let sql = format!(
            "{PRELUDE}\
             CREATE CAST (app.a AS app.b) WITH FUNCTION app.conv(app.a);\n\
             CREATE CAST (app.a AS app.b) WITH INOUT;"
        );
        let err = parse_source(&sql).expect_err("should reject duplicate");
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("duplicate cast"), "msg: {msg}");
    }

    // ── Unit-level parse_create ──────────────────────────────────────────────

    #[test]
    fn parse_create_unit_appends() {
        let parsed = pg_query::parse("CREATE CAST (app.a AS app.b) WITH INOUT;").unwrap();
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .unwrap();
        let NodeEnum::CreateCastStmt(stmt) = node else {
            panic!("expected CreateCastStmt");
        };
        let mut acc: Vec<Cast> = Vec::new();
        parse_create(&stmt, None, &loc(), &mut acc).expect("ok");
        assert_eq!(acc.len(), 1);
        assert_eq!(acc[0].source.to_string(), "app.a");
        assert_eq!(acc[0].target.to_string(), "app.b");
        assert_eq!(acc[0].method, CastMethod::Inout);
        assert_eq!(acc[0].context, CastContext::Explicit);
    }

    /// Verify `type_name_to_qname` for the sanity-check example requested:
    /// `CREATE CAST (app.a AS app.b) WITH FUNCTION app.conv(app.a) AS IMPLICIT`.
    #[test]
    fn debug_output_sanity_check() {
        let sql = format!(
            "{PRELUDE}CREATE CAST (app.a AS app.b) WITH FUNCTION app.conv(app.a) AS IMPLICIT;"
        );
        let cat = parse_source(&sql).expect("parses");
        let c = &cat.casts[0];
        // Paste-able debug output for the task report:
        println!("Cast debug: {c:#?}");
        assert_eq!(c.source.to_string(), "app.a");
        assert_eq!(c.target.to_string(), "app.b");
        assert_eq!(c.context, CastContext::Implicit);
        let CastMethod::Function { name, arg_types } = &c.method else {
            panic!("expected Function method");
        };
        assert_eq!(name.to_string(), "app.conv");
        assert_eq!(arg_types.len(), 1);
    }
}
