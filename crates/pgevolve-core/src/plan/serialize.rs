//! Writers for the three on-disk plan files: `plan.sql`, `intent.toml`,
//! `manifest.toml`. See spec §7.

use std::io::Write;
use std::path::Path;

use serde::Serialize;
use time::format_description::well_known::Rfc3339;

use crate::ir::catalog::Catalog;
use crate::plan::io_error::PlanIoError;
use crate::plan::plan::{Plan, kind_name};
use crate::plan::raw_step::RawStep;

// ---------------------------------------------------------------------------
// plan.sql
// ---------------------------------------------------------------------------

/// Write a plan's `plan.sql` to `w`.
///
/// Output is canonical bytes-out: the same plan always produces the same
/// bytes. The only non-determinism would be `metadata.created_at`, which is
/// captured at `Plan::from_grouped` time.
pub fn write_plan_sql(plan: &Plan, w: &mut dyn Write) -> Result<(), PlanIoError> {
    let created = plan
        .metadata
        .created_at
        .format(&Rfc3339)
        .map_err(|e| PlanIoError::MalformedDirective(format!("created_at format: {e}")))?;
    writeln!(
        w,
        "-- @pgevolve plan id={} version={} ruleset={} created={}",
        plan.id.short(),
        plan.metadata.pgevolve_version,
        plan.metadata.planner_ruleset_version,
        created,
    )?;
    if let Some(rev) = &plan.metadata.source_rev {
        writeln!(w, "-- @pgevolve source_rev={rev}")?;
    }
    writeln!(w, "-- @pgevolve target={}", plan.metadata.target_identity)?;
    writeln!(w, "-- @pgevolve intents_required={}", plan.intents.len())?;
    writeln!(w)?;

    for group in &plan.groups {
        writeln!(
            w,
            "-- @pgevolve group id={} transactional={}",
            group.id, group.transactional,
        )?;
        if group.transactional {
            writeln!(w, "BEGIN;")?;
        }
        for step in &group.steps {
            write_step_directive(w, step)?;
            // Steps always end their SQL with `;` (every sql:: helper appends
            // a trailing semicolon). Trailing newline gives one statement per
            // line block for readability.
            writeln!(w, "{}", step.sql)?;
        }
        if group.transactional {
            writeln!(w, "COMMIT;")?;
        }
        writeln!(w)?;
    }
    Ok(())
}

fn write_step_directive(w: &mut dyn Write, s: &RawStep) -> Result<(), PlanIoError> {
    write!(
        w,
        "-- @pgevolve step={} kind={} destructive={}",
        s.step_no,
        kind_name(s.kind),
        s.destructive,
    )?;
    if let Some(intent_id) = s.intent_id {
        write!(w, " intent_id={intent_id}")?;
    }
    write!(w, " targets=")?;
    for (i, t) in s.targets.iter().enumerate() {
        if i > 0 {
            write!(w, ",")?;
        }
        write!(w, "{t}")?;
    }
    writeln!(w)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// intent.toml
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct IntentDoc<'a> {
    plan_id: String,
    #[serde(rename = "intent")]
    intents: Vec<IntentRow<'a>>,
}

#[derive(Serialize)]
struct IntentRow<'a> {
    id: u32,
    step: u32,
    kind: &'a str,
    target: &'a str,
    reason: &'a str,
    approved: bool,
}

/// Write `intent.toml` to `w`. Every row's `approved` field starts as `false`;
/// the user must flip it explicitly before applying.
pub fn write_intent_toml(plan: &Plan, w: &mut dyn Write) -> Result<(), PlanIoError> {
    let doc = IntentDoc {
        plan_id: plan.id.short(),
        intents: plan
            .intents
            .iter()
            .map(|i| IntentRow {
                id: i.id,
                step: i.step,
                kind: &i.kind,
                target: &i.target,
                reason: &i.reason,
                approved: false,
            })
            .collect(),
    };
    let s = toml::to_string_pretty(&doc)?;
    w.write_all(s.as_bytes())
        .map_err(|e| PlanIoError::Io("intent.toml".into(), e))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// manifest.toml
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ManifestDoc<'a> {
    plan_id: String,
    plan_hash: String,
    pgevolve_version: &'a str,
    planner_ruleset_version: u32,
    source_rev: Option<&'a str>,
    target_identity: &'a str,
    created_at: String,
    /// Embedded pre-image `Catalog` as a pretty-printed JSON string. (v0.1
    /// used YAML here; we switched to JSON to drop the archived
    /// `serde_yaml` crate. The field is still a TOML string — only the
    /// payload format inside it changed.)
    target_snapshot_json: String,
}

