//! SQL rendering for object grants + ownership + default privileges.
//!
//! PG keywords are uppercase per the established sql.rs convention.
//! Identifiers go through `Identifier::render_sql` / `QualifiedName::render_sql`
//! / `OwnedObjectId::render_sql` (the last picks the right shape per object).

use crate::diff::owner_op::{OwnedObjectId, OwnerObjectKind};
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_privileges::DefaultPrivObjectType;
use crate::ir::grant::{Grant, GrantTarget};

/// `ALTER <objkind> <id>[<signature>] OWNER TO <new_owner>;`
#[must_use]
pub fn alter_object_owner(
    kind: OwnerObjectKind,
    id: &OwnedObjectId,
    signature: &str,
    new_owner: &Identifier,
) -> String {
    format!(
        "ALTER {} {}{} OWNER TO {};",
        kind.sql_keyword(),
        id.render_sql(),
        signature,
        new_owner.render_sql(),
    )
}

/// `GRANT priv ON <objkind> qname TO grantee [WITH GRANT OPTION];`
#[must_use]
pub fn grant_object_privilege(
    kind: OwnerObjectKind,
    qname: &QualifiedName,
    signature: &str,
    grant: &Grant,
) -> String {
    let grantee_sql = render_grantee(&grant.grantee);
    let wgo = if grant.with_grant_option {
        " WITH GRANT OPTION"
    } else {
        ""
    };
    if matches!(kind, OwnerObjectKind::Schema) {
        return format!(
            "GRANT {} ON SCHEMA {} TO {grantee_sql}{wgo};",
            grant.privilege.sql_keyword(),
            qname.name.render_sql(),
        );
    }
    let objkind_token = kind.sql_keyword();
    format!(
        "GRANT {} ON {objkind_token} {}{} TO {grantee_sql}{wgo};",
        grant.privilege.sql_keyword(),
        qname.render_sql(),
        signature,
    )
}

/// `REVOKE priv ON <objkind> qname FROM grantee;`
#[must_use]
pub fn revoke_object_privilege(
    kind: OwnerObjectKind,
    qname: &QualifiedName,
    signature: &str,
    grant: &Grant,
) -> String {
    let grantee_sql = render_grantee(&grant.grantee);
    if matches!(kind, OwnerObjectKind::Schema) {
        return format!(
            "REVOKE {} ON SCHEMA {} FROM {grantee_sql};",
            grant.privilege.sql_keyword(),
            qname.name.render_sql(),
        );
    }
    let objkind_token = kind.sql_keyword();
    format!(
        "REVOKE {} ON {objkind_token} {}{} FROM {grantee_sql};",
        grant.privilege.sql_keyword(),
        qname.render_sql(),
        signature,
    )
}

/// `GRANT priv (col, col) ON TABLE qname TO grantee [WITH GRANT OPTION];`
///
/// # Panics
///
/// Panics if `grant.columns` is `None`. Caller is responsible for routing
/// only column-level grants here.
#[must_use]
pub fn grant_column_privilege(qname: &QualifiedName, grant: &Grant) -> String {
    // SAFETY: caller dispatches by grant.columns.is_some() before invoking.
    let cols = grant
        .columns
        .as_ref()
        .expect("grant_column_privilege called with object-level grant — caller routing bug");
    let col_list: Vec<String> = cols.iter().map(Identifier::render_sql).collect();
    let grantee_sql = render_grantee(&grant.grantee);
    let wgo = if grant.with_grant_option {
        " WITH GRANT OPTION"
    } else {
        ""
    };
    format!(
        "GRANT {} ({}) ON TABLE {} TO {grantee_sql}{wgo};",
        grant.privilege.sql_keyword(),
        col_list.join(", "),
        qname.render_sql(),
    )
}

/// `REVOKE priv (col, col) ON TABLE qname FROM grantee;`
///
/// # Panics
///
/// Panics if `grant.columns` is `None`. Caller must route correctly.
#[must_use]
pub fn revoke_column_privilege(qname: &QualifiedName, grant: &Grant) -> String {
    // SAFETY: caller dispatches by grant.columns.is_some() before invoking.
    let cols = grant
        .columns
        .as_ref()
        .expect("revoke_column_privilege called with object-level grant — caller routing bug");
    let col_list: Vec<String> = cols.iter().map(Identifier::render_sql).collect();
    let grantee_sql = render_grantee(&grant.grantee);
    format!(
        "REVOKE {} ({}) ON TABLE {} FROM {grantee_sql};",
        grant.privilege.sql_keyword(),
        col_list.join(", "),
        qname.render_sql(),
    )
}

