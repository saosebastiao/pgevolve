//! `SequenceOp` — per-sequence field updates.
//!
//! Carried inside [`Change::AlterSequence`](super::change::Change::AlterSequence).
//!
//! Note: `START` is intentionally not represented here. Postgres requires
//! `RESTART`, which has different semantics (it touches the live counter, not
//! the declared starting point), so v0.1 emits a recreate when `start` differs.

use serde::{Deserialize, Serialize};

use crate::ir::column_type::ColumnType;
use crate::ir::sequence::SequenceOwner;

use super::destructiveness::Destructiveness;

/// One sequence-field op paired with its destructiveness classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceOpEntry {
    /// The sequence operation.
    pub op: SequenceOp,
    /// Risk classification.
    pub destructiveness: Destructiveness,
}

/// One field-level update on a sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SequenceOp {
    /// Set `INCREMENT BY`.
    SetIncrement(i64),
    /// Set `MINVALUE` (`None` = NO MINVALUE).
    SetMinValue(Option<i64>),
    /// Set `MAXVALUE` (`None` = NO MAXVALUE).
    SetMaxValue(Option<i64>),
    /// Set `CACHE`.
    SetCache(i64),
    /// Set `CYCLE` / `NO CYCLE`.
    SetCycle(bool),
    /// Set the sequence's data type.
    SetDataType(ColumnType),
    /// Set or clear `OWNED BY`.
    SetOwnedBy(Option<SequenceOwner>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_increment_serde_round_trip() {
        let entry = SequenceOpEntry {
            op: SequenceOp::SetIncrement(2),
            destructiveness: Destructiveness::Safe,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: SequenceOpEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn set_data_type_serde_round_trip() {
        let entry = SequenceOpEntry {
            op: SequenceOp::SetDataType(ColumnType::BigInt),
            destructiveness: Destructiveness::RequiresApproval {
                reason: "may overflow current value".into(),
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: SequenceOpEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn set_owned_by_none_serde_round_trip() {
        let entry = SequenceOpEntry {
            op: SequenceOp::SetOwnedBy(None),
            destructiveness: Destructiveness::Safe,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: SequenceOpEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }
}
