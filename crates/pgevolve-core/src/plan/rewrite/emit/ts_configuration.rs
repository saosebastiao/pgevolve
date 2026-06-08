//! Dispatcher for `Change::TsConfiguration(TsConfigurationChange)`.
//!
//! All steps are `Safe` and run `InTransaction`. `Replace` decomposes into a
//! `Drop` followed by the full `Create` sequence (same pattern as aggregates
//! and dictionaries), because Postgres provides no in-place `ALTER` for the
//! `parser` field.

use crate::diff::change::TsConfigurationChange;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::text_search::TsConfiguration;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

pub fn emit(
    change: TsConfigurationChange,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    match change {
        TsConfigurationChange::Create(cfg) => {
            emit_create(&cfg, destructive, destructive_reason, out);
        }
        TsConfigurationChange::Replace { from, to } => {
            // Drop the old — no data, so never destructive in this path.
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropTsConfiguration,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![from.qname.clone()],
                sql: drop_sql(&from.qname),
                transactional: TransactionConstraint::InTransaction,
            });
            // Create the new plus follow-up mapping/owner/comment steps.
            emit_create(&to, false, None, out);
        }
        TsConfigurationChange::Drop { qname } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropTsConfiguration,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: drop_sql(&qname),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        TsConfigurationChange::AddMapping {
            qname,
            token_type,
            dictionaries,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AddTsConfigMapping,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: add_mapping_sql(&qname, &token_type, &dictionaries),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        TsConfigurationChange::AlterMapping {
            qname,
            token_type,
            dictionaries,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterTsConfigMapping,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: alter_mapping_sql(&qname, &token_type, &dictionaries),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        TsConfigurationChange::DropMapping { qname, token_type } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropTsConfigMapping,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: drop_mapping_sql(&qname, &token_type),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        TsConfigurationChange::AlterOwner { qname, owner } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterTsConfigurationOwner,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: alter_owner_sql(&qname, &owner),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        TsConfigurationChange::CommentOn { qname, comment } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnTsConfiguration,
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

/// Emit a `CREATE TEXT SEARCH CONFIGURATION` step, followed by one
/// `ADD MAPPING` step per mapping in `cfg.mappings` order, then an optional
/// `OWNER TO` step and an optional `COMMENT ON` step.
fn emit_create(
    cfg: &TsConfiguration,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::CreateTsConfiguration,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: vec![cfg.qname.clone()],
        sql: create_sql(cfg),
        transactional: TransactionConstraint::InTransaction,
    });
    for mapping in &cfg.mappings {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::AddTsConfigMapping,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![cfg.qname.clone()],
            sql: add_mapping_sql(&cfg.qname, &mapping.token_type, &mapping.dictionaries),
            transactional: TransactionConstraint::InTransaction,
        });
    }
    if let Some(owner) = &cfg.owner {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterTsConfigurationOwner,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![cfg.qname.clone()],
            sql: alter_owner_sql(&cfg.qname, owner),
            transactional: TransactionConstraint::InTransaction,
        });
    }
    if let Some(comment) = &cfg.comment {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::CommentOnTsConfiguration,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![cfg.qname.clone()],
            sql: comment_sql(&cfg.qname, Some(comment.as_str())),
            transactional: TransactionConstraint::InTransaction,
        });
    }
}

// ---------------------------------------------------------------------------
// SQL helpers
// ---------------------------------------------------------------------------

/// `CREATE TEXT SEARCH CONFIGURATION qname (PARSER = parser);`
fn create_sql(cfg: &TsConfiguration) -> String {
    format!(
        "CREATE TEXT SEARCH CONFIGURATION {} (PARSER = {});",
        cfg.qname.render_sql(),
        cfg.parser.render_sql(),
    )
}

