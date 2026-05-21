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
use crate::ir::column_type::ColumnType;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::extension::Extension;
use crate::ir::function::{Function, NormalizedArgTypes};
use crate::ir::index::Index;
use crate::ir::procedure::Procedure;
use crate::ir::schema::Schema;
use crate::ir::sequence::Sequence;
use crate::ir::table::Table;
use crate::ir::user_type::{CompositeAttribute, DomainCheck, UserType};
use crate::ir::trigger::Trigger;
use crate::ir::view::{MaterializedView, View};

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

    /// A view-level change.
    View(ViewChange),
    /// A materialized-view-level change.
    Mv(MvChange),
    /// A user-defined type change (enum, domain, composite).
    UserType(UserTypeChange),
    /// A user-defined function change.
    Function(FunctionChange),
    /// A user-defined procedure change.
    Procedure(ProcedureChange),
    /// An extension change.
    Extension(ExtensionChange),
    /// A trigger change.
    Trigger(TriggerChange),
}

/// A structural change to a single user-defined type.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum UserTypeChange {
    /// Create a new user-defined type.
    Create(UserType),
    /// Drop a user-defined type by qualified name.
    Drop(QualifiedName),

    /// Add a new label to an existing enum type.
    EnumAddValue {
        /// Qualified name of the enum type.
        qname: QualifiedName,
        /// The new label string.
        value: String,
        /// If `Some`, the new value is placed immediately before this label.
        before: Option<String>,
        /// If `Some`, the new value is placed immediately after this label.
        after: Option<String>,
    },
    /// Rename an existing enum label.
    EnumRenameValue {
        /// Qualified name of the enum type.
        qname: QualifiedName,
        /// The existing label to rename.
        from: String,
        /// The new label name.
        to: String,
    },

    /// Add a CHECK constraint to a domain.
    DomainAddCheck {
        /// Qualified name of the domain type.
        qname: QualifiedName,
        /// The constraint to add.
        constraint: DomainCheck,
    },
    /// Drop a named CHECK constraint from a domain.
    DomainDropCheck {
        /// Qualified name of the domain type.
        qname: QualifiedName,
        /// The constraint name to drop.
        name: Identifier,
    },
    /// Set (or clear) the DEFAULT expression on a domain.
    DomainSetDefault {
        /// Qualified name of the domain type.
        qname: QualifiedName,
        /// New default expression (`None` clears the default).
        default: Option<NormalizedExpr>,
    },
    /// Toggle the `NOT NULL` constraint on a domain.
    DomainSetNotNull {
        /// Qualified name of the domain type.
        qname: QualifiedName,
        /// `true` means NOT NULL (i.e., `nullable = false`).
        not_null: bool,
    },

    /// Add a new attribute to a composite type.
    CompositeAddAttribute {
        /// Qualified name of the composite type.
        qname: QualifiedName,
        /// The attribute to add.
        attribute: CompositeAttribute,
    },
    /// Drop an attribute from a composite type.
    CompositeDropAttribute {
        /// Qualified name of the composite type.
        qname: QualifiedName,
        /// The attribute name to drop.
        name: Identifier,
    },
    /// Change the type of an existing composite attribute.
    CompositeAlterAttributeType {
        /// Qualified name of the composite type.
        qname: QualifiedName,
        /// The attribute name whose type is being changed.
        attribute: Identifier,
        /// The new column type.
        new_type: ColumnType,
    },

    /// Set (or clear) the `COMMENT ON TYPE` for this type.
    SetComment {
        /// Qualified name of the type.
        qname: QualifiedName,
        /// New comment (`None` clears the comment).
        comment: Option<String>,
    },

    /// Emitted when the requested change cannot be done in place via `ALTER`.
    ///
    /// T8's cascade walker appends `ReplaceBody` for any views/MVs that depend
    /// on the type so they are recreated after the type is rebuilt. T9's SQL
    /// emitter expands this single entry into `DROP TYPE … CASCADE` plus
    /// `CREATE TYPE`, then the recreated dependents follow in topological
    /// order. The planner never emits this entry as one raw SQL step.
    ReplaceWithCascade {
        /// The desired type (source).
        source: UserType,
        /// The existing type (catalog / live database).
        catalog: UserType,
    },
}

/// A structural change to a single view.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ViewChange {
    /// Create a new view.
    Create(View),
    /// Drop an existing view.
    Drop(QualifiedName),
    /// Replace the SELECT body of a view.
    ///
    /// `compatible` is `true` when Postgres's `CREATE OR REPLACE VIEW` rules
    /// are satisfied (same column names and types at the same indexes, with
    /// new columns only appended at the end). When `compatible` is `false`,
    /// the planner must `DROP` then re-`CREATE` the view (and rebuild its
    /// dependents).
    ReplaceBody {
        /// The view as it should exist (source SQL).
        source: View,
        /// The view as it currently exists (live catalog).
        catalog: View,
        /// Whether `CREATE OR REPLACE VIEW` can be used (`true`) or a
        /// `DROP` + `CREATE` cycle is required (`false`).
        compatible: bool,
    },
    /// Change `WITH (security_barrier = ..., security_invoker = ...)` reloptions.
    SetReloption {
        /// View qname.
        qname: QualifiedName,
        /// Desired `security_barrier` value (`None` clears the option).
        security_barrier: Option<bool>,
        /// Desired `security_invoker` value (`None` clears the option).
        security_invoker: Option<bool>,
    },
    /// Set (or clear) the view-level `COMMENT ON VIEW`.
    SetComment {
        /// View qname.
        qname: QualifiedName,
        /// New comment (`None` clears the comment).
        comment: Option<String>,
    },
    /// Set (or clear) a `COMMENT ON COLUMN view.col`.
    SetColumnComment {
        /// View qname.
        qname: QualifiedName,
        /// Column name.
        column: Identifier,
        /// New comment (`None` clears the comment).
        comment: Option<String>,
    },
}

