//! Dispatcher for `Change::TsDictionary(TsDictionaryChange)`.
//!
//! All steps are `Safe` and run `InTransaction`. `Replace` decomposes into a
//! `Drop` followed by the full `Create` sequence (same pattern as aggregates),
//! because Postgres provides no in-place ALTER for the `template` field.

use crate::diff::change::TsDictionaryChange;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::text_search::TsDictionary;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

pub fn emit(
    change: TsDictionaryChange,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    match change {
        TsDictionaryChange::Create(d) => {
            emit_create(&d, destructive, destructive_reason, out);
        }
        TsDictionaryChange::Replace { from, to } => {
            // Drop the old (no data, so never destructive in this path).
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropTsDictionary,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![from.qname.clone()],
                sql: drop_sql(&from.qname),
                transactional: TransactionConstraint::InTransaction,
            });
            // Create the new (safe) plus follow-up owner/comment steps.
            emit_create(&to, false, None, out);
        }
        TsDictionaryChange::Drop { qname } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropTsDictionary,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: drop_sql(&qname),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        TsDictionaryChange::AlterOptions { qname, options } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterTsDictionary,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: alter_options_sql(&qname, &options),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        TsDictionaryChange::AlterOwner { qname, owner } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterTsDictionaryOwner,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: alter_owner_sql(&qname, &owner),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        TsDictionaryChange::CommentOn { qname, comment } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnTsDictionary,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: comment_sql(&qname, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}

/// Emit a `CREATE TEXT SEARCH DICTIONARY` step plus optional follow-up
/// `OWNER TO` and `COMMENT ON` steps.
///
/// `CREATE TEXT SEARCH DICTIONARY` has no inline `OWNER` or `COMMENT` clause;
/// both are issued as separate `ALTER … OWNER TO` / `COMMENT ON …` steps when
/// the desired state requires them — mirroring the aggregate emitter.
fn emit_create(
    dict: &TsDictionary,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::CreateTsDictionary,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: vec![dict.qname.clone()],
        sql: create_sql(dict),
        transactional: TransactionConstraint::InTransaction,
    });
    if let Some(owner) = &dict.owner {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterTsDictionaryOwner,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![dict.qname.clone()],
            sql: alter_owner_sql(&dict.qname, owner),
            transactional: TransactionConstraint::InTransaction,
        });
    }
    if let Some(comment) = &dict.comment {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::CommentOnTsDictionary,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![dict.qname.clone()],
            sql: comment_sql(&dict.qname, Some(comment.as_str())),
            transactional: TransactionConstraint::InTransaction,
        });
    }
}

// ---------------------------------------------------------------------------
// SQL helpers
// ---------------------------------------------------------------------------

/// `CREATE TEXT SEARCH DICTIONARY qname (TEMPLATE = template[, key = 'val', …]);`
fn create_sql(dict: &TsDictionary) -> String {
    let mut sql = format!(
        "CREATE TEXT SEARCH DICTIONARY {} (TEMPLATE = {}",
        dict.qname.render_sql(),
        dict.template.render_sql(),
    );
    for (key, val) in &dict.options {
        sql.push_str(&format!(
            ", {} = '{}'",
            key,
            crate::plan::rewrite::sql::escape_sql_literal_body(val)
        ));
    }
    sql.push_str(");");
    sql
}

/// `ALTER TEXT SEARCH DICTIONARY qname (key = 'val', …);`
fn alter_options_sql(qname: &QualifiedName, options: &[(String, String)]) -> String {
    let mut sql = format!("ALTER TEXT SEARCH DICTIONARY {} (", qname.render_sql());
    let mut first = true;
    for (key, val) in options {
        if !first {
            sql.push_str(", ");
        }
        first = false;
        sql.push_str(&format!(
            "{} = '{}'",
            key,
            crate::plan::rewrite::sql::escape_sql_literal_body(val)
        ));
    }
    sql.push_str(");");
    sql
}

/// `DROP TEXT SEARCH DICTIONARY qname;`
fn drop_sql(qname: &QualifiedName) -> String {
    format!("DROP TEXT SEARCH DICTIONARY {};", qname.render_sql())
}

