//! Cluster diffing.
//!
//! Pair-by-name on roles; emit one [`ClusterChange`] per difference. All ops
//! are catalog-only metadata (DDL through `pg_authid`), so they're safe by
//! default — except `DropRole`, which is intent-gated because it can orphan
//! grants in other DBs.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::diff::destructiveness::Destructiveness;
use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::ir::cluster::role::{Role, RoleAttributes};

/// One change to apply to a cluster's role layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClusterChange {
    /// Create a new role with the given definition.
    CreateRole(Role),
    /// Drop an existing role by name.
    DropRole {
        /// Name of the role to drop.
        name: Identifier,
    },
    /// Alter the attributes of an existing role.
    ///
    /// Both `from` and `to` are carried so that downstream lint rules (e.g.
    /// `role-loses-superuser`) can inspect the before state.
    AlterRoleAttributes {
        /// Name of the role being altered.
        name: Identifier,
        /// Attribute values before the change.
        from: RoleAttributes,
        /// Attribute values after the change.
        to: RoleAttributes,
    },
    /// Grant membership of `role` to `member`.
    GrantRoleMembership {
        /// Who gains the membership.
        member: Identifier,
        /// Which role they become a member of.
        role: Identifier,
    },
    /// Revoke membership of `role` from `member`.
    RevokeRoleMembership {
        /// Who loses the membership.
        member: Identifier,
        /// Which role they are removed from.
        role: Identifier,
    },
    /// Set or clear the comment on a role.
    CommentOnRole {
        /// Name of the role.
        name: Identifier,
        /// New comment value; `None` clears the existing comment.
        comment: Option<String>,
    },
}

/// A single entry in a [`ClusterChangeSet`], pairing a change with its risk level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterChangeEntry {
    /// The structural change.
    pub change: ClusterChange,
    /// Risk classification for the change.
    pub destructiveness: Destructiveness,
}

/// The full set of changes needed to converge one [`ClusterCatalog`] to another.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClusterChangeSet {
    /// Ordered list of change entries.
    pub entries: Vec<ClusterChangeEntry>,
}

