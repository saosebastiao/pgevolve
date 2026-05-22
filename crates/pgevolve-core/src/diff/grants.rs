//! Shared grant-list diffing with lenient drift policy.
//!
//! The central rule: never silently REVOKE a grant to a role that is not
//! mentioned anywhere in the source catalog. Surface unmanaged grants in the
//! third return slot instead so downstream lint rules (Stage 11) can decide
//! what to do with them.

use std::collections::BTreeSet;

use crate::identifier::Identifier;
use crate::ir::catalog::Catalog;
use crate::ir::grant::{Grant, GrantTarget};

/// Compute additions and removals between target (catalog) and source (desired).
///
/// `managed_roles`: the set of role names mentioned anywhere in the source
/// catalog. Grants whose grantee is not in this set are considered unmanaged
/// and excluded from the revoke side (lenient policy). `PUBLIC` is always
/// considered managed.
///
/// Returns `(to_add, to_revoke, unmanaged_observed)`.
#[must_use]
pub fn diff_grants(
    target: &[Grant],
    source: &[Grant],
    managed_roles: &BTreeSet<Identifier>,
) -> (Vec<Grant>, Vec<Grant>, Vec<Grant>) {
    let target_set: BTreeSet<&Grant> = target.iter().collect();
    let source_set: BTreeSet<&Grant> = source.iter().collect();

    let to_add: Vec<Grant> = source_set
        .difference(&target_set)
        .map(|g| (*g).clone())
        .collect();

    let mut to_revoke = Vec::new();
    let mut unmanaged_observed = Vec::new();
    for g in target_set.difference(&source_set) {
        if grantee_is_managed(&g.grantee, managed_roles) {
            to_revoke.push((*g).clone());
        } else {
            unmanaged_observed.push((*g).clone());
        }
    }
    (to_add, to_revoke, unmanaged_observed)
}

fn grantee_is_managed(target: &GrantTarget, managed_roles: &BTreeSet<Identifier>) -> bool {
    match target {
        GrantTarget::Public => true,
        GrantTarget::Role(name) => managed_roles.contains(name),
    }
}

/// Collect every role name referenced anywhere in the source catalog —
/// in grants, owners, and default-privilege rules. Used as input to
/// [`diff_grants`].
#[must_use]
pub fn collect_managed_roles(cat: &Catalog) -> BTreeSet<Identifier> {
    let mut out = BTreeSet::new();

    for s in &cat.schemas {
        collect_grants_into(&s.grants, &mut out);
        if let Some(o) = &s.owner {
            out.insert(o.clone());
        }
    }
    for s in &cat.sequences {
        collect_grants_into(&s.grants, &mut out);
        if let Some(o) = &s.owner {
            out.insert(o.clone());
        }
    }
    for t in &cat.tables {
        collect_grants_into(&t.grants, &mut out);
        if let Some(o) = &t.owner {
            out.insert(o.clone());
        }
    }
    for v in &cat.views {
        collect_grants_into(&v.grants, &mut out);
        if let Some(o) = &v.owner {
            out.insert(o.clone());
        }
    }
    for m in &cat.materialized_views {
        collect_grants_into(&m.grants, &mut out);
        if let Some(o) = &m.owner {
            out.insert(o.clone());
        }
    }
    for f in &cat.functions {
        collect_grants_into(&f.grants, &mut out);
        if let Some(o) = &f.owner {
            out.insert(o.clone());
        }
    }
    for p in &cat.procedures {
        collect_grants_into(&p.grants, &mut out);
        if let Some(o) = &p.owner {
            out.insert(o.clone());
        }
    }
    for t in &cat.types {
        collect_grants_into(&t.grants, &mut out);
        if let Some(o) = &t.owner {
            out.insert(o.clone());
        }
    }
    for r in &cat.default_privileges {
        out.insert(r.target_role.clone());
        collect_grants_into(&r.grants, &mut out);
    }

    out
}

/// Insert the role name of every named-role grantee from `grants` into `set`.
fn collect_grants_into(grants: &[Grant], set: &mut BTreeSet<Identifier>) {
    for g in grants {
        if let GrantTarget::Role(name) = &g.grantee {
            set.insert(name.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::grant::{Grant, GrantTarget, Privilege};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn grant_to_role(role: &str, priv_: Privilege) -> Grant {
        Grant {
            grantee: GrantTarget::Role(id(role)),
            privilege: priv_,
            with_grant_option: false,
            columns: None,
        }
    }

    fn grant_public(priv_: Privilege) -> Grant {
        Grant {
            grantee: GrantTarget::Public,
            privilege: priv_,
            with_grant_option: false,
            columns: None,
        }
    }

    fn managed(roles: &[&str]) -> BTreeSet<Identifier> {
        roles.iter().map(|r| id(r)).collect()
    }

    #[test]
    fn empty_lists_yield_empty_diff() {
        let (add, rev, unmanaged) = diff_grants(&[], &[], &managed(&[]));
        assert!(add.is_empty());
        assert!(rev.is_empty());
        assert!(unmanaged.is_empty());
    }

    #[test]
    fn add_only_grant_in_source_not_in_target() {
        let g = grant_to_role("alice", Privilege::Select);
        let (add, rev, unmanaged) =
            diff_grants(&[], std::slice::from_ref(&g), &managed(&["alice"]));
        assert_eq!(add, vec![g]);
        assert!(rev.is_empty());
        assert!(unmanaged.is_empty());
    }

    #[test]
    fn revoke_managed_grantee() {
        // grant in target not in source, grantee is managed → to_revoke
        let g = grant_to_role("alice", Privilege::Select);
        let (add, rev, unmanaged) =
            diff_grants(std::slice::from_ref(&g), &[], &managed(&["alice"]));
        assert!(add.is_empty());
        assert_eq!(rev, vec![g]);
        assert!(unmanaged.is_empty());
    }

    #[test]
    fn ignore_unmanaged_grantee() {
        // grant in target not in source, grantee not in managed_roles → unmanaged_observed
        let g = grant_to_role("external_role", Privilege::Select);
        let (add, rev, unmanaged) = diff_grants(std::slice::from_ref(&g), &[], &managed(&[]));
        assert!(add.is_empty());
        assert!(rev.is_empty());
        assert_eq!(unmanaged, vec![g]);
    }

    #[test]
    fn public_always_managed() {
        // PUBLIC grantee is never unmanaged — even when managed_roles is empty
        let g = grant_public(Privilege::Usage);
        let (add, rev, unmanaged) = diff_grants(std::slice::from_ref(&g), &[], &managed(&[]));
        assert!(add.is_empty());
        assert_eq!(rev, vec![g]);
        assert!(unmanaged.is_empty());
    }
}