/// `ALTER TEXT SEARCH CONFIGURATION qname ADD MAPPING FOR token_type WITH dict1, dict2;`
fn add_mapping_sql(qname: &QualifiedName, token_type: &str, dicts: &[QualifiedName]) -> String {
    let dict_list = render_dict_list(dicts);
    format!(
        "ALTER TEXT SEARCH CONFIGURATION {} ADD MAPPING FOR {} WITH {};",
        qname.render_sql(),
        render_token_type(token_type),
        dict_list,
    )
}

/// `ALTER TEXT SEARCH CONFIGURATION qname ALTER MAPPING FOR token_type WITH dict1, dict2;`
fn alter_mapping_sql(qname: &QualifiedName, token_type: &str, dicts: &[QualifiedName]) -> String {
    let dict_list = render_dict_list(dicts);
    format!(
        "ALTER TEXT SEARCH CONFIGURATION {} ALTER MAPPING FOR {} WITH {};",
        qname.render_sql(),
        render_token_type(token_type),
        dict_list,
    )
}

/// `ALTER TEXT SEARCH CONFIGURATION qname DROP MAPPING IF EXISTS FOR token_type;`
fn drop_mapping_sql(qname: &QualifiedName, token_type: &str) -> String {
    format!(
        "ALTER TEXT SEARCH CONFIGURATION {} DROP MAPPING IF EXISTS FOR {};",
        qname.render_sql(),
        render_token_type(token_type),
    )
}

/// `DROP TEXT SEARCH CONFIGURATION qname;`
fn drop_sql(qname: &QualifiedName) -> String {
    format!("DROP TEXT SEARCH CONFIGURATION {};", qname.render_sql())
}

/// `ALTER TEXT SEARCH CONFIGURATION qname OWNER TO owner;`
fn alter_owner_sql(qname: &QualifiedName, owner: &Identifier) -> String {
    format!(
        "ALTER TEXT SEARCH CONFIGURATION {} OWNER TO {};",
        qname.render_sql(),
        owner.render_sql(),
    )
}

/// `COMMENT ON TEXT SEARCH CONFIGURATION qname IS '...';` or `IS NULL;`
fn comment_sql(qname: &QualifiedName, comment: Option<&str>) -> String {
    match comment {
        Some(c) => format!(
            "COMMENT ON TEXT SEARCH CONFIGURATION {} IS '{}';",
            qname.render_sql(),
            c.replace('\'', "''"),
        ),
        None => format!(
            "COMMENT ON TEXT SEARCH CONFIGURATION {} IS NULL;",
            qname.render_sql(),
        ),
    }
}

/// Render a token-type alias as an identifier.
///
/// Token-type aliases (e.g. `word`, `asciiword`) are simple lowercase names
/// that are safe bare identifiers in Postgres. We quote defensively via
/// `Identifier::from_unquoted`; if the name fails the unquoted check we fall
/// back to the raw string (which is always a parser-defined alias and therefore
/// safe).
fn render_token_type(token_type: &str) -> String {
    Identifier::from_unquoted(token_type)
        .map(|i| i.render_sql())
        .unwrap_or_else(|_| token_type.to_string())
}

