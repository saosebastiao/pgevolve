//! `TableOp` — per-table column / constraint operations.
//!
//! Carried inside [`Change::AlterTable`](super::change::Change::AlterTable).

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::column::{Column, Generated, Identity};
use crate::ir::column_type::ColumnType;
use crate::ir::constraint::Constraint;
use crate::ir::default_expr::{DefaultExpr, NormalizedExpr};

use super::destructiveness::Destructiveness;

/// One table-level op paired with its destructiveness classification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableOpEntry {
    /// The table operation.
    pub op: TableOp,
    /// Risk classification.
    pub destructiveness: Destructiveness,
}

/// One column / constraint / comment operation on a table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TableOp {
    /// Add a column.
    AddColumn(Column),
    /// Drop a column.
    DropColumn {
        /// Column name.
        name: Identifier,
        /// Whether the column is on a populated table; the planner refines this
        /// from `pg_class.reltuples`. Differ leaves `false` if unknown.
        is_populated: bool,
    },
    /// Change a column's data type. Carries an optional `USING` clause; v0.1
    /// emits `None` and leaves the rewrite pass to decide.
    AlterColumnType {
        /// Column name.
        name: Identifier,
        /// Existing type in the target.
        from: ColumnType,
        /// Desired type in the source.
        to: ColumnType,
        /// Optional `USING` expression.
        using: Option<NormalizedExpr>,
    },
    /// Toggle a column's `NOT NULL`-ness.
    SetColumnNullable {
        /// Column name.
        name: Identifier,
        /// Target nullability (`true` = nullable, `false` = `NOT NULL`).
        nullable: bool,
    },
    /// Set or clear a column's `DEFAULT` expression.
    SetColumnDefault {
        /// Column name.
        name: Identifier,
        /// New default (`None` = drop the default).
        default: Option<DefaultExpr>,
    },
    /// Set or clear a column's identity specification.
    SetColumnIdentity {
        /// Column name.
        name: Identifier,
        /// New identity (`None` = drop identity).
        identity: Option<Identity>,
    },
    /// Set or clear a column's generated-column expression.
    SetColumnGenerated {
        /// Column name.
        name: Identifier,
        /// New generated spec (`None` = drop generated-ness).
        generated: Option<Generated>,
    },
    /// Set or clear a column-level comment.
    SetColumnComment {
        /// Column name.
        name: Identifier,
        /// New comment (`None` clears).
        comment: Option<String>,
    },

    /// Add a table constraint.
    AddConstraint(Constraint),
    /// Drop a table constraint by name.
    DropConstraint {
        /// Constraint name (no schema — constraint names live in the table's namespace).
        name: Identifier,
    },
    /// Set or clear a constraint comment.
    SetConstraintComment {
        /// Constraint name.
        name: Identifier,
        /// New comment.
        comment: Option<String>,
    },

    /// Set or clear a table-level comment.
    SetTableComment {
        /// New comment (`None` clears).
        comment: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn add_column_serde_round_trip() {
        let op = TableOp::AddColumn(Column {
            name: id("email"),
            ty: ColumnType::Text,
            nullable: true,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        });
        let entry = TableOpEntry {
            op,
            destructiveness: Destructiveness::Safe,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: TableOpEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn drop_column_serde_round_trip() {
        let entry = TableOpEntry {
            op: TableOp::DropColumn {
                name: id("email"),
                is_populated: false,
            },
            destructiveness: Destructiveness::RequiresApprovalAndDataLossWarning {
                reason: "drops column email".into(),
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: TableOpEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn alter_column_type_serde_round_trip() {
        let entry = TableOpEntry {
            op: TableOp::AlterColumnType {
                name: id("count"),
                from: ColumnType::Integer,
                to: ColumnType::BigInt,
                using: None,
            },
            destructiveness: Destructiveness::Safe,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: TableOpEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn set_table_comment_serde_round_trip() {
        let entry = TableOpEntry {
            op: TableOp::SetTableComment {
                comment: Some("the users table".into()),
            },
            destructiveness: Destructiveness::Safe,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: TableOpEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }
}
