//! `AGGREGATE` IR — a schema-scoped object. Ordinary aggregates only
//! (state function + state type + optional final function / initcond).

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column_type::ColumnType;
use crate::ir::difference::Difference;
use crate::ir::eq::{Equiv, field_difference};

/// A `CREATE AGGREGATE` object. Identity is `(qname, arg_types)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Aggregate {
    /// Schema-qualified aggregate name.
    pub qname: QualifiedName,
    /// Aggregate argument types (part of identity; aggregates are overloadable).
    pub arg_types: Vec<ColumnType>,
    /// State type (`STYPE`).
    pub state_type: ColumnType,
    /// State transition function (`SFUNC`) — a managed function name.
    pub sfunc: QualifiedName,
    /// Optional final function (`FINALFUNC`).
    pub finalfunc: Option<QualifiedName>,
    /// Optional initial condition (`INITCOND`), as text.
    pub initcond: Option<String>,
    /// Lenient owner (`None` = unmanaged).
    pub owner: Option<Identifier>,
    /// Optional comment.
    pub comment: Option<String>,
}

impl Equiv for Aggregate {
    fn differences(&self, other: &Self) -> Vec<Difference> {
        // Field-completeness guard: the compiler errors if a field is added
        // without being handled below. Bindings are unused (read via `self`).
        let Self {
            qname: _,
            arg_types: _,
            state_type: _,
            sfunc: _,
            finalfunc: _,
            initcond: _,
            owner: _,
            comment: _,
        } = self;
        let mut out = Vec::new();
        out.extend(field_difference("qname", &self.qname, &other.qname));
        out.extend(field_difference(
            "arg_types",
            &format!("{:?}", self.arg_types),
            &format!("{:?}", other.arg_types),
        ));
        out.extend(field_difference(
            "state_type",
            &format!("{:?}", self.state_type),
            &format!("{:?}", other.state_type),
        ));
        out.extend(field_difference("sfunc", &self.sfunc, &other.sfunc));
        out.extend(field_difference(
            "finalfunc",
            &format!("{:?}", self.finalfunc),
            &format!("{:?}", other.finalfunc),
        ));
        out.extend(field_difference(
            "initcond",
            &format!("{:?}", self.initcond),
            &format!("{:?}", other.initcond),
        ));
        out.extend(field_difference(
            "owner",
            &format!("{:?}", self.owner),
            &format!("{:?}", other.owner),
        ));
        out.extend(field_difference(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn sample_aggregate() -> Aggregate {
        Aggregate {
            qname: qname("app", "my_sum"),
            arg_types: vec![ColumnType::Integer],
            state_type: ColumnType::BigInt,
            sfunc: qname("app", "my_sum_sfunc"),
            finalfunc: Some(qname("app", "my_sum_final")),
            initcond: Some("0".to_string()),
            owner: Some(id("app_owner")),
            comment: Some("A custom sum aggregate.".to_string()),
        }
    }

    #[test]
    fn aggregate_serde_round_trip() {
        let agg = sample_aggregate();
        let json = serde_json::to_string(&agg).unwrap();
        let back: Aggregate = serde_json::from_str(&json).unwrap();
        assert_eq!(agg, back);
    }
}
