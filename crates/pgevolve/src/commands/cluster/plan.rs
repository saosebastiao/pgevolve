//! `pgevolve cluster plan` — write `cluster-plans/<plan_id>/` directory
//! using the canonical Plan + 3-file serializer.

use std::path::Path;

use anyhow::{Context, Result};

use pgevolve_core::plan::write_plan_dir;

use crate::api::cluster::build_cluster_plan;
use crate::cluster_config::ClusterConfig;
use crate::target_identity::compute_cluster_target_identity;

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

    // Compute target_identity by opening a fresh connection. Don't reuse the
    // catalog connection — it was consumed by build_cluster_plan via
    // spawn_blocking.
    let (client, connection) = tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
        .await
        .context("connecting to cluster for target_identity")?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!(?err, "cluster plan target_identity connection ended");
        }
    });
    let target_identity = compute_cluster_target_identity(&client)
        .await
        .map_err(|e| anyhow::anyhow!("compute target_identity: {e}"))?;

    let core_plan_id =
        pgevolve_core::plan::PlanId::from_hex(&plan_id).context("plan_id hex parse")?;

    // Save advisory_findings before plan is consumed by to_plan.
    let advisory_findings = plan.advisory_findings.clone();

    let core_plan = plan
        .to_plan(core_plan_id, target_identity)
        .context("ClusterPlan::to_plan")?;

    let plan_dir = project_root.join("cluster-plans").join(&plan_id);
    std::fs::create_dir_all(&plan_dir)
        .with_context(|| format!("creating {}", plan_dir.display()))?;

    write_plan_dir(&core_plan, &plan_dir)
        .with_context(|| format!("writing {}", plan_dir.display()))?;

    println!("Wrote {}", plan_dir.display());
    println!(
        "  plan.sql ({} steps)",
        core_plan
            .groups
            .iter()
            .map(|g| g.steps.len())
            .sum::<usize>()
    );
    println!(
        "  intent.toml ({} destructive intents)",
        core_plan.intents.len()
    );
    println!("  manifest.toml");

    for finding in &advisory_findings {
        eprintln!(
            "pgevolve cluster plan: advisory [{}]: {}",
            finding.rule, finding.message
        );
    }

    Ok(0)
}

/// Compute a short plan id by hashing the canonical serialized source and
/// target cluster catalogs.
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
}
