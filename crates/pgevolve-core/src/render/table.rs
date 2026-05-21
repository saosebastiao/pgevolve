//! Table renderer.
//!
//! `render_table` emits a `CREATE TABLE` statement with inline columns and
//! non-FK constraints (PK, UK, CHECK).  Foreign-key constraints are deliberately
//! excluded from the inline form; callers should emit them separately via
//! `render_add_fk` after all tables have been created.  This avoids forward-
//! reference failures when two tables have FKs to each other.
//!
//! Column comments and the table comment are appended as separate
//! `COMMENT ON ...` statements.

use crate::identifier::QualifiedName;
use crate::ir::constraint::{Constraint, ConstraintKind};
use crate::ir::table::Table;
use crate::plan::rewrite::sql as rewrite_sql;

/// Render a `Table` as SQL.
///
/// Produces:
/// 1. `CREATE TABLE <qname> (...);`  — with inline columns and all non-FK constraints.
/// 2. Optional `COMMENT ON TABLE ... IS '...';`
/// 3. Optional `COMMENT ON COLUMN ...` for each column that has a comment.
///
/// FK constraints are NOT included inline; use [`render_add_fk`] for those.
#[must_use]
pub fn render_table(t: &Table) -> String {
    let mut out = String::new();

    // Build a version of the table that excludes FK constraints for the
    // inline CREATE TABLE body.
    let table_without_fks = Table {
        qname: t.qname.clone(),
        columns: t.columns.clone(),
        constraints: t
            .constraints
            .iter()
            .filter(|c| !matches!(c.kind, ConstraintKind::ForeignKey(_)))
            .cloned()
            .collect(),
        partition_by: None,
        partition_of: None,
        comment: t.comment.clone(),
    };

    out.push_str(&rewrite_sql::create_table(&table_without_fks));
    out.push('\n');

    // Table comment.
    if let Some(comment) = &t.comment {
        out.push_str(&rewrite_sql::comment_on_table(
            &t.qname,
            Some(comment.as_str()),
        ));
        out.push('\n');
    }

    // Per-column comments.
    for col in &t.columns {
        if let Some(comment) = &col.comment {
            out.push_str(&rewrite_sql::comment_on_column(
                &t.qname,
                &col.name,
                Some(comment.as_str()),
            ));
            out.push('\n');
        }
    }

    // Constraint comments (non-FK only; FK comments are handled by `render_add_fk`).
    for c in &t.constraints {
        if matches!(c.kind, ConstraintKind::ForeignKey(_)) {
            continue;
        }
        if let Some(comment) = &c.comment {
            out.push_str(&rewrite_sql::comment_on_constraint(
                &t.qname,
                &c.qname.name,
                Some(comment.as_str()),
            ));
            out.push('\n');
        }
    }

    out
}

