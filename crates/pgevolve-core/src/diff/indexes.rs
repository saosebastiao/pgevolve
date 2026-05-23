//! Index-level diffing.
//!
//! Pairs indexes by [`QualifiedName`]. Postgres has very limited
//! `ALTER INDEX` support — column lists, expressions, predicates, opclasses,
//! sort/nulls order, INCLUDE columns, and the access method are all immutable.
//! We therefore express any property change as [`Change::ReplaceIndex`]
//! (drop-then-create), and let the planner decide how to schedule the rebuild.
//!
//! The one exception is comment-only differences, which we represent by
//! emitting `ReplaceIndex` as well — Postgres does support
//! `COMMENT ON INDEX`, but representing it as a top-level `Change` would mean
//! a new variant; for v0.1 we keep the surface area small and treat it as a
//! replace. (We may revisit if comment-only churn becomes noisy.)

use std::collections::BTreeMap;

use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::ir::index::Index;

use super::change::Change;
use super::changeset::ChangeSet;
use super::destructiveness::Destructiveness;

/// Diff indexes in `target` against `source`, appending entries to `out`.
pub fn diff_indexes(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<&QualifiedName, &Index> =
        target.indexes.iter().map(|i| (&i.qname, i)).collect();
    let source_map: BTreeMap<&QualifiedName, &Index> =
        source.indexes.iter().map(|i| (&i.qname, i)).collect();

    for (qname, source_index) in &source_map {
        if !target_map.contains_key(qname) {
            out.push(
                Change::CreateIndex((*source_index).clone()),
                Destructiveness::Safe,
            );
        }
    }

    for (qname, target_index) in &target_map {
        match source_map.get(qname) {
            None => {
                out.push(
                    Change::DropIndex((*qname).clone()),
                    Destructiveness::RequiresApproval {
                        reason: format!("drops index {qname}"),
                    },
                );
            }
            Some(source_index) => {
                // Compute the storage delta first, then decide which change to emit
                // based on whether the non-storage fields match.
                let storage_delta = crate::diff::reloptions::index_delta(
                    &target_index.storage,
                    &source_index.storage,
                );

                if !target_index.structurally_eq(source_index) {
                    // Structural change (columns, method, predicate, …) → DROP + CREATE.
                    // The new `to` index already carries the desired storage options,
                    // so no separate SET step is needed.
                    out.push(
                        Change::ReplaceIndex {
                            from: (*target_index).clone(),
                            to: (*source_index).clone(),
                        },
                        Destructiveness::RequiresApproval {
                            reason: format!(
                                "replaces index {qname} (drop + create — Postgres cannot ALTER index properties in place)"
                            ),
                        },
                    );
                } else if !storage_delta.is_empty() {
                    // Only storage reloptions differ → emit ALTER INDEX SET (…).
                    out.push(
                        Change::SetIndexStorage {
                            qname: (*qname).clone(),
                            options: storage_delta,
                        },
                        Destructiveness::Safe,
                    );
                }
                // else: fully equal — no-op.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::index::{
        IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
    };

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(name: &str) -> QualifiedName {
        QualifiedName::new(id("app"), id(name))
    }

    fn col(name: &str) -> IndexColumn {
        IndexColumn {
            expr: IndexColumnExpr::Column(id(name)),
            collation: None,
            opclass: None,
            sort_order: SortOrder::Asc,
            nulls_order: NullsOrder::NullsLast,
        }
    }

    fn ix(name: &str, cols: Vec<IndexColumn>, unique: bool) -> Index {
        Index {
            qname: qn(name),
            on: IndexParent::Table(qn("users")),
            method: IndexMethod::BTree,
            columns: cols,
            include: vec![],
            unique,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
            storage: crate::ir::reloptions::IndexStorageOptions::default(),
        }
    }

    #[test]
    fn add_index_is_safe() {
        let target = Catalog::empty();
        let mut source = Catalog::empty();
        source
            .indexes
            .push(ix("users_email_idx", vec![col("email")], true));
        let mut cs = ChangeSet::new();
        diff_indexes(&target, &source, &mut cs);
        assert_eq!(cs.len(), 1);
        let entry = &cs.entries[0];
        assert!(matches!(entry.change, Change::CreateIndex(_)));
        assert_eq!(entry.destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn drop_index_requires_approval() {
        let mut target = Catalog::empty();
        target
            .indexes
            .push(ix("users_email_idx", vec![col("email")], true));
        let source = Catalog::empty();
        let mut cs = ChangeSet::new();
        diff_indexes(&target, &source, &mut cs);
        assert_eq!(cs.len(), 1);
        let entry = &cs.entries[0];
        assert!(matches!(entry.change, Change::DropIndex(_)));
        assert!(entry.destructiveness.requires_approval());
        assert!(!entry.destructiveness.data_loss_risk());
    }

    #[test]
    fn unique_change_emits_replace() {
        let mut target = Catalog::empty();
        target.indexes.push(ix("ix1", vec![col("email")], false));
        let mut source = Catalog::empty();
        source.indexes.push(ix("ix1", vec![col("email")], true));
        let mut cs = ChangeSet::new();
        diff_indexes(&target, &source, &mut cs);
        assert_eq!(cs.len(), 1);
        let entry = &cs.entries[0];
        match &entry.change {
            Change::ReplaceIndex { from, to } => {
                assert!(!from.unique);
                assert!(to.unique);
            }
            other => panic!("expected ReplaceIndex, got {other:?}"),
        }
        assert!(entry.destructiveness.requires_approval());
    }

    #[test]
    fn column_list_change_emits_replace() {
        let mut target = Catalog::empty();
        target.indexes.push(ix("ix1", vec![col("a")], false));
        let mut source = Catalog::empty();
        source
            .indexes
            .push(ix("ix1", vec![col("a"), col("b")], false));
        let mut cs = ChangeSet::new();
        diff_indexes(&target, &source, &mut cs);
        assert_eq!(cs.len(), 1);
        assert!(matches!(cs.entries[0].change, Change::ReplaceIndex { .. }));
    }

    #[test]
    fn equal_indexes_emit_nothing() {
        let mut target = Catalog::empty();
        target.indexes.push(ix("ix1", vec![col("email")], true));
        let mut source = Catalog::empty();
        source.indexes.push(ix("ix1", vec![col("email")], true));
        let mut cs = ChangeSet::new();
        diff_indexes(&target, &source, &mut cs);
        assert!(cs.is_empty());
    }

    #[test]
    fn storage_only_change_emits_set_not_replace() {
        // Indexes that differ ONLY in a reloption (fillfactor) must emit
        // SetIndexStorage, not ReplaceIndex.
        let base = ix("ix1", vec![col("email")], true);
        let mut target_cat = Catalog::empty();
        target_cat.indexes.push(Index {
            storage: crate::ir::reloptions::IndexStorageOptions {
                fillfactor: Some(70),
                ..Default::default()
            },
            ..base.clone()
        });
        let mut source_cat = Catalog::empty();
        source_cat.indexes.push(Index {
            storage: crate::ir::reloptions::IndexStorageOptions {
                fillfactor: Some(80),
                ..Default::default()
            },
            ..base
        });
        let mut cs = ChangeSet::new();
        diff_indexes(&target_cat, &source_cat, &mut cs);
        assert_eq!(cs.len(), 1, "expected exactly one change");
        match &cs.entries[0].change {
            Change::SetIndexStorage { options, .. } => {
                assert_eq!(options.fillfactor, Some(80));
            }
            other => panic!("expected SetIndexStorage, got {other:?}"),
        }
        assert_eq!(
            cs.entries[0].destructiveness,
            Destructiveness::Safe,
            "storage-only change should be safe"
        );
    }

    #[test]
    fn structural_change_emits_replace_not_set_storage() {
        // Indexes that differ structurally (different column list) must emit
        // ReplaceIndex, regardless of whether storage also differs.
        let base = ix("ix1", vec![col("a")], false);
        let mut target_cat = Catalog::empty();
        target_cat.indexes.push(Index {
            storage: crate::ir::reloptions::IndexStorageOptions {
                fillfactor: Some(70),
                ..Default::default()
            },
            ..base.clone()
        });
        let mut source_cat = Catalog::empty();
        source_cat.indexes.push(Index {
            columns: vec![col("a"), col("b")],
            storage: crate::ir::reloptions::IndexStorageOptions {
                fillfactor: Some(80),
                ..Default::default()
            },
            ..base
        });
        let mut cs = ChangeSet::new();
        diff_indexes(&target_cat, &source_cat, &mut cs);
        assert_eq!(cs.len(), 1, "expected exactly one change");
        assert!(
            matches!(cs.entries[0].change, Change::ReplaceIndex { .. }),
            "structural change must emit ReplaceIndex, got {:?}",
            cs.entries[0].change
        );
        assert!(
            cs.entries[0].destructiveness.requires_approval(),
            "ReplaceIndex must require approval"
        );
    }
}
