//! SQL emission for user-defined type planner steps.
//!
//! Each `emit_*` function produces a single canonical SQL statement string
//! (always ending with `;`) suitable for direct embedding in `plan.sql`.
//! Output is deterministic: same input IR always produces the same bytes.

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column_type::ColumnType;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::user_type::{CompositeAttribute, DomainCheck, UserType, UserTypeKind};

// ---------------------------------------------------------------------------
// CREATE TYPE
// ---------------------------------------------------------------------------

/// `CREATE TYPE qname AS ENUM (…)` / `CREATE DOMAIN …` / `CREATE TYPE … AS (…)`
pub(crate) fn emit_create_type(ut: &UserType) -> String {
    match &ut.kind {
        UserTypeKind::Enum { values } => emit_create_enum(&ut.qname, values),
        UserTypeKind::Domain {
            base,
            nullable,
            default,
            check_constraints,
            collation,
        } => emit_create_domain(
            &ut.qname,
            base,
            *nullable,
            default.as_ref(),
            check_constraints,
            collation.as_ref(),
        ),
        UserTypeKind::Composite { attributes } => emit_create_composite(&ut.qname, attributes),
    }
}

fn emit_create_enum(qname: &QualifiedName, values: &[crate::ir::user_type::EnumValue]) -> String {
    let mut sql = format!("CREATE TYPE {} AS ENUM (", qname.render_sql());
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        sql.push('\'');
        sql.push_str(&v.name.replace('\'', "''"));
        sql.push('\'');
    }
    sql.push_str(");");
    sql
}

fn emit_create_domain(
    qname: &QualifiedName,
    base: &ColumnType,
    nullable: bool,
    default: Option<&NormalizedExpr>,
    check_constraints: &[DomainCheck],
    collation: Option<&QualifiedName>,
) -> String {
    let mut sql = format!(
        "CREATE DOMAIN {} AS {}",
        qname.render_sql(),
        base.render_sql()
    );
    if let Some(collation) = collation {
        sql.push_str(" COLLATE ");
        sql.push_str(&collation.render_sql());
    }
    if let Some(default) = default {
        sql.push_str(" DEFAULT ");
        sql.push_str(&default.canonical_text);
    }
    if !nullable {
        sql.push_str(" NOT NULL");
    }
    for check in check_constraints {
        sql.push_str(" CONSTRAINT ");
        sql.push_str(&check.name.render_sql());
        sql.push_str(" CHECK (");
        sql.push_str(&check.expression.canonical_text);
        sql.push(')');
    }
    sql.push(';');
    sql
}

fn emit_create_composite(qname: &QualifiedName, attributes: &[CompositeAttribute]) -> String {
    let mut sql = format!("CREATE TYPE {} AS (\n", qname.render_sql());
    for (i, attr) in attributes.iter().enumerate() {
        if i > 0 {
            sql.push_str(",\n");
        }
        sql.push_str("    ");
        sql.push_str(&attr.name.render_sql());
        sql.push(' ');
        sql.push_str(&attr.ty.render_sql());
        if let Some(collation) = &attr.collation {
            sql.push_str(" COLLATE ");
            sql.push_str(&collation.render_sql());
        }
    }
    sql.push_str("\n);");
    sql
}

// ---------------------------------------------------------------------------
// DROP TYPE
// ---------------------------------------------------------------------------

/// `DROP TYPE qname;`
pub(crate) fn emit_drop_type(qname: &QualifiedName) -> String {
    format!("DROP TYPE {};", qname.render_sql())
}

/// `DROP TYPE qname CASCADE;`
pub(crate) fn emit_drop_type_cascade(qname: &QualifiedName) -> String {
    format!("DROP TYPE {} CASCADE;", qname.render_sql())
}

// ---------------------------------------------------------------------------
// ALTER TYPE … (enum)
// ---------------------------------------------------------------------------

