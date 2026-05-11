//! `pgevolve lint` — universal rules + profile rules. Exit 1 on any
//! error-severity finding.

use anyhow::Result;

use pgevolve_core::lint::{
    run as run_lint, LintInputs, ManagedConfig, Profile, Severity, SourceTree,
};

use crate::cli::LintArgs;
use crate::config::PgevolveConfig;

/// Run `pgevolve lint`.
pub fn run(_args: LintArgs, cfg: &PgevolveConfig) -> Result<i32> {
    let schema_dir = &cfg.project.schema_dir;
    if !schema_dir.is_dir() {
        return Err(anyhow::anyhow!(
            "schema directory not found at {}",
            schema_dir.display(),
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

    if findings.is_empty() {
        println!("pgevolve lint: 0 findings");
        return Ok(0);
    }

    let mut errors = 0;
    for f in &findings {
        let loc = f
            .location
            .as_ref()
            .map(|l| format!(" ({}:{}:{})", l.file.display(), l.line, l.column))
            .unwrap_or_default();
        eprintln!("{}: [{}] {}{}", f.severity, f.rule, f.message, loc);
        if matches!(f.severity, Severity::Error) {
            errors += 1;
        }
    }
    eprintln!(
        "pgevolve lint: {} finding(s), {} error(s)",
        findings.len(),
        errors,
    );
    Ok(i32::from(errors > 0))
}