/// Render a comma-separated list of dictionary qualified names.
fn render_dict_list(dicts: &[QualifiedName]) -> String {
    dicts
        .iter()
        .map(QualifiedName::render_sql)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::text_search::TsMapping;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    /// Minimal configuration with only a parser (no mappings, owner, or comment).
    fn make_cfg() -> TsConfiguration {
        TsConfiguration {
            qname: qname("public", "english_config"),
            parser: qname("pg_catalog", "default"),
            mappings: vec![],
            owner: None,
            comment: None,
        }
    }

    // --- create_sql ---

    #[test]
    fn create_sql_parser_only() {
        let cfg = make_cfg();
        let sql = create_sql(&cfg);
        // `default` is a reserved keyword; `render_sql` quotes it.
        assert_eq!(
            sql,
            "CREATE TEXT SEARCH CONFIGURATION public.english_config \
             (PARSER = pg_catalog.\"default\");"
        );
    }

    // --- emit: Create with parser only => 1 step ---

    #[test]
    fn emit_create_parser_only_produces_one_step() {
        let cfg = make_cfg();
        let mut out = Vec::new();
        emit(TsConfigurationChange::Create(cfg), false, None, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CreateTsConfiguration);
        assert!(!out[0].destructive);
        assert!(
            out[0].sql.starts_with("CREATE TEXT SEARCH CONFIGURATION"),
            "got: {}",
            out[0].sql
        );
    }

    // --- emit: Create with parser + 2 mappings => CREATE + 2 ADD MAPPING steps ---

    #[test]
    fn emit_create_with_two_mappings_produces_three_steps() {
        let mut cfg = make_cfg();
        cfg.mappings = vec![
            TsMapping {
                token_type: "word".to_string(),
                dictionaries: vec![qname("public", "english_stem")],
            },
            TsMapping {
                token_type: "asciiword".to_string(),
                dictionaries: vec![qname("public", "english_stem")],
            },
        ];
        let mut out = Vec::new();
        emit(TsConfigurationChange::Create(cfg), false, None, &mut out);
        assert_eq!(out.len(), 3, "expected CREATE + 2 ADD MAPPING");
        assert_eq!(out[0].kind, StepKind::CreateTsConfiguration);
        assert!(out[0].sql.starts_with("CREATE TEXT SEARCH CONFIGURATION"));
        assert_eq!(out[1].kind, StepKind::AddTsConfigMapping);
        assert!(
            out[1].sql.contains("ADD MAPPING FOR word"),
            "step 1: {}",
            out[1].sql
        );
        assert_eq!(out[2].kind, StepKind::AddTsConfigMapping);
        assert!(
            out[2].sql.contains("ADD MAPPING FOR asciiword"),
            "step 2: {}",
            out[2].sql
        );
    }

    // --- emit: Create with owner + comment => CREATE + OWNER + COMMENT ---

    #[test]
    fn emit_create_with_owner_and_comment_produces_three_steps() {
        let mut cfg = make_cfg();
        cfg.owner = Some(id("app_owner"));
        cfg.comment = Some("English config.".to_string());
        let mut out = Vec::new();
        emit(TsConfigurationChange::Create(cfg), false, None, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].kind, StepKind::CreateTsConfiguration);
        assert_eq!(out[1].kind, StepKind::AlterTsConfigurationOwner);
        assert!(out[1].sql.contains("OWNER TO app_owner"));
        assert_eq!(out[2].kind, StepKind::CommentOnTsConfiguration);
        assert!(out[2].sql.contains("English config"));
    }

    // --- emit: Create with mappings + owner + comment => 5 steps ---

    #[test]
    fn emit_create_full_produces_correct_step_sequence() {
        let mut cfg = make_cfg();
        cfg.mappings = vec![
            TsMapping {
                token_type: "word".to_string(),
                dictionaries: vec![
                    qname("public", "english_stem"),
                    qname("pg_catalog", "simple"),
                ],
            },
            TsMapping {
                token_type: "asciiword".to_string(),
                dictionaries: vec![qname("public", "english_stem")],
            },
        ];
        cfg.owner = Some(id("app_owner"));
        cfg.comment = Some("Full config.".to_string());
        let mut out = Vec::new();
        emit(TsConfigurationChange::Create(cfg), false, None, &mut out);
        // CREATE + 2 ADD MAPPING + OWNER + COMMENT = 5 steps
        assert_eq!(out.len(), 5);
        assert_eq!(out[0].kind, StepKind::CreateTsConfiguration);
        assert_eq!(out[1].kind, StepKind::AddTsConfigMapping);
        assert_eq!(out[2].kind, StepKind::AddTsConfigMapping);
        assert_eq!(out[3].kind, StepKind::AlterTsConfigurationOwner);
        assert_eq!(out[4].kind, StepKind::CommentOnTsConfiguration);
    }

    // --- add_mapping_sql ---

    #[test]
    fn add_mapping_sql_single_dict() {
        let sql = add_mapping_sql(
            &qname("public", "english_config"),
            "word",
            &[qname("public", "english_stem")],
        );
        assert_eq!(
            sql,
            "ALTER TEXT SEARCH CONFIGURATION public.english_config \
             ADD MAPPING FOR word WITH public.english_stem;"
        );
    }

    #[test]
    fn add_mapping_sql_multi_dict_chain() {
        let sql = add_mapping_sql(
            &qname("public", "english_config"),
            "word",
            &[
                qname("public", "english_stem"),
                qname("pg_catalog", "simple"),
            ],
        );
        assert_eq!(
            sql,
            "ALTER TEXT SEARCH CONFIGURATION public.english_config \
             ADD MAPPING FOR word WITH public.english_stem, pg_catalog.simple;"
        );
    }

    // --- alter_mapping_sql ---

    #[test]
    fn alter_mapping_sql_renders_correctly() {
        let sql = alter_mapping_sql(
            &qname("public", "english_config"),
            "asciiword",
            &[qname("public", "english_stem")],
        );
        assert_eq!(
            sql,
            "ALTER TEXT SEARCH CONFIGURATION public.english_config \
             ALTER MAPPING FOR asciiword WITH public.english_stem;"
        );
    }

    // --- drop_mapping_sql ---

    #[test]
    fn drop_mapping_sql_renders_correctly() {
        let sql = drop_mapping_sql(&qname("public", "english_config"), "numword");
        assert_eq!(
            sql,
            "ALTER TEXT SEARCH CONFIGURATION public.english_config \
             DROP MAPPING IF EXISTS FOR numword;"
        );
    }

    // --- drop_sql ---

    #[test]
    fn drop_sql_renders_correctly() {
        let sql = drop_sql(&qname("public", "english_config"));
        assert_eq!(sql, "DROP TEXT SEARCH CONFIGURATION public.english_config;");
    }

    // --- emit: Drop ---

    #[test]
    fn emit_drop_produces_one_step() {
        let mut out = Vec::new();
        emit(
            TsConfigurationChange::Drop {
                qname: qname("public", "english_config"),
            },
            true,
            Some("removing config".to_string()),
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::DropTsConfiguration);
        assert!(out[0].destructive);
        assert!(out[0].sql.contains("DROP TEXT SEARCH CONFIGURATION"));
    }

    // --- emit: AddMapping ---

    #[test]
    fn emit_add_mapping_produces_one_step() {
        let mut out = Vec::new();
        emit(
            TsConfigurationChange::AddMapping {
                qname: qname("public", "english_config"),
                token_type: "word".to_string(),
                dictionaries: vec![qname("public", "english_stem")],
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::AddTsConfigMapping);
        assert!(out[0].sql.contains("ADD MAPPING FOR word"));
    }

    // --- emit: AlterMapping ---

    #[test]
    fn emit_alter_mapping_produces_one_step() {
        let mut out = Vec::new();
        emit(
            TsConfigurationChange::AlterMapping {
                qname: qname("public", "english_config"),
                token_type: "asciiword".to_string(),
                dictionaries: vec![qname("public", "english_stem")],
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::AlterTsConfigMapping);
        assert!(out[0].sql.contains("ALTER MAPPING FOR asciiword"));
    }

    // --- emit: DropMapping ---

    #[test]
    fn emit_drop_mapping_produces_one_step() {
        let mut out = Vec::new();
        emit(
            TsConfigurationChange::DropMapping {
                qname: qname("public", "english_config"),
                token_type: "numword".to_string(),
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::DropTsConfigMapping);
        assert!(out[0].sql.contains("DROP MAPPING IF EXISTS FOR numword"));
    }

    // --- alter_owner_sql ---

    #[test]
    fn alter_owner_sql_renders_correctly() {
        let sql = alter_owner_sql(&qname("public", "english_config"), &id("app_owner"));
        assert_eq!(
            sql,
            "ALTER TEXT SEARCH CONFIGURATION public.english_config OWNER TO app_owner;"
        );
    }

    // --- emit: AlterOwner ---

    #[test]
    fn emit_alter_owner_produces_one_step() {
        let mut out = Vec::new();
        emit(
            TsConfigurationChange::AlterOwner {
                qname: qname("public", "english_config"),
                owner: id("newrole"),
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::AlterTsConfigurationOwner);
        assert!(out[0].sql.contains("OWNER TO newrole"));
    }

    // --- comment_sql ---

    #[test]
    fn comment_sql_set() {
        let sql = comment_sql(&qname("public", "english_config"), Some("a config"));
        assert_eq!(
            sql,
            "COMMENT ON TEXT SEARCH CONFIGURATION public.english_config IS 'a config';"
        );
    }

    #[test]
    fn comment_sql_clear_is_null() {
        let sql = comment_sql(&qname("public", "english_config"), None);
        assert_eq!(
            sql,
            "COMMENT ON TEXT SEARCH CONFIGURATION public.english_config IS NULL;"
        );
    }

    #[test]
    fn comment_sql_escapes_single_quotes() {
        let sql = comment_sql(&qname("public", "english_config"), Some("O'Brien config"));
        assert!(sql.contains("IS 'O''Brien config'"), "got: {sql}");
    }

    // --- emit: CommentOn (set and NULL) ---

    #[test]
    fn emit_comment_on_set_produces_one_step() {
        let mut out = Vec::new();
        emit(
            TsConfigurationChange::CommentOn {
                qname: qname("public", "english_config"),
                comment: Some("my comment".to_string()),
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CommentOnTsConfiguration);
        assert!(out[0].sql.contains("my comment"));
    }

    #[test]
    fn emit_comment_on_none_renders_is_null() {
        let mut out = Vec::new();
        emit(
            TsConfigurationChange::CommentOn {
                qname: qname("public", "english_config"),
                comment: None,
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CommentOnTsConfiguration);
        assert!(out[0].sql.contains("IS NULL"));
    }

    // --- emit: Replace = drop then create + re-add mappings ---

    #[test]
    fn emit_replace_first_step_is_drop_then_create() {
        let from = make_cfg();
        let mut to = make_cfg();
        to.parser = qname("pg_catalog", "ngram");
        let mut out = Vec::new();
        emit(
            TsConfigurationChange::Replace { from, to },
            true,
            None,
            &mut out,
        );
        assert!(
            out.len() >= 2,
            "expected at least 2 steps, got {}",
            out.len()
        );
        assert_eq!(out[0].kind, StepKind::DropTsConfiguration);
        // Configurations carry no data: the drop in a Replace is always safe.
        assert!(!out[0].destructive);
        assert_eq!(out[1].kind, StepKind::CreateTsConfiguration);
        assert!(!out[1].destructive);
    }

    #[test]
    fn emit_replace_re_adds_mappings_from_to() {
        let from = make_cfg();
        let mut to = make_cfg();
        to.parser = qname("pg_catalog", "ngram");
        to.mappings = vec![
            TsMapping {
                token_type: "word".to_string(),
                dictionaries: vec![qname("public", "english_stem")],
            },
            TsMapping {
                token_type: "asciiword".to_string(),
                dictionaries: vec![qname("public", "english_stem")],
            },
        ];
        let mut out = Vec::new();
        emit(
            TsConfigurationChange::Replace { from, to },
            false,
            None,
            &mut out,
        );
        // DROP + CREATE + 2 ADD MAPPING = 4 steps
        assert_eq!(
            out.len(),
            4,
            "steps: {:?}",
            out.iter().map(|s| s.kind).collect::<Vec<_>>()
        );
        assert_eq!(out[0].kind, StepKind::DropTsConfiguration);
        assert_eq!(out[1].kind, StepKind::CreateTsConfiguration);
        assert_eq!(out[2].kind, StepKind::AddTsConfigMapping);
        assert_eq!(out[3].kind, StepKind::AddTsConfigMapping);
    }
}
