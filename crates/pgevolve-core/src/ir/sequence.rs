//! `Sequence` — a standalone or column-owned Postgres sequence.

use serde::{Deserialize, Serialize};

use crate::identifier::QualifiedName;
use crate::ir::column_type::ColumnType;
use crate::ir::difference::Difference;
use crate::ir::eq::{Equiv, field_difference};

/// A Postgres sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sequence {
    /// Schema-qualified sequence name.
    pub qname: QualifiedName,
    /// Sequence data type (always one of `SmallInt`, `Integer`, `BigInt`).
    pub data_type: ColumnType,
    /// Start value.
    pub start: i64,
    /// Increment.
    pub increment: i64,
    /// Min value (`None` = type's minimum).
    pub min_value: Option<i64>,
    /// Max value (`None` = type's maximum).
    pub max_value: Option<i64>,
    /// Cache size.
    pub cache: i64,
    /// Whether the sequence cycles.
    pub cycle: bool,
    /// Owning column, if any (e.g., from `SERIAL` / `IDENTITY`).
    pub owned_by: Option<SequenceOwner>,
    /// Optional comment.
    pub comment: Option<String>,
    /// Object owner. `None` = unmanaged (the differ ignores ownership).
    /// `Some(role)` = managed: diff emits `ALTER SEQUENCE ... OWNER TO role`.
    pub owner: Option<crate::identifier::Identifier>,
    /// Grants on this object. Empty = no grants. Canonicalized.
    pub grants: Vec<crate::ir::grant::Grant>,
}

impl Equiv for Sequence {
    fn differences(&self, other: &Self) -> Vec<Difference> {
        let Self {
            qname: _,
            data_type: _,
            start: _,
            increment: _,
            min_value: _,
            max_value: _,
            cache: _,
            cycle: _,
            owned_by: _,
            comment: _,
            owner: _,
            grants: _,
        } = self;
        let mut out = Vec::new();
        out.extend(field_difference("qname", &self.qname, &other.qname));
        out.extend(field_difference(
            "data_type",
            &format!("{:?}", self.data_type),
            &format!("{:?}", other.data_type),
        ));
        out.extend(field_difference("start", &self.start, &other.start));
        out.extend(field_difference(
            "increment",
            &self.increment,
            &other.increment,
        ));
        out.extend(field_difference(
            "min_value",
            &format!("{:?}", self.min_value),
            &format!("{:?}", other.min_value),
        ));
        out.extend(field_difference(
            "max_value",
            &format!("{:?}", self.max_value),
            &format!("{:?}", other.max_value),
        ));
        out.extend(field_difference("cache", &self.cache, &other.cache));
        out.extend(field_difference("cycle", &self.cycle, &other.cycle));
        out.extend(field_difference(
            "owned_by",
            &format!("{:?}", self.owned_by),
            &format!("{:?}", other.owned_by),
        ));
        out.extend(field_difference(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out.extend(field_difference(
            "owner",
            &format!("{:?}", self.owner),
            &format!("{:?}", other.owner),
        ));
        out.extend(field_difference(
            "grants",
            &format!("{:?}", self.grants),
            &format!("{:?}", other.grants),
        ));
        out
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
    use crate::ir::eq::Equiv;

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
        let d = base().differences(&other);
        assert!(d.iter().any(|x| x.path == "increment"));
    }

    #[test]
    fn sequence_diff_reports_qname_change() {
        let mut other = base();
        other.qname = s("seq2");
        let d = base().differences(&other);
        assert!(d.iter().any(|x| x.path == "qname"));
    }

    #[test]
    fn owner_change_diffs() {
        let mut b = base();
        b.owner = Some(Identifier::from_unquoted("new_owner").unwrap());
        assert!(base().differences(&b).iter().any(|x| x.path == "owner"));
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
        assert!(base().differences(&b).iter().any(|x| x.path == "grants"));
    }
}
