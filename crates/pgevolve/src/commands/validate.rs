//! `pgevolve validate` — parse source IR + lint stub. With `--shadow`,
//! Phase 12 will round-trip through an ephemeral PG.

use anyhow::Result;

use crate::cli::ValidateArgs;
use crate::config::PgevolveConfig;

/// Run `pgevolve validate`.
pub fn run(args: &ValidateArgs, cfg: &PgevolveConfig) -> Result<i32> {
    let schema_dir = &cfg.project.schema_dir;
    if !schema_dir.is_dir() {
        return Err(anyhow::anyhow!(
            "schema directory not found at {}",
            schema_dir.display(),
        ));
    }
    let _source = pgevolve_core::parse::parse_directory(schema_dir, &[])
        .map_err(|e| anyhow::anyhow!("parse error: {e}"))?;
    if args.shadow {
        return Err(anyhow::anyhow!(
            "--shadow validation is not implemented until Phase 12",
        ));
    }
    println!("pgevolve validate: source parses cleanly; 0 lint findings (Phase 10 wires rules)");
    Ok(0)
}
