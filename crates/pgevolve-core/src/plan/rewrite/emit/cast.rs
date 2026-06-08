//! Dispatcher for `Change::Cast(CastChange)`.
//!
//! Casts carry no user data (they are pure type-system objects), so every step
//! here is `Safe` — including `Drop` steps that arise from a `Replace`.  The
//! entry-level destructiveness flags are forwarded only to the outermost
//! `Drop` / `Create` in a plain `Drop` variant; a `Replace` always forces the
//! intermediate drop to `Safe` (no data loss possible).

use crate::diff::change::CastChange;
use crate::identifier::QualifiedName;
use crate::ir::cast::{Cast, CastContext, CastMethod};
use crate::ir::column_type::ColumnType;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

pub fn emit(
    cc: CastChange,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    match cc {
        CastChange::Create(cast) => {
            emit_create(&cast, destructive, destructive_reason, out);
        }
        CastChange::Replace { from, to } => {
            // Casts carry no user data, so the drop half of a Replace is safe.
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropCast,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: cast_targets(&from.source, &from.target),
                sql: drop_sql(&from.source, &from.target),
                transactional: TransactionConstraint::InTransaction,
            });
            emit_create(&to, false, None, out);
        }
        CastChange::Drop { source, target } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropCast,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: cast_targets(&source, &target),
                sql: drop_sql(&source, &target),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        CastChange::CommentOn {
            source,
            target,
            comment,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnCast,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: cast_targets(&source, &target),
                sql: comment_sql(&source, &target, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}

/// Emit a `CREATE CAST` step plus an optional follow-up `COMMENT ON CAST` step.
///
/// `CREATE CAST` has no inline `COMMENT` clause; the comment is issued as a
/// separate `COMMENT ON CAST` step when the desired state requires one.
fn emit_create(
    cast: &Cast,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::CreateCast,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: cast_targets(&cast.source, &cast.target),
        sql: create_sql(cast),
        transactional: TransactionConstraint::InTransaction,
    });
    if let Some(comment) = &cast.comment {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::CommentOnCast,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: cast_targets(&cast.source, &cast.target),
            sql: comment_sql(&cast.source, &cast.target, Some(comment.as_str())),
            transactional: TransactionConstraint::InTransaction,
        });
    }
}

// ---------------------------------------------------------------------------
// SQL helpers
// ---------------------------------------------------------------------------

/// Build the `targets` vec for a cast (source and target types).
fn cast_targets(source: &QualifiedName, target: &QualifiedName) -> Vec<QualifiedName> {
    vec![source.clone(), target.clone()]
}

/// Render the parenthesised arg-type list for a `WITH FUNCTION` cast.
///
/// Returns `"(arg1, arg2, …)"` — never empty because a conversion function
/// must take at least the source type as its first argument.
fn arg_list(arg_types: &[ColumnType]) -> String {
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

/// Render the context suffix appended after the method clause.
///
/// `Explicit` → no suffix; `Assignment` → ` AS ASSIGNMENT`; `Implicit` →
/// ` AS IMPLICIT`.
const fn context_suffix(context: CastContext) -> &'static str {
    match context {
        CastContext::Explicit => "",
        CastContext::Assignment => " AS ASSIGNMENT",
        CastContext::Implicit => " AS IMPLICIT",
    }
}

/// `CREATE CAST (source AS target) <method><context>;`
fn create_sql(cast: &Cast) -> String {
    let pair = format!(
        "({} AS {})",
        cast.source.render_sql(),
        cast.target.render_sql()
    );
    let method_clause = match &cast.method {
        CastMethod::Function { name, arg_types } => {
            format!("WITH FUNCTION {}{}", name.render_sql(), arg_list(arg_types))
        }
        CastMethod::Inout => "WITH INOUT".to_string(),
        CastMethod::Binary => "WITHOUT FUNCTION".to_string(),
    };
    format!(
        "CREATE CAST {pair} {method_clause}{suffix};",
        suffix = context_suffix(cast.context)
    )
}

/// `DROP CAST (source AS target);`
fn drop_sql(source: &QualifiedName, target: &QualifiedName) -> String {
    format!(
        "DROP CAST ({} AS {});",
        source.render_sql(),
        target.render_sql()
    )
}

