//! `CREATE MATERIALIZED VIEW` → [`crate::ir::view::MaterializedView`].
//!
//! Postgres parses `CREATE MATERIALIZED VIEW` as a `CreateTableAsStmt` with
//! `objtype = ObjectType::ObjectMatview`.  The dispatcher in
//! [`crate::parse::statement`] identifies the variant and routes here.
//!
//! Produces a *provisional* IR record: `body_canonical` is set to the empty
//! sentinel and `body_dependencies` is empty. T4's AST canonicalization pass
//! fills in both fields immediately after source IR is assembled.

use pg_query::NodeEnum;
use pg_query::protobuf::CreateTableAsStmt;

use crate::identifier::Identifier;
use crate::ir::view::{MaterializedView, ViewColumn};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};
use crate::parse::normalize_body::NormalizedBody;

/// Build a provisional [`MaterializedView`] from a `CREATE MATERIALIZED VIEW`
/// AST node (represented as `CreateTableAsStmt` in `pg_query`).
///
/// Column names are taken from the explicit alias list (`col_names` on
/// `IntoClause`) when present; otherwise `columns` is empty until T4 fills it.
pub(crate) fn build_materialized_view(
    stmt: &CreateTableAsStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<MaterializedView, ParseError> {
    let into = stmt.into.as_ref().ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: "CREATE MATERIALIZED VIEW missing INTO clause".into(),
    })?;
    let rel = into.rel.as_ref().ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: "CREATE MATERIALIZED VIEW INTO clause missing relation".into(),
    })?;
    let qname = shared::resolve_qname(rel, default_schema, location)?;
    let columns = mv_columns_from_col_names(&into.col_names, location)?;

    // Extract a deparseable SELECT body from the query node. T4's
    // canonicalization pass will re-parse this string to fill
    // body_canonical and body_dependencies.
    let raw_body = extract_query_body(stmt.query.as_deref(), location)?;

    // MVs share the same reloption key set as tables.
    let storage = crate::parse::builder::reloptions::decode_table_options(&into.options, location)?;

    Ok(MaterializedView {
        qname,
        columns,
        body_canonical: NormalizedBody::empty(),
        body_dependencies: Vec::new(),
        comment: None,
        raw_body,
        owner: None,
        grants: vec![],
        storage,
    })
}

/// Deparse the query node of a `CREATE MATERIALIZED VIEW` into a SELECT SQL
/// string.
///
/// The deparsed text may differ in whitespace and keyword case from the
/// original source, but is semantically equivalent. T4 canonicalizes it
/// further via [`NormalizedBody::from_sql`].
fn extract_query_body(
    query_node: Option<&pg_query::protobuf::Node>,
    location: &SourceLocation,
) -> Result<String, ParseError> {
    let Some(node) = query_node else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "CREATE MATERIALIZED VIEW missing query body".into(),
        });
    };
    let Some(node_inner) = &node.node else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "CREATE MATERIALIZED VIEW query body node is empty".into(),
        });
    };
    // Use NodeRef::deparse() which correctly sets PG_VERSION_NUM in the
    // internal ParseResult it builds.
    let deparsed = node_inner
        .to_ref()
        .deparse()
        .map_err(|e| ParseError::Structural {
            location: location.clone(),
            message: format!("failed to deparse materialized view query body: {e}"),
        })?;
    if deparsed.trim().is_empty() {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "deparsed materialized view query body is empty (node type: {:?})",
                std::mem::discriminant(node_inner)
            ),
        });
    }
    Ok(deparsed)
}

/// Extract explicit column alias list from
/// `CREATE MATERIALIZED VIEW mv(a, b, ...) AS ...`.
///
/// Returns an empty vec when no alias list was provided (the common case).
/// When the explicit alias list is empty, columns are derived from the
/// SELECT target list. This derivation requires walking the SELECT AST,
/// which T4's AST canonicalization pass already does for `body_canonical`
/// and `body_dependencies`. T3 leaves `columns` as an empty Vec; T4 fills
/// it during the same AST walk per PG's column-naming algorithm:
///   1. `ResTarget.name` (explicit alias) wins
///   2. Otherwise, the rightmost name in a `ColumnRef` (`users.email` → "email")
///   3. Otherwise, `"?column?"` (PG's fallback)
///
/// See arch spec views sub-spec §5.1 for the AST-canonicalization-pass
/// contract that includes column derivation.
fn mv_columns_from_col_names(
    col_names: &[pg_query::protobuf::Node],
    location: &SourceLocation,
) -> Result<Vec<ViewColumn>, ParseError> {
    if col_names.is_empty() {
        return Ok(Vec::new());
    }
    col_names
        .iter()
        .map(|node| {
            let Some(NodeEnum::String(s)) = node.node.as_ref() else {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: "expected string node in materialized view column name list".into(),
                });
            };
            let name = shared::ident(&s.sval, location)?;
            Ok(ViewColumn {
                name,
                // type resolved later by ast_canon
                column_type: None,
                comment: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pg_query::protobuf::ObjectType;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn parse_matview(sql: &str) -> CreateTableAsStmt {
        let parsed = pg_query::parse(sql).expect("parses");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreateTableAsStmt(s) = node else {
            panic!("expected CreateTableAsStmt, got: {node:?}");
        };
        // Verify the discriminator.
        assert_eq!(
            ObjectType::try_from(s.objtype),
            Ok(ObjectType::ObjectMatview),
            "expected OBJECT_MATVIEW"
        );
        *s
    }

    fn build(sql: &str) -> MaterializedView {
        let stmt = parse_matview(sql);
        build_materialized_view(&stmt, None, &loc()).expect("build_materialized_view")
    }

    #[test]
    fn simple_matview() {
        let mv = build(
            "CREATE MATERIALIZED VIEW app.summary AS SELECT count(*) FROM app.events WITH NO DATA;",
        );
        assert_eq!(mv.qname.to_string(), "app.summary");
        assert!(mv.columns.is_empty());
        assert!(mv.body_canonical.canonical_text().is_empty());
    }

    #[test]
    fn matview_with_column_aliases() {
        let mv = build("CREATE MATERIALIZED VIEW app.mv(a, b) AS SELECT 1, 2 WITH NO DATA;");
        assert_eq!(mv.columns.len(), 2);
        assert_eq!(mv.columns[0].name.as_str(), "a");
        assert_eq!(mv.columns[1].name.as_str(), "b");
    }

    #[test]
    fn unqualified_mv_uses_default_schema() {
        let stmt = parse_matview("CREATE MATERIALIZED VIEW myview AS SELECT 1 WITH NO DATA;");
        let app = Identifier::from_unquoted("app").unwrap();
        let mv = build_materialized_view(&stmt, Some(&app), &loc()).unwrap();
        assert_eq!(mv.qname.to_string(), "app.myview");
    }

    #[test]
    fn unqualified_mv_without_schema_errors() {
        let stmt = parse_matview("CREATE MATERIALIZED VIEW myview AS SELECT 1 WITH NO DATA;");
        let err = build_materialized_view(&stmt, None, &loc()).unwrap_err();
        assert!(matches!(err, ParseError::UnqualifiedName { .. }));
    }

    #[test]
    fn body_canonical_is_empty_sentinel() {
        let mv = build("CREATE MATERIALIZED VIEW app.mv AS SELECT 1 WITH NO DATA;");
        assert!(mv.body_canonical.canonical_text().is_empty());
        assert_eq!(mv.body_canonical.canonical_hash(), &[0u8; 32]);
    }
}