/// `ALTER TEXT SEARCH DICTIONARY qname OWNER TO owner;`
fn alter_owner_sql(qname: &QualifiedName, owner: &Identifier) -> String {
    format!(
        "ALTER TEXT SEARCH DICTIONARY {} OWNER TO {};",
        qname.render_sql(),
        owner.render_sql(),
    )
}

/// `COMMENT ON TEXT SEARCH DICTIONARY qname IS '...';` or `IS NULL;`
fn comment_sql(qname: &QualifiedName, comment: Option<&str>) -> String {
    match comment {
        Some(c) => format!(
            "COMMENT ON TEXT SEARCH DICTIONARY {} IS '{}';",
            qname.render_sql(),
            crate::plan::rewrite::sql::escape_sql_literal_body(c),
        ),
        None => format!(
            "COMMENT ON TEXT SEARCH DICTIONARY {} IS NULL;",
            qname.render_sql(),
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

    /// A `public.english_stem` dictionary using the `pg_catalog.snowball` template
    /// with no options, owner, or comment.
    fn make_dict() -> TsDictionary {
        TsDictionary {
            qname: qname("public", "english_stem"),
            template: qname("pg_catalog", "snowball"),
            options: vec![],
            owner: None,
            comment: None,
        }
    }

    // --- create_sql ---

    #[test]
    fn create_sql_template_only() {
        let dict = make_dict();
        let sql = create_sql(&dict);
        assert_eq!(
            sql,
            "CREATE TEXT SEARCH DICTIONARY public.english_stem (TEMPLATE = pg_catalog.snowball);"
        );
    }

    #[test]
    fn create_sql_with_two_options() {
        let mut dict = make_dict();
        dict.options = vec![
            ("language".to_string(), "english".to_string()),
            ("stopwords".to_string(), "english".to_string()),
        ];
        let sql = create_sql(&dict);
        assert_eq!(
            sql,
            "CREATE TEXT SEARCH DICTIONARY public.english_stem \
             (TEMPLATE = pg_catalog.snowball, language = 'english', stopwords = 'english');"
        );
    }

    #[test]
    fn create_sql_option_value_escapes_single_quote() {
        let mut dict = make_dict();
        dict.options = vec![("stopwords".to_string(), "O'Brien".to_string())];
        let sql = create_sql(&dict);
        assert!(sql.contains("stopwords = 'O''Brien'"), "got: {sql}");
    }

    // --- emit: create with owner and comment produces 3 steps ---

    #[test]
    fn emit_create_owner_comment_produces_three_steps() {
        let mut dict = make_dict();
        dict.owner = Some(id("app_owner"));
        dict.comment = Some("English snowball stemmer.".to_string());
        let mut out = Vec::new();
        emit(TsDictionaryChange::Create(dict), false, None, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].kind, StepKind::CreateTsDictionary);
        assert!(out[0].sql.starts_with("CREATE TEXT SEARCH DICTIONARY"));
        assert_eq!(out[1].kind, StepKind::AlterTsDictionaryOwner);
        assert!(out[1].sql.contains("OWNER TO app_owner"));
        assert_eq!(out[2].kind, StepKind::CommentOnTsDictionary);
        assert!(out[2].sql.contains("English snowball stemmer"));
    }

    // --- emit: create without owner/comment produces 1 step ---

    #[test]
    fn emit_create_simple_produces_one_step() {
        let dict = make_dict();
        let mut out = Vec::new();
        emit(TsDictionaryChange::Create(dict), false, None, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CreateTsDictionary);
        assert!(!out[0].destructive);
    }

    // --- alter_options_sql ---

    #[test]
    fn alter_options_sql_two_options() {
        let sql = alter_options_sql(
            &qname("public", "english_stem"),
            &[
                ("language".to_string(), "english".to_string()),
                ("stopwords".to_string(), "english".to_string()),
            ],
        );
        assert_eq!(
            sql,
            "ALTER TEXT SEARCH DICTIONARY public.english_stem \
             (language = 'english', stopwords = 'english');"
        );
    }

    #[test]
    fn alter_options_sql_escapes_single_quote() {
        let sql = alter_options_sql(
            &qname("public", "english_stem"),
            &[("stopwords".to_string(), "O'Brien".to_string())],
        );
        assert!(sql.contains("stopwords = 'O''Brien'"), "got: {sql}");
    }

    // --- emit: AlterOptions ---

    #[test]
    fn emit_alter_options_produces_one_step() {
        let mut out = Vec::new();
        emit(
            TsDictionaryChange::AlterOptions {
                qname: qname("public", "english_stem"),
                options: vec![
                    ("language".to_string(), "english".to_string()),
                    ("stopwords".to_string(), "english".to_string()),
                ],
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::AlterTsDictionary);
        assert!(
            out[0].sql.contains("language = 'english'"),
            "got: {}",
            out[0].sql
        );
    }

    // --- drop_sql ---

    #[test]
    fn drop_sql_renders_correctly() {
        let sql = drop_sql(&qname("public", "english_stem"));
        assert_eq!(sql, "DROP TEXT SEARCH DICTIONARY public.english_stem;");
    }

    // --- emit: Drop ---

    #[test]
    fn emit_drop_produces_one_step() {
        let mut out = Vec::new();
        emit(
            TsDictionaryChange::Drop {
                qname: qname("public", "english_stem"),
            },
            true,
            Some("removing dictionary".to_string()),
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::DropTsDictionary);
        assert!(out[0].destructive);
        assert!(out[0].sql.contains("DROP TEXT SEARCH DICTIONARY"));
    }

    // --- alter_owner_sql ---

    #[test]
    fn alter_owner_sql_renders_correctly() {
        let sql = alter_owner_sql(&qname("public", "english_stem"), &id("app_owner"));
        assert_eq!(
            sql,
            "ALTER TEXT SEARCH DICTIONARY public.english_stem OWNER TO app_owner;"
        );
    }

    // --- emit: AlterOwner ---

    #[test]
    fn emit_alter_owner_produces_one_step() {
        let mut out = Vec::new();
        emit(
            TsDictionaryChange::AlterOwner {
                qname: qname("public", "english_stem"),
                owner: id("newrole"),
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::AlterTsDictionaryOwner);
        assert!(out[0].sql.contains("OWNER TO newrole"));
    }

    // --- comment_sql ---

    #[test]
    fn comment_sql_set() {
        let sql = comment_sql(&qname("public", "english_stem"), Some("a dict"));
        assert_eq!(
            sql,
            "COMMENT ON TEXT SEARCH DICTIONARY public.english_stem IS 'a dict';"
        );
    }

    #[test]
    fn comment_sql_clear_is_null() {
        let sql = comment_sql(&qname("public", "english_stem"), None);
        assert_eq!(
            sql,
            "COMMENT ON TEXT SEARCH DICTIONARY public.english_stem IS NULL;"
        );
    }

    #[test]
    fn comment_sql_escapes_single_quotes() {
        let sql = comment_sql(&qname("public", "english_stem"), Some("O'Brien dict"));
        assert!(sql.contains("IS 'O''Brien dict'"), "got: {sql}");
    }

    // --- emit: CommentOn (set and NULL) ---

    #[test]
    fn emit_comment_on_set_produces_one_step() {
        let mut out = Vec::new();
        emit(
            TsDictionaryChange::CommentOn {
                qname: qname("public", "english_stem"),
                comment: Some("my comment".to_string()),
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CommentOnTsDictionary);
        assert!(out[0].sql.contains("my comment"));
    }

    #[test]
    fn emit_comment_on_none_renders_is_null() {
        let mut out = Vec::new();
        emit(
            TsDictionaryChange::CommentOn {
                qname: qname("public", "english_stem"),
                comment: None,
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CommentOnTsDictionary);
        assert!(out[0].sql.contains("IS NULL"));
    }

    // --- emit: Replace = drop then create ---

    #[test]
    fn emit_replace_first_step_is_drop_second_is_create() {
        let from = make_dict();
        let mut to = make_dict();
        to.template = qname("pg_catalog", "ispell");
        let mut out = Vec::new();
        emit(
            TsDictionaryChange::Replace { from, to },
            true,
            None,
            &mut out,
        );
        assert!(
            out.len() >= 2,
            "expected at least 2 steps, got {}",
            out.len()
        );
        assert_eq!(out[0].kind, StepKind::DropTsDictionary);
        // Dictionaries carry no data: the drop in a Replace is always safe.
        assert!(!out[0].destructive);
        assert_eq!(out[1].kind, StepKind::CreateTsDictionary);
        assert!(!out[1].destructive);
    }
}
