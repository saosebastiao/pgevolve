//! Canon rules for table policies.

use crate::ir::grant::GrantTarget;
use crate::ir::policy::Policy;
use crate::ir::table::Table;

/// Sort each policy's roles + sort policies by name on a single table.
pub fn run_on_table(t: &mut Table) {
    for p in &mut t.policies {
        normalize_roles(p);
    }
    t.policies
        .sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
}

/// Sort `roles` lexicographically (with `Public` first per v0.3.1's
/// `GrantTarget::Ord` impl), and canonicalize PUBLIC-supersedence.
///
/// Postgres treats `PUBLIC` as "all roles, including future ones". When a
/// policy lists both `PUBLIC` and named roles (e.g. `TO PUBLIC, app_owner`),
/// `pg_policy.polroles` stores only the public pseudo-role — the named roles
/// are silently dropped because they are already covered. Keeping
/// `[Public, X]` in the IR therefore creates an impossible live state that
/// diverges from `[Public]` on round-trip.
///
/// Canonicalization rule: if `Public` appears anywhere in the list, replace
/// the whole list with `[Public]`.
fn normalize_roles(p: &mut Policy) {
    if p.roles.contains(&GrantTarget::Public) {
        p.roles = vec![GrantTarget::Public];
        return;
    }
    p.roles.sort();
    p.roles.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::grant::GrantTarget;
    use crate::ir::policy::{Policy, PolicyCommand};
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn policy(name: &str, roles: Vec<GrantTarget>) -> Policy {
        Policy {
            name: id(name),
            permissive: true,
            command: PolicyCommand::All,
            roles,
            using: None,
            with_check: None,
        }
    }

    fn empty_table(qname: QualifiedName) -> Table {
        Table {
            qname,
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
        }
    }

    #[test]
    fn sorts_policies_by_name() {
        let mut t = empty_table(qn("app", "users"));
        t.policies = vec![
            policy("zebra", vec![]),
            policy("alpha", vec![]),
            policy("middle", vec![]),
        ];
        run_on_table(&mut t);
        let names: Vec<_> = t
            .policies
            .iter()
            .map(|p| p.name.as_str().to_owned())
            .collect();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }

    #[test]
    fn sorts_roles_without_public() {
        // No Public present — roles are sorted lexicographically.
        let mut t = empty_table(qn("app", "users"));
        t.policies = vec![policy(
            "p",
            vec![
                GrantTarget::Role(id("zelda")),
                GrantTarget::Role(id("alice")),
            ],
        )];
        run_on_table(&mut t);
        assert_eq!(t.policies[0].roles[0], GrantTarget::Role(id("alice")));
        assert_eq!(t.policies[0].roles[1], GrantTarget::Role(id("zelda")));
    }

    #[test]
    fn dedupes_duplicate_roles_without_public() {
        // No Public present — duplicate named roles are deduped.
        let mut t = empty_table(qn("app", "users"));
        t.policies = vec![policy(
            "p",
            vec![
                GrantTarget::Role(id("alice")),
                GrantTarget::Role(id("alice")),
                GrantTarget::Role(id("bob")),
            ],
        )];
        run_on_table(&mut t);
        assert_eq!(t.policies[0].roles.len(), 2);
    }

    /// Regression for issue #31.
    ///
    /// PG silently drops named roles from `pg_policy.polroles` when PUBLIC is
    /// present — PUBLIC covers all roles, so the extras are redundant. The IR
    /// was treating `[Public, X]` and `[Public]` as distinct catalogs, which
    /// caused `round_trip` and `e2e` soak failures on PG 14 / 16 / 17.
    ///
    /// Canon must collapse `[Public, ...]` to `[Public]`.
    #[test]
    fn public_plus_named_role_collapses_to_public_only() {
        let mut t = empty_table(qn("app", "users"));
        t.policies = vec![policy(
            "p",
            vec![GrantTarget::Public, GrantTarget::Role(id("app_owner"))],
        )];
        run_on_table(&mut t);
        assert_eq!(
            t.policies[0].roles,
            vec![GrantTarget::Public],
            "expected [Public] but got {:?}",
            t.policies[0].roles
        );
    }

    /// Already-canonical `[Public]` must be left unchanged.
    #[test]
    fn public_alone_is_unchanged() {
        let mut t = empty_table(qn("app", "users"));
        t.policies = vec![policy("p", vec![GrantTarget::Public])];
        run_on_table(&mut t);
        assert_eq!(t.policies[0].roles, vec![GrantTarget::Public]);
    }

    /// A roles list with only named roles (no Public) must not be touched.
    #[test]
    fn named_roles_without_public_are_unchanged() {
        let mut t = empty_table(qn("app", "users"));
        t.policies = vec![policy(
            "p",
            vec![GrantTarget::Role(id("alice")), GrantTarget::Role(id("bob"))],
        )];
        run_on_table(&mut t);
        // Sorted deduped, still both present.
        assert_eq!(t.policies[0].roles.len(), 2);
        // No Public was introduced.
        assert!(!t.policies[0].roles.contains(&GrantTarget::Public));
    }

    #[test]
    fn run_is_idempotent() {
        let mut t = empty_table(qn("app", "users"));
        t.policies = vec![
            policy("b", vec![GrantTarget::Role(id("alice"))]),
            policy("a", vec![GrantTarget::Public]),
        ];
        run_on_table(&mut t);
        let snap1 = format!("{t:?}");
        run_on_table(&mut t);
        let snap2 = format!("{t:?}");
        assert_eq!(snap1, snap2);
    }
}
