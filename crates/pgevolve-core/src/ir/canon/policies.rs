//! Canon rules for table policies.

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
/// `GrantTarget::Ord` impl).
fn normalize_roles(p: &mut Policy) {
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
    fn sorts_roles_with_public_first() {
        let mut t = empty_table(qn("app", "users"));
        t.policies = vec![policy(
            "p",
            vec![
                GrantTarget::Role(id("zelda")),
                GrantTarget::Public,
                GrantTarget::Role(id("alice")),
            ],
        )];
        run_on_table(&mut t);
        // Public sorts first per GrantTarget::Ord.
        assert!(matches!(t.policies[0].roles[0], GrantTarget::Public));
        assert_eq!(t.policies[0].roles.len(), 3);
    }

    #[test]
    fn dedupes_duplicate_roles() {
        let mut t = empty_table(qn("app", "users"));
        t.policies = vec![policy(
            "p",
            vec![
                GrantTarget::Role(id("alice")),
                GrantTarget::Role(id("alice")),
                GrantTarget::Public,
                GrantTarget::Public,
            ],
        )];
        run_on_table(&mut t);
        assert_eq!(t.policies[0].roles.len(), 2);
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
