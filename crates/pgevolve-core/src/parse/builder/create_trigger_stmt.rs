//! Source parser for `CREATE [CONSTRAINT] TRIGGER` statements.
//!
//! Accepts the full PG syntax:
//!
//! ```sql
//! CREATE [ CONSTRAINT ] TRIGGER name { BEFORE | AFTER | INSTEAD OF }
//!     { event [ OR ... ] }
//!     ON table_name
//!     [ NOT DEFERRABLE | DEFERRABLE [ INITIALLY IMMEDIATE | INITIALLY DEFERRED ] ]
//!     [ REFERENCING { { OLD | NEW } TABLE [ AS ] transition_relation_name } [ ... ] ]
//!     [ FOR [ EACH ] { ROW | STATEMENT } ]
//!     [ WHEN ( condition ) ]
//!     EXECUTE { FUNCTION | PROCEDURE } function_name ( arguments )
//! ```
//!
//! Rejects `FROM referenced_table_name` (the constraint-trigger `FROM` clause)
//! with [`ParseError::Structural`].
//!
//! Validates that `CONSTRAINT TRIGGER` is `AFTER` + `FOR EACH ROW`.
//!
//! # WHEN clause canonicalization
//!
//! The WHEN expression is canonicalized by wrapping it in a synthetic
//! `SELECT <expr>` scaffold, deparsing via `pg_query::deparse`, stripping
//! the `SELECT ` prefix, and feeding the result to
//! [`NormalizedExpr::from_text`] after keyword lowercasing. This is the
//! same approach used by [`crate::parse::normalize_expr::from_pg_node`].

use pg_query::NodeEnum;
use pg_query::protobuf::{CreateTrigStmt, Node};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::constraint::Deferrable;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::trigger::{
    TransitionKind, TransitionTable, Trigger, TriggerEvent, TriggerLevel, TriggerTiming,
};
use crate::parse::error::{ParseError, SourceLocation};
use crate::parse::normalize_expr;

/// Build a [`Trigger`] from a parsed `CreateTrigStmt` AST node.
pub(crate) fn build_trigger(
    stmt: &CreateTrigStmt,
    location: &SourceLocation,
) -> Result<Trigger, ParseError> {
    let name = Identifier::from_unquoted(&stmt.trigname).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("CREATE TRIGGER: invalid name '{}': {e}", stmt.trigname),
    })?;

    let table = range_var_to_qname(stmt.relation.as_ref(), location, "trigger target table")?;
    // Trigger qname uses the table's schema.
    let qname = QualifiedName::new(table.schema.clone(), name);

    if stmt.constrrel.is_some() {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "{qname}: CREATE CONSTRAINT TRIGGER ... FROM ref_table is not supported."
            ),
        });
    }

    let timing = parse_timing(stmt.timing, location, &qname)?;
    let events = parse_events(stmt.events, &stmt.columns, location, &qname)?;
    let level = if stmt.row {
        TriggerLevel::Row
    } else {
        TriggerLevel::Statement
    };

    let when_clause = match stmt.when_clause.as_deref() {
        Some(node) => Some(node_to_normalized_expr(node, location, &qname)?),
        None => None,
    };

    let transition_tables = parse_transition_rels(&stmt.transition_rels, location, &qname)?;

    let function_qname =
        qualified_name_from_list(&stmt.funcname, location, "trigger function")?;
    let function_args = string_list(&stmt.args);

    let is_constraint = stmt.isconstraint;
    let deferrable = if is_constraint {
        if stmt.deferrable {
            Deferrable::Deferrable {
                initially_deferred: stmt.initdeferred,
            }
        } else {
            Deferrable::NotDeferrable
        }
    } else {
        // Non-constraint triggers are always NotDeferrable in PG.
        Deferrable::NotDeferrable
    };

    // Constraint triggers must be AFTER + FOR EACH ROW (PG enforces this).
    if is_constraint {
        if !matches!(timing, TriggerTiming::After) {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("{qname}: CONSTRAINT TRIGGER must be AFTER."),
            });
        }
        if !matches!(level, TriggerLevel::Row) {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("{qname}: CONSTRAINT TRIGGER must be FOR EACH ROW."),
            });
        }
    }

    Ok(Trigger {
        qname,
        table,
        timing,
        events,
        level,
        when_clause,
        transition_tables,
        function_qname,
        function_args,
        is_constraint,
        deferrable,
        comment: None,
    })
}

