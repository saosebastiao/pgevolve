//! `role-membership-cycle` lint rule.
//!
//! Errors when the projected post-apply membership graph contains a cycle.
//!
//! Approach: build the membership graph from the current source IR plus the
//! changeset's pending grants/revokes; check for a cycle reachable from each
//! pending grant's `member`. PG rejects cycles at apply time; we catch them
//! pre-plan for a better error.

use std::collections::{BTreeMap, BTreeSet};

use crate::diff::cluster::{ClusterChange, ClusterChangeSet};
use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::lint::finding::{Finding, Severity};

/// Rule ID emitted on the finding; matches the file name.
pub const RULE_ID: &str = "role-membership-cycle";

/// Cycle detection needs the source IR (for existing membership edges) plus
/// the changeset (for pending grants). Signature differs from
/// `check_changeset`'s single-arg shape; see `universal::check_cluster_changeset`
/// for how it gets called.
pub fn check(source: &ClusterCatalog, cs: &ClusterChangeSet) -> Vec<Finding> {
    // Build the post-apply membership graph: member → set of parent roles.
    let mut graph: BTreeMap<Identifier, BTreeSet<Identifier>> = BTreeMap::new();
    for r in &source.roles {
        graph.insert(r.name.clone(), r.member_of.iter().cloned().collect());
    }
    for entry in &cs.entries {
        match &entry.change {
            ClusterChange::GrantRoleMembership { member, role } => {
                graph
                    .entry(member.clone())
                    .or_default()
                    .insert(role.clone());
            }
            ClusterChange::RevokeRoleMembership { member, role } => {
                if let Some(set) = graph.get_mut(member) {
                    set.remove(role);
                }
            }
            _ => {}
        }
    }

    // For each pending grant, check that the post-apply graph from `role`
    // (the parent) doesn't reach back to `member`.
    let mut findings = Vec::new();
    for entry in &cs.entries {
        if let ClusterChange::GrantRoleMembership { member, role } = &entry.change
            && reaches(&graph, role, member)
        {
            findings.push(Finding {
                severity: Severity::Error,
                rule: RULE_ID,
                message: format!(
                    "GRANT {role} TO {member} creates a role-membership cycle; \
                     Postgres will reject this at apply time"
                ),
                location: None,
            });
        }
    }
    findings
}

/// Returns `true` if `target` is reachable from `from` by following membership
/// edges in `graph` (DFS, cycle-safe via a visited set).
fn reaches(
    graph: &BTreeMap<Identifier, BTreeSet<Identifier>>,
    from: &Identifier,
    target: &Identifier,
) -> bool {
    if from == target {
        return true;
    }
    let mut stack = vec![from.clone()];
    let mut seen = BTreeSet::new();
    while let Some(node) = stack.pop() {
        if !seen.insert(node.clone()) {
            continue;
        }
        if let Some(parents) = graph.get(&node) {
            for p in parents {
                if p == target {
                    return true;
                }
                stack.push(p.clone());
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::cluster::{ClusterChange, ClusterChangeEntry, ClusterChangeSet};
    use crate::diff::destructiveness::Destructiveness;
    use crate::identifier::Identifier;
    use crate::ir::cluster::role::{Role, RoleAttributes};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn role(name: &str, parents: Vec<&str>) -> Role {
        Role {
            name: id(name),
            attributes: RoleAttributes::default(),
            member_of: parents.into_iter().map(id).collect(),
            comment: None,
        }
    }

    fn grant(member: &str, role_name: &str) -> ClusterChangeEntry {
        ClusterChangeEntry {
            change: ClusterChange::GrantRoleMembership {
                member: id(member),
                role: id(role_name),
            },
            destructiveness: Destructiveness::Safe,
        }
    }

    #[test]
    fn direct_cycle_fires() {
        // a -> b already exists in source. Pending: b -> a. Cycle.
        let src = ClusterCatalog {
            roles: vec![role("a", vec!["b"]), role("b", vec![])],
        };
        let cs = ClusterChangeSet {
            entries: vec![grant("b", "a")],
        };
        let f = check(&src, &cs);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Error);
        assert_eq!(f[0].rule, RULE_ID);
    }

    #[test]
    fn self_cycle_fires() {
        let src = ClusterCatalog {
            roles: vec![role("a", vec![])],
        };
        let cs = ClusterChangeSet {
            entries: vec![grant("a", "a")],
        };
        let f = check(&src, &cs);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn dag_silent() {
        let src = ClusterCatalog {
            roles: vec![role("a", vec![]), role("b", vec![])],
        };
        let cs = ClusterChangeSet {
            entries: vec![grant("a", "b")],
        };
        assert!(check(&src, &cs).is_empty());
    }

    #[test]
    fn revoke_breaks_cycle() {
        // a -> b -> a cycle currently exists in source. Pending revoke a->b
        // means no cycle is reachable; no pending grants, so no findings.
        let src = ClusterCatalog {
            roles: vec![role("a", vec!["b"]), role("b", vec!["a"])],
        };
        let cs = ClusterChangeSet {
            entries: vec![ClusterChangeEntry {
                change: ClusterChange::RevokeRoleMembership {
                    member: id("a"),
                    role: id("b"),
                },
                destructiveness: Destructiveness::Safe,
            }],
        };
        // No pending grants → no cycle findings (rule only checks grants).
        assert!(check(&src, &cs).is_empty());
    }
}
