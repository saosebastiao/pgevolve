//! Object permissions — `Grant`, `Privilege`, `GrantTarget`.
//!
//! One [`Grant`] = one ACL entry on a grantable object. Shared by every
//! object kind that gains a `grants: Vec<Grant>` field in v0.3.1.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;

/// One ACL entry on a grantable object.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Grant {
    /// Who receives the privilege.
    pub grantee: GrantTarget,
    /// Which privilege.
    pub privilege: Privilege,
    /// `WITH GRANT OPTION` flag. Defaults to false.
    #[serde(default)]
    pub with_grant_option: bool,
    /// Column-level grants. `None` = object-level. `Some(cols)` = only those
    /// columns. Only valid for `Table`/`View`/`MaterializedView`; canon
    /// rejects `Some(_)` on other object kinds.
    #[serde(default)]
    pub columns: Option<Vec<Identifier>>,
}

/// Who a grant targets.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantTarget {
    /// `GRANT ... TO PUBLIC` — sorts before any named role for canon stability.
    Public,
    /// `GRANT ... TO <rolename>`.
    Role(Identifier),
}

/// The full set of privilege keywords pgevolve manages.
///
/// Database-level (`CONNECT`, `TEMPORARY`) and cluster-level (`SET`,
/// `ALTER SYSTEM`) privileges are intentionally absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Privilege {
    /// `SELECT` — read rows.
    Select,
    /// `INSERT` — add rows.
    Insert,
    /// `UPDATE` — modify rows.
    Update,
    /// `DELETE` — remove rows.
    Delete,
    /// `TRUNCATE` — empty a table.
    Truncate,
    /// `REFERENCES` — create foreign-key constraints referencing the table.
    References,
    /// `TRIGGER` — create triggers on the table.
    Trigger,
    /// `USAGE` — use a schema, sequence, type, or language.
    Usage,
    /// `EXECUTE` — call a function or procedure.
    Execute,
    /// `CREATE` — create objects within a schema.
    Create,
}

impl Privilege {
    /// PG single-letter ACL code (the form used in `aclitem` text).
    #[must_use]
    pub const fn acl_letter(self) -> char {
        match self {
            Self::Select => 'r',
            Self::Update => 'w',
            Self::Insert => 'a',
            Self::Delete => 'd',
            Self::Truncate => 'D',
            Self::References => 'x',
            Self::Trigger => 't',
            Self::Execute => 'X',
            Self::Usage => 'U',
            Self::Create => 'C',
        }
    }

    /// SQL keyword used in GRANT/REVOKE rendering. Always uppercase per the
    /// `sql.rs` casing convention.
    #[must_use]
    pub const fn sql_keyword(self) -> &'static str {
        match self {
            Self::Select => "SELECT",
            Self::Insert => "INSERT",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
            Self::Truncate => "TRUNCATE",
            Self::References => "REFERENCES",
            Self::Trigger => "TRIGGER",
            Self::Usage => "USAGE",
            Self::Execute => "EXECUTE",
            Self::Create => "CREATE",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn public_sorts_before_role() {
        let public = GrantTarget::Public;
        let role = GrantTarget::Role(id("foo"));
        assert!(
            public < role,
            "Public should sort first for canon stability"
        );
    }

    #[test]
    fn role_targets_sort_lexicographically() {
        let a = GrantTarget::Role(id("alice"));
        let b = GrantTarget::Role(id("bob"));
        assert!(a < b);
    }

    #[test]
    fn acl_letters_match_pg() {
        assert_eq!(Privilege::Select.acl_letter(), 'r');
        assert_eq!(Privilege::Insert.acl_letter(), 'a');
        assert_eq!(Privilege::Update.acl_letter(), 'w');
        assert_eq!(Privilege::Delete.acl_letter(), 'd');
        assert_eq!(Privilege::Truncate.acl_letter(), 'D');
        assert_eq!(Privilege::References.acl_letter(), 'x');
        assert_eq!(Privilege::Trigger.acl_letter(), 't');
        assert_eq!(Privilege::Execute.acl_letter(), 'X');
        assert_eq!(Privilege::Usage.acl_letter(), 'U');
        assert_eq!(Privilege::Create.acl_letter(), 'C');
    }

    #[test]
    fn grants_sort_by_grantee_then_privilege() {
        let g1 = Grant {
            grantee: GrantTarget::Role(id("alice")),
            privilege: Privilege::Update,
            with_grant_option: false,
            columns: None,
        };
        let g2 = Grant {
            grantee: GrantTarget::Role(id("alice")),
            privilege: Privilege::Select,
            with_grant_option: false,
            columns: None,
        };
        let g3 = Grant {
            grantee: GrantTarget::Public,
            privilege: Privilege::Select,
            with_grant_option: false,
            columns: None,
        };
        let mut grants = vec![g1.clone(), g2.clone(), g3.clone()];
        grants.sort();
        assert_eq!(grants, vec![g3, g2, g1]); // Public, then alice/Select, then alice/Update
    }
}
