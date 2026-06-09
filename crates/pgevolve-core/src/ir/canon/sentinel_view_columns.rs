//! Collapse view and materialized-view column types to a shared
//! sentinel.
//!
//! Source-side parsing produces placeholder `ColumnType::Other` values
//! for view columns because static type-resolution of an arbitrary
//! SELECT body without running it through PG is non-trivial. The
//! catalog reader produces real types via `format_type` on the view's
//! `pg_class` row. Body-level changes are already captured by
//! `body_canonical` (a canonicalized AST hash), so per-output-column
//! types are redundant info derived from the body. We normalize them
//! to a single sentinel on both sides so byte-equality holds without
//! a source-side analyzer.

use crate::ir::IrError;
use crate::ir::catalog::Catalog;
use crate::ir::column_type::ColumnType;

/// Replace every view and MV column's `column_type` with the
/// `view_column` sentinel.
///
/// This pass also enforces resolution: a column whose `column_type` is still
/// `None` (the unresolved alias-list marker) at canon time is an internal
/// resolver bug — [`canonicalize_view_bodies`] is expected to have filled it
/// in. Such a column raises [`IrError::UnresolvedViewColumn`] rather than
/// silently collapsing to the sentinel, guaranteeing no unresolved type ever
/// reaches a serialized catalog.
///
/// [`canonicalize_view_bodies`]: crate::parse::ast_canon::canonicalize_view_bodies
pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    let sentinel = ColumnType::Other {
        raw: "view_column".to_string(),
    };
    for v in &mut cat.views {
        for c in &mut v.columns {
            if c.column_type.is_none() {
                return Err(IrError::UnresolvedViewColumn {
                    view: v.qname.clone(),
                    column: c.name.clone(),
                });
            }
            c.column_type = Some(sentinel.clone());
        }
    }
    for m in &mut cat.materialized_views {
        for c in &mut m.columns {
            if c.column_type.is_none() {
                return Err(IrError::UnresolvedViewColumn {
                    view: m.qname.clone(),
                    column: c.name.clone(),
                });
            }
            c.column_type = Some(sentinel.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::view::{View, ViewColumn};
    use crate::parse::normalize_body::NormalizedBody;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn replaces_bigint_view_column_with_sentinel() {
        let mut cat = Catalog::empty();
        cat.views.push(View {
            qname: QualifiedName::new(id("app"), id("v")),
            columns: vec![ViewColumn {
                name: id("id"),
                column_type: Some(ColumnType::BigInt),
                comment: None,
            }],
            body_canonical: NormalizedBody::empty(),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        });
        run(&mut cat).expect("resolved columns canonicalize");
        assert!(matches!(
            &cat.views[0].columns[0].column_type,
            Some(ColumnType::Other { raw }) if raw == "view_column",
        ));
    }

    #[test]
    fn unresolved_view_column_rejected() {
        use crate::ir::IrError;
        let mut cat = Catalog::empty();
        cat.views.push(View {
            qname: QualifiedName::new(id("app"), id("v")),
            columns: vec![ViewColumn {
                name: id("id"),
                column_type: None,
                comment: None,
            }],
            body_canonical: NormalizedBody::empty(),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        });
        let err = run(&mut cat).unwrap_err();
        assert!(matches!(err, IrError::UnresolvedViewColumn { .. }));
    }
}
