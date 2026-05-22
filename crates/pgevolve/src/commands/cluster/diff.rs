//! `pgevolve cluster diff` — show the diff between source and live cluster.
//!
//! Prints one line per change to stdout. Advisory lint findings
//! (e.g. `role-loses-superuser`, `role-membership-cycle`) are printed to
//! stderr so they reach the user without polluting parseable stdout output.

use std::path::Path;

use anyhow::Result;

use pgevolve_core::diff::cluster::ClusterChange;

use crate::api::cluster::build_cluster_plan;
use crate::cluster_config::ClusterConfig;

/// Run `pgevolve cluster diff`.
pub async fn run(project_root: &Path, cfg: &ClusterConfig) -> Result<i32> {
    let plan = build_cluster_plan(project_root, cfg)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if plan.changes.is_empty() {
        println!("No changes.");
        return Ok(0);
    }

    for entry in &plan.changes.entries {
        println!("{}", describe(&entry.change));
    }

    // Advisory findings MUST be printed to stderr so they reach the user.
    for finding in &plan.advisory_findings {
        eprintln!(
            "pgevolve cluster diff: advisory [{}]: {}",
            finding.rule, finding.message
        );
    }

    Ok(0)
}

fn describe(change: &ClusterChange) -> String {
    match change {
        ClusterChange::CreateRole(r) => format!("+ CREATE ROLE {}", r.name),
        ClusterChange::DropRole { name } => format!("- DROP ROLE {name}"),
        ClusterChange::AlterRoleAttributes { name, .. } => format!("~ ALTER ROLE {name}"),
        ClusterChange::GrantRoleMembership { member, role } => {
            format!("+ GRANT {role} TO {member}")
        }
        ClusterChange::RevokeRoleMembership { member, role } => {
            format!("- REVOKE {role} FROM {member}")
        }
        ClusterChange::CommentOnRole { name, .. } => format!("~ COMMENT ON ROLE {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgevolve_core::diff::cluster::ClusterChange;
    use pgevolve_core::identifier::Identifier;
    use pgevolve_core::ir::cluster::role::{Role, RoleAttributes};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn describe_create_role() {
        let r = Role {
            name: id("app_user"),
            attributes: RoleAttributes::default(),
            member_of: vec![],
            comment: None,
        };
        let s = describe(&ClusterChange::CreateRole(r));
        assert_eq!(s, "+ CREATE ROLE app_user");
    }

    #[test]
    fn describe_drop_role() {
        let s = describe(&ClusterChange::DropRole { name: id("old") });
        assert_eq!(s, "- DROP ROLE old");
    }

    #[test]
    fn describe_grant() {
        let s = describe(&ClusterChange::GrantRoleMembership {
            member: id("app"),
            role: id("readers"),
        });
        assert_eq!(s, "+ GRANT readers TO app");
    }

    #[test]
    fn describe_revoke() {
        let s = describe(&ClusterChange::RevokeRoleMembership {
            member: id("app"),
            role: id("readers"),
        });
        assert_eq!(s, "- REVOKE readers FROM app");
    }
}
