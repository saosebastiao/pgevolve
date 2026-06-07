//! `AGGREGATE` IR — a schema-scoped object. Ordinary aggregates only
//! (state function + state type + optional final function / initcond).

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column_type::ColumnType;

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
