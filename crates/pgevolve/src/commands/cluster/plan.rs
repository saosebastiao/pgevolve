//! `pgevolve cluster plan` — write `cluster-plans/<plan_id>/` directory.
//!
//! The directory layout mirrors per-DB plans but lives under `cluster-plans/`
//! to make it unambiguous which pipeline produced the plan:
//!
//! ```text
//! cluster-plans/<id>/
//!   plan.sql        — DDL steps in emission order
//!   intent.toml     — destructive steps requiring approval
//!   manifest.toml   — plan id + project name
//! ```
//!
//! The plan id is a BLAKE3 hash of the serialized source + target catalogs
//! using the domain-separator `pgevolve-cluster-plan-id-v1`, distinct from
//! the per-DB domain separator to avoid cross-type collisions.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};

use pgevolve_core::plan::kind_name;

use crate::api::cluster::build_cluster_plan;
use crate::cluster_config::ClusterConfig;

/// Run `pgevolve cluster plan`.
pub async fn run(project_root: &Path, cfg: &ClusterConfig) -> Result<i32> {
    let plan = build_cluster_plan(project_root, cfg)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if plan.changes.is_empty() {
        println!("No changes.");
        return Ok(0);
    }

    let plan_id = compute_cluster_plan_id(&plan.source, &plan.target)?;
    let plan_dir = project_root.join("cluster-plans").join(&plan_id);
    std::fs::create_dir_all(&plan_dir)
        .with_context(|| format!("creating {}", plan_dir.display()))?;

    // plan.sql — one step per line block, separated by a blank line.
    let sql_lines: Vec<&str> = plan.steps.iter().map(|s| s.sql.as_str()).collect();
    let plan_sql = sql_lines.join("\n\n") + "\n";
    std::fs::write(plan_dir.join("plan.sql"), plan_sql)
        .with_context(|| format!("writing {}", plan_dir.join("plan.sql").display()))?;

    // intent.toml — list destructive steps that need approval.
    let intent_toml = build_intent_toml(&plan.steps);
    std::fs::write(plan_dir.join("intent.toml"), intent_toml)
        .with_context(|| format!("writing {}", plan_dir.join("intent.toml").display()))?;

    // manifest.toml — minimal for v0.3.0.
    let manifest = format!(
        "plan_id = \"{plan_id}\"\nproject_name = \"{name}\"\n",
        name = cfg.project.name,
    );
    std::fs::write(plan_dir.join("manifest.toml"), manifest)
        .with_context(|| format!("writing {}", plan_dir.join("manifest.toml").display()))?;

    println!("Wrote {}", plan_dir.display());
    println!("  plan.sql ({} steps)", plan.steps.len());

    // Advisory findings MUST be printed to stderr so they reach the user.
    for finding in &plan.advisory_findings {
        eprintln!(
            "pgevolve cluster plan: advisory [{}]: {}",
            finding.rule, finding.message
        );
    }

    Ok(0)
}

/// Compute a short plan id by hashing the canonical serialized source and
/// target catalogs.
///
/// Uses the domain separator `pgevolve-cluster-plan-id-v1` so cluster plan
/// ids never collide with per-DB plan ids even if the byte contents were
/// identical. Returns the first 8 bytes of the digest as lowercase hex.
fn compute_cluster_plan_id(
    source: &pgevolve_core::ir::cluster::catalog::ClusterCatalog,
    target: &pgevolve_core::ir::cluster::catalog::ClusterCatalog,
) -> Result<String> {
    let source_bytes =
        serde_json::to_vec(source).context("serializing source catalog for plan id")?;
    let target_bytes =
        serde_json::to_vec(target).context("serializing target catalog for plan id")?;
    let mut h = blake3::Hasher::new();
    h.update(b"pgevolve-cluster-plan-id-v1\n");
    h.update(&source_bytes);
    h.update(&[0]);
    h.update(&target_bytes);
    Ok(hex::encode(&h.finalize().as_bytes()[..8]))
}

/// Build the `intent.toml` content listing destructive steps.
fn build_intent_toml(steps: &[pgevolve_core::plan::raw_step::RawStep]) -> String {
    let mut out = String::from("# Approve each destructive step to allow apply.\n\n");
    let mut intent_idx: u32 = 1;
    for (i, step) in steps.iter().enumerate() {
        if step.destructive {
            let kind = kind_name(step.kind);
            let reason = step
                .destructive_reason
                .as_deref()
                .unwrap_or("destructive operation");
            let step_no = i + 1;
            // Infallible: writing to a String never errors.
            let _ = write!(
                out,
                "[[destructive_intent]]\n\
                 id = {intent_idx}\n\
                 step = {step_no}\n\
                 kind = \"{kind}\"\n\
                 reason = \"{reason}\"\n\
                 approved = false\n\n",
            );
            intent_idx += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgevolve_core::ir::cluster::catalog::ClusterCatalog;

    #[test]
    fn plan_id_differs_for_different_catalogs() {
        let a = ClusterCatalog::empty();
        let mut b = ClusterCatalog::empty();
        b.roles.push(pgevolve_core::ir::cluster::role::Role {
            name: pgevolve_core::identifier::Identifier::from_unquoted("reader").unwrap(),
            attributes: pgevolve_core::ir::cluster::role::RoleAttributes::default(),
            member_of: vec![],
            comment: None,
        });
        let id_a = compute_cluster_plan_id(&a, &a).unwrap();
        let id_b = compute_cluster_plan_id(&a, &b).unwrap();
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn plan_id_is_deterministic() {
        let c = ClusterCatalog::empty();
        let id1 = compute_cluster_plan_id(&c, &c).unwrap();
        let id2 = compute_cluster_plan_id(&c, &c).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn plan_id_is_8_bytes_hex() {
        let c = ClusterCatalog::empty();
        let id = compute_cluster_plan_id(&c, &c).unwrap();
        assert_eq!(id.len(), 16, "8 bytes = 16 hex chars, got: {id}");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn intent_toml_empty_for_no_destructive_steps() {
        let out = build_intent_toml(&[]);
        assert!(!out.contains("[[destructive_intent]]"));
    }
}
