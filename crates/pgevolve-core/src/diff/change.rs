//! `Change` — one entry in a [`ChangeSet`](super::changeset::ChangeSet).
//!
//! A `Change` describes a structural difference between a target catalog and
//! a source catalog at the level of a top-level object (schema, table, index,
//! sequence). Per-column / per-constraint operations live inside the
//! [`AlterTable`](Change::AlterTable) variant as a list of
//! [`TableOpEntry`]; per-field sequence updates
//! live inside [`AlterSequence`](Change::AlterSequence) as
//! [`SequenceOpEntry`].

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::collation::Collation;
use crate::ir::column_type::ColumnType;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::default_privileges::DefaultPrivObjectType;
use crate::ir::extension::Extension;
use crate::ir::function::{Function, NormalizedArgTypes};
use crate::ir::grant::Grant;
use crate::ir::index::Index;
use crate::ir::partition::PartitionBounds;
use crate::ir::procedure::Procedure;
use crate::ir::schema::Schema;
use crate::ir::sequence::Sequence;
use crate::ir::table::Table;
use crate::ir::trigger::Trigger;
use crate::ir::user_type::{CompositeAttribute, DomainCheck, UserType};
use crate::ir::view::{MaterializedView, View};

use super::destructiveness::Destructiveness;
use super::owner_op::{AlterObjectOwner, OwnerObjectKind};
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
    /// `CREATE OR REPLACE VIEW … WITH [LOCAL|CASCADED] CHECK OPTION` or the
    /// inverse (set/unset check option on an existing view). PG has no direct
    /// `ALTER VIEW … SET CHECK OPTION`; pgevolve emits a full
    /// `CREATE OR REPLACE VIEW` carrying the new option.
    AlterViewSetCheckOption {
        /// Schema-qualified view name.
        qname: QualifiedName,
        /// The desired check option state in the source.
        /// `None` = source declares no check option (clear it).
        new_value: Option<crate::ir::view::CheckOption>,
    },
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
    /// A partition-membership change (ATTACH / DETACH PARTITION).
    Table(TableChange),
    /// Grant an object-level privilege on a grantable object (non-column).
    ///
    /// Emitted when a grant appears in source but not in the target catalog.
    GrantObjectPrivilege {
        /// Qualified name of the grantable object (schema, table, sequence,
        /// view, function, procedure, or type).
        qname: QualifiedName,
        /// Which kind of object this is (drives the SQL keyword in the renderer).
        kind: OwnerObjectKind,
        /// Argument signature for routines (e.g., `"(int, text)"`).
        /// Empty string for non-routine object kinds.
        #[serde(default)]
        signature: String,
        /// The full grant to apply.
        grant: Grant,
    },
    /// Revoke an object-level privilege from a grantable object (non-column).
    ///
    /// Only emitted for managed grantees (see [`super::grants::diff_grants`]).
    RevokeObjectPrivilege {
        /// Qualified name of the grantable object.
        qname: QualifiedName,
        /// Which kind of object this is.
        kind: OwnerObjectKind,
        /// Argument signature for routines (e.g., `"(int, text)"`).
        /// Empty string for non-routine object kinds.
        #[serde(default)]
        signature: String,
        /// The full grant to revoke.
        grant: Grant,
    },
    /// Grant a column-level privilege on a table, view, or materialized view.
    ///
    /// Emitted when the grant's `columns` field is `Some(_)`.
    GrantColumnPrivilege {
        /// Qualified name of the table / view / materialized view.
        qname: QualifiedName,
        /// The full grant (including the `columns` list).
        grant: Grant,
    },
    /// Revoke a column-level privilege from a table, view, or materialized view.
    ///
    /// Only emitted for managed grantees.
    RevokeColumnPrivilege {
        /// Qualified name of the table / view / materialized view.
        qname: QualifiedName,
        /// The full grant (including the `columns` list).
        grant: Grant,
    },
    /// Change the owner of a grantable object.
    ///
    /// Emitted when the source declares an owner (`owner: Some(_)`) and the
    /// target owner differs. When the source has `owner: None`, ownership is
    /// unmanaged and no change is emitted.
    AlterObjectOwner(AlterObjectOwner),
    /// Add or remove a default privilege for a `(FOR ROLE, IN SCHEMA?,
    /// object-type)` key.
    AlterDefaultPrivileges {
        /// `FOR ROLE x` — the grantor role.
        target_role: Identifier,
        /// `IN SCHEMA y` — scope. `None` = global.
        schema: Option<Identifier>,
        /// Object-type discriminant.
        object_type: DefaultPrivObjectType,
        /// `true` = GRANT step, `false` = REVOKE step.
        is_grant: bool,
        /// The grantee and privilege being adjusted.
        grant: Grant,
    },

    /// Create a new policy on the named table.
    CreatePolicy {
        /// Table the policy belongs to.
        table: QualifiedName,
        /// The policy to create.
        policy: crate::ir::policy::Policy,
    },
    /// Drop a policy from the named table.
    DropPolicy {
        /// Table the policy belongs to.
        table: QualifiedName,
        /// Name of the policy to drop.
        name: Identifier,
    },
    /// Alter a policy's roles / USING / WITH CHECK.
    ///
    /// Note: PG rejects `ALTER POLICY` when the command kind changes — the
    /// differ emits `DropPolicy` + `CreatePolicy` in that case instead.
    AlterPolicy {
        /// Table the policy belongs to.
        table: QualifiedName,
        /// The desired policy state.
        policy: crate::ir::policy::Policy,
    },
    /// Toggle a table's `ROW LEVEL SECURITY`.
    SetTableRowSecurity {
        /// Qualified name of the table.
        qname: QualifiedName,
        /// `true` = `ENABLE ROW LEVEL SECURITY`, `false` = `DISABLE`.
        enable: bool,
    },
    /// Toggle a table's `FORCE ROW LEVEL SECURITY`.
    SetTableForceRowSecurity {
        /// Qualified name of the table.
        qname: QualifiedName,
        /// `true` = `FORCE ROW LEVEL SECURITY`, `false` = `NO FORCE`.
        force: bool,
    },

    /// Set table storage reloptions. `options` carries the sparse delta — only
    /// the fields whose source value differs from the catalog.
    ///
    /// No `Reset*` variant: the lenient policy treats source `None` as "skip".
    SetTableStorage {
        /// Qualified name of the table.
        qname: QualifiedName,
        /// Sparse delta of options to apply.
        options: crate::ir::reloptions::TableStorageOptions,
    },
    /// Set index storage reloptions. Sparse delta.
    ///
    /// No `Reset*` variant: the lenient policy treats source `None` as "skip".
    SetIndexStorage {
        /// Qualified name of the index.
        qname: QualifiedName,
        /// Sparse delta of options to apply.
        options: crate::ir::reloptions::IndexStorageOptions,
    },
    /// Set materialized view storage reloptions. Sparse delta.
    ///
    /// No `Reset*` variant: the lenient policy treats source `None` as "skip".
    SetMaterializedViewStorage {
        /// Qualified name of the materialized view.
        qname: QualifiedName,
        /// Sparse delta of options to apply.
        options: crate::ir::reloptions::TableStorageOptions,
    },

    /// A nested change to a single publication. See [`PublicationChange`].
    Publication(PublicationChange),
    /// A nested change to a single subscription. See [`SubscriptionChange`].
    Subscription(SubscriptionChange),
    /// A nested change to a single statistic. See [`StatisticChange`].
    Statistic(StatisticChange),
    /// A nested change to a single collation. See [`CollationChange`].
    Collation(CollationChange),

    /// A change that cannot be performed in-place.
    ///
    /// Emitted by the differ when it detects a structural difference that has
    /// no safe automatic migration path (e.g., changing a table's `PARTITION BY`
    /// clause). The ordering phase converts this into a [`PlanError`] so the
    /// plan never reaches execution.
    ///
    /// [`PlanError`]: crate::plan::error::PlanError
    UnsupportedDiff {
        /// Human-readable explanation of what changed and why it cannot be
        /// performed in-place.
        reason: String,
    },
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
    /// FUNCTION`. Only valid when `function_can_or_replace` returns `true`.
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

