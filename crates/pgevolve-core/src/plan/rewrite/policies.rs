//! SQL rendering for policies + RLS toggles.

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::grant::GrantTarget;
use crate::ir::policy::Policy;

/// `CREATE POLICY name ON qname AS PERMISSIVE FOR ALL TO public USING (...) WITH CHECK (...);`
///
/// Always renders the explicit form for AS, FOR, TO — byte-stable round-trip,
/// unambiguous fixture diffs, clear human-readable output. USING and WITH CHECK
/// are omitted when absent on the policy.
#[must_use]
pub fn create_policy(table: &QualifiedName, p: &Policy) -> String {
    let mut sql = format!(
        "CREATE POLICY {} ON {} AS {} FOR {} TO {}",
        p.name.render_sql(),
        table.render_sql(),
        if p.permissive {
            "PERMISSIVE"
        } else {
            "RESTRICTIVE"
        },
        p.command.sql_keyword(),
        render_roles(&p.roles),
    );
    if let Some(u) = &p.using {
        sql.push_str(&format!(" USING ({})", u.canonical_text));
    }
    if let Some(c) = &p.with_check {
        sql.push_str(&format!(" WITH CHECK ({})", c.canonical_text));
    }
    sql.push(';');
    sql
}

/// `ALTER POLICY name ON qname TO ... USING (...) WITH CHECK (...);`
///
/// ALTER POLICY does NOT change the command kind (that's handled by
/// DROP + CREATE in the differ). It can change TO clause, USING, WITH CHECK.
#[must_use]
pub fn alter_policy(table: &QualifiedName, p: &Policy) -> String {
    let mut sql = format!(
        "ALTER POLICY {} ON {} TO {}",
        p.name.render_sql(),
        table.render_sql(),
        render_roles(&p.roles),
    );
    if let Some(u) = &p.using {
        sql.push_str(&format!(" USING ({})", u.canonical_text));
    }
    if let Some(c) = &p.with_check {
        sql.push_str(&format!(" WITH CHECK ({})", c.canonical_text));
    }
    sql.push(';');
    sql
}

/// `DROP POLICY name ON qname;`
#[must_use]
pub fn drop_policy(table: &QualifiedName, name: &Identifier) -> String {
    format!(
        "DROP POLICY {} ON {};",
        name.render_sql(),
        table.render_sql()
    )
}

/// `ALTER TABLE qname { ENABLE | DISABLE } ROW LEVEL SECURITY;`
#[must_use]
pub fn set_table_row_security(qname: &QualifiedName, enable: bool) -> String {
    let verb = if enable { "ENABLE" } else { "DISABLE" };
    format!(
        "ALTER TABLE {} {} ROW LEVEL SECURITY;",
        qname.render_sql(),
        verb
    )
}

/// `ALTER TABLE qname { FORCE | NO FORCE } ROW LEVEL SECURITY;`
#[must_use]
pub fn set_table_force_row_security(qname: &QualifiedName, force: bool) -> String {
    let verb = if force { "FORCE" } else { "NO FORCE" };
    format!(
        "ALTER TABLE {} {} ROW LEVEL SECURITY;",
        qname.render_sql(),
        verb
    )
}

fn render_roles(roles: &[GrantTarget]) -> String {
    let parts: Vec<String> = roles
        .iter()
        .map(|r| match r {
            GrantTarget::Public => "PUBLIC".to_string(),
            GrantTarget::Role(id) => id.render_sql(),
        })
        .collect();
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::default_expr::NormalizedExpr;
    use crate::ir::policy::PolicyCommand;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn simple_policy() -> Policy {
        Policy {
            name: id("p1"),
            permissive: true,
            command: PolicyCommand::All,
            roles: vec![GrantTarget::Public],
            using: Some(NormalizedExpr::from_canonical_text("true")),
            with_check: None,
        }
    }

    #[test]
    fn renders_create_policy() {
        let sql = create_policy(&qn("app", "docs"), &simple_policy());
        assert_eq!(
            sql,
            "CREATE POLICY p1 ON app.docs AS PERMISSIVE FOR ALL TO PUBLIC USING (true);"
        );
    }

    #[test]
    fn renders_restrictive_with_check() {
        let mut p = simple_policy();
        p.permissive = false;
        p.command = PolicyCommand::Insert;
        p.with_check = Some(NormalizedExpr::from_canonical_text("author = current_user"));
        p.using = None;
        let sql = create_policy(&qn("app", "docs"), &p);
        assert_eq!(
            sql,
            "CREATE POLICY p1 ON app.docs AS RESTRICTIVE FOR INSERT TO PUBLIC WITH CHECK (author = current_user);"
        );
    }

    #[test]
    fn renders_multi_role_to_clause() {
        let mut p = simple_policy();
        p.roles = vec![GrantTarget::Public, GrantTarget::Role(id("readers"))];
        let sql = create_policy(&qn("app", "docs"), &p);
        assert!(sql.contains("TO PUBLIC, readers"), "got: {sql}");
    }

    #[test]
    fn renders_alter_policy() {
        let sql = alter_policy(&qn("app", "docs"), &simple_policy());
        assert_eq!(sql, "ALTER POLICY p1 ON app.docs TO PUBLIC USING (true);");
    }

    #[test]
    fn renders_drop_policy() {
        let sql = drop_policy(&qn("app", "docs"), &id("p1"));
        assert_eq!(sql, "DROP POLICY p1 ON app.docs;");
    }

    #[test]
    fn renders_enable_disable_rls() {
        assert_eq!(
            set_table_row_security(&qn("app", "docs"), true),
            "ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;"
        );
        assert_eq!(
            set_table_row_security(&qn("app", "docs"), false),
            "ALTER TABLE app.docs DISABLE ROW LEVEL SECURITY;"
        );
    }

    #[test]
    fn renders_force_no_force_rls() {
        assert_eq!(
            set_table_force_row_security(&qn("app", "docs"), true),
            "ALTER TABLE app.docs FORCE ROW LEVEL SECURITY;"
        );
        assert_eq!(
            set_table_force_row_security(&qn("app", "docs"), false),
            "ALTER TABLE app.docs NO FORCE ROW LEVEL SECURITY;"
        );
    }
}