/// Write `manifest.toml` to `w`.
///
/// The `target_snapshot_json` field embeds the pre-image `Catalog` as
/// pretty-printed JSON — recoverable by
/// [`read_manifest_toml`](crate::plan::deserialize::read_manifest_toml).
pub fn write_manifest_toml(plan: &Plan, w: &mut dyn Write) -> Result<(), PlanIoError> {
    let created = plan
        .metadata
        .created_at
        .format(&Rfc3339)
        .map_err(|e| PlanIoError::MalformedDirective(format!("created_at format: {e}")))?;
    let snapshot_json = render_catalog_json(&plan.metadata.target_snapshot)?;
    let doc = ManifestDoc {
        plan_id: plan.id.short(),
        plan_hash: plan.id.to_hex(),
        pgevolve_version: &plan.metadata.pgevolve_version,
        planner_ruleset_version: plan.metadata.planner_ruleset_version,
        source_rev: plan.metadata.source_rev.as_deref(),
        target_identity: &plan.metadata.target_identity,
        created_at: created,
        target_snapshot_json: snapshot_json,
    };
    let s = toml::to_string_pretty(&doc)?;
    w.write_all(s.as_bytes())
        .map_err(|e| PlanIoError::Io("manifest.toml".into(), e))?;
    Ok(())
}

fn render_catalog_json(c: &Catalog) -> Result<String, PlanIoError> {
    serde_json::to_string_pretty(c).map_err(PlanIoError::Json)
}

// ---------------------------------------------------------------------------
// Plan::write_to_dir
// ---------------------------------------------------------------------------