/// A partition-membership change on a child table.
///
/// These changes are emitted by the differ when the `partition_of` field of a
/// table changes. Wiring to SQL steps lands in PART9.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TableChange {
    /// Attach a child table to a parent partitioned table.
    ///
    /// Corresponds to `ALTER TABLE <parent> ATTACH PARTITION <child> <bounds>`.
    AttachPartition {
        /// The partitioned parent table.
        parent: QualifiedName,
        /// The child table being attached.
        child: QualifiedName,
        /// The partition bounds clause.
        bounds: PartitionBounds,
    },
    /// Detach a child table from a parent partitioned table.
    ///
    /// Corresponds to `ALTER TABLE <parent> DETACH PARTITION <child>`.
    DetachPartition {
        /// The partitioned parent table.
        parent: QualifiedName,
        /// The child table being detached.
        child: QualifiedName,
    },
}

/// A structural change to a single publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PublicationChange {
    /// `CREATE PUBLICATION ...`
    Create(crate::ir::publication::Publication),
    /// `DROP PUBLICATION ...` — destructive.
    Drop {
        /// Publication name.
        name: Identifier,
    },
    /// `DROP PUBLICATION old; CREATE PUBLICATION new;` — destructive; used
    /// when the publication's scope mode switches (`AllTables` ↔ `Selective`).
    Replace {
        /// The publication as it exists in the target.
        from: crate::ir::publication::Publication,
        /// The publication as it should exist in the source.
        to: crate::ir::publication::Publication,
    },
    /// `ALTER PUBLICATION p ADD TABLE x [(cols)] [WHERE (filter)]`
    AddTable {
        /// Publication name.
        publication: Identifier,
        /// The table entry to add.
        table: crate::ir::publication::PublishedTable,
    },
    /// `ALTER PUBLICATION p DROP TABLE x`
    DropTable {
        /// Publication name.
        publication: Identifier,
        /// Qualified name of the table to drop.
        qname: QualifiedName,
    },
    /// `ALTER PUBLICATION p SET TABLE x (cols) WHERE (filter)`
    SetTable {
        /// Publication name.
        publication: Identifier,
        /// The desired table entry state.
        table: crate::ir::publication::PublishedTable,
    },
    /// `ALTER PUBLICATION p ADD TABLES IN SCHEMA s` (PG15+)
    AddSchema {
        /// Publication name.
        publication: Identifier,
        /// Schema to add.
        schema: Identifier,
    },
    /// `ALTER PUBLICATION p DROP TABLES IN SCHEMA s` (PG15+)
    DropSchema {
        /// Publication name.
        publication: Identifier,
        /// Schema to drop.
        schema: Identifier,
    },
    /// `ALTER PUBLICATION p SET (publish = '...')`
    SetPublish {
        /// Publication name.
        publication: Identifier,
        /// Desired publish-kinds bitset.
        kinds: crate::ir::publication::PublishKinds,
    },
    /// `ALTER PUBLICATION p SET (publish_via_partition_root = ...)`
    SetViaRoot {
        /// Publication name.
        publication: Identifier,
        /// Desired value.
        value: bool,
    },
    /// `COMMENT ON PUBLICATION p IS '...'`
    CommentOn {
        /// Publication name.
        name: Identifier,
        /// New comment value (`None` clears the comment).
        comment: Option<String>,
    },
}

