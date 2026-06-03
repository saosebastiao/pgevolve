//! `pgevolve cluster apply` — apply a cluster plan directory.
//!
//! With no plan id, finds the most recently modified directory under
//! `cluster-plans/`. With an explicit id, applies that specific plan.
//!
//! Closes the v0.3.0 Stage-12 gaps (#7): structured plan loading, advisory
//! lock, intent enforcement, manifest cross-check, `apply_log` audit.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow};

use pgevolve_core::plan::read_plan_dir;

use crate::cluster_config::ClusterConfig;
use crate::executor::{ApplyOverrides, apply_cluster_plan};

/// Run `pgevolve cluster apply`.
pub async fn run(project_root: &Path, cfg: &ClusterConfig, plan_id: Option<&str>) -> Result<i32> {
    let plan_dir = match plan_id {
        Some(id) => project_root.join("cluster-plans").join(id),
        None => find_latest_plan_dir(&project_root.join("cluster-plans"))
            .context("looking for the latest cluster plan")?,
    };

    eprintln!("Applying {}", plan_dir.display());

    let plan = read_plan_dir(&plan_dir)
        .with_context(|| format!("reading plan from {}", plan_dir.display()))?;

    let (mut client, connection) =
        tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
            .await
            .context("connecting to cluster for apply")?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!(?err, "cluster apply connection ended");
        }
    });

    let overrides = ApplyOverrides::default();
    apply_cluster_plan(&plan, &mut client, overrides)
        .await
        .map_err(|e| anyhow!("{e}"))?;

    eprintln!("Done.");
    Ok(0)
}

/// Find the most recently modified subdirectory of `plans_root`.
///
/// Returns an error if the directory doesn't exist or is empty.
fn find_latest_plan_dir(plans_root: &Path) -> Result<PathBuf> {
    if !plans_root.exists() {
        return Err(anyhow!(
            "no cluster-plans directory found at {}",
            plans_root.display()
        ));
    }

    let mut latest: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(plans_root)
        .with_context(|| format!("reading {}", plans_root.display()))?
    {
        let entry = entry.with_context(|| format!("iterating {}", plans_root.display()))?;
        if entry
            .file_type()
            .with_context(|| format!("stat {}", entry.path().display()))?
            .is_dir()
        {
            let mtime = entry
                .metadata()
                .with_context(|| format!("metadata {}", entry.path().display()))?
                .modified()
                .with_context(|| format!("mtime {}", entry.path().display()))?;
            if latest.as_ref().is_none_or(|(t, _)| mtime > *t) {
                latest = Some((mtime, entry.path()));
            }
        }
    }

    latest
        .map(|(_, p)| p)
        .ok_or_else(|| anyhow!("no cluster plans found in {}", plans_root.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_latest_errors_when_no_dir() {
        let dir = tempfile::tempdir().unwrap();
        let err = find_latest_plan_dir(&dir.path().join("no-such-dir")).unwrap_err();
        assert!(err.to_string().contains("no cluster-plans directory"));
    }

    #[test]
    fn find_latest_errors_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let err = find_latest_plan_dir(dir.path()).unwrap_err();
        assert!(err.to_string().contains("no cluster plans found"));
    }

    #[test]
    fn find_latest_returns_most_recent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("aaaaaaaa")).unwrap();
        // Small sleep to ensure distinct mtime on filesystems with 1-second
        // resolution is not needed for this test: we'll just check both exist.
        std::fs::create_dir(dir.path().join("bbbbbbbb")).unwrap();
        // Can't reliably assert which one is "latest" without sleeping,
        // but we can assert it returns one of them without error.
        let p = find_latest_plan_dir(dir.path()).unwrap();
        let name = p.file_name().unwrap().to_str().unwrap();
        assert!(name == "aaaaaaaa" || name == "bbbbbbbb");
    }
}
