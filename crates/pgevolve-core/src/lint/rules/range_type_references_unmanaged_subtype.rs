//! Warns when a `CREATE TYPE … AS RANGE` declares a subtype that is neither
//! a Postgres built-in scalar nor a managed user-defined type in source.
//!
//! Bypassed when the subtype is:
//! - Explicitly schema-qualified as `pg_catalog.<name>`.
//! - Unqualified or `pg_catalog`-qualified and matches a name in
//!   [`BUILTIN_RANGE_SUBTYPES`] (PG-seeded scalar types commonly used as
//!   range elements).
//! - Found in `source.types` (matched on qname).
//!
//! Source-only rule — registered via
//! [`crate::lint::universal::check_plan_time_catalog`].

use crate::ir::catalog::Catalog;
use crate::ir::user_type::UserTypeKind;
use crate::lint::finding::Finding;

pub const RULE_ID: &str = "range-type-references-unmanaged-subtype";

/// Postgres built-in scalar types that may safely be used as range subtypes
/// without an accompanying managed type declaration.
///
/// Stored as bare names because the AST surfaces both unqualified
/// (`subtype = int4`) and `pg_catalog`-qualified (`pg_catalog.int4`) forms;
/// the check accepts either spelling.
pub const BUILTIN_RANGE_SUBTYPES: &[&str] = &[
    "int2",
    "int4",
    "int8",
    "numeric",
    "float4",
    "float8",
    "text",
    "varchar",
    "bpchar",
    "date",
    "timestamp",
    "timestamptz",
    "time",
    "timetz",
    "interval",
    "inet",
    "cidr",
    "uuid",
    "oid",
];

pub fn check(source: &Catalog) -> Vec<Finding> {
    let mut out = Vec::new();

    for ut in &source.types {
        let UserTypeKind::Range { subtype, .. } = &ut.kind else {
            continue;
        };

        if subtype.schema.as_str() == "pg_catalog" {
            continue;
        }
        if BUILTIN_RANGE_SUBTYPES.contains(&subtype.name.as_str()) {
            // Accept both unqualified and any-schema spellings of built-in
            // scalar names — the AST is inconsistent here.
            continue;
        }
        if source.types.iter().any(|t| &t.qname == subtype) {
            continue;
        }

        out.push(Finding::warning(
            RULE_ID,
            format!(
                "range type {} references unmanaged subtype {subtype}",
                ut.qname,
            ),
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::QualifiedName;
    use crate::ir::column_type::ColumnType;
    use crate::ir::schema::Schema;
    use crate::ir::user_type::{UserType, UserTypeKind};
    use crate::lint::finding::Severity;
    use crate::lint::test_helpers::{id, qn};

    fn make_range(schema: &str, name: &str, subtype: QualifiedName) -> UserType {
        UserType {
            qname: qn(schema, name),
            kind: UserTypeKind::Range {
                subtype,
                subtype_opclass: None,
                collation: None,
                canonical: None,
                subtype_diff: None,
                multirange_type_name: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn make_domain(schema: &str, name: &str) -> UserType {
        UserType {
            qname: qn(schema, name),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: None,
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    #[test]
    fn empty_catalog_silent() {
        let cat = Catalog::empty();
        assert!(check(&cat).is_empty());
    }

    #[test]
    fn pg_catalog_subtype_silent() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.types
            .push(make_range("app", "ir", qn("pg_catalog", "int4")));
        assert!(check(&cat).is_empty());
    }

    #[test]
    fn builtin_unqualified_subtype_silent() {
        // An AST might surface `subtype = int4` as an unqualified name; the
        // check should accept any schema (or empty schema) when the bare name
        // is in BUILTIN_RANGE_SUBTYPES.
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        // Use the `app` schema with a built-in bare name — still bypassed.
        cat.types.push(make_range("app", "ir", qn("app", "int4")));
        assert!(check(&cat).is_empty());
    }

    #[test]
    fn managed_user_type_subtype_silent() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.types.push(make_domain("app", "positive_int"));
        cat.types
            .push(make_range("app", "ir", qn("app", "positive_int")));
        assert!(check(&cat).is_empty());
    }

    #[test]
    fn unmanaged_subtype_fires() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.types
            .push(make_range("app", "ir", qn("ext", "custom_type")));
        let findings = check(&cat);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(findings[0].message.contains("app.ir"));
        assert!(findings[0].message.contains("ext.custom_type"));
    }

    #[test]
    fn non_range_types_ignored() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.types.push(make_domain("app", "positive_int"));
        assert!(check(&cat).is_empty());
    }
}