/// `ALTER TYPE qname ADD VALUE 'value' [BEFORE|AFTER 'ref'];`
pub(crate) fn emit_alter_type_add_value(
    qname: &QualifiedName,
    value: &str,
    before: Option<&str>,
    after: Option<&str>,
) -> String {
    let mut sql = format!(
        "ALTER TYPE {} ADD VALUE '{}'",
        qname.render_sql(),
        value.replace('\'', "''")
    );
    if let Some(b) = before {
        sql.push_str(" BEFORE '");
        sql.push_str(&b.replace('\'', "''"));
        sql.push('\'');
    } else if let Some(a) = after {
        sql.push_str(" AFTER '");
        sql.push_str(&a.replace('\'', "''"));
        sql.push('\'');
    }
    sql.push(';');
    sql
}

/// `ALTER TYPE qname RENAME VALUE 'from' TO 'to';`
pub(crate) fn emit_alter_type_rename_value(qname: &QualifiedName, from: &str, to: &str) -> String {
    format!(
        "ALTER TYPE {} RENAME VALUE '{}' TO '{}';",
        qname.render_sql(),
        from.replace('\'', "''"),
        to.replace('\'', "''"),
    )
}

// ---------------------------------------------------------------------------
// ALTER DOMAIN … (domain constraints / default / not null)
// ---------------------------------------------------------------------------

/// `ALTER DOMAIN qname ADD CONSTRAINT name CHECK (expr);`
pub(crate) fn emit_alter_domain_add_check(qname: &QualifiedName, c: &DomainCheck) -> String {
    format!(
        "ALTER DOMAIN {} ADD CONSTRAINT {} CHECK ({});",
        qname.render_sql(),
        c.name.render_sql(),
        c.expression.canonical_text,
    )
}

/// `ALTER DOMAIN qname DROP CONSTRAINT name;`
pub(crate) fn emit_alter_domain_drop_check(qname: &QualifiedName, name: &Identifier) -> String {
    format!(
        "ALTER DOMAIN {} DROP CONSTRAINT {};",
        qname.render_sql(),
        name.render_sql(),
    )
}

/// `ALTER DOMAIN qname SET DEFAULT expr;` or `ALTER DOMAIN qname DROP DEFAULT;`
pub(crate) fn emit_alter_domain_set_default(
    qname: &QualifiedName,
    default: Option<&NormalizedExpr>,
) -> String {
    match default {
        Some(expr) => format!(
            "ALTER DOMAIN {} SET DEFAULT {};",
            qname.render_sql(),
            expr.canonical_text,
        ),
        None => format!("ALTER DOMAIN {} DROP DEFAULT;", qname.render_sql()),
    }
}

/// `ALTER DOMAIN qname SET NOT NULL;` or `ALTER DOMAIN qname DROP NOT NULL;`
pub(crate) fn emit_alter_domain_set_not_null(qname: &QualifiedName, not_null: bool) -> String {
    if not_null {
        format!("ALTER DOMAIN {} SET NOT NULL;", qname.render_sql())
    } else {
        format!("ALTER DOMAIN {} DROP NOT NULL;", qname.render_sql())
    }
}

// ---------------------------------------------------------------------------
// ALTER TYPE … (composite attributes)
// ---------------------------------------------------------------------------

/// `ALTER TYPE qname ADD ATTRIBUTE name type [COLLATE collation];`
pub(crate) fn emit_alter_type_add_attribute(
    qname: &QualifiedName,
    attr: &CompositeAttribute,
) -> String {
    let mut sql = format!(
        "ALTER TYPE {} ADD ATTRIBUTE {} {}",
        qname.render_sql(),
        attr.name.render_sql(),
        attr.ty.render_sql(),
    );
    if let Some(collation) = &attr.collation {
        sql.push_str(" COLLATE ");
        sql.push_str(&collation.render_sql());
    }
    sql.push(';');
    sql
}

