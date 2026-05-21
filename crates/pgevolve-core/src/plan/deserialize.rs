//! Readers for the three on-disk plan files. See spec §7.
//!
//! `plan.sql` is parsed by a small line-based scanner that recognizes the
//! `-- @pgevolve ...` directive lines and groups intervening lines into the
//! preceding step's SQL body. `intent.toml` and `manifest.toml` are
//! deserialized through `serde` / `toml` directly; manifest's embedded
//! `target_snapshot_yaml` round-trips through `serde_yaml`.

use std::path::Path;

use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::plan::grouping::TransactionGroup;
use crate::plan::io_error::PlanIoError;
use crate::plan::plan::{
    DestructiveIntent, LintWaiver, Plan, PlanId, PlanMetadata, RecordedFinding, StepOverride,
    parse_kind_name,
};
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

// ---------------------------------------------------------------------------
// plan.sql
// ---------------------------------------------------------------------------

/// Loosely-typed view of a parsed `plan.sql`. Final `Plan` assembly happens in
/// [`read_plan_dir`], which cross-references this with `intent.toml` and
/// `manifest.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialPlan {
    /// `plan id=` value — the 16-char short hash.
    pub id_short: String,
    /// `version=` value.
    pub pgevolve_version: String,
    /// `ruleset=` value.
    pub planner_ruleset_version: u32,
    /// `source_rev=` value, if present.
    pub source_rev: Option<String>,
    /// `target=` value (opaque target identity).
    pub target_identity: String,
    /// `intents_required=` value.
    pub intents_required: u32,
    /// Parsed `created=` timestamp.
    pub created_at: OffsetDateTime,
    /// Recovered groups (each step's `destructive_reason` is `None` here; the
    /// reason lives in `intent.toml` and is grafted on in [`read_plan_dir`]).
    pub groups: Vec<TransactionGroup>,
}

