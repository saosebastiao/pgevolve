//! [`RawStep`] — the smallest unit of work the executor will attempt.
//!
//! After the rewrite pass, every step's SQL is fixed: the executor performs
//! no further transformation. `intent_id` is populated later, in the plan
//! serializer, once destructive intents have been collated.

use serde::{Deserialize, Serialize};

use crate::identifier::QualifiedName;

/// Whether a step can run inside a `BEGIN; ... COMMIT;` block.
///
/// Used by [`group_steps`](super::grouping::group_steps) to partition the
/// step list into transactional vs. non-transactional groups. `CONCURRENTLY`
/// index ops are the typical [`OutsideTransaction`](Self::OutsideTransaction)
/// case in v0.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionConstraint {
    /// May execute inside a `BEGIN ... COMMIT`.
    InTransaction,
    /// Must execute outside a transaction (e.g., `CREATE INDEX CONCURRENTLY`).
    OutsideTransaction,
}

/// What kind of operation a [`RawStep`] performs.
///
/// Serialized via `serde` as the `kind=` value in the plan's
/// `-- @pgevolve step ...` directive comments (spec §7.1). The
/// `snake_case` rename keeps the on-disk form stable across renames here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// `CREATE SCHEMA`.
    CreateSchema,
    /// `DROP SCHEMA`.
    DropSchema,
    /// `COMMENT ON SCHEMA`.
    AlterSchemaComment,

    /// `CREATE TABLE`.
    CreateTable,
    /// `DROP TABLE`.
    DropTable,
    /// `COMMENT ON TABLE`.
    AlterTableSetComment,

    /// `ALTER TABLE ... ADD COLUMN`.
    AddColumn,
    /// `ALTER TABLE ... DROP COLUMN`.
    DropColumn,
    /// `ALTER TABLE ... ALTER COLUMN ... TYPE`.
    AlterColumnType,
    /// `ALTER TABLE ... ALTER COLUMN ... SET/DROP NOT NULL`.
    SetColumnNullable,
    /// `ALTER TABLE ... ALTER COLUMN ... SET/DROP DEFAULT`.
    SetColumnDefault,
    /// `COMMENT ON COLUMN`.
    SetColumnComment,
    /// `ALTER TABLE ... ALTER COLUMN ... ADD/DROP IDENTITY`.
    SetColumnIdentity,
    /// `ALTER TABLE ... ALTER COLUMN ... SET/DROP EXPRESSION`.
    SetColumnGenerated,

    /// `ALTER TABLE ... ADD CONSTRAINT` (validated immediately).
    AddConstraint,
    /// `ALTER TABLE ... ADD CONSTRAINT ... NOT VALID`.
    AddConstraintNotValid,
    /// `ALTER TABLE ... VALIDATE CONSTRAINT`.
    ValidateConstraint,
    /// `ALTER TABLE ... DROP CONSTRAINT`.
    DropConstraint,
    /// `COMMENT ON CONSTRAINT`.
    SetConstraintComment,

    /// `CREATE INDEX`.
    CreateIndex,
    /// `CREATE INDEX CONCURRENTLY`.
    CreateIndexConcurrent,
    /// `DROP INDEX`.
    DropIndex,
    /// `DROP INDEX CONCURRENTLY`.
    DropIndexConcurrent,

    /// `CREATE SEQUENCE`.
    CreateSequence,
    /// `DROP SEQUENCE`.
    DropSequence,
    /// `ALTER SEQUENCE`.
    AlterSequence,

    /// Intermediate `ADD CONSTRAINT __pgevolve_chk_<col> CHECK (col IS NOT NULL) NOT VALID`
    /// step in the SET NOT NULL pattern (spec §6.5).
    AddCheckForNotNull,

    // --- v0.2 view / materialized-view step kinds ---
    /// `CREATE [OR REPLACE] VIEW`.
    CreateView,
    /// `DROP VIEW`.
    DropView,
    /// `CREATE MATERIALIZED VIEW ... WITH NO DATA`.
    CreateMaterializedView,
    /// `DROP MATERIALIZED VIEW`.
    DropMaterializedView,
    /// `REFRESH MATERIALIZED VIEW [CONCURRENTLY]`.
    RefreshMaterializedView,
    /// `ALTER VIEW ... SET (...)`.
    AlterViewSetReloption,
    /// `COMMENT ON VIEW / MATERIALIZED VIEW / COLUMN` for views and MVs.
    CommentOnView,

    // --- v0.2 user-defined type step kinds ---
    /// `CREATE TYPE` (enum, domain, or composite).
    CreateType,
    /// `DROP TYPE`.
    DropType,
    /// `ALTER TYPE … ADD VALUE`.
    AlterTypeAddValue,
    /// `ALTER TYPE … RENAME VALUE`.
    AlterTypeRenameValue,
    /// `ALTER DOMAIN … ADD CONSTRAINT … CHECK (…)`.
    AlterDomainAddConstraint,
    /// `ALTER DOMAIN … DROP CONSTRAINT`.
    AlterDomainDropConstraint,
    /// `ALTER DOMAIN … SET DEFAULT` / `DROP DEFAULT`.
    AlterDomainSetDefault,
    /// `ALTER DOMAIN … SET NOT NULL` / `DROP NOT NULL`.
    AlterDomainSetNotNull,
    /// `ALTER TYPE … ADD ATTRIBUTE`.
    AlterTypeAddAttribute,
    /// `ALTER TYPE … DROP ATTRIBUTE`.
    AlterTypeDropAttribute,
    /// `ALTER TYPE … ALTER ATTRIBUTE … TYPE`.
    AlterTypeAlterAttributeType,
    /// `COMMENT ON TYPE` / `COMMENT ON DOMAIN`.
    CommentOnType,

    // --- v0.2 function / procedure step kinds ---
    /// `CREATE OR REPLACE FUNCTION`.
    CreateOrReplaceFunction,
    /// `DROP FUNCTION`.
    DropFunction,
    /// `COMMENT ON FUNCTION`.
    CommentOnFunction,
    /// `CREATE OR REPLACE PROCEDURE`.
    CreateOrReplaceProcedure,
    /// `DROP PROCEDURE`.
    DropProcedure,
    /// `COMMENT ON PROCEDURE`.
    CommentOnProcedure,

    // --- v0.2 extension step kinds ---
    /// `CREATE EXTENSION [IF NOT EXISTS] name [WITH SCHEMA s] [VERSION 'v']`.
    CreateExtension,
    /// `DROP EXTENSION name CASCADE`. Destructive (intent required).
    DropExtension,
    /// `ALTER EXTENSION name UPDATE TO 'v'`.
    AlterExtensionUpdate,
    /// `COMMENT ON EXTENSION name IS '...'`.
    CommentOnExtension,

    // --- v0.2 trigger step kinds ---
    /// `CREATE [CONSTRAINT] TRIGGER name ... ON table ...`.
    CreateTrigger,
    /// `DROP TRIGGER name ON table`.
    DropTrigger,
    /// `COMMENT ON TRIGGER name ON table IS '...'`.
    CommentOnTrigger,

    // --- v0.2 partition step kinds ---
    /// `ALTER TABLE parent ATTACH PARTITION child FOR VALUES ...`.
    AttachPartition,
    /// `ALTER TABLE parent DETACH PARTITION child`.
    DetachPartition,
}

