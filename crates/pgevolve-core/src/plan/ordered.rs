//! [`OrderedChangeSet`] — the planner's ordered output.
//!
//! Three buckets, each in dependency-correct order, plus a list of FK
//! constraints that had to be deferred to a post-pass to break cycles.

use crate::diff::change::ChangeEntry;
use crate::identifier::QualifiedName;
use crate::ir::constraint::Constraint;

/// A `ChangeSet` partitioned and sorted by the planner.
///
/// Bucket semantics:
/// - `creates_and_adds`: `CreateSchema/Table/Index/Sequence`, in dependency
///   order (dependencies before dependents).
/// - `modifies`: `AlterSchema/Table/Sequence`, `ReplaceIndex`, in dependency
///   order over the source-side graph.
/// - `drops`: `DropSchema/Table/Index/Sequence`, in **reverse** dependency
///   order over the target-side graph.
/// - `deferred_fks`: FK constraints removed from the create graph to break a
///   cycle; emitted after `creates_and_adds` as `ALTER TABLE ... ADD CONSTRAINT`.
// `ChangeEntry` is only `PartialEq` (it transitively contains `f64` literals),
// so this struct cannot derive `Eq`. The clippy `derive_partial_eq_without_eq`
// lint is suppressed accordingly.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OrderedChangeSet {
    /// Creates and additive operations, in dependency order.
    pub creates_and_adds: Vec<ChangeEntry>,
    /// Modify-in-place operations, in dependency order.
    pub modifies: Vec<ChangeEntry>,
    /// Drops, in reverse dependency order.
    pub drops: Vec<ChangeEntry>,
    /// FK constraints deferred to break create-graph cycles.
    pub deferred_fks: Vec<DeferredFkAdd>,
}

/// A single FK constraint extracted from the create graph for a post-pass
/// `ALTER TABLE ... ADD CONSTRAINT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredFkAdd {
    /// Table that owns the constraint.
    pub table: QualifiedName,
    /// The full constraint definition (still a `Constraint::ForeignKey(_)`).
    pub constraint: Constraint,
}

impl OrderedChangeSet {
    /// True iff every bucket is empty.
    pub const fn is_empty(&self) -> bool {
        self.creates_and_adds.is_empty()
            && self.modifies.is_empty()
            && self.drops.is_empty()
            && self.deferred_fks.is_empty()
    }

    /// Total entries across all buckets, including deferred FKs.
    pub const fn len(&self) -> usize {
        self.creates_and_adds.len()
            + self.modifies.len()
            + self.drops.len()
            + self.deferred_fks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::change::Change;
    use crate::diff::destructiveness::Destructiveness;
    use crate::identifier::Identifier;
    use crate::ir::constraint::{
        ConstraintKind, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
    };
    use crate::ir::schema::Schema;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn fk_constraint() -> Constraint {
        Constraint {
            qname: qn("app", "fk1"),
            kind: ConstraintKind::ForeignKey(ForeignKey {
                columns: vec![id("ref_id")],
                referenced_table: qn("app", "b"),
                referenced_columns: vec![id("id")],
                on_update: ReferentialAction::NoAction,
                on_delete: ReferentialAction::NoAction,
                match_type: FkMatchType::Simple,
            }),
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    #[test]
    fn default_is_empty() {
        let o = OrderedChangeSet::default();
        assert!(o.is_empty());
        assert_eq!(o.len(), 0);
    }

    #[test]
    fn len_sums_all_buckets() {
        let o = OrderedChangeSet {
            creates_and_adds: vec![ChangeEntry {
                change: Change::CreateSchema(Schema::new(id("app"))),
                destructiveness: Destructiveness::Safe,
            }],
            modifies: vec![],
            drops: vec![ChangeEntry {
                change: Change::DropSchema(id("old")),
                destructiveness: Destructiveness::RequiresApproval {
                    reason: "drop".into(),
                },
            }],
            deferred_fks: vec![DeferredFkAdd {
                table: qn("app", "a"),
                constraint: fk_constraint(),
            }],
        };
        assert!(!o.is_empty());
        assert_eq!(o.len(), 3);
    }

    #[test]
    fn is_empty_iff_all_buckets_empty() {
        let mut o = OrderedChangeSet::default();
        assert!(o.is_empty());
        o.deferred_fks.push(DeferredFkAdd {
            table: qn("app", "a"),
            constraint: fk_constraint(),
        });
        assert!(!o.is_empty());
    }
}