/// A structural change to a single materialized view.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MvChange {
    /// Create a new materialized view.
    Create(MaterializedView),
    /// Drop an existing materialized view.
    ///
    /// Unlike `ViewChange::Drop`, this is classified as `Safe` because
    /// materialized views are derived data that can be recreated by refreshing.
    Drop(QualifiedName),
    /// Replace the SELECT body of a materialized view.
    ///
    /// Materialized views do not support `CREATE OR REPLACE`; the planner must
    /// always `DROP` then `CREATE` the MV (and rebuild any indexes on it).
    ReplaceBody {
        /// The MV as it should exist (source SQL).
        source: MaterializedView,
        /// The MV as it currently exists (live catalog).
        catalog: MaterializedView,
    },
    /// Set (or clear) the MV-level `COMMENT ON MATERIALIZED VIEW`.
    SetComment {
        /// MV qname.
        qname: QualifiedName,
        /// New comment (`None` clears the comment).
        comment: Option<String>,
    },
    /// Set (or clear) a `COMMENT ON COLUMN mv.col`.
    SetColumnComment {
        /// MV qname.
        qname: QualifiedName,
        /// Column name.
        column: Identifier,
        /// New comment (`None` clears the comment).
        comment: Option<String>,
    },
}

/// A structural change to a single user-defined function.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum FunctionChange {
    /// Create a new function.
    Create(Function),
    /// Drop an existing function by qualified name and normalized arg types.
    Drop {
        /// Qualified name of the function.
        qname: QualifiedName,
        /// Normalized argument types (IN/INOUT/VARIADIC only) used to
        /// disambiguate overloads in the DROP FUNCTION statement.
        args: NormalizedArgTypes,
    },
    /// Replace the function body and/or attributes using `CREATE OR REPLACE
    /// FUNCTION`. Only valid when [`function_can_or_replace`] returns `true`.
    CreateOrReplace(Function),
    /// The function's return type or language changed in a way that PG's
    /// `CREATE OR REPLACE FUNCTION` rejects. The planner must emit
    /// `DROP FUNCTION … CASCADE` followed by `CREATE FUNCTION`.
    ReplaceWithCascade {
        /// The desired function (source).
        source: Function,
        /// The existing function (catalog / live database).
        catalog: Function,
    },
    /// Set (or clear) `COMMENT ON FUNCTION`.
    SetComment {
        /// Qualified name of the function.
        qname: QualifiedName,
        /// Normalized argument types for disambiguation.
        args: NormalizedArgTypes,
        /// New comment (`None` clears the comment).
        comment: Option<String>,
    },
}

/// A structural change to a single user-defined procedure.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ProcedureChange {
    /// Create a new procedure.
    Create(Procedure),
    /// Drop an existing procedure by qualified name.
    Drop(QualifiedName),
    /// Replace the procedure body and/or attributes using `CREATE OR REPLACE
    /// PROCEDURE`.
    CreateOrReplace(Procedure),
    /// Set (or clear) `COMMENT ON PROCEDURE`.
    SetComment {
        /// Qualified name of the procedure.
        qname: QualifiedName,
        /// New comment (`None` clears the comment).
        comment: Option<String>,
    },
}

/// Change to one extension. Pair-by-name semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ExtensionChange {
    /// Install a new extension.
    Create(Extension),
    /// Drop an extension by name. Emits `DROP EXTENSION ... CASCADE`.
    Drop(Identifier),
    /// Bump extension version: `ALTER EXTENSION ... UPDATE TO 'v'`.
    AlterUpdate {
        /// Extension name.
        name: Identifier,
        /// New version to update to.
        to_version: String,
    },
    /// Schema-changing replace (DROP CASCADE + CREATE).
    ReplaceWithCascade(Extension),
    /// Change the `COMMENT ON EXTENSION` text.
    CommentOn {
        /// Extension name.
        name: Identifier,
        /// New comment value (`None` clears the comment).
        comment: Option<String>,
    },
}

/// Change to one trigger. Pair-by-qname semantics; any structural
/// difference emits `Replace`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TriggerChange {
    /// Install a new trigger.
    Create(Trigger),
    /// Drop a trigger by qname (needs the table for `DROP TRIGGER name ON table`).
    Drop {
        /// Qualified name of the trigger.
        qname: QualifiedName,
        /// Owning table (needed for `DROP TRIGGER name ON table`).
        table: QualifiedName,
    },
    /// Any structural change: drop + create.
    Replace(Trigger),
    /// Change the `COMMENT ON TRIGGER` text.
    CommentOn {
        /// Qualified name of the trigger.
        qname: QualifiedName,
        /// Owning table (needed for `COMMENT ON TRIGGER name ON table`).
        table: QualifiedName,
        /// New comment value (`None` clears the comment).
        comment: Option<String>,
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