impl ClusterChangeSet {
    /// Returns `true` when no changes are present.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Diff `source` against `target`. `target` = current live cluster state;
/// `source` = desired state from `roles/*.sql`. Resulting ops applied to
/// `target` produce `source`.
#[must_use]
pub fn diff_cluster(target: &ClusterCatalog, source: &ClusterCatalog) -> ClusterChangeSet {
    let mut entries = Vec::new();

    let target_map: BTreeMap<&Identifier, &Role> =
        target.roles.iter().map(|r| (&r.name, r)).collect();
    let source_map: BTreeMap<&Identifier, &Role> =
        source.roles.iter().map(|r| (&r.name, r)).collect();

    // Adds.
    for (name, source_role) in &source_map {
        if !target_map.contains_key(name) {
            entries.push(ClusterChangeEntry {
                change: ClusterChange::CreateRole((*source_role).clone()),
                destructiveness: Destructiveness::Safe,
            });
        }
    }

    // Drops + alters.
    for (name, target_role) in &target_map {
        match source_map.get(name) {
            None => entries.push(ClusterChangeEntry {
                change: ClusterChange::DropRole {
                    name: (*name).clone(),
                },
                destructiveness: Destructiveness::RequiresApprovalAndDataLossWarning {
                    reason: format!("drops role {name} — may orphan grants in other DBs"),
                },
            }),
            Some(source_role) => diff_role(target_role, source_role, &mut entries),
        }
    }

    ClusterChangeSet { entries }
}

fn diff_role(target: &Role, source: &Role, out: &mut Vec<ClusterChangeEntry>) {
    if target.attributes != source.attributes {
        out.push(ClusterChangeEntry {
            change: ClusterChange::AlterRoleAttributes {
                name: target.name.clone(),
                from: target.attributes.clone(),
                to: source.attributes.clone(),
            },
            destructiveness: Destructiveness::Safe,
        });
    }

    // Membership: emit one Grant per added edge, one Revoke per removed.
    let target_membership: BTreeSet<&Identifier> = target.member_of.iter().collect();
    let source_membership: BTreeSet<&Identifier> = source.member_of.iter().collect();

    for added in source_membership.difference(&target_membership) {
        out.push(ClusterChangeEntry {
            change: ClusterChange::GrantRoleMembership {
                member: target.name.clone(),
                role: (*added).clone(),
            },
            destructiveness: Destructiveness::Safe,
        });
    }
    for removed in target_membership.difference(&source_membership) {
        out.push(ClusterChangeEntry {
            change: ClusterChange::RevokeRoleMembership {
                member: target.name.clone(),
                role: (*removed).clone(),
            },
            destructiveness: Destructiveness::Safe,
        });
    }

    if target.comment != source.comment {
        out.push(ClusterChangeEntry {
            change: ClusterChange::CommentOnRole {
                name: target.name.clone(),
                comment: source.comment.clone(),
            },
            destructiveness: Destructiveness::Safe,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn role(name: &str) -> Role {
        Role {
            name: id(name),
            attributes: RoleAttributes::default(),
            member_of: vec![],
            comment: None,
        }
    }

    #[test]
    fn equal_catalogs_yield_no_changes() {
        let c = ClusterCatalog {
            roles: vec![role("a")],
        };
        let cs = diff_cluster(&c, &c);
        assert!(cs.is_empty());
    }

    #[test]
    fn added_role_creates() {
        let target = ClusterCatalog::empty();
        let source = ClusterCatalog {
            roles: vec![role("a")],
        };
        let cs = diff_cluster(&target, &source);
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(cs.entries[0].change, ClusterChange::CreateRole(_)));
        assert_eq!(cs.entries[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn removed_role_drops_with_intent_gate() {
        let target = ClusterCatalog {
            roles: vec![role("a")],
        };
        let source = ClusterCatalog::empty();
        let cs = diff_cluster(&target, &source);
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(
            cs.entries[0].change,
            ClusterChange::DropRole { .. }
        ));
        assert!(cs.entries[0].destructiveness.requires_approval());
        assert!(cs.entries[0].destructiveness.data_loss_risk());
    }

    #[test]
    fn attribute_change_emits_alter() {
        let t = role("a");
        let mut s = role("a");
        s.attributes.login = true;
        let cs = diff_cluster(
            &ClusterCatalog { roles: vec![t] },
            &ClusterCatalog { roles: vec![s] },
        );
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(
            cs.entries[0].change,
            ClusterChange::AlterRoleAttributes { .. }
        ));
    }

    #[test]
    fn added_membership_emits_grant() {
        let t = role("a");
        let mut s = role("a");
        s.member_of.push(id("readers"));
        let cs = diff_cluster(
            &ClusterCatalog { roles: vec![t] },
            &ClusterCatalog { roles: vec![s] },
        );
        assert_eq!(cs.entries.len(), 1);
        match &cs.entries[0].change {
            ClusterChange::GrantRoleMembership { member, role } => {
                assert_eq!(member.as_str(), "a");
                assert_eq!(role.as_str(), "readers");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn removed_membership_emits_revoke() {
        let mut t = role("a");
        let s = role("a");
        t.member_of.push(id("readers"));
        let cs = diff_cluster(
            &ClusterCatalog { roles: vec![t] },
            &ClusterCatalog { roles: vec![s] },
        );
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(
            cs.entries[0].change,
            ClusterChange::RevokeRoleMembership { .. }
        ));
    }

    #[test]
    fn comment_change_emits_comment_op() {
        let t = role("a");
        let mut s = role("a");
        s.comment = Some("hello".into());
        let cs = diff_cluster(
            &ClusterCatalog { roles: vec![t] },
            &ClusterCatalog { roles: vec![s] },
        );
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(
            cs.entries[0].change,
            ClusterChange::CommentOnRole { .. }
        ));
    }
}
