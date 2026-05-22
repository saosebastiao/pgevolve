//! Diff `ALTER DEFAULT PRIVILEGES` rules.
//!
//! Rules are paired by `(target_role, schema, object_type)`. Within each pair
//! the grant list is diffed with the standard lenient policy from
//! [`super::grants::diff_grants`].

use std::collections::{BTreeMap, BTreeSet};

use crate::identifier::Identifier;
use crate::ir::default_privileges::{DefaultPrivObjectType, DefaultPrivilegeRule};
use crate::ir::grant::Grant;

/// One ADD or REVOKE step for a default-privilege rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultPrivilegeChange {
    /// `FOR ROLE x` — the grantor role this rule targets.
    pub target_role: Identifier,
    /// `IN SCHEMA y` — schema scope. `None` = global scope.
    pub schema: Option<Identifier>,
    /// Object-type discriminant.
    pub object_type: DefaultPrivObjectType,
    /// `true` = GRANT, `false` = REVOKE.
    pub is_grant: bool,
    /// The specific grant being added or revoked.
    pub grant: Grant,
}

type RuleKey = (Identifier, Option<Identifier>, DefaultPrivObjectType);

/// Compute `ALTER DEFAULT PRIVILEGES` changes needed to converge `target`
/// (live database) toward `source` (desired state).
///
/// `managed_roles` is passed to the inner [`super::grants::diff_grants`] call
/// so that the lenient policy applies: unmanaged grantees in the live catalog
/// never produce REVOKE steps.
#[must_use]
pub fn diff_default_privileges(
    target: &[DefaultPrivilegeRule],
    source: &[DefaultPrivilegeRule],
    managed_roles: &BTreeSet<Identifier>,
) -> Vec<DefaultPrivilegeChange> {
    let key = |r: &DefaultPrivilegeRule| -> RuleKey {
        (r.target_role.clone(), r.schema.clone(), r.object_type)
    };

    let target_map: BTreeMap<RuleKey, &DefaultPrivilegeRule> =
        target.iter().map(|r| (key(r), r)).collect();
    let source_map: BTreeMap<RuleKey, &DefaultPrivilegeRule> =
        source.iter().map(|r| (key(r), r)).collect();

    let mut all_keys: BTreeSet<RuleKey> = target_map.keys().cloned().collect();
    all_keys.extend(source_map.keys().cloned());

    let mut out = Vec::new();
    for k in all_keys {
        let target_grants = target_map
            .get(&k)
            .map_or(&[] as &[Grant], |r| r.grants.as_slice());
        let source_grants = source_map
            .get(&k)
            .map_or(&[] as &[Grant], |r| r.grants.as_slice());

        let (to_add, to_revoke, _unmanaged) =
            super::grants::diff_grants(target_grants, source_grants, managed_roles);

        for g in to_add {
            out.push(DefaultPrivilegeChange {
                target_role: k.0.clone(),
                schema: k.1.clone(),
                object_type: k.2,
                is_grant: true,
                grant: g,
            });
        }
        for g in to_revoke {
            out.push(DefaultPrivilegeChange {
                target_role: k.0.clone(),
                schema: k.1.clone(),
                object_type: k.2,
                is_grant: false,
                grant: g,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::default_privileges::{DefaultPrivObjectType, DefaultPrivilegeRule};
    use crate::ir::grant::{Grant, GrantTarget, Privilege};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn grant_to(role: &str, priv_: Privilege) -> Grant {
        Grant {
            grantee: GrantTarget::Role(id(role)),
            privilege: priv_,
            with_grant_option: false,
            columns: None,
        }
    }

    fn rule(
        target_role: &str,
        schema: Option<&str>,
        object_type: DefaultPrivObjectType,
        grants: Vec<Grant>,
    ) -> DefaultPrivilegeRule {
        DefaultPrivilegeRule {
            target_role: id(target_role),
            schema: schema.map(id),
            object_type,
            grants,
        }
    }

    fn managed(roles: &[&str]) -> BTreeSet<Identifier> {
        roles.iter().map(|r| id(r)).collect()
    }

    #[test]
    fn add_only() {
        // Source has a rule; target does not → all grants in source become ADDs.
        let src = rule(
            "app_owner",
            None,
            DefaultPrivObjectType::Tables,
            vec![grant_to("web_anon", Privilege::Select)],
        );
        let changes = diff_default_privileges(&[], &[src], &managed(&["app_owner", "web_anon"]));
        assert_eq!(changes.len(), 1);
        assert!(changes[0].is_grant, "expected is_grant=true");
        assert_eq!(changes[0].grant.privilege, Privilege::Select);
    }

    #[test]
    fn revoke_only() {
        // Target has a rule; source does not → grants in target become REVOKEs
        // (for managed grantees).
        let tgt = rule(
            "app_owner",
            None,
            DefaultPrivObjectType::Tables,
            vec![grant_to("web_anon", Privilege::Select)],
        );
        let changes = diff_default_privileges(&[tgt], &[], &managed(&["app_owner", "web_anon"]));
        assert_eq!(changes.len(), 1);
        assert!(!changes[0].is_grant, "expected is_grant=false");
        assert_eq!(changes[0].grant.privilege, Privilege::Select);
    }

    #[test]
    fn mixed_add_and_revoke() {
        // Target has SELECT; source has INSERT.
        // SELECT → REVOKE; INSERT → ADD.
        let tgt = rule(
            "app_owner",
            Some("app"),
            DefaultPrivObjectType::Sequences,
            vec![grant_to("alice", Privilege::Select)],
        );
        let src = rule(
            "app_owner",
            Some("app"),
            DefaultPrivObjectType::Sequences,
            vec![grant_to("alice", Privilege::Usage)],
        );
        let changes = diff_default_privileges(&[tgt], &[src], &managed(&["app_owner", "alice"]));
        assert_eq!(changes.len(), 2, "expected 2 changes, got: {changes:?}");
        let adds: Vec<_> = changes.iter().filter(|c| c.is_grant).collect();
        let revs: Vec<_> = changes.iter().filter(|c| !c.is_grant).collect();
        assert_eq!(adds.len(), 1);
        assert_eq!(revs.len(), 1);
        assert_eq!(adds[0].grant.privilege, Privilege::Usage);
        assert_eq!(revs[0].grant.privilege, Privilege::Select);
    }
}
