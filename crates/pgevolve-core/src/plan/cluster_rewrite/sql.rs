//! SQL rendering for cluster ops. Mirrors `plan/rewrite/sql.rs` style.
//!
//! Postgres SQL keywords and role attribute keywords are uppercase;
//! identifier names rendered by [`Identifier::render_sql`] follow their own
//! quoting rules.

use std::fmt::Write as _;

use crate::identifier::Identifier;
use crate::ir::cluster::role::{Role, RoleAttributes};

/// `CREATE ROLE r WITH <options>;`
///
/// If `role.member_of` is non-empty an `IN ROLE x, y` clause is appended
/// before the trailing semicolon.
#[must_use]
pub fn create_role(role: &Role) -> String {
    let mut out = format!("CREATE ROLE {}", role.name.render_sql());
    write_with_options(&mut out, &role.attributes);
    if !role.member_of.is_empty() {
        out.push_str(" IN ROLE ");
        let names: Vec<String> = role.member_of.iter().map(Identifier::render_sql).collect();
        out.push_str(&names.join(", "));
    }
    out.push(';');
    out
}

/// `DROP ROLE r;`
#[must_use]
pub fn drop_role(name: &Identifier) -> String {
    format!("DROP ROLE {};", name.render_sql())
}

/// `ALTER ROLE r WITH <only changed options>;`
///
/// Only attributes that differ between `from` and `to` are emitted, keeping
/// the generated SQL minimal and auditable.
#[must_use]
pub fn alter_role_attributes(
    name: &Identifier,
    from: &RoleAttributes,
    to: &RoleAttributes,
) -> String {
    let mut out = format!("ALTER ROLE {} WITH", name.render_sql());
    macro_rules! emit_bool {
        ($field:ident, $on:literal, $off:literal) => {
            if from.$field != to.$field {
                out.push(' ');
                out.push_str(if to.$field { $on } else { $off });
            }
        };
    }
    emit_bool!(superuser, "SUPERUSER", "NOSUPERUSER");
    emit_bool!(createdb, "CREATEDB", "NOCREATEDB");
    emit_bool!(createrole, "CREATEROLE", "NOCREATEROLE");
    emit_bool!(inherit, "INHERIT", "NOINHERIT");
    emit_bool!(login, "LOGIN", "NOLOGIN");
    emit_bool!(replication, "REPLICATION", "NOREPLICATION");
    emit_bool!(bypass_rls, "BYPASSRLS", "NOBYPASSRLS");
    if from.connection_limit != to.connection_limit {
        let n = to.connection_limit.unwrap_or(-1);
        // Writing to a String never fails; the Result is discarded.
        let _ = write!(out, " CONNECTION LIMIT {n}");
    }
    if from.valid_until != to.valid_until {
        match &to.valid_until {
            // Writing to a String never fails; the Result is discarded.
            Some(ts) => {
                let _ = write!(out, " VALID UNTIL '{ts}'");
            }
            None => out.push_str(" VALID UNTIL 'infinity'"),
        }
    }
    out.push(';');
    out
}

/// `GRANT role TO member;`
#[must_use]
pub fn grant_role_membership(role: &Identifier, member: &Identifier) -> String {
    format!("GRANT {} TO {};", role.render_sql(), member.render_sql())
}

/// `REVOKE role FROM member;`
#[must_use]
pub fn revoke_role_membership(role: &Identifier, member: &Identifier) -> String {
    format!("REVOKE {} FROM {};", role.render_sql(), member.render_sql())
}

/// `COMMENT ON ROLE r IS '...';` or `IS NULL` to clear.
///
/// Single quotes inside `text` are escaped by doubling (`''`), per the SQL
/// standard. `None` emits `IS NULL` which clears any existing comment.
#[must_use]
pub fn comment_on_role(name: &Identifier, comment: Option<&str>) -> String {
    comment.map_or_else(
        || format!("COMMENT ON ROLE {} IS NULL;", name.render_sql()),
        |text| {
            format!(
                "COMMENT ON ROLE {} IS '{}';",
                name.render_sql(),
                text.replace('\'', "''")
            )
        },
    )
}

// ---------------------------------------------------------------------------
// Internal helper
// ---------------------------------------------------------------------------

