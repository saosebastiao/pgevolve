//! Promote `ColumnType::Other { raw: "schema.name" }` to
//! `ColumnType::UserDefined(qname)` when `schema.name` matches a managed
//! user-defined type in the catalog.
//!
//! The catalog reader uses `format_type(atttypid, atttypmod)` to fetch each
//! column's type string. For a column whose type is a user-defined enum,
//! domain, composite, or range, that returns `"<schema>.<name>"`. The string
//! parser in `ColumnType::parse_from_pg_type_string` has no knowledge of the
//! catalog's managed types and falls through to `ColumnType::Other` for any
//! unrecognised name. The source-side parser, by contrast, produces
//! `ColumnType::UserDefined` directly because two-segment names in source
//! DDL are unambiguous user-type references.
//!
//! Without this pass the same column would round-trip as
//! `UserDefined → Other`, causing spurious diffs on every plan for any table
//! that references a user-defined type. This pass runs after the catalog has
//! been assembled, walks every column-type field, and rewrites the
//! `Other` placeholders that resolve to a known managed type. It is
//! idempotent — already-resolved `UserDefined` values are untouched — so
//! running the pass on a source-side catalog is a no-op in practice.
//!
//! Type names that do not appear in `catalog.types` are left as `Other`;
//! downstream validation (e.g. AST resolution) will flag those as unresolved
//! references when appropriate.

use std::collections::BTreeSet;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::column_type::ColumnType;
use crate::ir::function::ReturnType;
use crate::ir::user_type::UserTypeKind;

/// Promote `Other` column-type placeholders to `UserDefined` for every column
/// that refers to a managed user-defined type.
pub fn run(cat: &mut Catalog) {
    let known: BTreeSet<(String, String)> = cat
        .types
        .iter()
        .map(|t| {
            (
                t.qname.schema.as_str().to_string(),
                t.qname.name.as_str().to_string(),
            )
        })
        .collect();

    // 1. Table columns.
    for table in &mut cat.tables {
        for col in &mut table.columns {
            promote(&mut col.ty, &known);
        }
    }

    // 2. Composite attributes and domain bases (user types referencing other
    //    user types).
    for ut in &mut cat.types {
        match &mut ut.kind {
            UserTypeKind::Composite { attributes } => {
                for attr in attributes {
                    promote(&mut attr.ty, &known);
                }
            }
            UserTypeKind::Domain { base, .. } => {
                promote(base, &known);
            }
            UserTypeKind::Enum { .. } | UserTypeKind::Range { .. } => {
                // Enums hold no inner types. Range subtype is a QualifiedName
                // directly (already resolved by the assembler), not a
                // ColumnType, so no promotion is needed here.
            }
        }
    }

    // 3. Function and procedure argument types + return types.
    for f in &mut cat.functions {
        for arg in &mut f.args {
            promote(&mut arg.ty, &known);
        }
        promote_return(&mut f.return_type, &known);
    }
    for p in &mut cat.procedures {
        for arg in &mut p.args {
            promote(&mut arg.ty, &known);
        }
    }
}

/// Promote a single `ColumnType` in place. Recurses into array element types.
fn promote(ty: &mut ColumnType, known: &BTreeSet<(String, String)>) {
    match ty {
        ColumnType::Other { raw } => {
            if let Some(qname) = qname_from_raw(raw)
                && known.contains(&(
                    qname.schema.as_str().to_string(),
                    qname.name.as_str().to_string(),
                ))
            {
                *ty = ColumnType::UserDefined(qname);
            }
        }
        ColumnType::Array { element, .. } => {
            promote(element, known);
        }
        _ => {}
    }
}

fn promote_return(rt: &mut ReturnType, known: &BTreeSet<(String, String)>) {
    match rt {
        ReturnType::Scalar { ty } | ReturnType::SetOf { ty } => promote(ty, known),
        ReturnType::Table { columns } => {
            for c in columns {
                promote(&mut c.ty, known);
            }
        }
        ReturnType::Trigger | ReturnType::EventTrigger | ReturnType::Void => {}
    }
}

