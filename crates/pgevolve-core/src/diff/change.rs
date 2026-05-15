//! `Change` — one entry in a [`ChangeSet`](super::changeset::ChangeSet).
//!
//! A `Change` describes a structural difference between a target catalog and
//! a source catalog at the level of a top-level object (schema, table, index,
//! sequence). Per-column / per-constraint operations live inside the
//! [`AlterTable`](Change::AlterTable) variant as a list of
//! [`TableOpEntry`](super::table_op::TableOpEntry); per-field sequence updates
//! live inside [`AlterSequence`](Change::AlterSequence) as
//! [`SequenceOpEntry`](super::sequence_op::SequenceOpEntry).

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::index::Index;
use crate::ir::schema::Schema;
use crate::ir::sequence::Sequence;
use crate::ir::table::Table;

use super::destructiveness::Destructiveness;
use super::sequence_op::SequenceOpEntry;
use super::table_op::TableOpEntry;

/// A change paired with its destructiveness classification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeEntry {
    /// The structural change.
    pub change: Change,
    /// Risk classification for this change.
    pub destructiveness: Destructiveness,
}

/// One structural change between two catalogs.
///
/// `Change` is intentionally not boxed: the enum's footprint is dominated by
/// `CreateTable` / `CreateIndex` / `ReplaceIndex` payloads, but `ChangeSet`
/// only stores at most one of each per object, so `Vec` overhead is the
/// dominant cost and boxing would just add an extra allocation per entry.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Change {
    /// Add a schema.
    CreateSchema(Schema),
    /// Drop a schema by name.
    DropSchema(Identifier),
    /// Update schema metadata (only `comment` in v0.1).
    AlterSchema {
        /// Schema name.
        name: Identifier,
        /// New comment value (`None` clears the comment).
        comment: Option<String>,
    },

    /// Add a table.
    CreateTable(Table),
    /// Drop a table.
    DropTable {
        /// Table qname.
        qname: QualifiedName,
        /// Best-effort estimate of row count, populated by the planner from
        /// `pg_class.reltuples`. The differ leaves this `None`.
        row_count_estimate: Option<i64>,
    },
    /// Alter a table — column / constraint / comment operations.
    AlterTable {
        /// Target table qname.
        qname: QualifiedName,
        /// Per-table operations. Order is the planner's job; the differ emits
        /// these in arbitrary order.
        ops: Vec<TableOpEntry>,
    },

    /// Create an index.
    CreateIndex(Index),
    /// Drop an index.
    DropIndex(QualifiedName),
    /// Replace an index (DROP + CREATE) when a property change requires it.
    ReplaceIndex {
        /// The index as it exists in the target.
        from: Index,
        /// The index as it should exist in the source.
        to: Index,
    },

    /// Create a sequence.
    CreateSequence(Sequence),
    /// Drop a sequence.
    DropSequence(QualifiedName),
    /// Alter a sequence — per-field operations.
    AlterSequence {
        /// Target sequence qname.
        qname: QualifiedName,
        /// Per-sequence operations.
        ops: Vec<SequenceOpEntry>,
    },

    /// A constraint exists in the catalog but is `NOT VALID`
    /// (`pg_constraint.convalidated = false`). Planner emits
    /// `VALIDATE CONSTRAINT`. Non-destructive.
    ValidateConstraint {
        /// Qualified name of the table owning the constraint.
        table: QualifiedName,
        /// Constraint name.
        constraint: Identifier,
    },
    /// An index exists in the catalog but is `INVALID`
    /// (`pg_index.indisvalid = false`). Planner emits `DROP INDEX + CREATE
    /// INDEX` to rebuild it. Non-destructive.
    RecreateIndex {
        /// Qualified name of the invalid index.
        qname: QualifiedName,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn table_users() -> Table {
        Table {
            qname: qn("app", "users"),
            columns: vec![Column {
                name: id("id"),
                ty: ColumnType::BigInt,
                nullable: false,
                default: None,
                identity: None,
                generated: None,
                collation: None,
                comment: None,
            }],
            constraints: vec![],
            comment: None,
        }
    }

    #[test]
    fn create_table_entry_serde_round_trip() {
        let entry = ChangeEntry {
            change: Change::CreateTable(table_users()),
            destructiveness: Destructiveness::Safe,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: ChangeEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn drop_table_entry_serde_round_trip() {
        let entry = ChangeEntry {
            change: Change::DropTable {
                qname: qn("app", "users"),
                row_count_estimate: None,
            },
            destructiveness: Destructiveness::RequiresApprovalAndDataLossWarning {
                reason: "drops table app.users".into(),
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: ChangeEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn equal_create_table_changes_compare_equal() {
        let a = Change::CreateTable(table_users());
        let b = Change::CreateTable(table_users());
        assert_eq!(a, b);
    }
}
