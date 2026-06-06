//! Cluster diffing.
//!
//! Pair-by-name on roles and tablespaces; emit one [`ClusterChange`] per
//! difference. All ops are catalog-only metadata, so they're safe by default —
//! except `DropRole` and `DropTablespace`, which are intent-gated because they
//! can orphan objects in other parts of the cluster.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::diff::destructiveness::Destructiveness;
use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::ir::cluster::role::{Role, RoleAttributes};
use crate::ir::cluster::tablespace::Tablespace;

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

    /// `CREATE TABLESPACE …`
    CreateTablespace(Tablespace),
    /// `DROP TABLESPACE …` — destructive.
    DropTablespace {
        /// Name of the tablespace to drop.
        name: Identifier,
    },
    /// `ALTER TABLESPACE name OWNER TO owner;`
    AlterTablespaceOwner {
        /// Name of the tablespace.
        name: Identifier,
        /// New owner role.
        owner: Identifier,
    },
    /// `ALTER TABLESPACE name SET (k = v, …);` — only source-declared options
    /// that differ from live (lenient: live-only options are never reset).
    SetTablespaceOptions {
        /// Name of the tablespace.
        name: Identifier,
        /// Subset of options whose value differs from live.
        options: BTreeMap<String, String>,
    },
    /// `COMMENT ON TABLESPACE name IS …;`
    CommentOnTablespace {
        /// Name of the tablespace.
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

    // Tablespaces. Lenient owner/options; location is immutable in PG
    // (drift → tablespace-location-drift lint, not a change).
    let target_ts: BTreeMap<&Identifier, &Tablespace> =
        target.tablespaces.iter().map(|t| (&t.name, t)).collect();
    let source_ts: BTreeMap<&Identifier, &Tablespace> =
        source.tablespaces.iter().map(|t| (&t.name, t)).collect();

    for (name, s) in &source_ts {
        match target_ts.get(name) {
            None => entries.push(ClusterChangeEntry {
                change: ClusterChange::CreateTablespace((*s).clone()),
                destructiveness: Destructiveness::Safe,
            }),
            Some(t) => {
                // Owner (lenient): only emit if source declares an owner and it
                // differs from live.
                if let Some(src_owner) = &s.owner
                    && t.owner.as_ref() != Some(src_owner)
                {
                    entries.push(ClusterChangeEntry {
                        change: ClusterChange::AlterTablespaceOwner {
                            name: (*name).clone(),
                            owner: src_owner.clone(),
                        },
                        destructiveness: Destructiveness::Safe,
                    });
                }

                // Options (lenient): emit only source options whose value
                // differs from live; never reset live-only options.
                let mut set: BTreeMap<String, String> = BTreeMap::new();
                for (k, v) in &s.options {
                    if t.options.get(k) != Some(v) {
                        set.insert(k.clone(), v.clone());
                    }
                }
                if !set.is_empty() {
                    entries.push(ClusterChangeEntry {
                        change: ClusterChange::SetTablespaceOptions {
                            name: (*name).clone(),
                            options: set,
                        },
                        destructiveness: Destructiveness::Safe,
                    });
                }

                // Comment.
                if t.comment != s.comment {
                    entries.push(ClusterChangeEntry {
                        change: ClusterChange::CommentOnTablespace {
                            name: (*name).clone(),
                            comment: s.comment.clone(),
                        },
                        destructiveness: Destructiveness::Safe,
                    });
                }

                // Location: immutable in PG — drift is handled by the
                // tablespace-location-drift lint, NOT a change here.
            }
        }
    }

    for name in target_ts.keys() {
        if !source_ts.contains_key(name) {
            entries.push(ClusterChangeEntry {
                change: ClusterChange::DropTablespace {
                    name: (*name).clone(),
                },
                destructiveness: Destructiveness::RequiresApprovalAndDataLossWarning {
                    reason: format!("drops tablespace {name} — objects using it will fail"),
                },
            });
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
            tablespaces: vec![],
        };
        let cs = diff_cluster(&c, &c);
        assert!(cs.is_empty());
    }

    #[test]
    fn added_role_creates() {
        let target = ClusterCatalog::empty();
        let source = ClusterCatalog {
            roles: vec![role("a")],
            tablespaces: vec![],
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
            tablespaces: vec![],
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
            &ClusterCatalog {
                roles: vec![t],
                tablespaces: vec![],
            },
            &ClusterCatalog {
                roles: vec![s],
                tablespaces: vec![],
            },
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
            &ClusterCatalog {
                roles: vec![t],
                tablespaces: vec![],
            },
            &ClusterCatalog {
                roles: vec![s],
                tablespaces: vec![],
            },
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
            &ClusterCatalog {
                roles: vec![t],
                tablespaces: vec![],
            },
            &ClusterCatalog {
                roles: vec![s],
                tablespaces: vec![],
            },
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
            &ClusterCatalog {
                roles: vec![t],
                tablespaces: vec![],
            },
            &ClusterCatalog {
                roles: vec![s],
                tablespaces: vec![],
            },
        );
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(
            cs.entries[0].change,
            ClusterChange::CommentOnRole { .. }
        ));
    }

    // -----------------------------------------------------------------------
    // Tablespace tests
    // -----------------------------------------------------------------------

    fn ts(name: &str) -> Tablespace {
        Tablespace {
            name: id(name),
            location: "/mnt/ssd".to_string(),
            owner: None,
            options: BTreeMap::new(),
            comment: None,
        }
    }

    #[test]
    fn source_only_tablespace_creates() {
        let target = ClusterCatalog::empty();
        let source = ClusterCatalog {
            roles: vec![],
            tablespaces: vec![ts("fast")],
        };
        let cs = diff_cluster(&target, &source);
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(
            cs.entries[0].change,
            ClusterChange::CreateTablespace(_)
        ));
        assert_eq!(cs.entries[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn live_only_tablespace_drops_with_intent_gate() {
        let target = ClusterCatalog {
            roles: vec![],
            tablespaces: vec![ts("fast")],
        };
        let source = ClusterCatalog::empty();
        let cs = diff_cluster(&target, &source);
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(
            cs.entries[0].change,
            ClusterChange::DropTablespace { .. }
        ));
        assert!(cs.entries[0].destructiveness.requires_approval());
        assert!(cs.entries[0].destructiveness.data_loss_risk());
    }

    #[test]
    fn owner_change_emits_alter_owner() {
        let t = ts("fast");
        let mut s = ts("fast");
        s.owner = Some(id("dba"));
        let cs = diff_cluster(
            &ClusterCatalog {
                roles: vec![],
                tablespaces: vec![t],
            },
            &ClusterCatalog {
                roles: vec![],
                tablespaces: vec![s],
            },
        );
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(
            cs.entries[0].change,
            ClusterChange::AlterTablespaceOwner { .. }
        ));
        assert_eq!(cs.entries[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn source_owner_none_emits_nothing_even_if_live_has_owner() {
        // Lenient: source owner = None means "unmanaged" — do not emit an alter.
        let mut t = ts("fast");
        t.owner = Some(id("postgres"));
        let s = ts("fast"); // owner = None
        let cs = diff_cluster(
            &ClusterCatalog {
                roles: vec![],
                tablespaces: vec![t],
            },
            &ClusterCatalog {
                roles: vec![],
                tablespaces: vec![s],
            },
        );
        assert!(cs.is_empty());
    }

    #[test]
    fn options_diff_emits_only_source_options() {
        // source has {a:1, b:2}, live has {a:1, c:9}
        // → emit SetTablespaceOptions for {b:2} only; live-only c is NOT reset.
        let mut t = ts("fast");
        t.options.insert("a".to_string(), "1".to_string());
        t.options.insert("c".to_string(), "9".to_string());
        let mut s = ts("fast");
        s.options.insert("a".to_string(), "1".to_string());
        s.options.insert("b".to_string(), "2".to_string());
        let cs = diff_cluster(
            &ClusterCatalog {
                roles: vec![],
                tablespaces: vec![t],
            },
            &ClusterCatalog {
                roles: vec![],
                tablespaces: vec![s],
            },
        );
        assert_eq!(cs.entries.len(), 1);
        match &cs.entries[0].change {
            ClusterChange::SetTablespaceOptions { name, options } => {
                assert_eq!(name.as_str(), "fast");
                assert_eq!(options.len(), 1);
                assert_eq!(options.get("b").map(String::as_str), Some("2"));
            }
            other => panic!("expected SetTablespaceOptions, got {other:?}"),
        }
    }

    #[test]
    fn comment_change_emits_comment_on_tablespace() {
        let t = ts("fast");
        let mut s = ts("fast");
        s.comment = Some("fast storage".to_string());
        let cs = diff_cluster(
            &ClusterCatalog {
                roles: vec![],
                tablespaces: vec![t],
            },
            &ClusterCatalog {
                roles: vec![],
                tablespaces: vec![s],
            },
        );
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(
            cs.entries[0].change,
            ClusterChange::CommentOnTablespace { .. }
        ));
    }

    #[test]
    fn location_change_emits_nothing() {
        // PG cannot relocate a tablespace; drift is handled by the
        // tablespace-location-drift lint, not a ClusterChange.
        let t = ts("fast"); // location = "/mnt/ssd"
        let mut s = ts("fast");
        s.location = "/mnt/nvme".to_string();
        let cs = diff_cluster(
            &ClusterCatalog {
                roles: vec![],
                tablespaces: vec![t],
            },
            &ClusterCatalog {
                roles: vec![],
                tablespaces: vec![s],
            },
        );
        assert!(
            cs.is_empty(),
            "location drift must not produce a change; got {cs:?}"
        );
    }
}