/// Append ` WITH <all option flags>` to `out`, emitting every attribute
/// unconditionally. Used by [`create_role`] which always writes the full set.
fn write_with_options(out: &mut String, attrs: &RoleAttributes) {
    out.push_str(" WITH");
    out.push_str(if attrs.superuser {
        " SUPERUSER"
    } else {
        " NOSUPERUSER"
    });
    out.push_str(if attrs.createdb {
        " CREATEDB"
    } else {
        " NOCREATEDB"
    });
    out.push_str(if attrs.createrole {
        " CREATEROLE"
    } else {
        " NOCREATEROLE"
    });
    out.push_str(if attrs.inherit {
        " INHERIT"
    } else {
        " NOINHERIT"
    });
    out.push_str(if attrs.login { " LOGIN" } else { " NOLOGIN" });
    out.push_str(if attrs.replication {
        " REPLICATION"
    } else {
        " NOREPLICATION"
    });
    out.push_str(if attrs.bypass_rls {
        " BYPASSRLS"
    } else {
        " NOBYPASSRLS"
    });
    if let Some(n) = attrs.connection_limit {
        // Writing to a String never fails; the Result is discarded.
        let _ = write!(out, " CONNECTION LIMIT {n}");
    }
    if let Some(ts) = &attrs.valid_until {
        // Writing to a String never fails; the Result is discarded.
        let _ = write!(out, " VALID UNTIL '{ts}'");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::cluster::role::Role;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn default_role(name: &str) -> Role {
        Role {
            name: id(name),
            attributes: RoleAttributes::default(),
            member_of: vec![],
            comment: None,
        }
    }

    #[test]
    fn create_role_default_attributes() {
        let role = default_role("app_user");
        let sql = create_role(&role);
        // PG default: INHERIT true, all others false.
        assert!(sql.starts_with("CREATE ROLE app_user WITH"), "got: {sql}");
        assert!(sql.contains("NOSUPERUSER"), "got: {sql}");
        assert!(sql.contains("NOCREATEDB"), "got: {sql}");
        assert!(sql.contains("NOCREATEROLE"), "got: {sql}");
        assert!(sql.contains("INHERIT"), "got: {sql}");
        assert!(!sql.contains("NOINHERIT"), "got: {sql}");
        assert!(sql.contains("NOLOGIN"), "got: {sql}");
        assert!(sql.contains("NOREPLICATION"), "got: {sql}");
        assert!(sql.contains("NOBYPASSRLS"), "got: {sql}");
        assert!(sql.ends_with(';'), "got: {sql}");
    }

    #[test]
    fn create_role_with_membership() {
        let mut role = default_role("app_user");
        role.member_of = vec![id("readers"), id("writers")];
        let sql = create_role(&role);
        assert!(sql.contains("IN ROLE readers, writers"), "got: {sql}");
    }

    #[test]
    fn alter_role_only_emits_changed_attrs() {
        let from = RoleAttributes::default();
        let to = RoleAttributes {
            login: true,
            createdb: true,
            ..RoleAttributes::default()
        };
        let sql = alter_role_attributes(&id("app_user"), &from, &to);
        // Only LOGIN and CREATEDB changed — no other tokens expected.
        assert!(sql.contains("LOGIN"), "got: {sql}");
        assert!(sql.contains("CREATEDB"), "got: {sql}");
        assert!(!sql.contains("SUPERUSER"), "got: {sql}");
        assert!(!sql.contains("INHERIT"), "got: {sql}");
        assert!(sql.starts_with("ALTER ROLE app_user WITH"), "got: {sql}");
    }

    #[test]
    fn grant_revoke_membership() {
        let grant = grant_role_membership(&id("readers"), &id("bob"));
        assert_eq!(grant, "GRANT readers TO bob;");

        let revoke = revoke_role_membership(&id("readers"), &id("bob"));
        assert_eq!(revoke, "REVOKE readers FROM bob;");
    }

    #[test]
    fn comment_quotes_apostrophes() {
        let sql = comment_on_role(&id("app_user"), Some("it's fine"));
        assert!(sql.contains("it''s fine"), "got: {sql}");
    }

    #[test]
    fn drop_role_renders() {
        let sql = drop_role(&id("old_role"));
        assert_eq!(sql, "DROP ROLE old_role;");
    }

    #[test]
    fn alter_role_connection_limit_back_to_unlimited() {
        let from = RoleAttributes {
            connection_limit: Some(10),
            ..RoleAttributes::default()
        };
        let to = RoleAttributes::default(); // connection_limit = None
        let sql = alter_role_attributes(&id("app_user"), &from, &to);
        assert!(sql.contains("CONNECTION LIMIT -1"), "got: {sql}");
    }

    #[test]
    fn alter_role_valid_until_cleared() {
        let from = RoleAttributes {
            valid_until: Some("2030-01-01T00:00:00Z".into()),
            ..RoleAttributes::default()
        };
        let to = RoleAttributes::default(); // valid_until = None
        let sql = alter_role_attributes(&id("app_user"), &from, &to);
        assert!(sql.contains("VALID UNTIL 'infinity'"), "got: {sql}");
    }

    #[test]
    fn comment_on_role_none_emits_is_null() {
        let sql = comment_on_role(&id("app_user"), None);
        assert_eq!(sql, "COMMENT ON ROLE app_user IS NULL;");
    }

    #[test]
    fn alter_role_connection_limit_set() {
        let from = RoleAttributes::default();
        let to = RoleAttributes {
            connection_limit: Some(50),
            ..RoleAttributes::default()
        };
        let sql = alter_role_attributes(&id("app_user"), &from, &to);
        assert!(sql.contains("CONNECTION LIMIT 50"), "got: {sql}");
    }
}
