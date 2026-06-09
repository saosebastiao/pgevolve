//! SQL rendering for object grants + ownership + default privileges.
//!
//! PG keywords are uppercase per the established sql.rs convention.
//! Identifiers go through `Identifier::render_sql` / `QualifiedName::render_sql`
//! / `GrantableObject::render_target` (the last picks the right name + optional
//! routine signature per object).

use crate::diff::change::GrantDirection;
use crate::diff::owner_op::GrantableObject;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_privileges::DefaultPrivObjectType;
use crate::ir::grant::{Grant, GrantTarget};

/// `ALTER <objkind> <target>[<signature>] OWNER TO <new_owner>;`
#[must_use]
pub fn alter_object_owner(object: &GrantableObject, new_owner: &Identifier) -> String {
    format!(
        "ALTER {} {} OWNER TO {};",
        object.sql_keyword(),
        object.render_target(),
        new_owner.render_sql(),
    )
}

/// `GRANT priv ON <objkind> <target> TO grantee [WITH GRANT OPTION];`
#[must_use]
pub fn grant_object_privilege(object: &GrantableObject, grant: &Grant) -> String {
    let grantee_sql = render_grantee(&grant.grantee);
    let wgo = if grant.with_grant_option {
        " WITH GRANT OPTION"
    } else {
        ""
    };
    format!(
        "GRANT {} ON {} {} TO {grantee_sql}{wgo};",
        grant.privilege.sql_keyword(),
        object.sql_keyword(),
        object.render_target(),
    )
}

/// `REVOKE priv ON <objkind> <target> FROM grantee;`
#[must_use]
pub fn revoke_object_privilege(object: &GrantableObject, grant: &Grant) -> String {
    let grantee_sql = render_grantee(&grant.grantee);
    format!(
        "REVOKE {} ON {} {} FROM {grantee_sql};",
        grant.privilege.sql_keyword(),
        object.sql_keyword(),
        object.render_target(),
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
    direction: GrantDirection,
    grant: &Grant,
) -> String {
    let mut sql = format!(
        "ALTER DEFAULT PRIVILEGES FOR ROLE {}",
        target_role.render_sql()
    );
    if let Some(sch) = schema {
        sql.push_str(&format!(" IN SCHEMA {}", sch.render_sql()));
    }
    let is_grant = matches!(direction, GrantDirection::Grant);
    let verb = if is_grant { "GRANT" } else { "REVOKE" };
    let preposition = if is_grant { "TO" } else { "FROM" };
    let wgo = if is_grant && grant.with_grant_option {
        " WITH GRANT OPTION"
    } else {
        ""
    };
    sql.push_str(&format!(
        " {verb} {} ON {} {preposition} {}{wgo};",
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
    use crate::diff::owner_op::RoutineSignature;
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
        let sql = alter_object_owner(&GrantableObject::Table(qn("app", "users")), &id("alice"));
        assert_eq!(sql, "ALTER TABLE app.users OWNER TO alice;");
    }

    #[test]
    fn alter_owner_schema_no_double_qualify() {
        let sql = alter_object_owner(&GrantableObject::Schema(id("app")), &id("alice"));
        assert_eq!(sql, "ALTER SCHEMA app OWNER TO alice;");
    }

    #[test]
    fn alter_owner_publication_no_schema_qualifier() {
        let sql = alter_object_owner(&GrantableObject::Publication(id("my_pub")), &id("alice"));
        assert_eq!(sql, "ALTER PUBLICATION my_pub OWNER TO alice;");
    }

    #[test]
    fn alter_owner_function_with_signature() {
        let sql = alter_object_owner(
            &GrantableObject::Function {
                name: qn("app", "do_thing"),
                signature: RoutineSignature("(integer, text)".to_string()),
            },
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
        let sql = grant_object_privilege(&GrantableObject::Table(qn("app", "users")), &g);
        assert_eq!(sql, "GRANT SELECT ON TABLE app.users TO reader;");
    }

    #[test]
    fn grant_object_usage_schema_no_double_qualify() {
        let g = grant_to("reader", Privilege::Usage);
        let sql = grant_object_privilege(&GrantableObject::Schema(id("app")), &g);
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
        let sql = grant_object_privilege(&GrantableObject::Table(qn("app", "orders")), &g);
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
        let sql = grant_object_privilege(&GrantableObject::Table(qn("app", "users")), &g);
        assert_eq!(sql, "GRANT SELECT ON TABLE app.users TO PUBLIC;");
    }

    // ---- revoke_object_privilege ----

    #[test]
    fn revoke_object_table() {
        let g = grant_to("reader", Privilege::Select);
        let sql = revoke_object_privilege(&GrantableObject::Table(qn("app", "users")), &g);
        assert_eq!(sql, "REVOKE SELECT ON TABLE app.users FROM reader;");
    }

    #[test]
    fn revoke_object_schema() {
        let g = grant_to("reader", Privilege::Usage);
        let sql = revoke_object_privilege(&GrantableObject::Schema(id("app")), &g);
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
            &GrantableObject::Function {
                name: qn("app", "foo"),
                signature: RoutineSignature("(integer, text)".to_string()),
            },
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
            &GrantableObject::Function {
                name: qn("app", "foo"),
                signature: RoutineSignature("(integer, text)".to_string()),
            },
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
            &GrantableObject::Procedure {
                name: qn("app", "do_work"),
                signature: RoutineSignature("(integer)".to_string()),
            },
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
        let sql = grant_object_privilege(
            &GrantableObject::Function {
                name: qn("app", "foo"),
                signature: RoutineSignature("()".to_string()),
            },
            &g,
        );
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
            GrantDirection::Grant,
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
            GrantDirection::Revoke,
            &g,
        );
        assert_eq!(
            sql,
            "ALTER DEFAULT PRIVILEGES FOR ROLE app_owner REVOKE EXECUTE ON FUNCTIONS FROM reader;"
        );
    }
}
