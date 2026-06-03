//! Diff `ALTER DEFAULT PRIVILEGES` rules.
//!
//! Rules are paired by `(target_role, schema, object_type)`. Within each pair
//! the grant list is diffed with the standard lenient policy from
//! [`super::grants::diff_grants`].
//!
//! ## Managed-roles extension for owned rules
//!
//! When the source catalog explicitly declares a rule (same key present in
//! `source`), we treat every grantee that appears in the target's grant list
//! for that rule as "managed" — even if that role does not appear elsewhere in
//! the source catalog. Rationale: pgevolve previously applied the GRANTs via
//! that same rule, so the grantees are our responsibility to revoke when the
//! desired grant list shrinks. The lenient unmanaged-drift policy is reserved
//! for rules that exist **only** in the live database and have no corresponding
//! source declaration.

use std::collections::{BTreeMap, BTreeSet};

use crate::identifier::Identifier;
use crate::ir::default_privileges::{DefaultPrivObjectType, DefaultPrivilegeRule};
use crate::ir::grant::{Grant, GrantTarget};

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

        // When the source explicitly declares this rule, any grantee present in
        // the target's grant list was placed there by a previous pgevolve apply
        // and must be revocable. Extend the managed-roles set with those
        // grantees so that `diff_grants` does not suppress them as "unmanaged".
        let extended;
        let effective_managed: &BTreeSet<Identifier> = if source_map.contains_key(&k) {
            extended = target_grants
                .iter()
                .filter_map(|g| {
                    if let GrantTarget::Role(name) = &g.grantee {
                        Some(name.clone())
                    } else {
                        None
                    }
                })
                .fold(managed_roles.clone(), |mut acc, name| {
                    acc.insert(name);
                    acc
                });
            &extended
        } else {
            managed_roles
        };

        let (to_add, to_revoke, _unmanaged) =
            super::grants::diff_grants(target_grants, source_grants, effective_managed);

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

    /// Regression for issue #23: when the mutated (source) catalog removes
    /// grantees from a default-privilege rule but those grantees are not
    /// mentioned elsewhere in the source catalog, they were silently dropped
    /// from the REVOKE side (classified as "unmanaged"). The fix: when the
    /// source catalog explicitly declares the rule (same key exists in source),
    /// treat target grantees for that rule as managed regardless of the
    /// catalog-wide `managed_roles` set.
    #[test]
    fn revoke_unmanaged_grantee_when_source_owns_the_rule() {
        use crate::ir::grant::GrantTarget;

        // Target has 3 grants including Role("app") as grantee.
        let tgt = rule(
            "app",
            None,
            DefaultPrivObjectType::Schemas,
            vec![
                Grant {
                    grantee: GrantTarget::Public,
                    privilege: Privilege::Usage,
                    with_grant_option: false,
                    columns: None,
                },
                Grant {
                    grantee: GrantTarget::Role(id("app")),
                    privilege: Privilege::Usage,
                    with_grant_option: false,
                    columns: None,
                },
                Grant {
                    grantee: GrantTarget::Role(id("app")),
                    privilege: Privilege::Create,
                    with_grant_option: false,
                    columns: None,
                },
            ],
        );

        // Source has the same rule key but only 1 grant (Public/Usage).
        // "app" does NOT appear in managed_roles.
        let src = rule(
            "app",
            None,
            DefaultPrivObjectType::Schemas,
            vec![Grant {
                grantee: GrantTarget::Public,
                privilege: Privilege::Usage,
                with_grant_option: false,
                columns: None,
            }],
        );

        // "app" is not in managed_roles — only "app_owner" is.
        let changes = diff_default_privileges(&[tgt], &[src], &managed(&["app_owner"]));

        // Must emit 2 REVOKEs for Role("app")/Usage and Role("app")/Create.
        let revokes: Vec<_> = changes.iter().filter(|c| !c.is_grant).collect();
        assert_eq!(
            revokes.len(),
            2,
            "expected 2 revokes for Role(app) grants, got: {changes:?}"
        );
        let priv_set: std::collections::BTreeSet<Privilege> =
            revokes.iter().map(|c| c.grant.privilege).collect();
        assert!(
            priv_set.contains(&Privilege::Usage),
            "expected REVOKE Usage"
        );
        assert!(
            priv_set.contains(&Privilege::Create),
            "expected REVOKE Create"
        );
        // No spurious GRANTs.
        let grants_count = changes.iter().filter(|c| c.is_grant).count();
        assert_eq!(grants_count, 0, "expected no new grants, got: {changes:?}");
    }

    /// Regression for issue #30: same shape as #23 but the `target_role` of the
    /// rule (`ops`) also appears as a grantee in the live (target) grants.
    /// If `ops` is not in `managed_roles` and does not appear in the source's
    /// grant list (only `readers` does), then prior to the fix the extended
    /// managed set would not include `ops` and the two REVOKE steps would be
    /// suppressed.
    ///
    /// Scenario from the bug report:
    ///   target: `[Grant{ops, Usage}, Grant{ops, Create}, Grant{readers, Create}]`
    ///   source: `[Grant{readers, Create}]`
    ///   `managed_roles`: `{readers}` (tight — `ops` is absent)
    ///
    /// Expected: 2 REVOKE steps (for the two `ops` grants).
    #[test]
    fn revoke_target_role_as_grantee_not_in_source_grants() {
        // target (live database state)
        let tgt = rule(
            "ops",
            None,
            DefaultPrivObjectType::Schemas,
            vec![
                grant_to("ops", Privilege::Usage),
                grant_to("ops", Privilege::Create),
                grant_to("readers", Privilege::Create),
            ],
        );

        // source (desired state): only readers/Create remains
        let src = rule(
            "ops",
            None,
            DefaultPrivObjectType::Schemas,
            vec![grant_to("readers", Privilege::Create)],
        );

        // managed_roles contains only `readers`; `ops` is deliberately absent
        let changes = diff_default_privileges(&[tgt], &[src], &managed(&["readers"]));

        let revokes: Vec<_> = changes.iter().filter(|c| !c.is_grant).collect();
        assert_eq!(
            revokes.len(),
            2,
            "expected 2 REVOKEs for the ops grants, got: {changes:?}"
        );
        let priv_set: std::collections::BTreeSet<Privilege> =
            revokes.iter().map(|c| c.grant.privilege).collect();
        assert!(
            priv_set.contains(&Privilege::Usage),
            "expected REVOKE Usage for ops"
        );
        assert!(
            priv_set.contains(&Privilege::Create),
            "expected REVOKE Create for ops"
        );
        // No spurious GRANTs.
        let grants_count = changes.iter().filter(|c| c.is_grant).count();
        assert_eq!(grants_count, 0, "expected no new grants, got: {changes:?}");
    }
}