fn parse_timing(
    timing: i32,
    location: &SourceLocation,
    qname: &QualifiedName,
) -> Result<TriggerTiming, ParseError> {
    // PG's CreateTrigStmt.timing is a bitmask. Mask to the timing bits only.
    const TRIGGER_TYPE_BEFORE: i32 = 1 << 1; // 2
    const TRIGGER_TYPE_INSTEAD: i32 = 1 << 6; // 64

    let bits = timing & (TRIGGER_TYPE_BEFORE | TRIGGER_TYPE_INSTEAD);
    match bits {
        b if b == TRIGGER_TYPE_BEFORE => Ok(TriggerTiming::Before),
        b if b == TRIGGER_TYPE_INSTEAD => Ok(TriggerTiming::InsteadOf),
        0 => Ok(TriggerTiming::After),
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: format!("{qname}: invalid trigger timing bits 0x{timing:x}"),
        }),
    }
}

fn parse_events(
    events: i32,
    update_columns: &[pg_query::protobuf::Node],
    location: &SourceLocation,
    qname: &QualifiedName,
) -> Result<Vec<TriggerEvent>, ParseError> {
    const TRIGGER_TYPE_INSERT: i32 = 1 << 2; // 4
    const TRIGGER_TYPE_DELETE: i32 = 1 << 3; // 8
    const TRIGGER_TYPE_UPDATE: i32 = 1 << 4; // 16
    const TRIGGER_TYPE_TRUNCATE: i32 = 1 << 5; // 32

    let mut out = Vec::new();
    if events & TRIGGER_TYPE_INSERT != 0 {
        out.push(TriggerEvent::Insert);
    }
    if events & TRIGGER_TYPE_UPDATE != 0 {
        let cols = update_columns
            .iter()
            .filter_map(|n| match n.node.as_ref() {
                Some(NodeEnum::String(s)) => Identifier::from_unquoted(&s.sval).ok(),
                _ => None,
            })
            .collect();
        out.push(TriggerEvent::Update { columns: cols });
    }
    if events & TRIGGER_TYPE_DELETE != 0 {
        out.push(TriggerEvent::Delete);
    }
    if events & TRIGGER_TYPE_TRUNCATE != 0 {
        out.push(TriggerEvent::Truncate);
    }
    if out.is_empty() {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("{qname}: trigger declares no events (bits=0x{events:x})"),
        });
    }
    Ok(out)
}

fn parse_transition_rels(
    rels: &[pg_query::protobuf::Node],
    location: &SourceLocation,
    qname: &QualifiedName,
) -> Result<Vec<TransitionTable>, ParseError> {
    let mut out = Vec::new();
    for node in rels {
        let Some(NodeEnum::TriggerTransition(trans)) = node.node.as_ref() else {
            continue;
        };
        let name = Identifier::from_unquoted(&trans.name).map_err(|e| ParseError::Structural {
            location: location.clone(),
            message: format!(
                "{qname}: invalid transition table name '{}': {e}",
                trans.name
            ),
        })?;
        let kind = if trans.is_new {
            TransitionKind::NewTable
        } else {
            TransitionKind::OldTable
        };
        // `is_table` should always be true (PG only allows TABLE form, not ROW).
        if !trans.is_table {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("{qname}: REFERENCING must use NEW/OLD TABLE form."),
            });
        }
        out.push(TransitionTable { name, kind });
    }
    Ok(out)
}

/// Convert a raw `Node` from the WHEN clause into a [`NormalizedExpr`].
///
/// Uses the same SELECT-scaffold deparse technique as
/// [`crate::parse::normalize_expr::from_pg_node`]: wraps the expression in
/// `SELECT <expr>`, deparses the whole statement, strips the `SELECT ` prefix,
/// then lowercases reserved keywords before hashing.
fn node_to_normalized_expr(
    node: &Node,
    location: &SourceLocation,
    qname: &QualifiedName,
) -> Result<NormalizedExpr, ParseError> {
    let inner_enum = node.node.as_ref().ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: format!("{qname}: WHEN clause node has no inner node"),
    })?;
    normalize_expr::from_pg_node(inner_enum, None, location).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("{qname}: failed to canonicalize WHEN clause: {e}"),
    })
}

