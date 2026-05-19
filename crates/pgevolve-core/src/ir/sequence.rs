//! `Sequence` — a standalone or column-owned Postgres sequence.

use serde::{Deserialize, Serialize};

use crate::identifier::QualifiedName;
use crate::ir::column_type::ColumnType;
use crate::ir::eq::DiffMacro;

/// A Postgres sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct Sequence {
    /// Schema-qualified sequence name.
    pub qname: QualifiedName,
    /// Sequence data type (always one of `SmallInt`, `Integer`, `BigInt`).
    #[diff(via_debug)]
    pub data_type: ColumnType,
    /// Start value.
    pub start: i64,
    /// Increment.
    pub increment: i64,
    /// Min value (`None` = type's minimum).
    #[diff(via_debug)]
    pub min_value: Option<i64>,
    /// Max value (`None` = type's maximum).
    #[diff(via_debug)]
    pub max_value: Option<i64>,
    /// Cache size.
    pub cache: i64,
    /// Whether the sequence cycles.
    pub cycle: bool,
    /// Owning column, if any (e.g., from `SERIAL` / `IDENTITY`).
    #[diff(via_debug)]
    pub owned_by: Option<SequenceOwner>,
    /// Optional comment.
    #[diff(via_debug)]
    pub comment: Option<String>,
}

impl Sequence {
    /// Normalize `min_value` / `max_value` to `None` when they equal the
    /// PG-implied default for the sequence's type and direction. PG stores
    /// explicit bounds even when the source omits them; the catalog reader
    /// normalizes the same way, so source-built catalogs must too for
    /// round-trip equality.
    #[must_use]
    pub fn canonicalize(mut self) -> Self {
        let (default_min, default_max) = default_bounds(&self.data_type, self.increment);
        if self.min_value == Some(default_min) {
            self.min_value = None;
        }
        if self.max_value == Some(default_max) {
            self.max_value = None;
        }
        self
    }
}

fn default_bounds(ty: &ColumnType, increment: i64) -> (i64, i64) {
    let (ty_min, ty_max) = match ty {
        ColumnType::SmallInt => (i64::from(i16::MIN), i64::from(i16::MAX)),
        ColumnType::Integer => (i64::from(i32::MIN), i64::from(i32::MAX)),
        _ => (i64::MIN, i64::MAX),
    };
    if increment >= 0 {
        (1, ty_max)
    } else {
        (ty_min, -1)
    }
}

/// Identifies a column that owns this sequence (Postgres `OWNED BY`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceOwner {
    /// Owning table.
    pub table: QualifiedName,
    /// Owning column name.
    pub column: crate::identifier::Identifier,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::eq::Diff;

    fn s(name: &str) -> QualifiedName {
        QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted(name).unwrap(),
        )
    }

    fn base() -> Sequence {
        Sequence {
            qname: s("seq1"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cache: 1,
            cycle: false,
            owned_by: None,
            comment: None,
        }
    }

    #[test]
    fn sequences_equal_when_identical() {
        assert!(base().canonical_eq(&base()));
    }

    #[test]
    fn sequence_diff_reports_increment_change() {
        let mut other = base();
        other.increment = 2;
        let d = base().diff(&other);
        assert!(d.iter().any(|x| x.path == "increment"));
    }

    #[test]
    fn sequence_diff_reports_qname_change() {
        let mut other = base();
        other.qname = s("seq2");
        let d = base().diff(&other);
        assert!(d.iter().any(|x| x.path == "qname"));
    }
}
