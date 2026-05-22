//! `ClusterCatalog` — the cluster-wide IR root.
//!
//! Currently contains roles only. Other cluster object kinds (tablespaces,
//! `cluster_settings`, `foreign_servers`, `user_mappings`, databases) land in
//! follow-up sub-specs.

use serde::{Deserialize, Serialize};

use crate::ir::cluster::role::Role;

/// The root cluster IR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ClusterCatalog {
    /// Managed roles, sorted by `name` after `canonicalize()`.
    pub roles: Vec<Role>,
}

impl ClusterCatalog {
    /// Empty cluster catalog (no roles).
    #[must_use]
    pub const fn empty() -> Self {
        Self { roles: Vec::new() }
    }

    /// Normalize the catalog: sorts roles by name, sorts each role's
    /// `member_of` lexicographically. Idempotent. See
    /// [`crate::ir::canon`] for the per-rule details.
    pub fn canonicalize(&mut self) {
        crate::ir::canon::cluster::run(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::cluster::role::{Role, RoleAttributes};

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
    fn empty_catalog_canonicalizes_idempotently() {
        let mut c = ClusterCatalog::empty();
        c.canonicalize();
        c.canonicalize();
        assert!(c.roles.is_empty());
    }

    #[test]
    fn canonicalize_sorts_roles_by_name() {
        let mut c = ClusterCatalog {
            roles: vec![role("zebra"), role("alpha"), role("middle")],
        };
        c.canonicalize();
        let names: Vec<_> = c.roles.iter().map(|r| r.name.as_str().to_owned()).collect();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }
}
