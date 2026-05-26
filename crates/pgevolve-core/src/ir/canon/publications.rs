//! Canon pass for publications. Validates and sorts.
//!
//! Invariants enforced:
//! - `Selective` with no tables and no schemas → error.
//! - `PublishKinds` with no enabled DML kinds → error.
//! - `PublishedTable.columns = Some(empty)` → error.
//! - Duplicate column in a `PublishedTable.columns` → error.
//!
//! Sorts:
//! - `Selective.tables` by `qname`.
//! - Each `PublishedTable.columns` by name (when `Some`).
//! - The publications collection itself is sorted by `sort_and_dedupe`,
//!   not here.

use crate::ir::IrError;
use crate::ir::catalog::Catalog;
use crate::ir::publication::{Publication, PublicationScope};

/// Validate and sort all publications in `cat`.
///
/// Returns the first [`IrError`] encountered (empty Selective scope,
/// empty publish bitset, empty column list, or duplicate column).
pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    for p in &mut cat.publications {
        validate_and_sort(p)?;
    }
    Ok(())
}

fn validate_and_sort(p: &mut Publication) -> Result<(), IrError> {
    if p.publish.is_empty() {
        return Err(IrError::EmptyPublishBitset(p.name.clone()));
    }
    if let PublicationScope::Selective { schemas, tables } = &mut p.scope {
        if schemas.is_empty() && tables.is_empty() {
            return Err(IrError::EmptyPublication(p.name.clone()));
        }
        // Tables: sort by qname; per-table column lists: validate + sort.
        tables.sort_by(|a, b| a.qname.cmp(&b.qname));
        for t in tables.iter_mut() {
            if let Some(cols) = &mut t.columns {
                if cols.is_empty() {
                    return Err(IrError::EmptyColumnList(p.name.clone(), t.qname.clone()));
                }
                cols.sort();
                for w in cols.windows(2) {
                    if w[0] == w[1] {
                        return Err(IrError::DuplicateColumnInPublication(
                            p.name.clone(),
                            t.qname.clone(),
                            w[0].clone(),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::publication::{PublishKinds, PublishedTable};
    use std::collections::BTreeSet;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(id(s), id(n))
    }

    fn pub_with_scope(scope: PublicationScope) -> Publication {
        Publication {
            name: id("p"),
            scope,
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn rejects_empty_selective() {
        let mut cat = Catalog::empty();
        cat.publications
            .push(pub_with_scope(PublicationScope::Selective {
                schemas: BTreeSet::new(),
                tables: Vec::new(),
            }));
        let err = run(&mut cat).unwrap_err();
        assert!(matches!(err, IrError::EmptyPublication(_)));
    }

    #[test]
    fn rejects_empty_publish_bitset() {
        let mut cat = Catalog::empty();
        let mut p = pub_with_scope(PublicationScope::AllTables);
        p.publish = PublishKinds {
            insert: false,
            update: false,
            delete: false,
            truncate: false,
        };
        cat.publications.push(p);
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::EmptyPublishBitset(_)
        ));
    }

    #[test]
    fn rejects_empty_column_list() {
        let mut cat = Catalog::empty();
        cat.publications
            .push(pub_with_scope(PublicationScope::Selective {
                schemas: BTreeSet::new(),
                tables: vec![PublishedTable {
                    qname: qn("app", "t"),
                    row_filter: None,
                    columns: Some(vec![]),
                }],
            }));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::EmptyColumnList(_, _)
        ));
    }

    #[test]
    fn rejects_duplicate_columns() {
        let mut cat = Catalog::empty();
        cat.publications
            .push(pub_with_scope(PublicationScope::Selective {
                schemas: BTreeSet::new(),
                tables: vec![PublishedTable {
                    qname: qn("app", "t"),
                    row_filter: None,
                    columns: Some(vec![id("a"), id("a")]),
                }],
            }));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::DuplicateColumnInPublication(_, _, _)
        ));
    }

    #[test]
    fn sorts_tables_and_columns() {
        let mut cat = Catalog::empty();
        cat.publications
            .push(pub_with_scope(PublicationScope::Selective {
                schemas: BTreeSet::new(),
                tables: vec![
                    PublishedTable {
                        qname: qn("app", "z"),
                        row_filter: None,
                        columns: Some(vec![id("c"), id("a"), id("b")]),
                    },
                    PublishedTable {
                        qname: qn("app", "a"),
                        row_filter: None,
                        columns: None,
                    },
                ],
            }));
        run(&mut cat).unwrap();
        let PublicationScope::Selective { tables, .. } = &cat.publications[0].scope else {
            panic!("expected Selective")
        };
        assert_eq!(tables[0].qname.name.as_str(), "a");
        assert_eq!(tables[1].qname.name.as_str(), "z");
        let cols = tables[1].columns.as_ref().unwrap();
        assert_eq!(cols[0].as_str(), "a");
        assert_eq!(cols[1].as_str(), "b");
        assert_eq!(cols[2].as_str(), "c");
    }

    #[test]
    fn all_tables_skips_selective_validation() {
        let mut cat = Catalog::empty();
        cat.publications
            .push(pub_with_scope(PublicationScope::AllTables));
        assert!(run(&mut cat).is_ok());
    }
}
