//! `CREATE TABLE` → [`crate::ir::table::Table`].

#![allow(clippy::similar_names, clippy::missing_const_for_fn)]

use pg_query::NodeEnum;
use pg_query::protobuf::{self, ColumnDef, ConstrType, Constraint as PgConstraint, CreateStmt};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column::{
    Column, Generated, GeneratedKind, Identity, IdentityKind, SequenceOptions,
};
use crate::ir::constraint::{
    Constraint, ConstraintKind, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
};
use crate::ir::default_expr::DefaultExpr;
use crate::ir::table::Table;
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};
use crate::parse::normalize_expr;

/// Build a [`Table`] from a `CREATE TABLE` AST.
pub fn build_table(
    create: &CreateStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Table, ParseError> {
    let relation = create
        .relation
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "CREATE TABLE missing relation".into(),
        })?;
    let qname = shared::resolve_qname(relation, default_schema, location)?;

    let mut columns: Vec<Column> = Vec::new();
    let mut constraints: Vec<Constraint> = Vec::new();
    let mut pk_columns: Option<Vec<Identifier>> = None;

    for elt in &create.table_elts {
        let Some(node) = elt.node.as_ref() else {
            continue;
        };
        match node {
            NodeEnum::ColumnDef(col) => {
                let (column, mut col_constraints, col_pk) =
                    build_column(col, &qname, default_schema, location)?;
                columns.push(column);
                constraints.append(&mut col_constraints);
                if let Some(c) = col_pk {
                    pk_columns = Some(vec![c]);
                }
            }
            NodeEnum::Constraint(con) => {
                if let Some(built) = build_table_constraint(con, &qname, default_schema, location)?
                {
                    if let ConstraintKind::PrimaryKey { columns: cols, .. } = &built.kind {
                        pk_columns = Some(cols.clone());
                    }
                    constraints.push(built);
                }
            }
            other => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!("unsupported table element: {}", node_kind_name(other)),
                });
            }
        }
    }

    // Apply implicit NOT NULL to columns covered by a PRIMARY KEY.
    if let Some(pk_cols) = pk_columns.as_ref() {
        for col in &mut columns {
            if pk_cols.contains(&col.name) {
                col.nullable = false;
            }
        }
    }

    Ok(Table {
        qname,
        columns,
        constraints,
        comment: None,
    })
}

/// Build a [`Column`] plus any inline-constraint [`Constraint`]s the column
/// declared. Returns `(column, table_constraints, pk_inline_column)`.
///
/// `pk_inline_column` is `Some(col_name)` if this column had `PRIMARY KEY`
/// inline — used to implicitly mark the column NOT NULL after the table is
/// fully assembled.
#[allow(clippy::too_many_lines)]
fn build_column(
    col: &ColumnDef,
    table_qname: &QualifiedName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<(Column, Vec<Constraint>, Option<Identifier>), ParseError> {
    let name = shared::ident(&col.colname, location)?;
    let type_name = col
        .type_name
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("column {} missing type", col.colname),
        })?;
    let ty = shared::type_name_to_column_type(type_name, location)?;

    let mut nullable = !col.is_not_null;
    let mut default: Option<DefaultExpr> = None;
    let mut identity: Option<Identity> = None;
    let mut generated: Option<Generated> = None;
    let comment: Option<String> = None;
    let mut produced_constraints: Vec<Constraint> = Vec::new();
    let mut pk_inline: Option<Identifier> = None;

    // Column-level default in raw_default (sometimes used) — fold into the
    // constraint scan below since most defaults arrive as ConstrDefault entries.
    if let Some(raw) = col.raw_default.as_ref()
        && let Some(node) = raw.node.as_ref()
    {
        default = Some(shared::build_default_expr(
            node,
            Some(&ty),
            default_schema,
            location,
        )?);
    }

    let collation = col
        .coll_clause
        .as_ref()
        .map(|cc| collate_clause_to_qname(cc, default_schema, location))
        .transpose()?;

    for c in &col.constraints {
        let Some(NodeEnum::Constraint(con)) = c.node.as_ref() else {
            continue;
        };
        let kind = ConstrType::try_from(con.contype).unwrap_or(ConstrType::Undefined);
        match kind {
            ConstrType::ConstrNotnull => {
                nullable = false;
            }
            ConstrType::ConstrNull => {
                nullable = true;
            }
            ConstrType::ConstrDefault => {
                if let Some(raw) = con.raw_expr.as_ref()
                    && let Some(node) = raw.node.as_ref()
                {
                    default = Some(shared::build_default_expr(
                        node,
                        Some(&ty),
                        default_schema,
                        location,
                    )?);
                }
            }
            ConstrType::ConstrIdentity => {
                identity = Some(Identity {
                    kind: parse_identity_kind(&con.generated_when),
                    sequence: SequenceOptions {
                        start: 1,
                        increment: 1,
                        min_value: None,
                        max_value: None,
                        cache: 1,
                        cycle: false,
                    },
                });
                nullable = false;
            }
            ConstrType::ConstrGenerated => {
                if let Some(raw) = con.raw_expr.as_ref()
                    && let Some(node) = raw.node.as_ref()
                {
                    let expr = normalize_expr::from_pg_node(node, None, location)?;
                    generated = Some(Generated {
                        kind: GeneratedKind::Stored,
                        expression: expr,
                    });
                }
            }
            ConstrType::ConstrPrimary => {
                pk_inline = Some(name.clone());
                nullable = false;
                produced_constraints.push(make_pk_constraint(
                    table_qname,
                    &con.conname,
                    vec![name.clone()],
                    location,
                )?);
            }
            ConstrType::ConstrUnique => {
                produced_constraints.push(make_unique_constraint(
                    table_qname,
                    con,
                    vec![name.clone()],
                    location,
                )?);
            }
            ConstrType::ConstrForeign => {
                produced_constraints.push(make_fk_constraint(
                    table_qname,
                    con,
                    vec![name.clone()],
                    default_schema,
                    location,
                )?);
            }
            ConstrType::ConstrCheck => {
                produced_constraints.push(make_check_constraint(table_qname, con, location)?);
            }
            _ => {}
        }
    }

    Ok((
        Column {
            name,
            ty,
            nullable,
            default,
            identity,
            generated,
            collation,
            comment,
        },
        produced_constraints,
        pk_inline,
    ))
}