fn range_var_to_qname(
    rv: Option<&pg_query::protobuf::RangeVar>,
    location: &SourceLocation,
    context: &str,
) -> Result<QualifiedName, ParseError> {
    let rv = rv.ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: format!("{context}: missing relation"),
    })?;
    if rv.schemaname.is_empty() {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("{context}: relation '{}' has no schema qualifier", rv.relname),
        });
    }
    let schema = Identifier::from_unquoted(&rv.schemaname).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("{context}: invalid schema '{}': {e}", rv.schemaname),
    })?;
    let name = Identifier::from_unquoted(&rv.relname).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("{context}: invalid name '{}': {e}", rv.relname),
    })?;
    Ok(QualifiedName::new(schema, name))
}

fn qualified_name_from_list(
    nodes: &[pg_query::protobuf::Node],
    location: &SourceLocation,
    context: &str,
) -> Result<QualifiedName, ParseError> {
    let parts: Vec<String> = nodes
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(NodeEnum::String(s)) => Some(s.sval.clone()),
            _ => None,
        })
        .collect();
    let (schema_str, name_str) = match parts.as_slice() {
        [s, n] => (s.clone(), n.clone()),
        [n] => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("{context}: '{n}' must be schema-qualified"),
            });
        }
        _ => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("{context}: unexpected name shape {parts:?}"),
            });
        }
    };
    let schema = Identifier::from_unquoted(&schema_str).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("{context}: invalid schema '{schema_str}': {e}"),
    })?;
    let name = Identifier::from_unquoted(&name_str).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("{context}: invalid name '{name_str}': {e}"),
    })?;
    Ok(QualifiedName::new(schema, name))
}