/// `COMMENT ON CAST (source AS target) IS '...';` or `… IS NULL;`
fn comment_sql(source: &QualifiedName, target: &QualifiedName, comment: Option<&str>) -> String {
    let pair = format!("({} AS {})", source.render_sql(), target.render_sql());
    match comment {
        Some(c) => format!("COMMENT ON CAST {pair} IS '{}';", c.replace('\'', "''")),
        None => format!("COMMENT ON CAST {pair} IS NULL;"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn cast_function_explicit() -> Cast {
        Cast {
            source: qname("app", "my_type"),
            target: qname("pg_catalog", "text"),
            method: CastMethod::Function {
                name: qname("app", "my_type_to_text"),
                arg_types: vec![ColumnType::Integer],
            },
            context: CastContext::Explicit,
            comment: None,
        }
    }

    fn cast_inout_assignment() -> Cast {
        Cast {
            source: qname("app", "dom_a"),
            target: qname("app", "dom_b"),
            method: CastMethod::Inout,
            context: CastContext::Assignment,
            comment: None,
        }
    }

    // --- create_sql ---

    #[test]
    fn create_sql_with_function_explicit() {
        let cast = cast_function_explicit();
        let sql = create_sql(&cast);
        assert_eq!(
            sql,
            "CREATE CAST (app.my_type AS pg_catalog.text) WITH FUNCTION app.my_type_to_text(integer);"
        );
    }

    #[test]
    fn create_sql_with_function_assignment() {
        let mut cast = cast_function_explicit();
        cast.context = CastContext::Assignment;
        let sql = create_sql(&cast);
        assert_eq!(
            sql,
            "CREATE CAST (app.my_type AS pg_catalog.text) WITH FUNCTION app.my_type_to_text(integer) AS ASSIGNMENT;"
        );
    }

    #[test]
    fn create_sql_with_function_implicit() {
        let mut cast = cast_function_explicit();
        cast.context = CastContext::Implicit;
        let sql = create_sql(&cast);
        assert_eq!(
            sql,
            "CREATE CAST (app.my_type AS pg_catalog.text) WITH FUNCTION app.my_type_to_text(integer) AS IMPLICIT;"
        );
    }

    #[test]
    fn create_sql_without_function() {
        let cast = Cast {
            source: qname("app", "type_x"),
            target: qname("app", "type_y"),
            method: CastMethod::Binary,
            context: CastContext::Explicit,
            comment: None,
        };
        let sql = create_sql(&cast);
        assert_eq!(
            sql,
            "CREATE CAST (app.type_x AS app.type_y) WITHOUT FUNCTION;"
        );
    }

    #[test]
    fn create_sql_with_inout_assignment() {
        let cast = cast_inout_assignment();
        let sql = create_sql(&cast);
        assert_eq!(
            sql,
            "CREATE CAST (app.dom_a AS app.dom_b) WITH INOUT AS ASSIGNMENT;"
        );
    }

    #[test]
    fn create_sql_with_inout_implicit() {
        let mut cast = cast_inout_assignment();
        cast.context = CastContext::Implicit;
        let sql = create_sql(&cast);
        assert_eq!(
            sql,
            "CREATE CAST (app.dom_a AS app.dom_b) WITH INOUT AS IMPLICIT;"
        );
    }

    // --- drop_sql ---

    #[test]
    fn drop_sql_renders_correctly() {
        let sql = drop_sql(&qname("app", "my_type"), &qname("pg_catalog", "text"));
        assert_eq!(sql, "DROP CAST (app.my_type AS pg_catalog.text);");
    }

    // --- comment_sql ---

    #[test]
    fn comment_sql_set() {
        let sql = comment_sql(
            &qname("app", "my_type"),
            &qname("pg_catalog", "text"),
            Some("converts my_type to text"),
        );
        assert_eq!(
            sql,
            "COMMENT ON CAST (app.my_type AS pg_catalog.text) IS 'converts my_type to text';"
        );
    }

    #[test]
    fn comment_sql_clear_is_null() {
        let sql = comment_sql(&qname("app", "my_type"), &qname("pg_catalog", "text"), None);
        assert_eq!(
            sql,
            "COMMENT ON CAST (app.my_type AS pg_catalog.text) IS NULL;"
        );
    }

    #[test]
    fn comment_sql_escapes_single_quotes() {
        let sql = comment_sql(
            &qname("app", "my_type"),
            &qname("pg_catalog", "text"),
            Some("O'Brien's cast"),
        );
        assert!(sql.contains("IS 'O''Brien''s cast'"), "got: {sql}");
    }

    // --- emit() integration ---

    #[test]
    fn emit_create_simple_produces_one_step() {
        let cast = cast_function_explicit();
        let mut out = Vec::new();
        emit(CastChange::Create(cast), false, None, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CreateCast);
        assert!(out[0].sql.contains("CREATE CAST"));
        assert!(!out[0].destructive);
    }

    #[test]
    fn emit_create_with_comment_produces_two_steps() {
        let mut cast = cast_function_explicit();
        cast.comment = Some("a cast".to_string());
        let mut out = Vec::new();
        emit(CastChange::Create(cast), false, None, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, StepKind::CreateCast);
        assert_eq!(out[1].kind, StepKind::CommentOnCast);
        assert!(out[1].sql.contains("a cast"));
    }

    #[test]
    fn emit_drop_produces_one_step() {
        let mut out = Vec::new();
        emit(
            CastChange::Drop {
                source: qname("app", "my_type"),
                target: qname("pg_catalog", "text"),
            },
            true,
            Some("removing cast".to_string()),
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::DropCast);
        assert!(out[0].destructive);
        assert!(out[0].sql.contains("DROP CAST"));
    }

    #[test]
    fn emit_replace_emits_drop_then_create() {
        let from = cast_function_explicit();
        let mut to = cast_function_explicit();
        to.context = CastContext::Implicit;
        let mut out = Vec::new();
        emit(CastChange::Replace { from, to }, true, None, &mut out);
        assert!(
            out.len() >= 2,
            "expected at least 2 steps, got {}",
            out.len()
        );
        assert_eq!(out[0].kind, StepKind::DropCast);
        // Replace drop is always safe — casts carry no user data.
        assert!(!out[0].destructive);
        assert_eq!(out[1].kind, StepKind::CreateCast);
        assert!(!out[1].destructive);
        assert!(out[1].sql.contains("AS IMPLICIT"));
    }

    #[test]
    fn emit_comment_on_set_produces_one_step() {
        let mut out = Vec::new();
        emit(
            CastChange::CommentOn {
                source: qname("app", "my_type"),
                target: qname("pg_catalog", "text"),
                comment: Some("my comment".to_string()),
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CommentOnCast);
        assert!(out[0].sql.contains("my comment"));
    }

    #[test]
    fn emit_comment_on_none_renders_is_null() {
        let mut out = Vec::new();
        emit(
            CastChange::CommentOn {
                source: qname("app", "my_type"),
                target: qname("pg_catalog", "text"),
                comment: None,
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CommentOnCast);
        assert!(out[0].sql.contains("IS NULL"));
    }
}