/// Build a table-level [`Constraint`] from a standalone `CONSTRAINT ...` clause
/// in the table-elements list. Returns `None` if the constraint kind is not
/// supported for table-level use (e.g., `NOT NULL` standalone — not legal
/// syntax anyway).
fn build_table_constraint(
    con: &PgConstraint,
    table_qname: &QualifiedName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Option<Constraint>, ParseError> {
    let kind = ConstrType::try_from(con.contype).unwrap_or(ConstrType::Undefined);
    let cols = key_idents(&con.keys, location)?;
    Ok(Some(match kind {
        ConstrType::ConstrPrimary => make_pk_constraint(table_qname, &con.conname, cols, location)?,
        ConstrType::ConstrUnique => make_unique_constraint(table_qname, con, cols, location)?,
        ConstrType::ConstrForeign => {
            let fk_cols = key_idents(&con.fk_attrs, location)?;
            make_fk_constraint(table_qname, con, fk_cols, default_schema, location)?
        }
        ConstrType::ConstrCheck => make_check_constraint(table_qname, con, location)?,
        _ => return Ok(None),
    }))
}

fn make_pk_constraint(
    table_qname: &QualifiedName,
    conname: &str,
    columns: Vec<Identifier>,
    location: &SourceLocation,
) -> Result<Constraint, ParseError> {
    let qname = constraint_qname(table_qname, conname, "pkey", location)?;
    Ok(Constraint {
        qname,
        kind: ConstraintKind::PrimaryKey {
            columns,
            include: vec![],
        },
        deferrable: Deferrable::NotDeferrable,
        comment: None,
    })
}

fn make_unique_constraint(
    table_qname: &QualifiedName,
    con: &PgConstraint,
    columns: Vec<Identifier>,
    location: &SourceLocation,
) -> Result<Constraint, ParseError> {
    let qname = constraint_qname(table_qname, &con.conname, "key", location)?;
    let include = key_idents(&con.including, location)?;
    Ok(Constraint {
        qname,
        kind: ConstraintKind::Unique {
            columns,
            include,
            nulls_distinct: !con.nulls_not_distinct,
        },
        deferrable: deferrable_from(con),
        comment: None,
    })
}

/// Build a FOREIGN KEY [`Constraint`] from an `ALTER TABLE ADD CONSTRAINT FK`.
///
/// The local columns come from `con.fk_attrs` (an inline column-spec list is
/// what populates them at parse time), so we forward straight into
/// [`make_fk_constraint`] after extracting them.
pub fn build_fk_for_alter(
    con: &PgConstraint,
    target_table: &QualifiedName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Constraint, ParseError> {
    let fk_cols = key_idents(&con.fk_attrs, location)?;
    make_fk_constraint(target_table, con, fk_cols, default_schema, location)
}

fn make_fk_constraint(
    table_qname: &QualifiedName,
    con: &PgConstraint,
    fk_cols: Vec<Identifier>,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Constraint, ParseError> {
    let qname = constraint_qname(table_qname, &con.conname, "fkey", location)?;
    let pk_cols = key_idents(&con.pk_attrs, location)?;
    let pktable = con.pktable.as_ref().ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: "FOREIGN KEY missing referenced table".into(),
    })?;
    let referenced_table = shared::resolve_qname(pktable, default_schema, location)?;
    Ok(Constraint {
        qname,
        kind: ConstraintKind::ForeignKey(ForeignKey {
            columns: fk_cols,
            referenced_table,
            referenced_columns: pk_cols,
            on_update: parse_referential_action(&con.fk_upd_action),
            on_delete: parse_referential_action(&con.fk_del_action),
            match_type: parse_match_type(&con.fk_matchtype),
        }),
        deferrable: deferrable_from(con),
        comment: None,
    })
}

fn make_check_constraint(
    table_qname: &QualifiedName,
    con: &PgConstraint,
    location: &SourceLocation,
) -> Result<Constraint, ParseError> {
    let qname = constraint_qname(table_qname, &con.conname, "check", location)?;
    let raw = con
        .raw_expr
        .as_ref()
        .and_then(|r| r.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "CHECK constraint missing expression".into(),
        })?;
    let expression = normalize_expr::from_pg_node(raw, None, location)?;
    Ok(Constraint {
        qname,
        kind: ConstraintKind::Check {
            expression,
            no_inherit: con.is_no_inherit,
        },
        deferrable: deferrable_from(con),
        comment: None,
    })
}

