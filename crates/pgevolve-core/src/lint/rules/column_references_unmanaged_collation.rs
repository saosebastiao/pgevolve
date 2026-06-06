//! Warns when a column / domain / range / composite-attribute references a
//! collation that is neither a Postgres built-in nor managed in source.
//!
//! Bypassed when the referenced collation is:
//! - Explicitly schema-qualified as `pg_catalog.<name>` (e.g. `pg_catalog.C`).
//! - Unqualified and matches a name in
//!   [`crate::ir::collation::BUILTIN_COLLATIONS`] (PG-seeded shortnames like
//!   `default`, `C`, `POSIX`, `und-x-icu`, `unicode`, `ucs_basic`).
//! - Found in `source.collations` (matched on qname).
//!
//! Otherwise emits a Warning naming both the referencing object and the
//! missing collation. Source-only rule (does not need a target catalog) —
//! registered via [`crate::lint::universal::check_plan_time_catalog`].

use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::ir::collation::BUILTIN_COLLATIONS;
use crate::ir::user_type::UserTypeKind;
use crate::lint::finding::Finding;

pub const RULE_ID: &str = "column-references-unmanaged-collation";

/// Returns `true` iff `qname` references a collation that is *not* covered
/// by the built-in / managed allow-list — i.e., the rule should fire.
fn is_unmanaged_collation(qname: &QualifiedName, source: &Catalog) -> bool {
    if qname.schema.as_str() == "pg_catalog" {
        return false;
    }
    if qname.schema.as_str().is_empty() && BUILTIN_COLLATIONS.contains(&qname.name.as_str()) {
        return false;
    }
    !source.collations.iter().any(|c| &c.qname == qname)
}

pub fn check(source: &Catalog) -> Vec<Finding> {
    let mut out = Vec::new();

    // Columns on every managed table.
    for t in &source.tables {
        for c in &t.columns {
            if let Some(coll) = &c.collation
                && is_unmanaged_collation(coll, source)
            {
                out.push(Finding::warning(
                    RULE_ID,
                    format!(
                        "column {}.{} references unmanaged collation {coll}",
                        t.qname, c.name,
                    ),
                ));
            }
        }
    }

    // Domain / Range / Composite-attribute collations on user-defined types.
    for ut in &source.types {
        match &ut.kind {
            UserTypeKind::Domain { collation, .. } => {
                if let Some(coll) = collation
                    && is_unmanaged_collation(coll, source)
                {
                    out.push(Finding::warning(
                        RULE_ID,
                        format!("domain {} references unmanaged collation {coll}", ut.qname),
                    ));
                }
            }
            UserTypeKind::Range { collation, .. } => {
                if let Some(coll) = collation
                    && is_unmanaged_collation(coll, source)
                {
                    out.push(Finding::warning(
                        RULE_ID,
                        format!(
                            "range type {} references unmanaged collation {coll}",
                            ut.qname,
                        ),
                    ));
                }
            }
            UserTypeKind::Composite { attributes } => {
                for attr in attributes {
                    if let Some(coll) = &attr.collation
                        && is_unmanaged_collation(coll, source)
                    {
                        out.push(Finding::warning(
                            RULE_ID,
                            format!(
                                "composite attribute {}.{} references unmanaged collation {coll}",
                                ut.qname, attr.name,
                            ),
                        ));
                    }
                }
            }
            UserTypeKind::Enum { .. } => {}
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::collation::{Collation, CollationProvider};
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::reloptions::TableStorageOptions;
    use crate::ir::schema::Schema;
    use crate::ir::table::Table;
    use crate::ir::user_type::{CompositeAttribute, UserType, UserTypeKind};
    use crate::lint::finding::Severity;
    use crate::lint::test_helpers::{id, qn};

    fn make_collation(schema: &str, name: &str) -> Collation {
        Collation {
            qname: qn(schema, name),
            provider: CollationProvider::Libc,
            lc_collate: "C".into(),
            lc_ctype: "C".into(),
            deterministic: true,
            version: None,
            owner: None,
            comment: None,
        }
    }

    fn make_column(name: &str, collation: Option<QualifiedName>) -> Column {
        Column {
            name: id(name),
            ty: ColumnType::Text,
            nullable: true,
            default: None,
            identity: None,
            generated: None,
            collation,
            storage: None,
            compression: None,
            comment: None,
        }
    }

    fn make_table(schema: &str, name: &str, columns: Vec<Column>) -> Table {
        Table {
            qname: qn(schema, name),
            columns,
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: TableStorageOptions::default(),
            access_method: None,
        }
    }

    fn make_user_type(qname: QualifiedName, kind: UserTypeKind) -> UserType {
        UserType {
            qname,
            kind,
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    #[test]
    fn no_collations_silent() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.tables
            .push(make_table("app", "users", vec![make_column("email", None)]));
        assert!(check(&cat).is_empty());
    }

    #[test]
    fn pg_catalog_collation_silent() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.tables.push(make_table(
            "app",
            "users",
            vec![make_column("email", Some(qn("pg_catalog", "C")))],
        ));
        assert!(check(&cat).is_empty());
    }

    #[test]
    fn managed_collation_silent() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.collations.push(make_collation("app", "ci"));
        cat.tables.push(make_table(
            "app",
            "users",
            vec![make_column("email", Some(qn("app", "ci")))],
        ));
        assert!(check(&cat).is_empty());
    }

    #[test]
    fn unmanaged_qualified_collation_fires_for_column() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.tables.push(make_table(
            "app",
            "users",
            vec![make_column("email", Some(qn("app", "ci")))],
        ));
        let findings = check(&cat);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(findings[0].message.contains("app.users"));
        assert!(findings[0].message.contains("email"));
        assert!(findings[0].message.contains("app.ci"));
    }

    #[test]
    fn domain_unmanaged_collation_fires() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.types.push(make_user_type(
            qn("app", "text_ci"),
            UserTypeKind::Domain {
                base: ColumnType::Text,
                nullable: true,
                default: None,
                check_constraints: vec![],
                collation: Some(qn("app", "ci")),
            },
        ));
        let findings = check(&cat);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("domain"));
        assert!(findings[0].message.contains("app.text_ci"));
        assert!(findings[0].message.contains("app.ci"));
    }

    #[test]
    fn range_unmanaged_collation_fires() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.types.push(make_user_type(
            qn("app", "text_range"),
            UserTypeKind::Range {
                subtype: qn("pg_catalog", "text"),
                subtype_opclass: None,
                collation: Some(qn("app", "ci")),
                canonical: None,
                subtype_diff: None,
                multirange_type_name: None,
            },
        ));
        let findings = check(&cat);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("range type"));
        assert!(findings[0].message.contains("app.text_range"));
        assert!(findings[0].message.contains("app.ci"));
    }

    #[test]
    fn composite_attribute_unmanaged_collation_fires() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.types.push(make_user_type(
            qn("app", "person"),
            UserTypeKind::Composite {
                attributes: vec![CompositeAttribute {
                    name: id("name"),
                    ty: ColumnType::Text,
                    collation: Some(qn("app", "ci")),
                }],
            },
        ));
        let findings = check(&cat);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("composite attribute"));
        assert!(findings[0].message.contains("app.person"));
        assert!(findings[0].message.contains("name"));
        assert!(findings[0].message.contains("app.ci"));
    }
}