/// Parse `plan.sql` from a string.
#[allow(clippy::too_many_lines)]
pub fn read_plan_sql(s: &str) -> Result<PartialPlan, PlanIoError> {
    let mut id_short: Option<String> = None;
    let mut version: Option<String> = None;
    let mut ruleset: Option<u32> = None;
    let mut created: Option<OffsetDateTime> = None;
    let mut source_rev: Option<String> = None;
    let mut target_identity: Option<String> = None;
    let mut intents_required: Option<u32> = None;

    let mut groups: Vec<TransactionGroup> = Vec::new();
    let mut current_group: Option<TransactionGroup> = None;
    let mut current_step: Option<(RawStep, Vec<String>)> = None;
    // Tracks the active dollar-quote tag while we're inside a `$tag$...$tag$`
    // literal in a step's SQL body. Function/procedure bodies are emitted
    // wrapped in `$pgevolve$...$pgevolve$` and may contain literal
    // `-- @pgevolve dep: ...` directives, `BEGIN;`, `COMMIT;`, etc. — all of
    // which would otherwise be mistaken for plan-level directives by the
    // line scanner. Track open/close and treat in-string lines as body text.
    let mut active_dollar_tag: Option<String> = None;

    for raw_line in s.lines() {
        let line = raw_line;
        let trimmed = line.trim_end();

        // If we're inside a dollar-quote, accumulate body text and only check
        // for the matching close tag; do not parse directives.
        if let Some(tag) = active_dollar_tag.as_ref() {
            if let Some((_, ref mut body)) = current_step {
                body.push(line.to_string());
            }
            // Check if this line closes the dollar-quote.
            let close = format!("${tag}$");
            if line.contains(&close) {
                active_dollar_tag = None;
            }
            continue;
        }

        if trimmed.is_empty() {
            // Blank lines are layout-only between directives; they end the
            // current step's SQL body if one is accumulating.
            continue;
        }

        // Pre-scan for dollar-quote OPEN on this line. The emitter writes
        // `AS $pgevolve$...` followed by the body and matching close. We need
        // to enter dollar-quote state if the line opens one and the close
        // isn't on the same line.
        if let Some(tag) = detect_dollar_quote_open(line) {
            if let Some((_, ref mut body)) = current_step {
                body.push(line.to_string());
            }
            active_dollar_tag = Some(tag);
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("-- @pgevolve ") {
            // Finalize any in-flight step before starting a new directive.
            flush_step(&mut current_step, &mut current_group);

            let kv = parse_kv(rest)?;
            let head = kv
                .first()
                .ok_or_else(|| PlanIoError::MalformedDirective(rest.into()))?;
            match head.0.as_str() {
                "plan" => {
                    for (k, v) in &kv {
                        match k.as_str() {
                            "plan" | "id" => id_short = Some(v.clone()),
                            "version" => version = Some(v.clone()),
                            "ruleset" => ruleset = Some(parse_u32(v)?),
                            "created" => {
                                created =
                                    Some(OffsetDateTime::parse(v, &Rfc3339).map_err(|e| {
                                        PlanIoError::MalformedDirective(format!("created={v}: {e}"))
                                    })?);
                            }
                            _ => {}
                        }
                    }
                }
                "source_rev" => source_rev = Some(head.1.clone()),
                "target" => target_identity = Some(head.1.clone()),
                "intents_required" => intents_required = Some(parse_u32(&head.1)?),
                "group" => {
                    if let Some(g) = current_group.take() {
                        groups.push(g);
                    }
                    let mut id = 0u32;
                    let mut transactional = true;
                    for (k, v) in &kv {
                        match k.as_str() {
                            "id" => id = parse_u32(v)?,
                            "transactional" => transactional = parse_bool(v)?,
                            // "group" sentinel head and any unknown key fall through.
                            _ => {}
                        }
                    }
                    current_group = Some(TransactionGroup {
                        id,
                        transactional,
                        steps: Vec::new(),
                    });
                }
                "step" => {
                    let mut step_no = 0u32;
                    let mut kind: Option<StepKind> = None;
                    let mut destructive = false;
                    let mut intent_id: Option<u32> = None;
                    let mut targets: Vec<QualifiedName> = Vec::new();
                    for (k, v) in &kv {
                        match k.as_str() {
                            "step" => step_no = parse_u32(v)?,
                            "kind" => {
                                kind = Some(parse_kind_name(v).ok_or_else(|| {
                                    PlanIoError::MalformedDirective(format!("kind={v}"))
                                })?);
                            }
                            "destructive" => destructive = parse_bool(v)?,
                            "intent_id" => intent_id = Some(parse_u32(v)?),
                            "targets" => targets = parse_targets(v)?,
                            _ => {}
                        }
                    }
                    let kind = kind.ok_or_else(|| {
                        PlanIoError::MalformedDirective("step missing kind".into())
                    })?;
                    let g = current_group.as_ref().ok_or_else(|| {
                        PlanIoError::MalformedDirective("step outside group".into())
                    })?;
                    let transactional = if g.transactional {
                        TransactionConstraint::InTransaction
                    } else {
                        TransactionConstraint::OutsideTransaction
                    };
                    let step = RawStep {
                        step_no,
                        kind,
                        destructive,
                        destructive_reason: None,
                        intent_id,
                        targets,
                        sql: String::new(),
                        transactional,
                    };
                    current_step = Some((step, Vec::new()));
                }
                other => {
                    return Err(PlanIoError::MalformedDirective(format!(
                        "unknown directive: {other}"
                    )));
                }
            }
            continue;
        }

        // Non-directive lines are either BEGIN/COMMIT or step SQL body lines.
        if trimmed == "BEGIN;" {
            // Group framing; not part of any step's SQL.
            continue;
        }
        if trimmed == "COMMIT;" {
            flush_step(&mut current_step, &mut current_group);
            if let Some(g) = current_group.take() {
                groups.push(g);
            }
            continue;
        }
        if let Some((_, ref mut body)) = current_step {
            body.push(line.to_string());
        }
        // Lines outside any step (e.g., blank padding) are silently dropped.
    }

    // Flush any trailing step / group at EOF.
    flush_step(&mut current_step, &mut current_group);
    if let Some(g) = current_group.take() {
        groups.push(g);
    }

    Ok(PartialPlan {
        id_short: id_short
            .ok_or_else(|| PlanIoError::MalformedDirective("missing plan id".into()))?,
        pgevolve_version: version
            .ok_or_else(|| PlanIoError::MalformedDirective("missing version".into()))?,
        planner_ruleset_version: ruleset
            .ok_or_else(|| PlanIoError::MalformedDirective("missing ruleset".into()))?,
        source_rev,
        target_identity: target_identity
            .ok_or_else(|| PlanIoError::MalformedDirective("missing target".into()))?,
        intents_required: intents_required
            .ok_or_else(|| PlanIoError::MalformedDirective("missing intents_required".into()))?,
        created_at: created
            .ok_or_else(|| PlanIoError::MalformedDirective("missing created".into()))?,
        groups,
    })
}

/// If `line` opens a `$tag$` dollar-quoted literal that isn't closed on the
/// same line, return the tag (excluding the surrounding `$` markers). Returns
/// `None` for lines with no open quote, or lines whose dollar-quotes are fully
/// closed on the same line.
///
/// Scans left-to-right looking for `$<tag>$` markers (tag = empty or
/// `[A-Za-z_][A-Za-z0-9_]*` per PG's dollar-quote grammar). Each occurrence
/// toggles "inside-quote" state; if we end the line still inside, return the
/// active tag.
fn detect_dollar_quote_open(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut active: Option<String> = None;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            // Scan tag chars.
            let tag_start = i + 1;
            let mut j = tag_start;
            while j < bytes.len() {
                let b = bytes[j];
                let valid = if j == tag_start {
                    b.is_ascii_alphabetic() || b == b'_'
                } else {
                    b.is_ascii_alphanumeric() || b == b'_'
                };
                if !valid {
                    break;
                }
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'$' {
                let tag = std::str::from_utf8(&bytes[tag_start..j])
                    .unwrap_or("")
                    .to_string();
                if let Some(open) = active.as_ref() {
                    if open == &tag {
                        active = None;
                    }
                } else {
                    active = Some(tag);
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    active
}

fn flush_step(
    current_step: &mut Option<(RawStep, Vec<String>)>,
    current_group: &mut Option<TransactionGroup>,
) {
    if let Some((mut step, body)) = current_step.take() {
        step.sql = body.join("\n").trim_end_matches('\n').to_string();
        if let Some(g) = current_group.as_mut() {
            g.steps.push(step);
        }
    }
}

fn parse_kv(s: &str) -> Result<Vec<(String, String)>, PlanIoError> {
    // Tokens are whitespace-separated; each token is `key=value`. The first
    // token may be a bare `head` keyword (e.g., `plan`, `group`, `step`) — we
    // treat that as `(head, "")` so callers can look it up uniformly.
    let mut out = Vec::new();
    let mut tokens = s.split_whitespace();
    if let Some(first) = tokens.next() {
        if let Some((k, v)) = first.split_once('=') {
            out.push((k.to_string(), v.to_string()));
        } else {
            out.push((first.to_string(), String::new()));
        }
    }
    for t in tokens {
        let (k, v) = t
            .split_once('=')
            .ok_or_else(|| PlanIoError::MalformedDirective(format!("bare token: {t}")))?;
        out.push((k.to_string(), v.to_string()));
    }
    Ok(out)
}

fn parse_u32(s: &str) -> Result<u32, PlanIoError> {
    s.parse()
        .map_err(|_| PlanIoError::MalformedDirective(format!("not a u32: {s}")))
}

fn parse_bool(s: &str) -> Result<bool, PlanIoError> {
    match s {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(PlanIoError::MalformedDirective(format!("not a bool: {s}"))),
    }
}

fn parse_targets(s: &str) -> Result<Vec<QualifiedName>, PlanIoError> {
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for part in s.split(',') {
        out.push(parse_qname(part)?);
    }
    Ok(out)
}

fn parse_qname(s: &str) -> Result<QualifiedName, PlanIoError> {
    let (schema, name) = s
        .split_once('.')
        .ok_or_else(|| PlanIoError::MalformedDirective(format!("expected schema.name: {s}")))?;
    let schema = Identifier::from_unquoted(schema)
        .map_err(|e| PlanIoError::MalformedDirective(format!("schema {schema}: {e}")))?;
    let name = Identifier::from_unquoted(name)
        .map_err(|e| PlanIoError::MalformedDirective(format!("name {name}: {e}")))?;
    Ok(QualifiedName::new(schema, name))
}

// ---------------------------------------------------------------------------
// intent.toml
// ---------------------------------------------------------------------------

/// Parsed view of `intent.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedIntent {
    /// The short plan id field used for cross-check.
    pub plan_id: String,
    /// The intent rows (one per destructive step).
    pub intents: Vec<DestructiveIntent>,
    /// Lint waivers from `[[lint_waiver]]` rows.
    pub lint_waivers: Vec<LintWaiver>,
    /// Step overrides from `[[step_override]]` rows.
    pub step_overrides: Vec<StepOverride>,
}

#[derive(Deserialize)]
struct IntentDocDe {
    plan_id: String,
    #[serde(default, rename = "intent")]
    intents: Vec<IntentRowDe>,
    #[serde(default, rename = "lint_waiver")]
    lint_waivers: Vec<LintWaiver>,
    #[serde(default, rename = "step_override")]
    step_overrides: Vec<StepOverride>,
}

#[derive(Deserialize)]
struct IntentRowDe {
    id: u32,
    step: u32,
    kind: String,
    target: String,
    reason: String,
    /// `approved = true/false` in `intent.toml`. Defaults to `false` (the
    /// writer always emits `approved = false`; the user flips it manually).
    /// Retained on `DestructiveIntent` so preflight can enforce approval.
    #[serde(default)]
    approved: bool,
}

/// Parse `intent.toml` from a string.
pub fn read_intent_toml(s: &str) -> Result<ParsedIntent, PlanIoError> {
    let doc: IntentDocDe = toml::from_str(s)?;
    Ok(ParsedIntent {
        plan_id: doc.plan_id,
        intents: doc
            .intents
            .into_iter()
            .map(|r| DestructiveIntent {
                id: r.id,
                step: r.step,
                kind: r.kind,
                target: r.target,
                reason: r.reason,
                approved: r.approved,
            })
            .collect(),
        lint_waivers: doc.lint_waivers,
        step_overrides: doc.step_overrides,
    })
}

// ---------------------------------------------------------------------------
// manifest.toml
// ---------------------------------------------------------------------------

/// Parsed view of `manifest.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedManifest {
    /// Short plan id for cross-check.
    pub plan_id: String,
    /// Full 64-char hex plan hash.
    pub plan_hash: String,
    /// pgevolve version at plan time.
    pub pgevolve_version: String,
    /// Planner ruleset version at plan time.
    pub planner_ruleset_version: u32,
    /// Optional source-tree revision.
    pub source_rev: Option<String>,
    /// Stable target-database identity.
    pub target_identity: String,
    /// Plan creation timestamp.
    pub created_at: OffsetDateTime,
    /// Recovered target catalog snapshot.
    pub target_snapshot: Catalog,
    /// `LintAtPlan` findings captured at plan time. Empty on older plans that
    /// predate this field (`#[serde(default)]` on the de side).
    pub lint_at_plan_findings: Vec<RecordedFinding>,
}

#[derive(Deserialize)]
struct ManifestDocDe {
    plan_id: String,
    plan_hash: String,
    pgevolve_version: String,
    planner_ruleset_version: u32,
    source_rev: Option<String>,
    target_identity: String,
    created_at: String,
    target_snapshot_json: String,
    #[serde(default)]
    lint_at_plan_findings: Vec<RecordedFinding>,
}

/// Parse `manifest.toml` from a string.
pub fn read_manifest_toml(s: &str) -> Result<ParsedManifest, PlanIoError> {
    let doc: ManifestDocDe = toml::from_str(s)?;
    let created_at = OffsetDateTime::parse(&doc.created_at, &Rfc3339).map_err(|e| {
        PlanIoError::MalformedDirective(format!("manifest created_at={}: {e}", doc.created_at))
    })?;
    let target_snapshot: Catalog = serde_json::from_str(&doc.target_snapshot_json)?;
    Ok(ParsedManifest {
        plan_id: doc.plan_id,
        plan_hash: doc.plan_hash,
        pgevolve_version: doc.pgevolve_version,
        planner_ruleset_version: doc.planner_ruleset_version,
        source_rev: doc.source_rev,
        target_identity: doc.target_identity,
        created_at,
        target_snapshot,
        lint_at_plan_findings: doc.lint_at_plan_findings,
    })
}

// ---------------------------------------------------------------------------
// Plan::read_from_dir
// ---------------------------------------------------------------------------

/// Read a plan directory (three files) back into a `Plan`.
///
/// Cross-checks the short `plan_id` value across the three files and rejects
/// inconsistent inputs. Destructive-step `destructive_reason` is grafted from
/// `intent.toml`'s `reason` field (the SQL writer does not carry it).
pub fn read_plan_dir(dir: &Path) -> Result<Plan, PlanIoError> {
    let sql_path = dir.join("plan.sql");
    let intent_path = dir.join("intent.toml");
    let manifest_path = dir.join("manifest.toml");

    let sql = std::fs::read_to_string(&sql_path).map_err(|e| PlanIoError::io(&sql_path, e))?;
    let intent_str =
        std::fs::read_to_string(&intent_path).map_err(|e| PlanIoError::io(&intent_path, e))?;
    let manifest_str =
        std::fs::read_to_string(&manifest_path).map_err(|e| PlanIoError::io(&manifest_path, e))?;

    let partial = read_plan_sql(&sql)?;
    let intent = read_intent_toml(&intent_str)?;
    let manifest = read_manifest_toml(&manifest_str)?;

    if partial.id_short != intent.plan_id || partial.id_short != manifest.plan_id {
        return Err(PlanIoError::PlanIdMismatch {
            sql: partial.id_short,
            intent: intent.plan_id,
            manifest: manifest.plan_id,
        });
    }

    let id = PlanId::from_full_hex(&manifest.plan_hash)?;

    // Graft destructive_reason from intent rows onto the matching steps.
    let mut groups = partial.groups;
    for g in &mut groups {
        for step in &mut g.steps {
            if let Some(intent_id) = step.intent_id
                && let Some(row) = intent.intents.iter().find(|i| i.id == intent_id)
            {
                step.destructive_reason = Some(row.reason.clone());
            }
        }
    }

    let metadata = PlanMetadata {
        pgevolve_version: manifest.pgevolve_version,
        planner_ruleset_version: manifest.planner_ruleset_version,
        source_rev: manifest.source_rev,
        target_identity: manifest.target_identity,
        target_snapshot: manifest.target_snapshot,
        created_at: manifest.created_at,
        lint_at_plan_findings: manifest.lint_at_plan_findings,
    };

    Ok(Plan {
        id,
        groups,
        intents: intent.intents,
        lint_waivers: intent.lint_waivers,
        step_overrides: intent.step_overrides,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::serialize::{write_plan_dir, write_plan_sql};
    use crate::plan::{
        grouping::TransactionGroup, plan::Plan, raw_step::RawStep, raw_step::StepKind,
        raw_step::TransactionConstraint,
    };

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
        use crate::ir::schema::Schema;
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
        .unwrap()
    }

    #[test]
    fn read_plan_sql_round_trips_simple_plan() {
        let plan = simple_plan();
        let mut buf = Vec::new();
        write_plan_sql(&plan, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        let partial = read_plan_sql(&s).unwrap();

        assert_eq!(partial.id_short, plan.id.short());
        assert_eq!(partial.pgevolve_version, "0.1.0");
        assert_eq!(partial.planner_ruleset_version, 1);
        assert_eq!(partial.source_rev.as_deref(), Some("git:abcdef0"));
        assert_eq!(partial.target_identity, "tid-xyz");
        assert_eq!(partial.intents_required, 1);
        assert_eq!(partial.groups.len(), 2);
        assert!(partial.groups[0].transactional);
        assert!(!partial.groups[1].transactional);
        assert_eq!(partial.groups[0].steps.len(), 2);
        assert_eq!(partial.groups[1].steps.len(), 1);
        // Step bodies survive verbatim.
        assert_eq!(partial.groups[0].steps[0].sql, "CREATE SCHEMA app;");
        assert_eq!(
            partial.groups[1].steps[0].sql,
            "CREATE INDEX CONCURRENTLY users_idx ON app.users USING btree (id);"
        );
        // Step numbers preserved.
        assert_eq!(partial.groups[0].steps[0].step_no, 1);
        assert_eq!(partial.groups[0].steps[1].step_no, 2);
        assert_eq!(partial.groups[1].steps[0].step_no, 3);
        // Destructive step recovered with intent_id.
        assert!(partial.groups[0].steps[1].destructive);
        assert_eq!(partial.groups[0].steps[1].intent_id, Some(1));
        // Targets list recovered.
        assert_eq!(
            partial.groups[1].steps[0].targets,
            vec![qn("app", "users_idx"), qn("app", "users")],
        );
    }

    #[test]
    fn read_plan_sql_rejects_missing_plan_header() {
        let s = "-- @pgevolve target=t\n";
        assert!(matches!(
            read_plan_sql(s),
            Err(PlanIoError::MalformedDirective(_))
        ));
    }

    #[test]
    fn read_plan_sql_rejects_unknown_directive() {
        let s = "-- @pgevolve plan id=abc version=0.1.0 ruleset=1 created=2026-05-09T18:42:11Z\n\
                 -- @pgevolve nope=true\n";
        assert!(matches!(
            read_plan_sql(s),
            Err(PlanIoError::MalformedDirective(_))
        ));
    }

    #[test]
    fn read_intent_toml_round_trips() {
        let plan = simple_plan();
        let mut buf = Vec::new();
        crate::plan::serialize::write_intent_toml(&plan, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        let parsed = read_intent_toml(&s).unwrap();
        assert_eq!(parsed.plan_id, plan.id.short());
        assert_eq!(parsed.intents.len(), 1);
        assert_eq!(parsed.intents[0].id, 1);
        assert_eq!(parsed.intents[0].kind, "drop_table");
        assert_eq!(parsed.intents[0].reason, "test reason");
    }

    #[test]
    fn read_manifest_toml_round_trips_catalog() {
        let plan = simple_plan();
        let mut buf = Vec::new();
        crate::plan::serialize::write_manifest_toml(&plan, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        let parsed = read_manifest_toml(&s).unwrap();
        assert_eq!(parsed.plan_id, plan.id.short());
        assert_eq!(parsed.plan_hash, plan.id.to_hex());
        assert_eq!(parsed.target_snapshot, plan.metadata.target_snapshot);
        assert_eq!(parsed.planner_ruleset_version, 1);
        assert_eq!(parsed.target_identity, "tid-xyz");
    }

    #[test]
    fn read_plan_dir_round_trips_whole_plan() {
        let plan = simple_plan();
        let dir = tempfile::tempdir().unwrap();
        write_plan_dir(&plan, dir.path()).unwrap();
        let recovered = read_plan_dir(dir.path()).unwrap();

        // The full Plan should compare equal modulo timestamp truncation; RFC3339
        // round-trip preserves UTC offsets and nanosecond precision via `time` v0.3,
        // so equality should hold.
        assert_eq!(recovered.id, plan.id);
        assert_eq!(recovered.intents, plan.intents);
        assert_eq!(
            recovered.metadata.target_snapshot,
            plan.metadata.target_snapshot
        );
        assert_eq!(
            recovered.metadata.pgevolve_version,
            plan.metadata.pgevolve_version
        );
        assert_eq!(
            recovered.metadata.target_identity,
            plan.metadata.target_identity
        );
        assert_eq!(recovered.groups.len(), plan.groups.len());
        for (a, b) in recovered.groups.iter().zip(plan.groups.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.transactional, b.transactional);
            assert_eq!(a.steps.len(), b.steps.len());
            for (sa, sb) in a.steps.iter().zip(b.steps.iter()) {
                assert_eq!(sa.step_no, sb.step_no);
                assert_eq!(sa.kind, sb.kind);
                assert_eq!(sa.destructive, sb.destructive);
                assert_eq!(sa.intent_id, sb.intent_id);
                assert_eq!(sa.targets, sb.targets);
                assert_eq!(sa.sql, sb.sql);
                assert_eq!(sa.transactional, sb.transactional);
                // destructive_reason is grafted from intent.toml.
                if sb.destructive {
                    assert_eq!(sa.destructive_reason, sb.destructive_reason);
                }
            }
        }
    }

    #[test]
    fn read_plan_dir_rejects_mismatched_plan_id() {
        let plan = simple_plan();
        let dir = tempfile::tempdir().unwrap();
        write_plan_dir(&plan, dir.path()).unwrap();
        // Tamper with intent.toml's plan_id.
        let intent_path = dir.path().join("intent.toml");
        let s = std::fs::read_to_string(&intent_path).unwrap();
        let tampered = s.replacen(&plan.id.short(), "deadbeef00000000", 1);
        std::fs::write(&intent_path, tampered).unwrap();

        let err = read_plan_dir(dir.path()).unwrap_err();
        assert!(matches!(err, PlanIoError::PlanIdMismatch { .. }));
    }

    #[test]
    fn read_plan_sql_handles_multi_line_step_body() {
        let plan_text = "\
-- @pgevolve plan id=abc1234567890123 version=0.1.0 ruleset=1 created=2026-05-09T18:42:11Z
-- @pgevolve target=tid
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.t
CREATE TABLE app.t (
    id bigint NOT NULL,
    name text
);
COMMIT;
";
        let partial = read_plan_sql(plan_text).unwrap();
        assert_eq!(partial.groups.len(), 1);
        let body = &partial.groups[0].steps[0].sql;
        assert!(body.starts_with("CREATE TABLE app.t ("));
        assert!(body.contains("id bigint NOT NULL,"));
        assert!(body.ends_with(");"));
    }

    #[test]
    fn read_plan_dir_round_trips_view_and_mv_step_kinds() {
        use crate::ir::catalog::Catalog;
        use crate::ir::schema::Schema;
        use crate::plan::grouping::TransactionGroup;
        use crate::plan::plan::Plan;
        use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
        use crate::plan::serialize::write_plan_dir;

        let id_id = |s: &str| Identifier::from_unquoted(s).unwrap();
        let qn = |schema: &str, name: &str| QualifiedName::new(id_id(schema), id_id(name));

        let view_step = |kind: StepKind, sql: &str, destructive: bool| -> RawStep {
            RawStep {
                step_no: 0,
                kind,
                destructive,
                destructive_reason: destructive.then(|| "test".to_string()),
                intent_id: None,
                targets: vec![qn("app", "my_view")],
                sql: sql.to_string(),
                transactional: TransactionConstraint::InTransaction,
            }
        };

        let groups = vec![TransactionGroup {
            id: 1,
            transactional: true,
            steps: vec![
                view_step(
                    StepKind::CreateView,
                    "CREATE VIEW app.my_view AS\nSELECT 1;",
                    false,
                ),
                view_step(StepKind::DropView, "DROP VIEW app.my_view;", true),
                view_step(
                    StepKind::CreateMaterializedView,
                    "CREATE MATERIALIZED VIEW app.my_view AS\nSELECT 1\nWITH NO DATA;",
                    false,
                ),
                view_step(
                    StepKind::DropMaterializedView,
                    "DROP MATERIALIZED VIEW app.my_view;",
                    false,
                ),
                view_step(
                    StepKind::RefreshMaterializedView,
                    "REFRESH MATERIALIZED VIEW app.my_view;",
                    false,
                ),
                view_step(
                    StepKind::AlterViewSetReloption,
                    "ALTER VIEW app.my_view SET (security_barrier = true);",
                    false,
                ),
                view_step(
                    StepKind::CommentOnView,
                    "COMMENT ON VIEW app.my_view IS 'a view';",
                    false,
                ),
            ],
        }];

        let mut snapshot = Catalog::empty();
        snapshot.schemas.push(Schema::new(id_id("app")));
        let plan = Plan::from_grouped(
            groups,
            &Catalog::empty(),
            &snapshot,
            "test-views-target".into(),
            None,
            "0.2.0",
            2,
        )
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        write_plan_dir(&plan, dir.path()).unwrap();
        let recovered = read_plan_dir(dir.path()).unwrap();

        assert_eq!(recovered.groups.len(), 1);
        let steps = &recovered.groups[0].steps;
        assert_eq!(steps.len(), 7);
        assert_eq!(steps[0].kind, StepKind::CreateView);
        assert_eq!(steps[1].kind, StepKind::DropView);
        assert_eq!(steps[2].kind, StepKind::CreateMaterializedView);
        assert_eq!(steps[3].kind, StepKind::DropMaterializedView);
        assert_eq!(steps[4].kind, StepKind::RefreshMaterializedView);
        assert_eq!(steps[5].kind, StepKind::AlterViewSetReloption);
        assert_eq!(steps[6].kind, StepKind::CommentOnView);

        // Destructive step (DropView) gets an intent_id.
        assert_eq!(steps[1].intent_id, Some(1));
        assert!(steps[1].destructive);

        // Non-destructive steps have no intent_id.
        assert!(steps[0].intent_id.is_none());
        assert!(steps[3].intent_id.is_none()); // DropMaterializedView is NOT destructive

        // SQL bodies survive the round-trip.
        assert_eq!(steps[0].sql, "CREATE VIEW app.my_view AS\nSELECT 1;");
        assert_eq!(
            steps[2].sql,
            "CREATE MATERIALIZED VIEW app.my_view AS\nSELECT 1\nWITH NO DATA;"
        );
        assert_eq!(steps[4].sql, "REFRESH MATERIALIZED VIEW app.my_view;");
    }
}