/// `ALTER TYPE qname DROP ATTRIBUTE name;`
pub(crate) fn emit_alter_type_drop_attribute(qname: &QualifiedName, name: &Identifier) -> String {
    format!(
        "ALTER TYPE {} DROP ATTRIBUTE {};",
        qname.render_sql(),
        name.render_sql(),
    )
}

/// `ALTER TYPE qname ALTER ATTRIBUTE attribute TYPE new_type;`
pub(crate) fn emit_alter_type_alter_attribute_type(
    qname: &QualifiedName,
    attribute: &Identifier,
    new_type: &ColumnType,
) -> String {
    format!(
        "ALTER TYPE {} ALTER ATTRIBUTE {} TYPE {};",
        qname.render_sql(),
        attribute.render_sql(),
        new_type.render_sql(),
    )
}

// ---------------------------------------------------------------------------
// COMMENT ON TYPE / DOMAIN
// ---------------------------------------------------------------------------

/// `COMMENT ON TYPE|DOMAIN qname IS '...'|NULL;`
pub(crate) fn emit_comment_on_type(
    qname: &QualifiedName,
    kind: &UserTypeKind,
    comment: Option<&str>,
) -> String {
    let keyword = match kind {
        UserTypeKind::Domain { .. } => "DOMAIN",
        _ => "TYPE",
    };
    match comment {
        Some(c) => format!(
            "COMMENT ON {keyword} {} IS '{}';",
            qname.render_sql(),
            c.replace('\'', "''"),
        ),
        None => format!("COMMENT ON {keyword} {} IS NULL;", qname.render_sql()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::user_type::EnumValue;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn simple_enum() -> UserType {
        UserType {
            qname: qn("app", "order_status"),
            kind: UserTypeKind::Enum {
                values: vec![
                    EnumValue {
                        name: "pending".into(),
                        sort_order: 1.0,
                    },
                    EnumValue {
                        name: "shipped".into(),
                        sort_order: 2.0,
                    },
                ],
            },
            comment: None,
        }
    }

    fn simple_domain() -> UserType {
        UserType {
            qname: qn("app", "positive_int"),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: None,
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
        }
    }

    fn simple_composite() -> UserType {
        UserType {
            qname: qn("app", "address"),
            kind: UserTypeKind::Composite {
                attributes: vec![
                    CompositeAttribute {
                        name: id("street"),
                        ty: ColumnType::Text,
                        collation: None,
                    },
                    CompositeAttribute {
                        name: id("zip"),
                        ty: ColumnType::Text,
                        collation: None,
                    },
                ],
            },
            comment: None,
        }
    }

    // --- emit_create_type (enum) ---

    #[test]
    fn create_enum_basic() {
        let ut = simple_enum();
        let sql = emit_create_type(&ut);
        assert_eq!(
            sql,
            "CREATE TYPE app.order_status AS ENUM ('pending', 'shipped');"
        );
    }

    #[test]
    fn create_enum_escapes_single_quotes() {
        let ut = UserType {
            qname: qn("app", "my_enum"),
            kind: UserTypeKind::Enum {
                values: vec![EnumValue {
                    name: "it's".into(),
                    sort_order: 1.0,
                }],
            },
            comment: None,
        };
        let sql = emit_create_type(&ut);
        assert!(sql.contains("'it''s'"), "got: {sql}");
    }

    #[test]
    fn create_enum_empty_values() {
        let ut = UserType {
            qname: qn("app", "empty_enum"),
            kind: UserTypeKind::Enum { values: vec![] },
            comment: None,
        };
        let sql = emit_create_type(&ut);
        assert_eq!(sql, "CREATE TYPE app.empty_enum AS ENUM ();");
    }

    // --- emit_create_type (domain) ---

    #[test]
    fn create_domain_basic() {
        let ut = simple_domain();
        let sql = emit_create_type(&ut);
        assert_eq!(sql, "CREATE DOMAIN app.positive_int AS integer;");
    }

    #[test]
    fn create_domain_with_default_and_check() {
        let check = DomainCheck {
            name: id("positive_check"),
            expression: NormalizedExpr::from_text("VALUE > 0"),
        };
        let ut = UserType {
            qname: qn("app", "positive_int"),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: Some(NormalizedExpr::from_text("1")),
                check_constraints: vec![check],
                collation: None,
            },
            comment: None,
        };
        let sql = emit_create_type(&ut);
        assert!(sql.contains("DEFAULT 1"), "got: {sql}");
        assert!(
            sql.contains("CONSTRAINT positive_check CHECK (VALUE > 0)"),
            "got: {sql}"
        );
    }

    #[test]
    fn create_domain_not_null() {
        let ut = UserType {
            qname: qn("app", "nn_int"),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: false,
                default: None,
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
        };
        let sql = emit_create_type(&ut);
        assert!(sql.contains("NOT NULL"), "got: {sql}");
    }

    // --- emit_create_type (composite) ---

    #[test]
    fn create_composite_basic() {
        let ut = simple_composite();
        let sql = emit_create_type(&ut);
        assert_eq!(
            sql,
            "CREATE TYPE app.address AS (\n    street text,\n    zip text\n);"
        );
    }

    // --- emit_drop_type ---

    #[test]
    fn drop_type_basic() {
        let sql = emit_drop_type(&qn("app", "order_status"));
        assert_eq!(sql, "DROP TYPE app.order_status;");
    }

    #[test]
    fn drop_type_cascade() {
        let sql = emit_drop_type_cascade(&qn("app", "order_status"));
        assert_eq!(sql, "DROP TYPE app.order_status CASCADE;");
    }

    // --- emit_alter_type_add_value ---

    #[test]
    fn add_value_at_end() {
        let sql = emit_alter_type_add_value(&qn("app", "order_status"), "delivered", None, None);
        assert_eq!(sql, "ALTER TYPE app.order_status ADD VALUE 'delivered';");
    }

    #[test]
    fn add_value_before() {
        let sql = emit_alter_type_add_value(
            &qn("app", "order_status"),
            "processing",
            Some("shipped"),
            None,
        );
        assert!(sql.contains("BEFORE 'shipped'"), "got: {sql}");
    }

    #[test]
    fn add_value_after() {
        let sql = emit_alter_type_add_value(
            &qn("app", "order_status"),
            "dispatched",
            None,
            Some("pending"),
        );
        assert!(sql.contains("AFTER 'pending'"), "got: {sql}");
    }

    // --- emit_alter_type_rename_value ---

    #[test]
    fn rename_value_basic() {
        let sql = emit_alter_type_rename_value(&qn("app", "order_status"), "pending", "awaiting");
        assert_eq!(
            sql,
            "ALTER TYPE app.order_status RENAME VALUE 'pending' TO 'awaiting';"
        );
    }

    // --- emit_alter_domain_add_check ---

    #[test]
    fn domain_add_check() {
        let check = DomainCheck {
            name: id("positive_chk"),
            expression: NormalizedExpr::from_text("VALUE > 0"),
        };
        let sql = emit_alter_domain_add_check(&qn("app", "positive_int"), &check);
        assert_eq!(
            sql,
            "ALTER DOMAIN app.positive_int ADD CONSTRAINT positive_chk CHECK (VALUE > 0);"
        );
    }

    // --- emit_alter_domain_drop_check ---

    #[test]
    fn domain_drop_check() {
        let sql = emit_alter_domain_drop_check(&qn("app", "positive_int"), &id("positive_chk"));
        assert_eq!(
            sql,
            "ALTER DOMAIN app.positive_int DROP CONSTRAINT positive_chk;"
        );
    }

    // --- emit_alter_domain_set_default ---

    #[test]
    fn domain_set_default_some() {
        let expr = NormalizedExpr::from_text("42");
        let sql = emit_alter_domain_set_default(&qn("app", "positive_int"), Some(&expr));
        assert_eq!(sql, "ALTER DOMAIN app.positive_int SET DEFAULT 42;");
    }

    #[test]
    fn domain_set_default_none() {
        let sql = emit_alter_domain_set_default(&qn("app", "positive_int"), None);
        assert_eq!(sql, "ALTER DOMAIN app.positive_int DROP DEFAULT;");
    }

    // --- emit_alter_domain_set_not_null ---

    #[test]
    fn domain_set_not_null_true() {
        let sql = emit_alter_domain_set_not_null(&qn("app", "positive_int"), true);
        assert_eq!(sql, "ALTER DOMAIN app.positive_int SET NOT NULL;");
    }

    #[test]
    fn domain_set_not_null_false() {
        let sql = emit_alter_domain_set_not_null(&qn("app", "positive_int"), false);
        assert_eq!(sql, "ALTER DOMAIN app.positive_int DROP NOT NULL;");
    }

    // --- emit_alter_type_add_attribute ---

    #[test]
    fn composite_add_attribute() {
        let attr = CompositeAttribute {
            name: id("city"),
            ty: ColumnType::Text,
            collation: None,
        };
        let sql = emit_alter_type_add_attribute(&qn("app", "address"), &attr);
        assert_eq!(sql, "ALTER TYPE app.address ADD ATTRIBUTE city text;");
    }

    // --- emit_alter_type_drop_attribute ---

    #[test]
    fn composite_drop_attribute() {
        let sql = emit_alter_type_drop_attribute(&qn("app", "address"), &id("zip"));
        assert_eq!(sql, "ALTER TYPE app.address DROP ATTRIBUTE zip;");
    }

    // --- emit_alter_type_alter_attribute_type ---

    #[test]
    fn composite_alter_attribute_type() {
        let sql = emit_alter_type_alter_attribute_type(
            &qn("app", "address"),
            &id("zip"),
            &ColumnType::Varchar { len: Some(10) },
        );
        assert_eq!(
            sql,
            "ALTER TYPE app.address ALTER ATTRIBUTE zip TYPE varchar(10);"
        );
    }

    // --- emit_comment_on_type ---

    #[test]
    fn comment_on_enum_uses_type_keyword() {
        let kind = UserTypeKind::Enum { values: vec![] };
        let sql = emit_comment_on_type(&qn("app", "order_status"), &kind, Some("an enum"));
        assert_eq!(sql, "COMMENT ON TYPE app.order_status IS 'an enum';");
    }

    #[test]
    fn comment_on_domain_uses_domain_keyword() {
        let kind = UserTypeKind::Domain {
            base: ColumnType::Integer,
            nullable: true,
            default: None,
            check_constraints: vec![],
            collation: None,
        };
        let sql = emit_comment_on_type(&qn("app", "positive_int"), &kind, Some("a domain"));
        assert_eq!(sql, "COMMENT ON DOMAIN app.positive_int IS 'a domain';");
    }

    #[test]
    fn comment_on_composite_uses_type_keyword() {
        let kind = UserTypeKind::Composite { attributes: vec![] };
        let sql = emit_comment_on_type(&qn("app", "address"), &kind, Some("an address"));
        assert_eq!(sql, "COMMENT ON TYPE app.address IS 'an address';");
    }

    #[test]
    fn comment_on_type_clear() {
        let kind = UserTypeKind::Enum { values: vec![] };
        let sql = emit_comment_on_type(&qn("app", "order_status"), &kind, None);
        assert_eq!(sql, "COMMENT ON TYPE app.order_status IS NULL;");
    }

    #[test]
    fn comment_on_type_escapes_single_quotes() {
        let kind = UserTypeKind::Enum { values: vec![] };
        let sql = emit_comment_on_type(&qn("app", "order_status"), &kind, Some("it's a type"));
        assert_eq!(sql, "COMMENT ON TYPE app.order_status IS 'it''s a type';");
    }
}
