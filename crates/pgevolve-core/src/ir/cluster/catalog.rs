//! `ClusterCatalog` — the cluster-wide IR root.
//!
//! Contains roles and tablespaces. Other cluster object kinds
//! (`cluster_settings`, `foreign_servers`, `user_mappings`, databases) land in
//! follow-up sub-specs.

use serde::{Deserialize, Serialize};

use crate::ir::IrError;
use crate::ir::cluster::role::Role;
use crate::ir::cluster::tablespace::Tablespace;

/// The root cluster IR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ClusterCatalog {
    /// Managed roles, sorted by `name` after `canonicalize()`.
    pub roles: Vec<Role>,
    /// Managed tablespaces, sorted by `name` after `canonicalize()`.
    pub tablespaces: Vec<Tablespace>,
}

impl ClusterCatalog {
    /// Empty cluster catalog (no roles, no tablespaces).
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            roles: Vec::new(),
            tablespaces: Vec::new(),
        }
    }

    /// Normalize the catalog: sorts roles by name, sorts each role's
    /// `member_of` lexicographically, sorts tablespaces by name, rejects
    /// duplicate tablespace names. Idempotent. See [`crate::ir::canon`] for
    /// the per-rule details.
    ///
    /// # Errors
    /// Returns [`IrError::DuplicateTablespace`] if two tablespaces share a
    /// name (after sorting).
    pub fn canonicalize(&mut self) -> Result<(), IrError> {
        crate::ir::canon::cluster::run(self)
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
        c.canonicalize().unwrap();
        c.canonicalize().unwrap();
        assert!(c.roles.is_empty());
    }

    #[test]
    fn canonicalize_sorts_roles_by_name() {
        let mut c = ClusterCatalog {
            roles: vec![role("zebra"), role("alpha"), role("middle")],
            tablespaces: vec![],
        };
        c.canonicalize().unwrap();
        let names: Vec<_> = c.roles.iter().map(|r| r.name.as_str().to_owned()).collect();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }
}