fn deferrable_from(con: &PgConstraint) -> Deferrable {
    if con.deferrable {
        Deferrable::Deferrable {
            initially_deferred: con.initdeferred,
        }
    } else {
        Deferrable::NotDeferrable
    }
}

fn constraint_qname(
    table_qname: &QualifiedName,
    conname: &str,
    kind_suffix: &str,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let name = if conname.is_empty() {
        format!("{}_{}", table_qname.name.as_str(), kind_suffix)
    } else {
        conname.to_string()
    };
    Ok(QualifiedName::new(
        table_qname.schema.clone(),
        shared::ident(&name, location)?,
    ))
}

fn key_idents(
    nodes: &[protobuf::Node],
    location: &SourceLocation,
) -> Result<Vec<Identifier>, ParseError> {
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        let Some(NodeEnum::String(s)) = n.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: "expected identifier in constraint key list".into(),
            });
        };
        out.push(shared::ident(&s.sval, location)?);
    }
    Ok(out)
}

fn collate_clause_to_qname(
    cc: &protobuf::CollateClause,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let parts: Vec<&str> = cc
        .collname
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(NodeEnum::String(s)) => Some(s.sval.as_str()),
            _ => None,
        })
        .collect();
    match parts.as_slice() {
        [name] => {
            let schema = default_schema
                .cloned()
                .ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: "COLLATE name must be schema-qualified or carry a directive".into(),
                })?;
            Ok(QualifiedName::new(schema, shared::ident(name, location)?))
        }
        [schema, name] => Ok(QualifiedName::new(
            shared::ident(schema, location)?,
            shared::ident(name, location)?,
        )),
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: "COLLATE name must have one or two components".into(),
        }),
    }
}

fn parse_identity_kind(generated_when: &str) -> IdentityKind {
    // Postgres encodes `ALWAYS` as "a" and `BY DEFAULT` as "d" in this string.
    match generated_when {
        "a" => IdentityKind::Always,
        _ => IdentityKind::ByDefault,
    }
}

fn parse_referential_action(s: &str) -> ReferentialAction {
    match s {
        "r" => ReferentialAction::Restrict,
        "c" => ReferentialAction::Cascade,
        "n" => ReferentialAction::SetNull(vec![]),
        "d" => ReferentialAction::SetDefault(vec![]),
        // "a" or empty → NO ACTION (default).
        _ => ReferentialAction::NoAction,
    }
}

fn parse_match_type(s: &str) -> FkMatchType {
    if s.eq_ignore_ascii_case("f") {
        FkMatchType::Full
    } else {
        FkMatchType::Simple
    }
}

