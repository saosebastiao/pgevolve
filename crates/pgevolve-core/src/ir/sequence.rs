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
    /// Object owner. `None` = unmanaged (the differ ignores ownership).
    /// `Some(role)` = managed: diff emits `ALTER SEQUENCE ... OWNER TO role`.
    #[diff(via_debug)]
    pub owner: Option<crate::identifier::Identifier>,
    /// Grants on this object. Empty = no grants. Canonicalized.
    #[diff(via_debug)]
    pub grants: Vec<crate::ir::grant::Grant>,
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
            owner: None,
            grants: Vec::new(),
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

    #[test]
    fn owner_change_diffs() {
        let mut b = base();
        b.owner = Some(Identifier::from_unquoted("new_owner").unwrap());
        assert!(base().diff(&b).iter().any(|x| x.path == "owner"));
    }

    #[test]
    fn grants_change_diffs() {
        let mut b = base();
        b.grants.push(crate::ir::grant::Grant {
            grantee: crate::ir::grant::GrantTarget::Public,
            privilege: crate::ir::grant::Privilege::Usage,
            with_grant_option: false,
            columns: None,
        });
        assert!(base().diff(&b).iter().any(|x| x.path == "grants"));
    }
}