fn string_list(nodes: &[pg_query::protobuf::Node]) -> Vec<String> {
    nodes
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(NodeEnum::String(s)) => Some(s.sval.clone()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc() -> SourceLocation {
        SourceLocation::new(std::path::PathBuf::from("<test>"), 1, 1)
    }

    fn parse_trigger(sql: &str) -> Result<Trigger, ParseError> {
        let parsed = pg_query::parse(sql).expect("pg_query");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .expect("at least one statement")
            .stmt
            .expect("stmt")
            .node
            .expect("node");
        match stmt {
            NodeEnum::CreateTrigStmt(s) => build_trigger(&s, &loc()),
            other => panic!("expected CreateTrigStmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_simple_before_insert_row_trigger() {
        let t = parse_trigger(
            "CREATE TRIGGER t1 BEFORE INSERT ON app.users \
             FOR EACH ROW EXECUTE FUNCTION app.f();",
        )
        .unwrap();
        assert_eq!(t.qname.to_string(), "app.t1");
        assert_eq!(t.table.to_string(), "app.users");
        assert!(matches!(t.timing, TriggerTiming::Before));
        assert_eq!(t.events, vec![TriggerEvent::Insert]);
        assert!(matches!(t.level, TriggerLevel::Row));
        assert!(t.when_clause.is_none());
        assert_eq!(t.function_qname.to_string(), "app.f");
        assert!(!t.is_constraint);
    }

    #[test]
    fn parses_after_statement_trigger() {
        let t = parse_trigger(
            "CREATE TRIGGER t2 AFTER INSERT ON app.users \
             FOR EACH STATEMENT EXECUTE FUNCTION app.f();",
        )
        .unwrap();
        assert!(matches!(t.timing, TriggerTiming::After));
        assert!(matches!(t.level, TriggerLevel::Statement));
    }

    #[test]
    fn parses_multi_event_trigger() {
        let t = parse_trigger(
            "CREATE TRIGGER t BEFORE INSERT OR UPDATE OR DELETE ON app.users \
             FOR EACH ROW EXECUTE FUNCTION app.f();",
        )
        .unwrap();
        assert_eq!(t.events.len(), 3);
        assert!(t.events.contains(&TriggerEvent::Insert));
        assert!(t.events.contains(&TriggerEvent::Delete));
        assert!(matches!(t.events[1], TriggerEvent::Update { .. }));
    }

    #[test]
    fn parses_update_of_columns() {
        let t = parse_trigger(
            "CREATE TRIGGER t BEFORE UPDATE OF a, b ON app.users \
             FOR EACH ROW EXECUTE FUNCTION app.f();",
        )
        .unwrap();
        let cols = match &t.events[0] {
            TriggerEvent::Update { columns } => columns,
            other => panic!("expected Update, got {other:?}"),
        };
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].as_str(), "a");
        assert_eq!(cols[1].as_str(), "b");
    }

    #[test]
    fn parses_constraint_trigger_deferrable_initially_deferred() {
        let t = parse_trigger(
            "CREATE CONSTRAINT TRIGGER t AFTER INSERT ON app.users \
             DEFERRABLE INITIALLY DEFERRED \
             FOR EACH ROW EXECUTE FUNCTION app.f();",
        )
        .unwrap();
        assert!(t.is_constraint);
        assert!(matches!(
            t.deferrable,
            Deferrable::Deferrable {
                initially_deferred: true
            }
        ));
    }

    #[test]
    fn rejects_constraint_trigger_with_before_timing() {
        // PostgreSQL itself rejects CONSTRAINT TRIGGER BEFORE at parse time,
        // so we construct the AST directly to exercise our validation path.
        use pg_query::protobuf::{CreateTrigStmt, RangeVar};
        // timing = BEFORE (bit 1 = 2), events = INSERT (bit 2 = 4)
        let stmt = CreateTrigStmt {
            replace: false,
            isconstraint: true,
            trigname: "t".into(),
            relation: Some(RangeVar {
                catalogname: String::new(),
                schemaname: "app".into(),
                relname: "users".into(),
                inh: true,
                relpersistence: "p".into(),
                alias: None,
                location: -1,
            }),
            funcname: {
                // build app.f node list
                let s1 = pg_query::protobuf::Node {
                    node: Some(NodeEnum::String(pg_query::protobuf::String {
                        sval: "app".into(),
                    })),
                };
                let s2 = pg_query::protobuf::Node {
                    node: Some(NodeEnum::String(pg_query::protobuf::String {
                        sval: "f".into(),
                    })),
                };
                vec![s1, s2]
            },
            args: vec![],
            row: true,
            timing: 2, // BEFORE
            events: 4, // INSERT
            columns: vec![],
            when_clause: None,
            transition_rels: vec![],
            deferrable: false,
            initdeferred: false,
            constrrel: None,
        };
        let err = build_trigger(&stmt, &loc()).unwrap_err();
        assert!(
            err.to_string().contains("CONSTRAINT TRIGGER must be AFTER"),
            "got {err}"
        );
    }

    #[test]
    fn parses_when_clause() {
        let t = parse_trigger(
            "CREATE TRIGGER t BEFORE INSERT ON app.users \
             FOR EACH ROW WHEN (NEW.id > 0) EXECUTE FUNCTION app.f();",
        )
        .unwrap();
        assert!(t.when_clause.is_some());
    }

    #[test]
    fn parses_transition_tables() {
        let t = parse_trigger(
            "CREATE TRIGGER t AFTER INSERT ON app.users \
             REFERENCING NEW TABLE AS new_rows \
             FOR EACH STATEMENT EXECUTE FUNCTION app.f();",
        )
        .unwrap();
        assert_eq!(t.transition_tables.len(), 1);
        assert_eq!(t.transition_tables[0].name.as_str(), "new_rows");
        assert!(matches!(
            t.transition_tables[0].kind,
            TransitionKind::NewTable
        ));
    }

    #[test]
    fn execute_procedure_is_synonym_for_execute_function() {
        let t = parse_trigger(
            "CREATE TRIGGER t BEFORE INSERT ON app.users \
             FOR EACH ROW EXECUTE PROCEDURE app.f();",
        )
        .unwrap();
        assert_eq!(t.function_qname.to_string(), "app.f");
    }
}