/// Parse a `"schema.name"` raw type string into a [`QualifiedName`]. Returns
/// `None` for any string that is not a clean two-segment identifier pair —
/// strings with whitespace, typmods (`(...)`), array suffixes (already
/// stripped by the type parser), or unparseable identifiers are skipped so
/// that promotion is conservative.
fn qname_from_raw(raw: &str) -> Option<QualifiedName> {
    let trimmed = raw.trim();
    let (schema, name) = trimmed.split_once('.')?;
    if schema.is_empty() || name.is_empty() {
        return None;
    }
    // Reject anything with a typmod, whitespace, or further dots — these are
    // not bare user-type references.
    if name.contains('.') || name.contains('(') || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    let schema_id = Identifier::from_unquoted(schema).ok()?;
    let name_id = Identifier::from_unquoted(name).ok()?;
    Some(QualifiedName::new(schema_id, name_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::column::Column;
    use crate::ir::table::Table;
    use crate::ir::user_type::{CompositeAttribute, UserType};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(id(s), id(n))
    }

    fn make_range(qname: QualifiedName) -> UserType {
        UserType {
            qname,
            kind: UserTypeKind::Range {
                subtype: qn("pg_catalog", "int4"),
                subtype_opclass: None,
                collation: None,
                canonical: None,
                subtype_diff: None,
                multirange_type_name: None,
            },
            owner: None,
            comment: None,
            grants: vec![],
        }
    }

    fn make_composite(qname: QualifiedName, attrs: Vec<CompositeAttribute>) -> UserType {
        UserType {
            qname,
            kind: UserTypeKind::Composite { attributes: attrs },
            owner: None,
            comment: None,
            grants: vec![],
        }
    }

    fn col_with_other(name: &str, raw: &str) -> Column {
        Column {
            name: id(name),
            ty: ColumnType::Other {
                raw: raw.to_string(),
            },
            nullable: true,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        }
    }

    fn empty_table(qname: QualifiedName, columns: Vec<Column>) -> Table {
        Table {
            qname,
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
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
            tablespace: None,
        }
    }

    #[test]
    fn promotes_table_column_other_to_user_defined_for_known_range() {
        let mut cat = Catalog::empty();
        cat.types.push(make_range(qn("app", "my_range")));
        cat.tables.push(empty_table(
            qn("app", "t"),
            vec![col_with_other("span", "app.my_range")],
        ));
        run(&mut cat);
        match &cat.tables[0].columns[0].ty {
            ColumnType::UserDefined(q) => assert_eq!(*q, qn("app", "my_range")),
            other => panic!("expected UserDefined, got {other:?}"),
        }
    }

    #[test]
    fn leaves_unknown_other_alone() {
        let mut cat = Catalog::empty();
        cat.tables.push(empty_table(
            qn("app", "t"),
            vec![col_with_other("span", "nope.never_heard_of_it")],
        ));
        run(&mut cat);
        assert!(matches!(
            &cat.tables[0].columns[0].ty,
            ColumnType::Other { raw } if raw == "nope.never_heard_of_it",
        ));
    }

    #[test]
    fn leaves_non_dotted_other_alone() {
        let mut cat = Catalog::empty();
        cat.tables.push(empty_table(
            qn("app", "t"),
            vec![col_with_other("c", "unknownscalar")],
        ));
        run(&mut cat);
        assert!(matches!(
            &cat.tables[0].columns[0].ty,
            ColumnType::Other { raw } if raw == "unknownscalar",
        ));
    }

    #[test]
    fn leaves_typmod_other_alone() {
        // Types that retain a typmod (e.g. user-defined types we don't recognise
        // that take parameters) must not be misread as bare qnames.
        let mut cat = Catalog::empty();
        cat.types.push(make_range(qn("app", "my_range")));
        cat.tables.push(empty_table(
            qn("app", "t"),
            vec![col_with_other("c", "app.my_range(5)")],
        ));
        run(&mut cat);
        assert!(matches!(
            &cat.tables[0].columns[0].ty,
            ColumnType::Other { raw } if raw == "app.my_range(5)",
        ));
    }

    #[test]
    fn promotes_array_element_when_known() {
        let mut cat = Catalog::empty();
        cat.types.push(make_range(qn("app", "my_range")));
        cat.tables.push(empty_table(
            qn("app", "t"),
            vec![Column {
                ty: ColumnType::Array {
                    element: Box::new(ColumnType::Other {
                        raw: "app.my_range".into(),
                    }),
                    dims: 1,
                },
                ..col_with_other("c", "ignored")
            }],
        ));
        run(&mut cat);
        let ColumnType::Array { element, .. } = &cat.tables[0].columns[0].ty else {
            panic!("expected Array");
        };
        match element.as_ref() {
            ColumnType::UserDefined(q) => assert_eq!(*q, qn("app", "my_range")),
            other => panic!("expected UserDefined inside Array, got {other:?}"),
        }
    }

    #[test]
    fn promotes_composite_attribute() {
        let mut cat = Catalog::empty();
        cat.types.push(make_range(qn("app", "my_range")));
        cat.types.push(make_composite(
            qn("app", "wrapper"),
            vec![CompositeAttribute {
                name: id("span"),
                ty: ColumnType::Other {
                    raw: "app.my_range".into(),
                },
                collation: None,
            }],
        ));
        run(&mut cat);
        let UserTypeKind::Composite { attributes } = &cat.types[1].kind else {
            panic!("expected Composite");
        };
        match &attributes[0].ty {
            ColumnType::UserDefined(q) => assert_eq!(*q, qn("app", "my_range")),
            other => panic!("expected UserDefined, got {other:?}"),
        }
    }

    #[test]
    fn is_idempotent_on_already_resolved() {
        let mut cat = Catalog::empty();
        cat.types.push(make_range(qn("app", "my_range")));
        cat.tables.push(empty_table(
            qn("app", "t"),
            vec![Column {
                ty: ColumnType::UserDefined(qn("app", "my_range")),
                ..col_with_other("c", "ignored")
            }],
        ));
        run(&mut cat);
        match &cat.tables[0].columns[0].ty {
            ColumnType::UserDefined(q) => assert_eq!(*q, qn("app", "my_range")),
            other => panic!("expected UserDefined, got {other:?}"),
        }
    }
}
