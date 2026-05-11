//! `pgevolve lint` — driver only; rule execution lands in Phase 10.

use std::path::PathBuf;

use anyhow::Result;

use crate::cli::LintArgs;
use crate::config::PgevolveConfig;

/// Run `pgevolve lint`. Phase 10 wires the rules; this driver returns 0 with
/// a placeholder message so the command surface is complete.
pub fn run(_args: LintArgs, cfg: &PgevolveConfig) -> Result<i32> {
    let schema_dir = resolve_schema_dir(cfg);
    if !schema_dir.is_dir() {
        return Err(anyhow::anyhow!(
            "schema directory not found at {}",
            schema_dir.display(),
        ));
    }
    println!("pgevolve lint: 0 findings (Phase 10 will plug in rule profiles)");
    Ok(0)
}

fn resolve_schema_dir(cfg: &PgevolveConfig) -> PathBuf {
    cfg.project.schema_dir.clone()
}
