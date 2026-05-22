//! Sequence-level diffing.
//!
//! Pairs sequences by [`QualifiedName`]. Sequences that are owned by a column
//! (`OWNED BY table.column`) are *skipped* — they are managed implicitly via
//! the owning column's identity / generated specification, and emitting
//! independent sequence ops for them would conflict with the column-driven
//! changes.
//!
//! `START` is intentionally absent from [`SequenceOp`] (Postgres requires
//! `RESTART`, which has different semantics). When `start` differs we emit a
//! drop+create pair so the new starting point is honored.

use std::collections::BTreeMap;

use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::ir::sequence::Sequence;

use super::change::Change;
use super::changeset::ChangeSet;
use super::destructiveness::Destructiveness;
use super::sequence_op::{SequenceOp, SequenceOpEntry};

/// Diff sequences in `target` against `source`, appending entries to `out`.
pub fn diff_sequences(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    // Skip column-owned sequences — they are driven by their owner column's diff.
    let target_map: BTreeMap<&QualifiedName, &Sequence> = target
        .sequences
        .iter()
        .filter(|s| s.owned_by.is_none())
        .map(|s| (&s.qname, s))
        .collect();
    let source_map: BTreeMap<&QualifiedName, &Sequence> = source
        .sequences
        .iter()
        .filter(|s| s.owned_by.is_none())
        .map(|s| (&s.qname, s))
        .collect();

    for (qname, source_seq) in &source_map {
        if !target_map.contains_key(qname) {
            out.push(
                Change::CreateSequence((*source_seq).clone()),
                Destructiveness::Safe,
            );
        }
    }

    for (qname, target_seq) in &target_map {
        match source_map.get(qname) {
            None => {
                out.push(
                    Change::DropSequence((*qname).clone()),
                    Destructiveness::RequiresApproval {
                        reason: format!("drops sequence {qname} (current value lost)"),
                    },
                );
            }
            Some(source_seq) => {
                if target_seq.start != source_seq.start {
                    // `START` cannot be altered in place; drop+create.
                    out.push(
                        Change::DropSequence((*qname).clone()),
                        Destructiveness::RequiresApproval {
                            reason: format!(
                                "drops sequence {qname} to change START (current value lost)"
                            ),
                        },
                    );
                    out.push(
                        Change::CreateSequence((*source_seq).clone()),
                        Destructiveness::Safe,
                    );
                    continue;
                }

                let ops = diff_sequence_fields(target_seq, source_seq);
                if !ops.is_empty() {
                    out.push(
                        Change::AlterSequence {
                            qname: (*qname).clone(),
                            ops,
                        },
                        Destructiveness::Safe,
                    );
                }
            }
        }
    }
}