/// A structural change to a single subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SubscriptionChange {
    /// `CREATE SUBSCRIPTION ...`
    Create(crate::ir::subscription::Subscription),
    /// `DROP SUBSCRIPTION ...` — destructive.
    Drop {
        /// Subscription name.
        name: Identifier,
    },
    /// `ALTER SUBSCRIPTION s CONNECTION '...'`
    AlterConnection {
        /// Subscription name.
        name: Identifier,
        /// New connection string (may contain `${VAR}` placeholders).
        new_connection: String,
    },
    /// `ALTER SUBSCRIPTION s ADD PUBLICATION p`
    AddPublication {
        /// Subscription name.
        name: Identifier,
        /// Publication to add.
        publication: Identifier,
    },
    /// `ALTER SUBSCRIPTION s DROP PUBLICATION p`
    DropPublication {
        /// Subscription name.
        name: Identifier,
        /// Publication to drop.
        publication: Identifier,
    },
    /// `ALTER SUBSCRIPTION s SET (option = value, ...)` — sparse-delta.
    ///
    /// `create_slot` and `copy_data` are NEVER included (CREATE-only PG options).
    SetOptions {
        /// Subscription name.
        name: Identifier,
        /// Sparse options delta — only changed fields are `Some`.
        options: crate::ir::subscription::SubscriptionOptions,
    },
    /// `COMMENT ON SUBSCRIPTION s IS '...'`
    CommentOn {
        /// Subscription name.
        name: Identifier,
        /// New comment value (`None` clears the comment).
        comment: Option<String>,
    },
}