/// `ALTER DEFAULT PRIVILEGES FOR ROLE x [IN SCHEMA y] GRANT/REVOKE priv ON … TO/FROM z;`
#[must_use]
pub fn alter_default_privileges(
    target_role: &Identifier,
    schema: Option<&Identifier>,
    object_type: DefaultPrivObjectType,
    is_grant: bool,
    grant: &Grant,
) -> String {
    let mut sql = format!(
        "ALTER DEFAULT PRIVILEGES FOR ROLE {}",
        target_role.render_sql()
    );
    if let Some(sch) = schema {
        sql.push_str(&format!(" IN SCHEMA {}", sch.render_sql()));
    }
    let verb = if is_grant { "GRANT" } else { "REVOKE" };
    let direction = if is_grant { "TO" } else { "FROM" };
    let wgo = if is_grant && grant.with_grant_option {
        " WITH GRANT OPTION"
    } else {
        ""
    };
    sql.push_str(&format!(
        " {verb} {} ON {} {direction} {}{wgo};",
        grant.privilege.sql_keyword(),
        object_type.sql_keyword(),
        render_grantee(&grant.grantee),
    ));
    sql
}

fn render_grantee(g: &GrantTarget) -> String {
    match g {
        GrantTarget::Public => "PUBLIC".to_string(),
        GrantTarget::Role(id) => id.render_sql(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::grant::{Grant, GrantTarget, Privilege};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn grant_to(role: &str, priv_: Privilege) -> Grant {
        Grant {
            grantee: GrantTarget::Role(id(role)),
            privilege: priv_,
            with_grant_option: false,
            columns: None,
        }
    }

    // ---- alter_object_owner ----

    #[test]
    fn alter_owner_table() {
        let sql = alter_object_owner(
            OwnerObjectKind::Table,
            &OwnedObjectId::Qualified(qn("app", "users")),
            "",
            &id("alice"),
        );
        assert_eq!(sql, "ALTER TABLE app.users OWNER TO alice;");
    }

    #[test]
    fn alter_owner_schema_no_double_qualify() {
        let sql = alter_object_owner(
            OwnerObjectKind::Schema,
            &OwnedObjectId::Schema(id("app")),
            "",
            &id("alice"),
        );
        assert_eq!(sql, "ALTER SCHEMA app OWNER TO alice;");
    }

    #[test]
    fn alter_owner_publication_no_schema_qualifier() {
        let sql = alter_object_owner(
            OwnerObjectKind::Publication,
            &OwnedObjectId::Cluster(id("my_pub")),
            "",
            &id("alice"),
        );
        assert_eq!(sql, "ALTER PUBLICATION my_pub OWNER TO alice;");
    }

    #[test]
    fn alter_owner_function_with_signature() {
        let sql = alter_object_owner(
            OwnerObjectKind::Function,
            &OwnedObjectId::Qualified(qn("app", "do_thing")),
            "(integer, text)",
            &id("alice"),
        );
        assert_eq!(
            sql,
            "ALTER FUNCTION app.do_thing(integer, text) OWNER TO alice;"
        );
    }

    // ---- grant_object_privilege ----

    #[test]
    fn grant_object_select_table() {
        let g = grant_to("reader", Privilege::Select);
        let sql = grant_object_privilege(OwnerObjectKind::Table, &qn("app", "users"), "", &g);
        assert_eq!(sql, "GRANT SELECT ON TABLE app.users TO reader;");
    }

    #[test]
    fn grant_object_usage_schema_no_double_qualify() {
        let g = grant_to("reader", Privilege::Usage);
        let sql = grant_object_privilege(OwnerObjectKind::Schema, &qn("app", "app"), "", &g);
        assert_eq!(sql, "GRANT USAGE ON SCHEMA app TO reader;");
    }

    #[test]
    fn grant_with_grant_option() {
        let g = Grant {
            grantee: GrantTarget::Role(id("alice")),
            privilege: Privilege::Insert,
            with_grant_option: true,
            columns: None,
        };
        let sql = grant_object_privilege(OwnerObjectKind::Table, &qn("app", "orders"), "", &g);
        assert_eq!(
            sql,
            "GRANT INSERT ON TABLE app.orders TO alice WITH GRANT OPTION;"
        );
    }

    #[test]
    fn grant_to_public() {
        let g = Grant {
            grantee: GrantTarget::Public,
            privilege: Privilege::Select,
            with_grant_option: false,
            columns: None,
        };
        let sql = grant_object_privilege(OwnerObjectKind::Table, &qn("app", "users"), "", &g);
        assert_eq!(sql, "GRANT SELECT ON TABLE app.users TO PUBLIC;");
    }

    // ---- revoke_object_privilege ----

    #[test]
    fn revoke_object_table() {
        let g = grant_to("reader", Privilege::Select);
        let sql = revoke_object_privilege(OwnerObjectKind::Table, &qn("app", "users"), "", &g);
        assert_eq!(sql, "REVOKE SELECT ON TABLE app.users FROM reader;");
    }

    #[test]
    fn revoke_object_schema() {
        let g = grant_to("reader", Privilege::Usage);
        let sql = revoke_object_privilege(OwnerObjectKind::Schema, &qn("app", "app"), "", &g);
        assert_eq!(sql, "REVOKE USAGE ON SCHEMA app FROM reader;");
    }

    // ---- grant_column_privilege ----

    #[test]
    fn grant_column_privilege_renders() {
        let g = Grant {
            grantee: GrantTarget::Role(id("reader")),
            privilege: Privilege::Select,
            with_grant_option: false,
            columns: Some(vec![id("email")]),
        };
        let sql = grant_column_privilege(&qn("app", "users"), &g);
        assert_eq!(sql, "GRANT SELECT (email) ON TABLE app.users TO reader;");
    }

    #[test]
    fn grant_column_multiple_cols_comma_separated() {
        let g = Grant {
            grantee: GrantTarget::Role(id("reader")),
            privilege: Privilege::Select,
            with_grant_option: false,
            columns: Some(vec![id("email"), id("phone")]),
        };
        let sql = grant_column_privilege(&qn("app", "users"), &g);
        assert_eq!(
            sql,
            "GRANT SELECT (email, phone) ON TABLE app.users TO reader;"
        );
    }

    // ---- revoke_column_privilege ----

    #[test]
    fn revoke_column_privilege_renders() {
        let g = Grant {
            grantee: GrantTarget::Role(id("reader")),
            privilege: Privilege::Select,
            with_grant_option: false,
            columns: Some(vec![id("email")]),
        };
        let sql = revoke_column_privilege(&qn("app", "users"), &g);
        assert_eq!(sql, "REVOKE SELECT (email) ON TABLE app.users FROM reader;");
    }

    // ---- grant/revoke with function signature ----

    #[test]
    fn grant_execute_function_with_signature() {
        let g = Grant {
            grantee: GrantTarget::Role(id("app_user")),
            privilege: Privilege::Execute,
            with_grant_option: false,
            columns: None,
        };
        let sql = grant_object_privilege(
            OwnerObjectKind::Function,
            &qn("app", "foo"),
            "(integer, text)",
            &g,
        );
        assert_eq!(
            sql,
            "GRANT EXECUTE ON FUNCTION app.foo(integer, text) TO app_user;"
        );
    }

    #[test]
    fn revoke_execute_function_with_signature() {
        let g = Grant {
            grantee: GrantTarget::Role(id("app_user")),
            privilege: Privilege::Execute,
            with_grant_option: false,
            columns: None,
        };
        let sql = revoke_object_privilege(
            OwnerObjectKind::Function,
            &qn("app", "foo"),
            "(integer, text)",
            &g,
        );
        assert_eq!(
            sql,
            "REVOKE EXECUTE ON FUNCTION app.foo(integer, text) FROM app_user;"
        );
    }

    #[test]
    fn grant_execute_procedure_with_signature() {
        let g = Grant {
            grantee: GrantTarget::Role(id("app_user")),
            privilege: Privilege::Execute,
            with_grant_option: false,
            columns: None,
        };
        let sql = grant_object_privilege(
            OwnerObjectKind::Procedure,
            &qn("app", "do_work"),
            "(integer)",
            &g,
        );
        assert_eq!(
            sql,
            "GRANT EXECUTE ON PROCEDURE app.do_work(integer) TO app_user;"
        );
    }

    #[test]
    fn grant_execute_function_no_args() {
        let g = Grant {
            grantee: GrantTarget::Role(id("app_user")),
            privilege: Privilege::Execute,
            with_grant_option: false,
            columns: None,
        };
        let sql = grant_object_privilege(OwnerObjectKind::Function, &qn("app", "foo"), "()", &g);
        assert_eq!(sql, "GRANT EXECUTE ON FUNCTION app.foo() TO app_user;");
    }

    // ---- alter_default_privileges ----

    #[test]
    fn alter_default_privileges_grant_in_schema() {
        let g = grant_to("reader", Privilege::Select);
        let sql = alter_default_privileges(
            &id("app_owner"),
            Some(&id("app")),
            DefaultPrivObjectType::Tables,
            true,
            &g,
        );
        assert_eq!(
            sql,
            "ALTER DEFAULT PRIVILEGES FOR ROLE app_owner IN SCHEMA app GRANT SELECT ON TABLES TO reader;"
        );
    }

    #[test]
    fn alter_default_privileges_revoke_global() {
        let g = grant_to("reader", Privilege::Execute);
        let sql = alter_default_privileges(
            &id("app_owner"),
            None,
            DefaultPrivObjectType::Functions,
            false,
            &g,
        );
        assert_eq!(
            sql,
            "ALTER DEFAULT PRIVILEGES FOR ROLE app_owner REVOKE EXECUTE ON FUNCTIONS FROM reader;"
        );
    }
}
