//! `pgevolve lint` — universal rules + profile rules. Exit 1 on any
//! error-severity finding. Supports `--format human` (default) and
//! `--format json`.

use anyhow::Result;

use pgevolve_core::lint::{
    Finding, LintInputs, ManagedConfig, Profile, Severity, SourceTree, run as run_lint,
};

use crate::cli::{LintArgs, OutputFormat};
use crate::config::PgevolveConfig;

/// Run `pgevolve lint`.
pub fn run(_args: LintArgs, cfg: &PgevolveConfig, format: OutputFormat) -> Result<i32> {
    let schema_dir = &cfg.project.schema_dir;
    if !schema_dir.is_dir() {
        return Err(anyhow::anyhow!(
            "schema directory not found at {}",
            schema_dir.display(),
        ));
    }

    if matches!(format, OutputFormat::Sql) {
        return Err(anyhow::anyhow!(
            "`--format sql` is only meaningful for `pgevolve diff`; lint supports `human` and `json`",
        ));
    }

    let tree =
        SourceTree::parse(schema_dir, &[]).map_err(|e| anyhow::anyhow!("parse error: {e}"))?;

    let profile = Profile::from_name(&cfg.project.layout_profile)
        .map_err(|e| anyhow::anyhow!("layout profile: {e}"))?;

    let managed = ManagedConfig {
        schemas: cfg
            .managed
            .schemas
            .iter()
            .filter_map(|s| pgevolve_core::identifier::Identifier::from_unquoted(s).ok())
            .collect(),
    };

    let findings = run_lint(&LintInputs {
        tree: &tree,
        managed: &managed,
        profile: &profile,
        schema_dir,
    });

    let errors = findings
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .count();

    match format {
        OutputFormat::Human => render_human(&findings, errors),
        OutputFormat::Json => render_json(&findings, errors)?,
        OutputFormat::Sql => unreachable!("rejected above"),
    }

    Ok(i32::from(errors > 0))
}

fn render_human(findings: &[Finding], errors: usize) {
    if findings.is_empty() {
        println!("pgevolve lint: 0 findings");
        return;
    }
    for f in findings {
        let loc = f
            .location
            .as_ref()
            .map(|l| format!(" ({}:{}:{})", l.file.display(), l.line, l.column))
            .unwrap_or_default();
        eprintln!("{}: [{}] {}{}", f.severity, f.rule, f.message, loc);
    }
    eprintln!(
        "pgevolve lint: {} finding(s), {} error(s)",
        findings.len(),
        errors,
    );
}

fn render_json(findings: &[Finding], errors: usize) -> Result<()> {
    let entries: Vec<_> = findings.iter().map(JsonFinding::from_finding).collect();
    let doc = JsonOutput {
        findings: entries,
        total: findings.len(),
        errors,
    };
    let rendered = serde_json::to_string_pretty(&doc)?;
    println!("{rendered}");
    Ok(())
}

/// Stable JSON wire format for `pgevolve lint --format json`.
///
/// Severity values are stringified (`"error"`, `"warning"`, `"lint-at-plan"`)
/// — matching `Severity`'s `Display` impl — so downstream consumers don't
/// have to track enum discriminants across pgevolve versions.
#[derive(serde::Serialize)]
struct JsonOutput<'a> {
    findings: Vec<JsonFinding<'a>>,
    total: usize,
    errors: usize,
}

#[derive(serde::Serialize)]
struct JsonFinding<'a> {
    severity: String,
    rule: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<JsonLocation>,
}

impl<'a> JsonFinding<'a> {
    fn from_finding(f: &'a Finding) -> Self {
        Self {
            severity: f.severity.to_string(),
            rule: f.rule,
            message: &f.message,
            location: f.location.as_ref().map(|l| JsonLocation {
                file: l.file.display().to_string(),
                line: l.line,
                column: l.column,
            }),
        }
    }
}

#[derive(serde::Serialize)]
struct JsonLocation {
    file: String,
    line: usize,
    column: usize,
}