fn diff_sequence_fields(target: &Sequence, source: &Sequence) -> Vec<SequenceOpEntry> {
    let mut ops = Vec::new();

    if target.increment != source.increment {
        ops.push(SequenceOpEntry {
            op: SequenceOp::SetIncrement(source.increment),
            destructiveness: Destructiveness::Safe,
        });
    }
    if target.min_value != source.min_value {
        ops.push(SequenceOpEntry {
            op: SequenceOp::SetMinValue(source.min_value),
            destructiveness: Destructiveness::Safe,
        });
    }
    if target.max_value != source.max_value {
        ops.push(SequenceOpEntry {
            op: SequenceOp::SetMaxValue(source.max_value),
            destructiveness: Destructiveness::Safe,
        });
    }
    if target.cache != source.cache {
        ops.push(SequenceOpEntry {
            op: SequenceOp::SetCache(source.cache),
            destructiveness: Destructiveness::Safe,
        });
    }
    if target.cycle != source.cycle {
        ops.push(SequenceOpEntry {
            op: SequenceOp::SetCycle(source.cycle),
            destructiveness: Destructiveness::Safe,
        });
    }
    if target.data_type != source.data_type {
        ops.push(SequenceOpEntry {
            op: SequenceOp::SetDataType(source.data_type.clone()),
            destructiveness: Destructiveness::RequiresApproval {
                reason: format!(
                    "changing data type may overflow current value of sequence {}",
                    target.qname
                ),
            },
        });
    }
    if target.owned_by != source.owned_by {
        ops.push(SequenceOpEntry {
            op: SequenceOp::SetOwnedBy(source.owned_by.clone()),
            destructiveness: Destructiveness::Safe,
        });
    }

    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::column_type::ColumnType;
    use crate::ir::sequence::SequenceOwner;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(name: &str) -> QualifiedName {
        QualifiedName::new(id("app"), id(name))
    }

    fn seq(name: &str) -> Sequence {
        Sequence {
            qname: qn(name),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cache: 1,
            cycle: false,
            owned_by: None,
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    #[test]
    fn add_sequence_is_safe() {
        let target = Catalog::empty();
        let mut source = Catalog::empty();
        source.sequences.push(seq("seq1"));
        let mut cs = ChangeSet::new();
        diff_sequences(&target, &source, &mut cs);
        assert_eq!(cs.len(), 1);
        assert!(matches!(cs.entries[0].change, Change::CreateSequence(_)));
        assert_eq!(cs.entries[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn drop_sequence_requires_approval() {
        let mut target = Catalog::empty();
        target.sequences.push(seq("seq1"));
        let source = Catalog::empty();
        let mut cs = ChangeSet::new();
        diff_sequences(&target, &source, &mut cs);
        assert_eq!(cs.len(), 1);
        let entry = &cs.entries[0];
        assert!(matches!(entry.change, Change::DropSequence(_)));
        assert!(entry.destructiveness.requires_approval());
    }

    #[test]
    fn increment_change_emits_alter_safe() {
        let mut target = Catalog::empty();
        target.sequences.push(seq("seq1"));
        let mut source = Catalog::empty();
        source.sequences.push(Sequence {
            increment: 2,
            ..seq("seq1")
        });
        let mut cs = ChangeSet::new();
        diff_sequences(&target, &source, &mut cs);
        assert_eq!(cs.len(), 1);
        let entry = &cs.entries[0];
        match &entry.change {
            Change::AlterSequence { qname, ops } => {
                assert_eq!(qname, &qn("seq1"));
                assert_eq!(ops.len(), 1);
                assert!(matches!(ops[0].op, SequenceOp::SetIncrement(2)));
                assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn data_type_change_requires_approval_inside_alter() {
        let mut target = Catalog::empty();
        target.sequences.push(seq("seq1"));
        let mut source = Catalog::empty();
        source.sequences.push(Sequence {
            data_type: ColumnType::Integer,
            ..seq("seq1")
        });
        let mut cs = ChangeSet::new();
        diff_sequences(&target, &source, &mut cs);
        let entry = &cs.entries[0];
        match &entry.change {
            Change::AlterSequence { ops, .. } => {
                assert_eq!(ops.len(), 1);
                assert!(ops[0].destructiveness.requires_approval());
                assert!(!ops[0].destructiveness.data_loss_risk());
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn cycle_min_max_cache_each_emit_safe_ops() {
        let mut target = Catalog::empty();
        target.sequences.push(seq("seq1"));
        let mut source = Catalog::empty();
        source.sequences.push(Sequence {
            cycle: true,
            min_value: Some(0),
            max_value: Some(1_000_000),
            cache: 32,
            ..seq("seq1")
        });
        let mut cs = ChangeSet::new();
        diff_sequences(&target, &source, &mut cs);
        match &cs.entries[0].change {
            Change::AlterSequence { ops, .. } => {
                assert_eq!(ops.len(), 4);
                assert!(
                    ops.iter()
                        .all(|o| o.destructiveness == Destructiveness::Safe)
                );
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn start_change_emits_drop_then_create() {
        let mut target = Catalog::empty();
        target.sequences.push(seq("seq1"));
        let mut source = Catalog::empty();
        source.sequences.push(Sequence {
            start: 100,
            ..seq("seq1")
        });
        let mut cs = ChangeSet::new();
        diff_sequences(&target, &source, &mut cs);
        assert_eq!(cs.len(), 2);
        assert!(matches!(cs.entries[0].change, Change::DropSequence(_)));
        assert!(matches!(cs.entries[1].change, Change::CreateSequence(_)));
    }

    #[test]
    fn owned_sequences_are_skipped() {
        let owned = Sequence {
            owned_by: Some(SequenceOwner {
                table: qn("users"),
                column: id("id"),
            }),
            ..seq("seq1")
        };
        let mut target = Catalog::empty();
        target.sequences.push(owned);
        let source = Catalog::empty(); // sequence missing in source — would normally drop
        let mut cs = ChangeSet::new();
        diff_sequences(&target, &source, &mut cs);
        assert!(
            cs.is_empty(),
            "owned sequences are driven by their owner column"
        );
    }

    #[test]
    fn equal_sequences_emit_nothing() {
        let mut target = Catalog::empty();
        target.sequences.push(seq("seq1"));
        let mut source = Catalog::empty();
        source.sequences.push(seq("seq1"));
        let mut cs = ChangeSet::new();
        diff_sequences(&target, &source, &mut cs);
        assert!(cs.is_empty());
    }
}
