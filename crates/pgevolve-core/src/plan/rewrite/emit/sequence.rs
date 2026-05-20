//! Dispatchers for `Change::CreateSequence`, `Change::DropSequence`,
//! `Change::AlterSequence`, plus the per-`SequenceOp` emitter.

use crate::diff::sequence_op::{SequenceOp, SequenceOpEntry};
use crate::identifier::QualifiedName;
use crate::ir::sequence::Sequence;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::sql;

use super::super::destructive_reason;

pub fn create(
    s: Sequence,
    _destructive: bool,
    _destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    let qname = s.qname.clone();
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::CreateSequence,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![qname.clone()],
        sql: sql::create_sequence(&s),
        transactional: TransactionConstraint::InTransaction,
    });
    if let Some(c) = &s.comment {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterSequence,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: format!(
                "COMMENT ON SEQUENCE {} IS '{}';",
                qname.render_sql(),
                c.replace('\'', "''"),
            ),
            transactional: TransactionConstraint::InTransaction,
        });
    }
}

pub fn drop_(
    qname: QualifiedName,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::DropSequence,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: vec![qname.clone()],
        sql: sql::drop_sequence(&qname),
        transactional: TransactionConstraint::InTransaction,
    });
}

pub fn alter(
    qname: QualifiedName,
    ops: Vec<SequenceOpEntry>,
    out: &mut Vec<RawStep>,
) {
    for op_entry in ops {
        op(&qname, op_entry, out);
    }
}

fn op(qname: &QualifiedName, entry: SequenceOpEntry, out: &mut Vec<RawStep>) {
    let destructive = entry.destructiveness.requires_approval();
    let destructive_reason = destructive_reason(&entry.destructiveness);
    let sql = match &entry.op {
        SequenceOp::SetIncrement(n) => sql::alter_sequence_increment(qname, *n),
        SequenceOp::SetMinValue(v) => sql::alter_sequence_min_value(qname, *v),
        SequenceOp::SetMaxValue(v) => sql::alter_sequence_max_value(qname, *v),
        SequenceOp::SetCache(n) => sql::alter_sequence_cache(qname, *n),
        SequenceOp::SetCycle(b) => sql::alter_sequence_cycle(qname, *b),
        SequenceOp::SetDataType(t) => sql::alter_sequence_data_type(qname, t),
        SequenceOp::SetOwnedBy(o) => sql::alter_sequence_owned_by(qname, o.as_ref()),
    };
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::AlterSequence,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: vec![qname.clone()],
        sql,
        transactional: TransactionConstraint::InTransaction,
    });
}