/// One unit of work the executor will attempt.
///
/// `step_no` and `intent_id` start at zero / `None` and are assigned later
/// by [`Plan::from_grouped`](crate::plan::Plan::from_grouped). The rewrite
/// pass (Phase 6) builds steps without that numbering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawStep {
    /// 1-indexed step number across the whole plan. `0` until assigned by
    /// `Plan::from_grouped`.
    pub step_no: u32,
    /// What kind of operation.
    pub kind: StepKind,
    /// Whether the step is destructive (requires explicit intent approval).
    pub destructive: bool,
    /// Human-readable reason for destructiveness, if any.
    pub destructive_reason: Option<String>,
    /// Intent id assigned by `Plan::from_grouped`; `None` until then.
    pub intent_id: Option<u32>,
    /// IR objects this step affects (used by directive comments).
    pub targets: Vec<QualifiedName>,
    /// Final SQL emitted to disk.
    pub sql: String,
    /// Whether the step can run inside a transaction.
    pub transactional: TransactionConstraint,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_kind_serializes_as_snake_case() {
        let s = serde_json::to_string(&StepKind::CreateIndexConcurrent).unwrap();
        assert_eq!(s, "\"create_index_concurrent\"");
    }

    #[test]
    fn step_kind_round_trips_through_serde() {
        for kind in [
            StepKind::CreateSchema,
            StepKind::DropSchema,
            StepKind::AlterSchemaComment,
            StepKind::CreateTable,
            StepKind::DropTable,
            StepKind::AlterTableSetComment,
            StepKind::AddColumn,
            StepKind::DropColumn,
            StepKind::AlterColumnType,
            StepKind::SetColumnNullable,
            StepKind::SetColumnDefault,
            StepKind::SetColumnComment,
            StepKind::SetColumnIdentity,
            StepKind::SetColumnGenerated,
            StepKind::AddConstraint,
            StepKind::AddConstraintNotValid,
            StepKind::ValidateConstraint,
            StepKind::DropConstraint,
            StepKind::SetConstraintComment,
            StepKind::CreateIndex,
            StepKind::CreateIndexConcurrent,
            StepKind::DropIndex,
            StepKind::DropIndexConcurrent,
            StepKind::CreateSequence,
            StepKind::DropSequence,
            StepKind::AlterSequence,
            StepKind::AddCheckForNotNull,
            StepKind::CreateView,
            StepKind::DropView,
            StepKind::CreateMaterializedView,
            StepKind::DropMaterializedView,
            StepKind::RefreshMaterializedView,
            StepKind::AlterViewSetReloption,
            StepKind::CommentOnView,
            StepKind::CreateType,
            StepKind::DropType,
            StepKind::AlterTypeAddValue,
            StepKind::AlterTypeRenameValue,
            StepKind::AlterDomainAddConstraint,
            StepKind::AlterDomainDropConstraint,
            StepKind::AlterDomainSetDefault,
            StepKind::AlterDomainSetNotNull,
            StepKind::AlterTypeAddAttribute,
            StepKind::AlterTypeDropAttribute,
            StepKind::AlterTypeAlterAttributeType,
            StepKind::CommentOnType,
            StepKind::CreateOrReplaceFunction,
            StepKind::DropFunction,
            StepKind::CommentOnFunction,
            StepKind::CreateOrReplaceProcedure,
            StepKind::DropProcedure,
            StepKind::CommentOnProcedure,
            StepKind::CreateExtension,
            StepKind::DropExtension,
            StepKind::AlterExtensionUpdate,
            StepKind::CommentOnExtension,
            StepKind::CreateTrigger,
            StepKind::DropTrigger,
            StepKind::CommentOnTrigger,
            StepKind::AttachPartition,
            StepKind::DetachPartition,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            let back: StepKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn transaction_constraint_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&TransactionConstraint::OutsideTransaction).unwrap(),
            "\"outside_transaction\"",
        );
    }
}
