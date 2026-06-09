//! Dispatcher for `Change::Aggregate(AggregateChange)`.
//!
//! Aggregates carry no data, so every step here is `Safe` except a `Drop`
//! that the entry-level destructiveness flags as requiring approval — in
//! which case the flag/reason are plumbed through unchanged, mirroring the
//! event-trigger emitter.

use crate::diff::change::AggregateChange;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::aggregate::Aggregate;
use crate::ir::column_type::ColumnType;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

pub fn emit(
    agg: AggregateChange,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    match agg {
        AggregateChange::Create(a) => {
            emit_create(&a, destructive, destructive_reason, out);
        }
        AggregateChange::Replace { from, to } => {
            // First: drop the old. Aggregates carry no data, so the drop is
            // safe regardless of the entry-level flag.
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropAggregate,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![from.qname.clone()],
                sql: drop_sql(&from.qname, &from.arg_types),
                transactional: TransactionConstraint::InTransaction,
            });
            // Then: create the new (safe) plus follow-ups.
            emit_create(&to, false, None, out);
        }
        AggregateChange::Drop { qname, arg_types } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropAggregate,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: drop_sql(&qname, &arg_types),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        AggregateChange::AlterOwner {
            qname,
            arg_types,
            owner,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterAggregateOwner,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: alter_owner_sql(&qname, &arg_types, &owner),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        AggregateChange::CommentOn {
            qname,
            arg_types,
            comment,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnAggregate,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: comment_sql(&qname, &arg_types, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}

/// Emit a CREATE step plus optional follow-up steps for owner/comment.
///
/// `CREATE AGGREGATE` has no inline OWNER or COMMENT clause; both are issued
/// as separate ALTER / COMMENT steps when the desired state requires them.
fn emit_create(
    agg: &Aggregate,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::CreateAggregate,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: vec![agg.qname.clone()],
        sql: create_sql(agg),
        transactional: TransactionConstraint::InTransaction,
    });
    if let Some(owner) = &agg.owner {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterAggregateOwner,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![agg.qname.clone()],
            sql: alter_owner_sql(&agg.qname, &agg.arg_types, owner),
            transactional: TransactionConstraint::InTransaction,
        });
    }
    if let Some(comment) = &agg.comment {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::CommentOnAggregate,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![agg.qname.clone()],
            sql: comment_sql(&agg.qname, &agg.arg_types, Some(comment.as_str())),
            transactional: TransactionConstraint::InTransaction,
        });
    }
}

// ---------------------------------------------------------------------------
// SQL helpers
// ---------------------------------------------------------------------------

/// Render the parenthesized argument-type list.
///
/// Postgres requires `(*)` for a zero-argument aggregate; otherwise the
/// comma-separated rendered types.
fn arg_list(arg_types: &[ColumnType]) -> String {
    if arg_types.is_empty() {
        return "(*)".to_string();
    }
    let mut s = String::from("(");
    let mut first = true;
    for t in arg_types {
        if !first {
            s.push_str(", ");
        }
        first = false;
        s.push_str(&t.render_sql());
    }
    s.push(')');
    s
}

/// `CREATE AGGREGATE qname(args) (SFUNC = …, STYPE = …[, FINALFUNC = …][, INITCOND = '…']);`
fn create_sql(agg: &Aggregate) -> String {
    let mut sql = format!(
        "CREATE AGGREGATE {}{} (SFUNC = {}, STYPE = {}",
        agg.qname.render_sql(),
        arg_list(&agg.arg_types),
        agg.sfunc.render_sql(),
        agg.state_type.render_sql(),
    );
    if let Some(finalfunc) = &agg.finalfunc {
        sql.push_str(&format!(", FINALFUNC = {}", finalfunc.render_sql()));
    }
    if let Some(initcond) = &agg.initcond {
        sql.push_str(&format!(
            ", INITCOND = '{}'",
            crate::plan::rewrite::sql::escape_sql_literal_body(initcond)
        ));
    }
    sql.push_str(");");
    sql
}

/// `DROP AGGREGATE qname(args);`
fn drop_sql(qname: &QualifiedName, arg_types: &[ColumnType]) -> String {
    format!(
        "DROP AGGREGATE {}{};",
        qname.render_sql(),
        arg_list(arg_types)
    )
}

