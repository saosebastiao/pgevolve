//! Canon rules for the cluster IR. Currently:
//!
//! - Sort `ClusterCatalog::roles` by name.
//! - Sort each role's `member_of` lexicographically.

use crate::ir::cluster::catalog::ClusterCatalog;
use crate::ir::cluster::role::Role;

/// Run every cluster-canon rule. Idempotent.
pub fn run(cat: &mut ClusterCatalog) {
    for role in &mut cat.roles {
        normalize_membership_order(role);
    }
    cat.roles
        .sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
}

/// Sort `member_of` lexicographically by role name.
fn normalize_membership_order(role: &mut Role) {
    role.member_of.sort_by(|a, b| a.as_str().cmp(b.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::cluster::role::{Role, RoleAttributes};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn role_with(name: &str, members: Vec<&str>) -> Role {
        Role {
            name: id(name),
            attributes: RoleAttributes::default(),
            member_of: members.into_iter().map(id).collect(),
            comment: None,
        }
    }

    #[test]
    fn sorts_member_of() {
        let mut r = role_with("r", vec!["zebra", "alpha", "middle"]);
        normalize_membership_order(&mut r);
        let names: Vec<_> = r.member_of.iter().map(|i| i.as_str().to_owned()).collect();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }

    #[test]
    fn run_is_idempotent() {
        let mut c = ClusterCatalog {
            roles: vec![
                role_with("z", vec!["b", "a"]),
                role_with("a", vec!["c", "b"]),
            ],
        };
        run(&mut c);
        let snap1 = format!("{c:?}");
        run(&mut c);
        let snap2 = format!("{c:?}");
        assert_eq!(snap1, snap2);
    }
}