fn node_kind_name(node: &NodeEnum) -> &'static str {
    match node {
        NodeEnum::ColumnDef(_) => "column definition",
        NodeEnum::Constraint(_) => "constraint",
        NodeEnum::TableLikeClause(_) => "LIKE clause",
        _ => "table element",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::column_type::ColumnType;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn build(sql: &str) -> Table {
        let parsed = pg_query::parse(sql).expect("parses");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreateStmt(create) = stmt else {
            panic!("not CreateStmt")
        };
        build_table(&create, None, &loc()).expect("builds")
    }

    #[test]
    fn single_column_table() {
        let t = build("CREATE TABLE app.users (id integer);");
        assert_eq!(t.qname.to_string(), "app.users");
        assert_eq!(t.columns.len(), 1);
        assert_eq!(t.columns[0].name.as_str(), "id");
        assert_eq!(t.columns[0].ty, ColumnType::Integer);
        assert!(t.columns[0].nullable);
    }

    #[test]
    fn not_null_flips_nullable() {
        let t = build("CREATE TABLE app.t (a integer NOT NULL);");
        assert!(!t.columns[0].nullable);
    }

    #[test]
    fn inline_pk_implies_not_null_and_constraint() {
        let t = build("CREATE TABLE app.t (id integer PRIMARY KEY);");
        assert!(!t.columns[0].nullable);
        assert_eq!(t.constraints.len(), 1);
        assert!(matches!(
            t.constraints[0].kind,
            ConstraintKind::PrimaryKey { .. }
        ));
    }

    #[test]
    fn composite_pk_table_constraint() {
        let t = build("CREATE TABLE app.t (a integer, b integer, PRIMARY KEY (a, b));");
        assert_eq!(t.constraints.len(), 1);
        if let ConstraintKind::PrimaryKey { columns, .. } = &t.constraints[0].kind {
            assert_eq!(columns.len(), 2);
            assert_eq!(columns[0].as_str(), "a");
            assert_eq!(columns[1].as_str(), "b");
        } else {
            panic!()
        }
        // PK columns implicitly NOT NULL.
        assert!(!t.columns[0].nullable);
        assert!(!t.columns[1].nullable);
    }

    #[test]
    fn inline_references() {
        let t = build(
            "CREATE TABLE app.t (org_id integer REFERENCES app.orgs (id) ON DELETE CASCADE);",
        );
        assert_eq!(t.constraints.len(), 1);
        if let ConstraintKind::ForeignKey(fk) = &t.constraints[0].kind {
            assert_eq!(fk.referenced_table.to_string(), "app.orgs");
            assert!(matches!(fk.on_delete, ReferentialAction::Cascade));
        } else {
            panic!("expected ForeignKey")
        }
    }

    #[test]
    fn inline_check() {
        let t = build("CREATE TABLE app.t (n integer CHECK (n > 0));");
        assert_eq!(t.constraints.len(), 1);
        assert!(matches!(
            t.constraints[0].kind,
            ConstraintKind::Check { .. }
        ));
    }

    #[test]
    fn default_now_is_expr() {
        let t = build("CREATE TABLE app.t (created_at timestamp DEFAULT now());");
        match &t.columns[0].default {
            Some(DefaultExpr::Expr(e)) => assert!(e.canonical_text.to_lowercase().contains("now")),
            other => panic!("expected Expr default, got {other:?}"),
        }
    }

    #[test]
    fn default_integer_literal() {
        let t = build("CREATE TABLE app.t (n integer DEFAULT 0);");
        assert!(matches!(
            t.columns[0].default,
            Some(DefaultExpr::Literal(
                crate::ir::default_expr::LiteralValue::Integer(0)
            ))
        ));
    }

    #[test]
    fn default_nextval_is_sequence() {
        let t = build("CREATE TABLE app.t (id integer DEFAULT nextval('app.seq1'));");
        match &t.columns[0].default {
            Some(DefaultExpr::Sequence(q)) => assert_eq!(q.to_string(), "app.seq1"),
            other => panic!("expected Sequence default, got {other:?}"),
        }
    }

    #[test]
    fn directive_default_schema_used() {
        let parsed = pg_query::parse("CREATE TABLE users (id integer);").unwrap();
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .unwrap();
        let NodeEnum::CreateStmt(create) = stmt else {
            panic!()
        };
        let app = Identifier::from_unquoted("app").unwrap();
        let t = build_table(&create, Some(&app), &loc()).unwrap();
        assert_eq!(t.qname.to_string(), "app.users");
    }

    #[test]
    fn unqualified_without_directive_errors() {
        let parsed = pg_query::parse("CREATE TABLE users (id integer);").unwrap();
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .unwrap();
        let NodeEnum::CreateStmt(create) = stmt else {
            panic!()
        };
        let err = build_table(&create, None, &loc()).unwrap_err();
        assert!(matches!(err, ParseError::UnqualifiedName { .. }));
    }
}
