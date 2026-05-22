//! `pgevolve cluster status` — list cluster plans.
//!
//! Prints the name of each subdirectory under `cluster-plans/`, sorted
//! lexicographically (which matches chronological order given that the plan id
//! is derived from a stable hash).
//!
//! v0.3.0 gap: the per-DB `status` command reads `pgevolve.apply_log` to mark
//! each plan as applied vs. pending. Cluster apply does not yet write to an
//! apply log (`pgevolve.apply_log` is per-DB). Tracking apply history for
//! cluster plans is deferred to Stage 12 / v0.3.0 hardening.

use std::path::Path;

use anyhow::{Context, Result};

use crate::cluster_config::ClusterConfig;

/// Run `pgevolve cluster status`.
pub fn run(project_root: &Path, _cfg: &ClusterConfig) -> Result<i32> {
    let plans_root = project_root.join("cluster-plans");
    if !plans_root.exists() {
        println!("No cluster plans.");
        return Ok(0);
    }

    let mut entries: Vec<_> = std::fs::read_dir(&plans_root)
        .with_context(|| format!("reading {}", plans_root.display()))?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .collect();

    // Sort lexicographically so output is deterministic.
    entries.sort_by_key(std::fs::DirEntry::path);

    if entries.is_empty() {
        println!("No cluster plans.");
        return Ok(0);
    }

    for entry in entries {
        println!("{}", entry.file_name().to_string_lossy());
    }

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster_config::ClusterConfig;

    fn dummy_cfg() -> ClusterConfig {
        toml::from_str(
            r#"
            [project]
            name = "test"
            [connection]
            dsn = "postgres://localhost"
        "#,
        )
        .unwrap()
    }

    #[test]
    fn prints_no_cluster_plans_when_directory_absent() {
        let dir = tempfile::tempdir().unwrap();
        // No cluster-plans dir exists; should return 0 without error.
        let code = run(dir.path(), &dummy_cfg()).unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn lists_plan_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let plans = dir.path().join("cluster-plans");
        std::fs::create_dir(&plans).unwrap();
        std::fs::create_dir(plans.join("aabbccdd")).unwrap();
        std::fs::create_dir(plans.join("eeff0011")).unwrap();
        // Should succeed and (visually) print both dirs.
        let code = run(dir.path(), &dummy_cfg()).unwrap();
        assert_eq!(code, 0);
    }
}