/// `ALTER AGGREGATE qname(args) OWNER TO owner;`
fn alter_owner_sql(qname: &QualifiedName, arg_types: &[ColumnType], owner: &Identifier) -> String {
    format!(
        "ALTER AGGREGATE {}{} OWNER TO {};",
        qname.render_sql(),
        arg_list(arg_types),
        owner.render_sql(),
    )
}

/// `COMMENT ON AGGREGATE qname(args) IS '...';` or `... IS NULL;`
fn comment_sql(qname: &QualifiedName, arg_types: &[ColumnType], comment: Option<&str>) -> String {
    match comment {
        Some(c) => format!(
            "COMMENT ON AGGREGATE {}{} IS '{}';",
            qname.render_sql(),
            arg_list(arg_types),
            crate::plan::rewrite::sql::escape_sql_literal_body(c),
        ),
        None => format!(
            "COMMENT ON AGGREGATE {}{} IS NULL;",
            qname.render_sql(),
            arg_list(arg_types),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    /// `app.my_sum(integer)` with `STYPE = bigint`, `SFUNC = app.sf`, no
    /// finalfunc / initcond / owner / comment.
    fn make_agg() -> Aggregate {
        Aggregate {
            qname: qname("app", "my_sum"),
            arg_types: vec![ColumnType::Integer],
            state_type: ColumnType::BigInt,
            sfunc: qname("app", "sf"),
            finalfunc: None,
            initcond: None,
            owner: None,
            comment: None,
        }
    }

    // --- create_sql ---

    #[test]
    fn create_sql_simple() {
        let agg = make_agg();
        let sql = create_sql(&agg);
        assert_eq!(
            sql,
            "CREATE AGGREGATE app.my_sum(integer) (SFUNC = app.sf, STYPE = bigint);"
        );
    }

    #[test]
    fn create_sql_with_finalfunc_and_initcond() {
        let mut agg = make_agg();
        agg.finalfunc = Some(qname("app", "ff"));
        agg.initcond = Some("0".to_string());
        let sql = create_sql(&agg);
        assert_eq!(
            sql,
            "CREATE AGGREGATE app.my_sum(integer) (SFUNC = app.sf, STYPE = bigint, FINALFUNC = app.ff, INITCOND = '0');"
        );
    }

    #[test]
    fn create_sql_initcond_escapes_single_quote() {
        let mut agg = make_agg();
        agg.initcond = Some("O'Brien".to_string());
        let sql = create_sql(&agg);
        assert!(sql.contains("INITCOND = 'O''Brien'"), "got: {sql}");
    }

    #[test]
    fn create_sql_zero_arg_uses_star() {
        let mut agg = make_agg();
        agg.arg_types = vec![];
        let sql = create_sql(&agg);
        assert_eq!(
            sql,
            "CREATE AGGREGATE app.my_sum(*) (SFUNC = app.sf, STYPE = bigint);"
        );
    }

    #[test]
    fn create_sql_multi_arg() {
        let mut agg = make_agg();
        agg.arg_types = vec![ColumnType::Integer, ColumnType::Text];
        let sql = create_sql(&agg);
        assert!(sql.contains("app.my_sum(integer, text)"), "got: {sql}");
    }

    // --- arg_list ---

    #[test]
    fn arg_list_empty_is_star() {
        assert_eq!(arg_list(&[]), "(*)");
    }

    #[test]
    fn arg_list_single() {
        assert_eq!(arg_list(&[ColumnType::Integer]), "(integer)");
    }

    #[test]
    fn arg_list_multiple() {
        assert_eq!(
            arg_list(&[ColumnType::Integer, ColumnType::BigInt]),
            "(integer, bigint)"
        );
    }

    // --- drop_sql ---

    #[test]
    fn drop_sql_renders_correctly() {
        let sql = drop_sql(&qname("app", "my_sum"), &[ColumnType::Integer]);
        assert_eq!(sql, "DROP AGGREGATE app.my_sum(integer);");
    }

    // --- alter_owner_sql ---

    #[test]
    fn alter_owner_renders_correctly() {
        let sql = alter_owner_sql(
            &qname("app", "my_sum"),
            &[ColumnType::Integer],
            &id("app_owner"),
        );
        assert_eq!(
            sql,
            "ALTER AGGREGATE app.my_sum(integer) OWNER TO app_owner;"
        );
    }

    // --- comment_sql ---

    #[test]
    fn comment_sql_set() {
        let sql = comment_sql(
            &qname("app", "my_sum"),
            &[ColumnType::Integer],
            Some("a sum"),
        );
        assert_eq!(sql, "COMMENT ON AGGREGATE app.my_sum(integer) IS 'a sum';");
    }

    #[test]
    fn comment_sql_clear_is_null() {
        let sql = comment_sql(&qname("app", "my_sum"), &[ColumnType::Integer], None);
        assert_eq!(sql, "COMMENT ON AGGREGATE app.my_sum(integer) IS NULL;");
    }

    #[test]
    fn comment_sql_escapes_single_quotes() {
        let sql = comment_sql(
            &qname("app", "my_sum"),
            &[ColumnType::Integer],
            Some("O'Brien"),
        );
        assert!(sql.contains("IS 'O''Brien'"), "got: {sql}");
    }

    // --- emit() integration ---

    #[test]
    fn emit_create_simple_produces_one_step() {
        let agg = make_agg();
        let mut out = Vec::new();
        emit(AggregateChange::Create(agg), false, None, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CreateAggregate);
        assert!(out[0].sql.contains("CREATE AGGREGATE"));
        assert!(!out[0].destructive);
    }

    #[test]
    fn emit_create_with_owner_and_comment_produces_three_steps() {
        let mut agg = make_agg();
        agg.owner = Some(id("app_owner"));
        agg.comment = Some("a sum".to_string());
        let mut out = Vec::new();
        emit(AggregateChange::Create(agg), false, None, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].kind, StepKind::CreateAggregate);
        assert_eq!(out[1].kind, StepKind::AlterAggregateOwner);
        assert!(out[1].sql.contains("OWNER TO app_owner"));
        assert_eq!(out[2].kind, StepKind::CommentOnAggregate);
        assert!(out[2].sql.contains("a sum"));
    }

    #[test]
    fn emit_drop_produces_one_step() {
        let mut out = Vec::new();
        emit(
            AggregateChange::Drop {
                qname: qname("app", "my_sum"),
                arg_types: vec![ColumnType::Integer],
            },
            true,
            Some("removing aggregate".to_string()),
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::DropAggregate);
        assert!(out[0].destructive);
        assert!(out[0].sql.contains("DROP AGGREGATE"));
    }

    #[test]
    fn emit_replace_first_step_is_drop() {
        let from = make_agg();
        let mut to = make_agg();
        to.state_type = ColumnType::Numeric { precision: None };
        let mut out = Vec::new();
        emit(AggregateChange::Replace { from, to }, true, None, &mut out);
        assert!(
            out.len() >= 2,
            "expected at least 2 steps, got {}",
            out.len()
        );
        assert_eq!(out[0].kind, StepKind::DropAggregate);
        // Aggregates carry no data: the drop in a Replace is always safe.
        assert!(!out[0].destructive);
        assert_eq!(out[1].kind, StepKind::CreateAggregate);
        assert!(!out[1].destructive);
    }

    #[test]
    fn emit_alter_owner_produces_one_step() {
        let mut out = Vec::new();
        emit(
            AggregateChange::AlterOwner {
                qname: qname("app", "my_sum"),
                arg_types: vec![ColumnType::Integer],
                owner: id("newrole"),
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::AlterAggregateOwner);
        assert!(out[0].sql.contains("OWNER TO newrole"));
    }

    #[test]
    fn emit_comment_on_set_produces_one_step() {
        let mut out = Vec::new();
        emit(
            AggregateChange::CommentOn {
                qname: qname("app", "my_sum"),
                arg_types: vec![ColumnType::Integer],
                comment: Some("my comment".to_string()),
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CommentOnAggregate);
        assert!(out[0].sql.contains("my comment"));
    }

    #[test]
    fn emit_comment_on_none_renders_is_null() {
        let mut out = Vec::new();
        emit(
            AggregateChange::CommentOn {
                qname: qname("app", "my_sum"),
                arg_types: vec![ColumnType::Integer],
                comment: None,
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CommentOnAggregate);
        assert!(out[0].sql.contains("IS NULL"));
    }
}
