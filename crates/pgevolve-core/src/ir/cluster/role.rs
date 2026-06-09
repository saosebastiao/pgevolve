//! `Role` and `RoleAttributes` — Postgres `pg_authid` row, normalized.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, diff_field, prefix_diffs};

/// A managed Postgres role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    /// Role name.
    pub name: Identifier,
    /// Boolean + numeric attributes from `pg_authid`.
    pub attributes: RoleAttributes,
    /// Roles this role is a member of (the `IN ROLE x` direction).
    /// Canonicalized to lexicographic order in [`crate::ir::canon`].
    pub member_of: Vec<Identifier>,
    /// Optional comment from `pg_shdescription`.
    pub comment: Option<String>,
}

impl Diff for Role {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let Self {
            name: _,
            attributes: _,
            member_of: _,
            comment: _,
        } = self;
        let mut out = Vec::new();
        out.extend(diff_field("name", &self.name, &other.name));
        out.extend(prefix_diffs(
            "attributes",
            Diff::diff(&self.attributes, &other.attributes),
        ));
        out.extend(diff_field(
            "member_of",
            &format!("{:?}", self.member_of),
            &format!("{:?}", other.member_of),
        ));
        out.extend(diff_field(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out
    }
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

impl Diff for RoleAttributes {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let Self {
            superuser: _,
            createdb: _,
            createrole: _,
            inherit: _,
            login: _,
            replication: _,
            bypass_rls: _,
            connection_limit: _,
            valid_until: _,
        } = self;
        let mut out = Vec::new();
        out.extend(diff_field("superuser", &self.superuser, &other.superuser));
        out.extend(diff_field("createdb", &self.createdb, &other.createdb));
        out.extend(diff_field(
            "createrole",
            &self.createrole,
            &other.createrole,
        ));
        out.extend(diff_field("inherit", &self.inherit, &other.inherit));
        out.extend(diff_field("login", &self.login, &other.login));
        out.extend(diff_field(
            "replication",
            &self.replication,
            &other.replication,
        ));
        out.extend(diff_field(
            "bypass_rls",
            &self.bypass_rls,
            &other.bypass_rls,
        ));
        out.extend(diff_field(
            "connection_limit",
            &format!("{:?}", self.connection_limit),
            &format!("{:?}", other.connection_limit),
        ));
        out.extend(diff_field(
            "valid_until",
            &format!("{:?}", self.valid_until),
            &format!("{:?}", other.valid_until),
        ));
        out
    }
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
        // Per-field path: "attributes.login", not the coarse "attributes".
        assert!(base().diff(&b).iter().any(|x| x.path == "attributes.login"));
    }

    #[test]
    fn connection_limit_change_diffs() {
        let mut b = base();
        b.attributes.connection_limit = Some(10);
        assert!(
            base()
                .diff(&b)
                .iter()
                .any(|x| x.path == "attributes.connection_limit")
        );
    }

    #[test]
    fn valid_until_change_diffs() {
        let mut b = base();
        b.attributes.valid_until = Some("2030-01-01T00:00:00Z".into());
        assert!(
            base()
                .diff(&b)
                .iter()
                .any(|x| x.path == "attributes.valid_until")
        );
    }

    #[test]
    fn attributes_diff_does_not_emit_coarse_path() {
        let mut b = base();
        b.attributes.superuser = true;
        // The diff must NOT produce the coarse "attributes" path; it should be
        // "attributes.superuser" so callers can introspect individual flags.
        assert!(
            !base().diff(&b).iter().any(|x| x.path == "attributes"),
            "coarse 'attributes' path must not appear; use per-field paths"
        );
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