/// Render a foreign-key constraint as `ALTER TABLE <table> ADD CONSTRAINT ...;`.
///
/// Also emits `COMMENT ON CONSTRAINT ...` when the constraint has a comment.
#[must_use]
pub fn render_add_fk(table_qname: &QualifiedName, c: &Constraint) -> String {
    let mut out = String::new();
    out.push_str(&rewrite_sql::alter_table_add_constraint(table_qname, c));
    out.push('\n');
    if let Some(comment) = &c.comment {
        out.push_str(&rewrite_sql::comment_on_constraint(
            table_qname,
            &c.qname.name,
            Some(comment.as_str()),
        ));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::constraint::{
        Constraint, ConstraintKind, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
    };
    use crate::ir::default_expr::{DefaultExpr, LiteralValue};
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn col(name: &str, ty: ColumnType) -> Column {
        Column {
            name: id(name),
            ty,
            nullable: false,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            comment: None,
        }
    }

    fn assert_pg_parseable(sql: &str) {
        // pg_query::parse accepts multi-statement SQL; pass the whole block.
        let r = pg_query::parse(sql);
        assert!(r.is_ok(), "pg_query rejected SQL:\n{sql}\nerr: {r:?}");
    }

    #[test]
    fn simple_table_renders() {
        let t = Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt),
                Column {
                    name: id("email"),
                    ty: ColumnType::Text,
                    nullable: true,
                    ..col("email", ColumnType::Text)
                },
            ],
            constraints: vec![Constraint {
                qname: qn("app", "users_pkey"),
                kind: ConstraintKind::PrimaryKey {
                    columns: vec![id("id")],
                    include: vec![],
                },
                deferrable: Deferrable::NotDeferrable,
                comment: None,
            }],
            partition_by: None,
            partition_of: None,
            comment: None,
        };
        let sql = render_table(&t);
        assert!(sql.contains("CREATE TABLE app.users"));
        assert!(sql.contains("id bigint NOT NULL"));
        assert!(sql.contains("CONSTRAINT users_pkey PRIMARY KEY (id)"));
        assert!(!sql.contains("FOREIGN KEY"), "FK must not appear inline");
        assert_pg_parseable(&sql);
    }

    #[test]
    fn fk_excluded_from_create_table() {
        let t = Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt),
                col("org_id", ColumnType::BigInt),
            ],
            constraints: vec![
                Constraint {
                    qname: qn("app", "users_pkey"),
                    kind: ConstraintKind::PrimaryKey {
                        columns: vec![id("id")],
                        include: vec![],
                    },
                    deferrable: Deferrable::NotDeferrable,
                    comment: None,
                },
                Constraint {
                    qname: qn("app", "users_org_fkey"),
                    kind: ConstraintKind::ForeignKey(ForeignKey {
                        columns: vec![id("org_id")],
                        referenced_table: qn("app", "orgs"),
                        referenced_columns: vec![id("id")],
                        on_update: ReferentialAction::NoAction,
                        on_delete: ReferentialAction::Cascade,
                        match_type: FkMatchType::Simple,
                    }),
                    deferrable: Deferrable::NotDeferrable,
                    comment: None,
                },
            ],
            partition_by: None,
            partition_of: None,
            comment: None,
        };
        let sql = render_table(&t);
        assert!(
            !sql.contains("FOREIGN KEY"),
            "FK must not appear in render_table output"
        );
        assert_pg_parseable(&sql);
    }

    #[test]
    fn render_add_fk_emits_alter_table() {
        let fk = Constraint {
            qname: qn("app", "users_org_fkey"),
            kind: ConstraintKind::ForeignKey(ForeignKey {
                columns: vec![id("org_id")],
                referenced_table: qn("app", "orgs"),
                referenced_columns: vec![id("id")],
                on_update: ReferentialAction::NoAction,
                on_delete: ReferentialAction::Cascade,
                match_type: FkMatchType::Simple,
            }),
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        };
        let sql = render_add_fk(&qn("app", "users"), &fk);
        assert!(sql.contains("ALTER TABLE app.users ADD"));
        assert!(sql.contains("FOREIGN KEY"));
        assert!(sql.contains("REFERENCES app.orgs"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn table_comment_rendered() {
        let t = Table {
            qname: qn("app", "orgs"),
            columns: vec![col("id", ColumnType::BigInt)],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: Some("organization records".into()),
        };
        let sql = render_table(&t);
        assert!(sql.contains("COMMENT ON TABLE app.orgs IS 'organization records';"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn column_comment_rendered() {
        let mut c = col("email", ColumnType::Text);
        c.comment = Some("email address".into());
        let t = Table {
            qname: qn("app", "users"),
            columns: vec![c],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
        };
        let sql = render_table(&t);
        assert!(sql.contains("COMMENT ON COLUMN app.users.email IS 'email address';"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn column_with_default_literal() {
        let mut c = col("active", ColumnType::Boolean);
        c.default = Some(DefaultExpr::Literal(LiteralValue::Bool(true)));
        c.nullable = true;
        let t = Table {
            qname: qn("app", "users"),
            columns: vec![c],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
        };
        let sql = render_table(&t);
        assert!(sql.contains("DEFAULT true"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn nullable_column() {
        let mut c = col("bio", ColumnType::Text);
        c.nullable = true;
        let t = Table {
            qname: qn("app", "users"),
            columns: vec![c],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
        };
        let sql = render_table(&t);
        // nullable columns must not have NOT NULL.
        assert!(!sql.contains("NOT NULL"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn check_constraint_inline() {
        use crate::ir::default_expr::NormalizedExpr;
        let check = Constraint {
            qname: qn("app", "users_email_check"),
            kind: ConstraintKind::Check {
                expression: NormalizedExpr::from_text("email ~* '^.+@.+$'"),
                no_inherit: false,
            },
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        };
        let t = Table {
            qname: qn("app", "users"),
            columns: vec![col("email", ColumnType::Text)],
            constraints: vec![check],
            partition_by: None,
            partition_of: None,
            comment: None,
        };
        let sql = render_table(&t);
        assert!(sql.contains("CHECK"), "expected CHECK in CREATE TABLE");
        assert_pg_parseable(&sql);
    }
}
