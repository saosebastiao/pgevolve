//! `ALTER TABLE ... ADD CONSTRAINT FOREIGN KEY ...` — the only `ALTER TABLE`
//! permitted in source DDL.
//!
//! Source SQL is declarative: tables, columns, constraints, etc. are stated as
//! the desired end-state via `CREATE`. The single exception is forward-referencing
//! foreign keys: when two tables reference each other, neither can declare its FK
//! inline because the other side does not exist yet at parse time. For that case,
//! source files are allowed to follow the second `CREATE TABLE` with an
//! `ALTER TABLE ... ADD CONSTRAINT <name> FOREIGN KEY (...) REFERENCES ...`,
//! which lands directly into the target table's `Constraint` list.

use pg_query::protobuf::{
    AlterTableCmd, AlterTableStmt, AlterTableType, ConstrType, Constraint as PgConstraint,
};
use pg_query::NodeEnum;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::constraint::Constraint;
use crate::parse::builder::create_stmt;
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// One forward-reference FK constraint to merge into a [`crate::ir::table::Table`]
/// after all tables have been built.
#[derive(Debug, Clone)]
pub struct PendingFk {
    /// Target table to attach the constraint to.
    pub target: QualifiedName,
    /// The constraint itself.
    pub constraint: Constraint,
}

/// Process an `ALTER TABLE` statement. Returns one or more pending FK
/// constraints that should be appended to their target tables once parsing
/// completes.
///
/// Any other `ALTER TABLE` subcommand (`DROP COLUMN`, `ADD COLUMN`,
/// `ALTER COLUMN`, etc.) raises [`ParseError::Structural`] with a message
/// pointing the user to the declarative source-of-truth model.
pub fn build_alter_table(
    stmt: &AlterTableStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Vec<PendingFk>, ParseError> {
    let relation = stmt
        .relation
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "ALTER TABLE missing relation".into(),
        })?;
    let target = shared::resolve_qname(relation, default_schema, location)?;

    let mut out = Vec::new();
    for cmd_node in &stmt.cmds {
        let Some(NodeEnum::AlterTableCmd(cmd)) = cmd_node.node.as_ref() else {
            return Err(unsupported_alter(location));
        };
        let pending = process_cmd(cmd, &target, default_schema, location)?;
        out.push(pending);
    }
    Ok(out)
}

fn process_cmd(
    cmd: &AlterTableCmd,
    target: &QualifiedName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<PendingFk, ParseError> {
    let subtype = AlterTableType::try_from(cmd.subtype).unwrap_or(AlterTableType::Undefined);
    if !matches!(subtype, AlterTableType::AtAddConstraint) {
        return Err(unsupported_alter(location));
    }
    let con = cmd
        .def
        .as_ref()
        .and_then(|d| d.node.as_ref())
        .and_then(|n| match n {
            NodeEnum::Constraint(c) => Some(c.as_ref()),
            _ => None,
        })
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "ALTER TABLE ADD CONSTRAINT missing constraint definition".into(),
        })?;

    let kind = ConstrType::try_from(con.contype).unwrap_or(ConstrType::Undefined);
    if !matches!(kind, ConstrType::ConstrForeign) {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "ALTER TABLE may only ADD CONSTRAINT FOREIGN KEY in source DDL — \
                     other constraint kinds belong inline in the CREATE TABLE that \
                     declares them"
                .into(),
        });
    }

    let constraint = build_fk_constraint(con, target, default_schema, location)?;
    Ok(PendingFk {
        target: target.clone(),
        constraint,
    })
}

/// Reuse the FK builder from `create_stmt` so source ALTER and inline
/// `REFERENCES` produce identical IR.
fn build_fk_constraint(
    con: &PgConstraint,
    target_table: &QualifiedName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Constraint, ParseError> {
    // Delegate to a public helper exposed by create_stmt to avoid duplicating
    // FK extraction logic.
    create_stmt::build_fk_for_alter(con, target_table, default_schema, location)
}

fn unsupported_alter(location: &SourceLocation) -> ParseError {
    ParseError::Structural {
        location: location.clone(),
        message: "ALTER TABLE in source DDL is restricted to ADD CONSTRAINT FOREIGN KEY \
                 for forward-referencing foreign keys; pgevolve treats source SQL as \
                 declarative — express the desired schema state via CREATE statements"
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::constraint::ConstraintKind;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn build(sql: &str) -> Result<Vec<PendingFk>, ParseError> {
        let parsed = pg_query::parse(sql).expect("parses");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::AlterTableStmt(s) = stmt else {
            panic!("not AlterTableStmt")
        };
        build_alter_table(&s, None, &loc())
    }

    #[test]
    fn allowed_add_fk() {
        let pendings = build(
            "ALTER TABLE app.invoices ADD CONSTRAINT invoices_customer_fk \
             FOREIGN KEY (customer_id) REFERENCES app.customers (id);",
        )
        .expect("builds");
        assert_eq!(pendings.len(), 1);
        let p = &pendings[0];
        assert_eq!(p.target.to_string(), "app.invoices");
        assert!(matches!(p.constraint.kind, ConstraintKind::ForeignKey(_)));
    }

    #[test]
    fn rejects_drop_column() {
        let err = build("ALTER TABLE app.users DROP COLUMN email;").unwrap_err();
        match err {
            ParseError::Structural { message, .. } => {
                assert!(
                    message.contains("declarative"),
                    "expected declarative message, got: {message}"
                );
            }
            other => panic!("expected Structural, got {other:?}"),
        }
    }

    #[test]
    fn rejects_add_column() {
        let err = build("ALTER TABLE app.users ADD COLUMN email text;").unwrap_err();
        match err {
            ParseError::Structural { message, .. } => {
                assert!(
                    message.contains("declarative") || message.contains("FOREIGN KEY"),
                    "got: {message}"
                );
            }
            other => panic!("expected Structural, got {other:?}"),
        }
    }

    #[test]
    fn rejects_add_check_via_alter() {
        let err = build("ALTER TABLE app.t ADD CONSTRAINT c1 CHECK (n > 0);").unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }
}
