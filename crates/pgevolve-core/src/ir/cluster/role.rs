//! `Role` and `RoleAttributes` — Postgres `pg_authid` row, normalized.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::eq::DiffMacro;

/// A managed Postgres role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct Role {
    /// Role name.
    pub name: Identifier,
    /// Boolean + numeric attributes from `pg_authid`.
    #[diff(via_debug)]
    pub attributes: RoleAttributes,
    /// Roles this role is a member of (the `IN ROLE x` direction).
    /// Canonicalized to lexicographic order in [`crate::ir::canon`].
    #[diff(via_debug)]
    pub member_of: Vec<Identifier>,
    /// Optional comment from `pg_shdescription`.
    #[diff(via_debug)]
    pub comment: Option<String>,
}

/// `pg_authid` attribute matrix. Passwords intentionally absent (set out-of-band).
// Each bool maps 1:1 to a `pg_authid` column (SUPERUSER, CREATEDB, …). Replacing
// them with two-variant enums would obscure the direct PG mapping without adding
// type-safety benefit — the columns are genuinely independent boolean flags.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleAttributes {
    /// `SUPERUSER` / `NOSUPERUSER`. Default false.
    pub superuser: bool,
    /// `CREATEDB` / `NOCREATEDB`. Default false.
    pub createdb: bool,
    /// `CREATEROLE` / `NOCREATEROLE`. Default false.
    pub createrole: bool,
    /// `INHERIT` / `NOINHERIT`. Default true (matches PG default).
    pub inherit: bool,
    /// `LOGIN` / `NOLOGIN`. Default false. `CREATE USER` sugar sets this true.
    pub login: bool,
    /// `REPLICATION` / `NOREPLICATION`. Default false.
    pub replication: bool,
    /// `BYPASSRLS` / `NOBYPASSRLS`. Default false.
    pub bypass_rls: bool,
    /// `CONNECTION LIMIT n`. `None` means unlimited (PG `-1`).
    pub connection_limit: Option<i64>,
    /// `VALID UNTIL 'ts'`. RFC 3339 string; opaque to differ.
    pub valid_until: Option<String>,
}

impl Default for RoleAttributes {
    fn default() -> Self {
        Self {
            superuser: false,
            createdb: false,
            createrole: false,
            inherit: true, // PG default
            login: false,
            replication: false,
            bypass_rls: false,
            connection_limit: None,
            valid_until: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::eq::Diff;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn base() -> Role {
        Role {
            name: id("app_user"),
            attributes: RoleAttributes::default(),
            member_of: vec![],
            comment: None,
        }
    }

    #[test]
    fn equal_roles_have_no_diff() {
        assert!(base().canonical_eq(&base()));
    }

    #[test]
    fn login_change_diffs() {
        let mut b = base();
        b.attributes.login = true;
        assert!(base().diff(&b).iter().any(|x| x.path == "attributes"));
    }

    #[test]
    fn membership_change_diffs() {
        let mut b = base();
        b.member_of.push(id("readers"));
        assert!(base().diff(&b).iter().any(|x| x.path == "member_of"));
    }

    #[test]
    fn comment_change_diffs() {
        let mut b = base();
        b.comment = Some("the app".into());
        assert!(base().diff(&b).iter().any(|x| x.path == "comment"));
    }

    #[test]
    fn default_attributes_match_postgres_defaults() {
        let a = RoleAttributes::default();
        assert!(a.inherit, "PG default for INHERIT is true");
        assert!(!a.superuser);
        assert!(!a.login);
        assert_eq!(a.connection_limit, None);
    }
}
