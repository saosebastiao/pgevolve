//! `Schema` — a Postgres namespace.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::eq::DiffMacro;

/// A Postgres schema (namespace).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, DiffMacro)]
pub struct Schema {
    /// Schema name.
    pub name: Identifier,
    /// Optional comment.
    #[diff(via_debug)]
    pub comment: Option<String>,
    /// Object owner. `None` = unmanaged (the differ ignores ownership).
    /// `Some(role)` = managed: diff emits `ALTER SCHEMA ... OWNER TO role`.
    #[diff(via_debug)]
    pub owner: Option<Identifier>,
    /// Grants on this object. Empty = no grants. Canonicalized.
    #[diff(via_debug)]
    pub grants: Vec<crate::ir::grant::Grant>,
}

impl Schema {
    /// Construct a `Schema`.
    pub const fn new(name: Identifier) -> Self {
        Self {
            name,
            comment: None,
            owner: None,
            grants: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::eq::Diff;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn base() -> Schema {
        Schema::new(id("app"))
    }

    #[test]
    fn equal_schemas_have_no_diff() {
        assert!(base().canonical_eq(&base()));
    }

    #[test]
    fn different_names_diff() {
        let a = base();
        let b = Schema::new(id("billing"));
        let d = a.diff(&b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "name");
    }

    #[test]
    fn comment_diffs() {
        let mut b = base();
        b.comment = Some("v2".into());
        let d = base().diff(&b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "comment");
    }

    #[test]
    fn owner_change_diffs() {
        let mut b = base();
        b.owner = Some(id("new_owner"));
        assert!(base().diff(&b).iter().any(|x| x.path == "owner"));
    }

    #[test]
    fn grants_change_diffs() {
        let mut b = base();
        b.grants.push(crate::ir::grant::Grant {
            grantee: crate::ir::grant::GrantTarget::Public,
            privilege: crate::ir::grant::Privilege::Usage,
            with_grant_option: false,
            columns: None,
        });
        assert!(base().diff(&b).iter().any(|x| x.path == "grants"));
    }
}