/// A structural change to a single statistic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum StatisticChange {
    /// `CREATE STATISTICS ...`
    Create(crate::ir::statistic::Statistic),
    /// `DROP STATISTICS ...` — destructive.
    Drop {
        /// Schema-qualified statistic name.
        qname: QualifiedName,
    },
    /// `DROP STATISTICS old; CREATE STATISTICS new;` — destructive; used
    /// when columns / kinds / target table differ (PG has no in-place ALTER
    /// for those fields).
    Replace {
        /// The statistic as it exists in the target.
        from: crate::ir::statistic::Statistic,
        /// The statistic as it should exist in the source.
        to: crate::ir::statistic::Statistic,
    },
    /// `ALTER STATISTICS s SET STATISTICS n` — analyze target.
    AlterSetTarget {
        /// Schema-qualified statistic name.
        qname: QualifiedName,
        /// New statistics target value.
        value: i32,
    },
    /// `COMMENT ON STATISTICS s IS '...'`
    CommentOn {
        /// Schema-qualified statistic name.
        qname: QualifiedName,
        /// New comment value (`None` clears the comment).
        comment: Option<String>,
    },
}

/// A structural change to a single collation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CollationChange {
    /// `CREATE COLLATION ...`
    Create(Collation),
    /// `DROP COLLATION qname` — destructive.
    ///
    /// Emitted by [`diff_schemas`][crate::diff::schemas::diff_schemas] when
    /// dropping a schema that contains collations: PG error 2BP01 fires if
    /// `DROP SCHEMA X` executes while collations in X are still live.
    ///
    /// Not emitted by `diff_collations` directly — collations are lenient
    /// there (no auto-drop for target-only collations; unmanaged drift
    /// surfaces via the `unmanaged-collation` lint instead).
    Drop {
        /// Schema-qualified collation name.
        qname: QualifiedName,
    },
    /// `ALTER COLLATION qname RENAME TO new_name`.
    ///
    /// Not emitted by the differ: rename intent is not structurally derivable
    /// from name-mismatched-but-same-shape collations (the user could equally
    /// have meant drop-old + create-new). This variant exists for the parser
    /// and future explicit-rename use cases.
    Rename {
        /// Existing qname.
        from: QualifiedName,
        /// New unqualified name (same schema).
        to: Identifier,
    },
    /// `DROP COLLATION old; CREATE COLLATION new;` — PG has no in-place
    /// ALTER for provider / locale / deterministic.
    Replace {
        /// The collation as it exists in the target.
        from: Collation,
        /// The collation as it should exist in the source.
        to: Collation,
    },
    /// `COMMENT ON COLLATION qname IS '...'`.
    CommentOn {
        /// Schema-qualified collation name.
        qname: QualifiedName,
        /// New comment (`None` clears).
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
                storage: None,
                compression: None,
                comment: None,
            }],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
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
