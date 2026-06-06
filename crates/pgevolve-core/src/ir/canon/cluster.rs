//! Canon rules for the cluster IR. Currently:
//!
//! - Sort `ClusterCatalog::roles` by name.
//! - Sort each role's `member_of` lexicographically.
//! - Sort `ClusterCatalog::tablespaces` by name; reject duplicates.

use crate::ir::IrError;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::ir::cluster::role::Role;

/// Run every cluster-canon rule. Idempotent.
///
/// # Errors
/// Returns [`IrError::DuplicateTablespace`] if two tablespaces share a name.
pub fn run(cat: &mut ClusterCatalog) -> Result<(), IrError> {
    for role in &mut cat.roles {
        normalize_membership_order(role);
    }
    cat.roles
        .sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    cat.tablespaces
        .sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    for w in cat.tablespaces.windows(2) {
        if w[0].name == w[1].name {
            return Err(IrError::DuplicateTablespace(w[0].name.clone()));
        }
    }
    Ok(())
}

/// Sort `member_of` lexicographically by role name.
fn normalize_membership_order(role: &mut Role) {
    role.member_of.sort_by(|a, b| a.as_str().cmp(b.as_str()));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::cluster::role::{Role, RoleAttributes};
    use crate::ir::cluster::tablespace::Tablespace;

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

    fn tablespace(name: &str) -> Tablespace {
        Tablespace {
            name: id(name),
            location: "/mnt/test".to_string(),
            owner: None,
            options: BTreeMap::new(),
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
            tablespaces: vec![],
        };
        run(&mut c).unwrap();
        let snap1 = format!("{c:?}");
        run(&mut c).unwrap();
        let snap2 = format!("{c:?}");
        assert_eq!(snap1, snap2);
    }

    #[test]
    fn tablespaces_sort_by_name() {
        let mut c = ClusterCatalog {
            roles: vec![],
            tablespaces: vec![
                tablespace("zebra"),
                tablespace("alpha"),
                tablespace("middle"),
            ],
        };
        run(&mut c).unwrap();
        let names: Vec<_> = c
            .tablespaces
            .iter()
            .map(|t| t.name.as_str().to_owned())
            .collect();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }

    #[test]
    fn duplicate_tablespace_names_error() {
        let mut c = ClusterCatalog {
            roles: vec![],
            tablespaces: vec![tablespace("ssd"), tablespace("ssd")],
        };
        assert!(matches!(
            run(&mut c).unwrap_err(),
            IrError::DuplicateTablespace(_)
        ));
    }
}