/// Write a `Plan` to a directory as `plan.sql` + `intent.toml` + `manifest.toml`.
///
/// Creates the directory if missing. Overwrites existing files of the same name.
pub fn write_plan_dir(plan: &Plan, dir: &Path) -> Result<(), PlanIoError> {
    std::fs::create_dir_all(dir).map_err(|e| PlanIoError::io(dir, e))?;

    let sql_path = dir.join("plan.sql");
    let mut sql = std::fs::File::create(&sql_path).map_err(|e| PlanIoError::io(&sql_path, e))?;
    write_plan_sql(plan, &mut sql)?;

    let intent_path = dir.join("intent.toml");
    let mut intent =
        std::fs::File::create(&intent_path).map_err(|e| PlanIoError::io(&intent_path, e))?;
    write_intent_toml(plan, &mut intent)?;

    let manifest_path = dir.join("manifest.toml");
    let mut manifest =
        std::fs::File::create(&manifest_path).map_err(|e| PlanIoError::io(&manifest_path, e))?;
    write_manifest_toml(plan, &mut manifest)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Convenience: convert io::Error → PlanIoError where the path is captured up-front.
// ---------------------------------------------------------------------------

impl From<std::io::Error> for PlanIoError {
    fn from(e: std::io::Error) -> Self {
        // Path-less variant; callers that know the path should use
        // [`PlanIoError::io`] for better diagnostics. This impl is here so
        // [`writeln!`] inside the SQL writer can return `?` cleanly.
        Self::Io(std::path::PathBuf::new(), e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::schema::Schema;
    use crate::plan::grouping::TransactionGroup;
    use crate::plan::plan::{Plan, PlanMetadata};
    use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

    fn id_id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id_id(schema), id_id(name))
    }

    fn step(
        kind: StepKind,
        sql: &str,
        destructive: bool,
        targets: Vec<QualifiedName>,
        c: TransactionConstraint,
    ) -> RawStep {
        RawStep {
            step_no: 0,
            kind,
            destructive,
            destructive_reason: destructive.then(|| "test reason".to_string()),
            intent_id: None,
            targets,
            sql: sql.to_string(),
            transactional: c,
        }
    }

    fn simple_plan() -> Plan {
        let groups = vec![
            TransactionGroup {
                id: 1,
                transactional: true,
                steps: vec![
                    step(
                        StepKind::CreateSchema,
                        "CREATE SCHEMA app;",
                        false,
                        vec![qn("app", "app")],
                        TransactionConstraint::InTransaction,
                    ),
                    step(
                        StepKind::DropTable,
                        "DROP TABLE app.legacy;",
                        true,
                        vec![qn("app", "legacy")],
                        TransactionConstraint::InTransaction,
                    ),
                ],
            },
            TransactionGroup {
                id: 2,
                transactional: false,
                steps: vec![step(
                    StepKind::CreateIndexConcurrent,
                    "CREATE INDEX CONCURRENTLY users_idx ON app.users USING btree (id);",
                    false,
                    vec![qn("app", "users_idx"), qn("app", "users")],
                    TransactionConstraint::OutsideTransaction,
                )],
            },
        ];
        let mut snapshot = Catalog::empty();
        snapshot.schemas.push(Schema::new(id_id("app")));
        Plan::from_grouped(
            groups,
            &Catalog::empty(),
            &snapshot,
            "tid-xyz".into(),
            Some("git:abcdef0".into()),
            "0.1.0",
            1,
        )
    }

    #[test]
    fn plan_sql_header_contains_id_version_ruleset_and_created() {
        let plan = simple_plan();
        let mut out = Vec::new();
        write_plan_sql(&plan, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        let header = s.lines().next().unwrap();
        assert!(header.starts_with("-- @pgevolve plan id="));
        assert!(header.contains("version=0.1.0"));
        assert!(header.contains("ruleset=1"));
        assert!(header.contains("created="));
    }

    #[test]
    fn plan_sql_emits_source_rev_when_present() {
        let plan = simple_plan();
        let mut out = Vec::new();
        write_plan_sql(&plan, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("-- @pgevolve source_rev=git:abcdef0"));
        assert!(s.contains("-- @pgevolve target=tid-xyz"));
        assert!(s.contains("-- @pgevolve intents_required=1"));
    }

    #[test]
    fn plan_sql_wraps_transactional_groups_in_begin_commit() {
        let plan = simple_plan();
        let mut out = Vec::new();
        write_plan_sql(&plan, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        // First group is transactional, second is not.
        let g1_pos = s.find("group id=1 transactional=true").unwrap();
        let g2_pos = s.find("group id=2 transactional=false").unwrap();
        let begin_pos = s.find("BEGIN;").unwrap();
        let commit_pos = s.find("COMMIT;").unwrap();
        assert!(g1_pos < begin_pos);
        assert!(begin_pos < commit_pos);
        assert!(commit_pos < g2_pos);
        // Second group has no BEGIN/COMMIT after it.
        assert!(!s[g2_pos..].contains("BEGIN;"));
        assert!(!s[g2_pos..].contains("COMMIT;"));
    }

    #[test]
    fn plan_sql_emits_step_directives_with_intent_when_destructive() {
        let plan = simple_plan();
        let mut out = Vec::new();
        write_plan_sql(&plan, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("step=1 kind=create_schema destructive=false"));
        assert!(s.contains("step=2 kind=drop_table destructive=true intent_id=1"));
        assert!(s.contains("step=3 kind=create_index_concurrent destructive=false"));
        assert!(s.contains("targets=app.users_idx,app.users"));
    }

    #[test]
    fn intent_toml_contains_one_row_per_destructive_step() {
        let plan = simple_plan();
        let mut out = Vec::new();
        write_intent_toml(&plan, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("plan_id ="));
        assert!(s.contains("[[intent]]"));
        assert!(s.contains("id = 1"));
        assert!(s.contains("step = 2"));
        assert!(s.contains("kind = \"drop_table\""));
        assert!(s.contains("approved = false"));
    }

    #[test]
    fn manifest_toml_contains_full_hex_and_embedded_json() {
        let plan = simple_plan();
        let mut out = Vec::new();
        write_manifest_toml(&plan, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("plan_id ="));
        assert!(s.contains("plan_hash ="));
        // 64 hex chars somewhere in the doc
        assert!(s.lines().any(|l| {
            l.contains("plan_hash") && l.chars().filter(char::is_ascii_hexdigit).count() >= 64
        }));
        assert!(s.contains("target_snapshot_json ="));
        // Embedded JSON body — TOML emits the field as a multi-line
        // basic-string literal, so quote-escape style depends on its
        // chosen form. Check for substring fragments that survive either
        // form ("schemas" + "name" appear as bare tokens with quote chars
        // around them).
        assert!(s.contains("schemas"));
        assert!(s.contains("\"app\"") || s.contains("\\\"app\\\""));
    }

    #[test]
    fn write_plan_dir_creates_three_files() {
        let plan = simple_plan();
        let dir = tempfile::tempdir().unwrap();
        write_plan_dir(&plan, dir.path()).unwrap();
        assert!(dir.path().join("plan.sql").exists());
        assert!(dir.path().join("intent.toml").exists());
        assert!(dir.path().join("manifest.toml").exists());
    }

    // Suppress unused-fn warnings while `PlanMetadata` isn't referenced
    // outside via this module (the test imports it as a sanity check that
    // it stays public).
    fn _meta_kept_in_scope(m: PlanMetadata) -> PlanMetadata {
        m
    }
}
