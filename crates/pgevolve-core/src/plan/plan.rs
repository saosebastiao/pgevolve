// The module is named `plan` inside the `plan/` parent — the inception is
// intentional: this is *the* canonical `Plan` definition for the planner.
#![allow(clippy::module_inception)]

//! [`Plan`] — the canonical in-memory artifact produced by the planner.
//!
//! Spec §6.6. A `Plan` is a set of [`TransactionGroup`]s plus the auxiliary
//! data needed to round-trip to/from the on-disk three-file layout
//! (`plan.sql` + `intent.toml` + `manifest.toml`, spec §7).
//!
//! [`PlanId`] is a 32-byte BLAKE3 hash over a deterministic serialization of
//! (source catalog, target catalog, pgevolve version, planner ruleset version).
//! Identical inputs always produce the same id across runs and machines.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ir::catalog::Catalog;
use crate::plan::error::PlanError;
use crate::plan::grouping::TransactionGroup;

/// A `LintAtPlan` finding captured at plan time for apply-time replay.
///
/// Persisted in `manifest.toml` under `lint_at_plan_findings`. At apply time,
/// preflight checks that each recorded finding still has a matching
/// `[[lint_waiver]]` row, catching the case where a waiver is removed from
/// `intent.toml` between plan and apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedFinding {
    /// The lint rule ID that fired, e.g. `"column-position-drift"`.
    pub rule: String,
    /// The qualified target the finding pointed at, e.g. `"app.users"`.
    /// Extracted from the leading `"<qname>: …"` of the finding message.
    pub target: String,
    /// Full finding message, used for substring matching against waiver targets.
    pub message: String,
}

/// 32-byte plan identity. See module docs and [`PlanId::compute`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlanId(pub [u8; 32]);

impl PlanId {
    /// Deterministic identity hash over the planner's logical inputs.
    ///
    /// The hash payload is: a domain-separator string, the pgevolve version,
    /// the planner ruleset version, and JSON-serialized source and target
    /// catalogs. `serde_json` field ordering is determined by struct
    /// declaration order; all map-like containers in `Catalog` use `Vec` or
    /// `BTreeMap`, so serialization is deterministic — same value, same bytes —
    /// which is the property `PlanId` requires.
    ///
    /// # Errors
    ///
    /// Returns `PlanError::Internal` if either catalog cannot be serialized to
    /// JSON (this would indicate a bug in the `Catalog` type, not a user error).
    pub fn compute(
        source: &Catalog,
        target: &Catalog,
        pgevolve_version: &str,
        planner_ruleset_version: u32,
    ) -> Result<Self, PlanError> {
        let mut h = blake3::Hasher::new();
        h.update(b"pgevolve-plan-id-v1\n");
        h.update(pgevolve_version.as_bytes());
        h.update(&[0]);
        h.update(&planner_ruleset_version.to_be_bytes());
        h.update(&[0]);
        let source_bytes = serde_json::to_vec(source)
            .map_err(|e| PlanError::Internal(format!("Catalog serialization failed: {e}")))?;
        let target_bytes = serde_json::to_vec(target)
            .map_err(|e| PlanError::Internal(format!("Catalog serialization failed: {e}")))?;
        h.update(&source_bytes);
        h.update(&[0]);
        h.update(&target_bytes);
        Ok(Self(*h.finalize().as_bytes()))
    }

    /// First 8 bytes hex-encoded (16 chars) — used in human-facing places like
    /// directive headers and intent/manifest cross-references.
    pub fn short(&self) -> String {
        hex::encode(&self.0[..8])
    }

    /// Full 64-char lowercase hex string.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse a full 64-char lowercase hex string.
    pub fn from_full_hex(s: &str) -> Result<Self, InvalidPlanHash> {
        let bytes = hex::decode(s).map_err(|_| InvalidPlanHash(s.to_string()))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| InvalidPlanHash(s.to_string()))?;
        Ok(Self(arr))
    }

    /// Parse a short 16-char lowercase hex string (8 bytes) into a `PlanId`.
    ///
    /// Used by the cluster plan CLI, which derives an 8-byte plan id from a
    /// BLAKE3 hash of the cluster catalogs. The 8 decoded bytes occupy
    /// positions 0–7 of the internal 32-byte array; bytes 8–31 are zeroed.
    /// This guarantees that `plan_id.short() == s` after construction, which
    /// is the invariant that `write_plan_dir`/`read_plan_dir` rely on for
    /// the cross-file `plan_id` consistency check.
    ///
    /// # Errors
    ///
    /// Returns `PlanError::Internal` if `s` is not valid hex or not exactly
    /// 16 characters (8 bytes) long.
    pub fn from_hex(s: &str) -> Result<Self, PlanError> {
        let decoded = hex::decode(s).map_err(|e| {
            PlanError::Internal(format!("plan_id hex decode failed for {s:?}: {e}"))
        })?;
        if decoded.len() != 8 {
            return Err(PlanError::Internal(format!(
                "plan_id hex length mismatch: expected 16 chars (8 bytes), got {} chars ({} bytes)",
                s.len(),
                decoded.len(),
            )));
        }
        let mut arr = [0u8; 32];
        arr[..8].copy_from_slice(&decoded);
        Ok(Self(arr))
    }
}

impl std::fmt::Display for PlanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Error returned by [`PlanId::from_full_hex`] when the input is not a valid
/// 64-character lowercase hex string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid plan hash: {0}")]
pub struct InvalidPlanHash(pub String);

/// One `[[step_override]]` row in `intent.toml`.
///
/// Non-destructive per-step modifier — the user can suppress an
/// auto-emitted step (e.g., the REFRESH MATERIALIZED VIEW that follows
/// every CREATE MATERIALIZED VIEW).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StepOverride {
    /// `StepKind` wire-form tag (`snake_case`): `"refresh_materialized_view"`,
    /// `"create_view"`, etc.
    pub kind: String,
    /// Target qname (matches the step's primary target).
    pub target: String,
    /// When true, the executor skips the step entirely.
    #[serde(default)]
    pub suppress: bool,
}

/// One `[[lint_waiver]]` row in `intent.toml`. Acknowledges a `LintAtPlan`
/// finding so that `pgevolve plan` can proceed despite the detected drift.
///
/// Waivers are matched against findings by (`rule`, `target`). The `target`
/// must appear as a substring of the finding's message (findings always lead
/// with the qualified table name, e.g. `"app.users: column position drift…"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintWaiver {
    /// The lint rule ID being waived, e.g. `"column-position-drift"`.
    pub rule: String,
    /// The qualified target the finding pointed at, e.g. `"app.users"`.
    pub target: String,
    /// Free-text reason; surfaces in audit logs.
    pub reason: String,
}

/// One destructive intent — a step whose execution requires the user to flip
/// the `approved` flag in `intent.toml` before the executor will run it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestructiveIntent {
    /// 1-indexed intent id, unique within a plan.
    pub id: u32,
    /// Step number (1-indexed across the whole plan) that this intent gates.
    pub step: u32,
    /// Human kind name (e.g., `drop_column`). Same vocabulary as
    /// [`StepKind`](crate::plan::raw_step::StepKind) serialized.
    pub kind: String,
    /// Rendered target (e.g., `app.users.legacy_email`).
    pub target: String,
    /// Human-readable reason copied from the diff `Destructiveness`.
    pub reason: String,
    /// Whether the user has set `approved = true` in `intent.toml`.
    ///
    /// Populated by `read_plan_dir` / `read_intent_toml`. Defaults to `false`
    /// (every newly written `intent.toml` starts with `approved = false`).
    #[serde(default)]
    pub approved: bool,
}

/// Metadata produced alongside a `Plan` and embedded into `manifest.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanMetadata {
    /// pgevolve crate version string at plan time.
    pub pgevolve_version: String,
    /// Planner ruleset version (from `PlannerPolicy`) at plan time.
    pub planner_ruleset_version: u32,
    /// Optional source-tree revision identifier (e.g., `git:abc1234`).
    pub source_rev: Option<String>,
    /// Stable identifier for the target database
    /// (hash of `host/port/dbname/system_identifier`, computed by the apply path).
    pub target_identity: String,
    /// Catalog snapshot used as the diff pre-image; the executor uses it for
    /// drift detection at apply time.
    pub target_snapshot: Catalog,
    /// UTC timestamp when the plan was constructed.
    pub created_at: OffsetDateTime,
    /// `LintAtPlan` findings present at plan time. Populated by `pgevolve plan`
    /// whenever drift lints fire. Empty when no `LintAtPlan` findings exist.
    /// Used by apply-time preflight to detect waiver removal between plan and apply.
    pub lint_at_plan_findings: Vec<RecordedFinding>,
}

/// The canonical in-memory representation of a plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Deterministic identity hash; see [`PlanId::compute`].
    pub id: PlanId,
    /// Steps partitioned into transaction groups; each step's `step_no` and
    /// `intent_id` are filled in by [`Plan::from_grouped`].
    pub groups: Vec<TransactionGroup>,
    /// Destructive intents, one per destructive step, in step order.
    pub intents: Vec<DestructiveIntent>,
    /// Lint waivers loaded from `[[lint_waiver]]` rows in `intent.toml`.
    ///
    /// When `pgevolve plan` detects unwaived `LintAtPlan` findings, it prints
    /// an example `[[lint_waiver]]` row to stderr for the user to copy into
    /// `intent.toml`; the field is omitted from serialized output when empty
    /// (`skip_serializing_if = "Vec::is_empty"`). The field is populated when
    /// reading back a plan directory whose `intent.toml` already contains
    /// `[[lint_waiver]]` rows.
    pub lint_waivers: Vec<LintWaiver>,
    /// Step overrides loaded from `[[step_override]]` rows in `intent.toml`.
    ///
    /// Each row can suppress a specific auto-emitted step at apply time.
    /// The executor checks this list before running each step and skips
    /// (recording as `skipped` in the audit log) any step whose `kind`
    /// and primary `target` match an override with `suppress = true`.
    pub step_overrides: Vec<StepOverride>,
    /// Plan metadata.
    pub metadata: PlanMetadata,
    /// Advisory (Warning-severity) lint findings produced at plan time.
    ///
    /// These are informational — they never block plan construction — and are
    /// **not** persisted to disk. Callers (CLI, conformance tests) can inspect
    /// this field and print or assert on them as needed.
    ///
    /// Currently populated by the `pgevolve::api::build_plan` shim from
    /// [`crate::lint::check_changeset`] (changeset-level rules such as
    /// `storage-downgrade-not-retroactive` and
    /// `compression-change-not-retroactive`).
    pub advisory_findings: Vec<RecordedFinding>,
}

impl Plan {
    /// Assemble a `Plan` from a step-grouped output of the rewrite pass.
    ///
    /// Walks `groups` in order to:
    /// 1. Assign 1-indexed `step_no` to every step (continuous across groups).
    /// 2. Allocate a `DestructiveIntent` (and `intent_id`) for every
    ///    destructive step, in step order.
    /// 3. Compute the deterministic `PlanId` over `(source, target, version,
    ///    ruleset_version)`.
    ///
    /// `target_identity` is opaque to the planner — the executor binary
    /// computes it from `(host, port, dbname, system_identifier)` at apply time.
    ///
    /// # Errors
    ///
    /// Returns `PlanError::Internal` if the source or target catalog cannot be
    /// serialized to JSON for hashing (see [`PlanId::compute`]).
    /// Assemble a `Plan` from a step-grouped output of the rewrite pass,
    /// with a caller-supplied `PlanId`.
    ///
    /// Used by cluster apply, which hashes `ClusterCatalog` (not per-DB
    /// `Catalog`) for its plan id and so cannot use [`Plan::from_grouped`].
    ///
    /// `target_snapshot` is left empty — per-DB snapshots only serve the
    /// per-DB drift recheck, which cluster apply does not perform (see
    /// design doc §3.1).
    ///
    /// # Errors
    ///
    /// Currently infallible; the `Result` shape matches `from_grouped` for
    /// future-proofing.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::missing_const_for_fn)] // construct-only; never const-callable
    pub fn from_grouped_with_id(
        mut groups: Vec<TransactionGroup>,
        id: PlanId,
        target_identity: String,
        source_rev: Option<String>,
        pgevolve_version: &str,
        planner_ruleset_version: u32,
    ) -> Result<Self, PlanError> {
        let mut step_no: u32 = 0;
        let mut intent_no: u32 = 0;
        let mut intents: Vec<DestructiveIntent> = Vec::new();
        for group in &mut groups {
            for step in &mut group.steps {
                step_no += 1;
                step.step_no = step_no;
                if step.destructive {
                    intent_no += 1;
                    step.intent_id = Some(intent_no);
                    intents.push(DestructiveIntent {
                        id: intent_no,
                        step: step_no,
                        kind: kind_name(step.kind).to_string(),
                        target: render_targets(&step.targets),
                        reason: step
                            .destructive_reason
                            .clone()
                            .unwrap_or_else(|| "destructive".to_string()),
                        approved: false,
                    });
                }
            }
        }
        let metadata = PlanMetadata {
            pgevolve_version: pgevolve_version.to_string(),
            planner_ruleset_version,
            source_rev,
            target_identity,
            target_snapshot: Catalog::empty(),
            created_at: OffsetDateTime::now_utc(),
            lint_at_plan_findings: Vec::new(),
        };
        Ok(Self {
            id,
            groups,
            intents,
            lint_waivers: Vec::new(),
            step_overrides: Vec::new(),
            metadata,
            advisory_findings: Vec::new(),
        })
    }

    /// Assemble a `Plan` from a step-grouped output of the rewrite pass.
    ///
    /// Walks `groups` in order to:
    /// 1. Assign 1-indexed `step_no` to every step (continuous across groups).
    /// 2. Allocate a `DestructiveIntent` (and `intent_id`) for every
    ///    destructive step, in step order.
    /// 3. Compute the deterministic `PlanId` over `(source, target, version,
    ///    ruleset_version)`.
    ///
    /// `target_identity` is opaque to the planner — the executor binary
    /// computes it from `(host, port, dbname, system_identifier)` at apply time.
    ///
    /// # Errors
    ///
    /// Returns `PlanError::Internal` if the source or target catalog cannot be
    /// serialized to JSON for hashing (see [`PlanId::compute`]).
    #[allow(clippy::too_many_arguments)]
    pub fn from_grouped(
        groups: Vec<TransactionGroup>,
        source: &Catalog,
        target: &Catalog,
        target_identity: String,
        source_rev: Option<String>,
        pgevolve_version: &str,
        planner_ruleset_version: u32,
    ) -> Result<Self, PlanError> {
        let id = PlanId::compute(source, target, pgevolve_version, planner_ruleset_version)?;
        let mut plan = Self::from_grouped_with_id(
            groups,
            id,
            target_identity,
            source_rev,
            pgevolve_version,
            planner_ruleset_version,
        )?;
        plan.metadata.target_snapshot = target.clone();
        Ok(plan)
    }

    /// Mark every destructive intent as `approved = true`.
    ///
    /// Intended for test harnesses that build plans programmatically and
    /// want to bypass the `intent.toml`-based approval workflow. Production
    /// apply must continue to require explicit approval in `intent.toml`.
    pub fn approve_all_intents(&mut self) {
        for intent in &mut self.intents {
            intent.approved = true;
        }
    }
}

/// Human-readable kind name used in directive comments and intent rows.
///
/// The vocabulary matches [`crate::plan::StepKind`]'s `snake_case` serde encoding; this
/// `const fn` exists so callers do not pay for a serde round-trip.
#[allow(clippy::too_many_lines)] // One arm per StepKind variant — extraction would obscure intent.
pub const fn kind_name(k: crate::plan::raw_step::StepKind) -> &'static str {
    use crate::plan::raw_step::StepKind as K;
    match k {
        K::CreateSchema => "create_schema",
        K::DropSchema => "drop_schema",
        K::AlterSchemaComment => "alter_schema_comment",
        K::CreateTable => "create_table",
        K::DropTable => "drop_table",
        K::AlterTableSetComment => "alter_table_set_comment",
        K::AlterTableSetTablespace => "alter_table_set_tablespace",
        K::AddColumn => "add_column",
        K::DropColumn => "drop_column",
        K::AlterColumnType => "alter_column_type",
        K::SetColumnNullable => "set_column_nullable",
        K::SetColumnDefault => "set_column_default",
        K::SetColumnComment => "set_column_comment",
        K::SetColumnIdentity => "set_column_identity",
        K::SetColumnGenerated => "set_column_generated",
        K::SetColumnStorage => "set_column_storage",
        K::SetColumnCompression => "set_column_compression",
        K::AddConstraint => "add_constraint",
        K::AddConstraintNotValid => "add_constraint_not_valid",
        K::ValidateConstraint => "validate_constraint",
        K::DropConstraint => "drop_constraint",
        K::SetConstraintComment => "set_constraint_comment",
        K::CreateIndex => "create_index",
        K::CreateIndexConcurrent => "create_index_concurrent",
        K::DropIndex => "drop_index",
        K::DropIndexConcurrent => "drop_index_concurrent",
        K::CreateSequence => "create_sequence",
        K::DropSequence => "drop_sequence",
        K::AlterSequence => "alter_sequence",
        K::AddCheckForNotNull => "add_check_for_not_null",
        K::CreateView => "create_view",
        K::DropView => "drop_view",
        K::AlterViewSetCheckOption => "alter_view_set_check_option",
        K::CreateMaterializedView => "create_materialized_view",
        K::DropMaterializedView => "drop_materialized_view",
        K::RefreshMaterializedView => "refresh_materialized_view",
        K::AlterViewSetReloption => "alter_view_set_reloption",
        K::CommentOnView => "comment_on_view",
        K::CreateType => "create_type",
        K::DropType => "drop_type",
        K::AlterTypeAddValue => "alter_type_add_value",
        K::AlterTypeRenameValue => "alter_type_rename_value",
        K::AlterDomainAddConstraint => "alter_domain_add_constraint",
        K::AlterDomainDropConstraint => "alter_domain_drop_constraint",
        K::AlterDomainSetDefault => "alter_domain_set_default",
        K::AlterDomainSetNotNull => "alter_domain_set_not_null",
        K::AlterTypeAddAttribute => "alter_type_add_attribute",
        K::AlterTypeDropAttribute => "alter_type_drop_attribute",
        K::AlterTypeAlterAttributeType => "alter_type_alter_attribute_type",
        K::CommentOnType => "comment_on_type",
        K::CreateOrReplaceFunction => "create_or_replace_function",
        K::DropFunction => "drop_function",
        K::CommentOnFunction => "comment_on_function",
        K::CreateOrReplaceProcedure => "create_or_replace_procedure",
        K::DropProcedure => "drop_procedure",
        K::CommentOnProcedure => "comment_on_procedure",
        K::CreateExtension => "create_extension",
        K::DropExtension => "drop_extension",
        K::AlterExtensionUpdate => "alter_extension_update",
        K::CommentOnExtension => "comment_on_extension",
        K::CreateTrigger => "create_trigger",
        K::DropTrigger => "drop_trigger",
        K::CommentOnTrigger => "comment_on_trigger",
        K::AttachPartition => "attach_partition",
        K::DetachPartition => "detach_partition",
        K::CreateRole => "create_role",
        K::DropRole => "drop_role",
        K::AlterRole => "alter_role",
        K::GrantRoleMembership => "grant_role_membership",
        K::RevokeRoleMembership => "revoke_role_membership",
        K::CommentOnRole => "comment_on_role",
        K::CreateTablespace => "create_tablespace",
        K::DropTablespace => "drop_tablespace",
        K::AlterTablespaceOwner => "alter_tablespace_owner",
        K::SetTablespaceOptions => "set_tablespace_options",
        K::CommentOnTablespace => "comment_on_tablespace",
        K::AlterObjectOwner => "alter_object_owner",
        K::GrantObjectPrivilege => "grant_object_privilege",
        K::RevokeObjectPrivilege => "revoke_object_privilege",
        K::GrantColumnPrivilege => "grant_column_privilege",
        K::RevokeColumnPrivilege => "revoke_column_privilege",
        K::AlterDefaultPrivileges => "alter_default_privileges",
        K::CreatePolicy => "create_policy",
        K::DropPolicy => "drop_policy",
        K::AlterPolicy => "alter_policy",
        K::SetTableRowSecurity => "set_table_row_security",
        K::SetTableForceRowSecurity => "set_table_force_row_security",
        K::SetTableStorage => "set_table_storage",
        K::SetIndexStorage => "set_index_storage",
        K::SetMaterializedViewStorage => "set_materialized_view_storage",
        K::CreatePublication => "create_publication",
        K::DropPublication => "drop_publication",
        K::ReplacePublication => "replace_publication",
        K::AlterPublicationAddTable => "alter_publication_add_table",
        K::AlterPublicationDropTable => "alter_publication_drop_table",
        K::AlterPublicationSetTable => "alter_publication_set_table",
        K::AlterPublicationAddSchema => "alter_publication_add_schema",
        K::AlterPublicationDropSchema => "alter_publication_drop_schema",
        K::AlterPublicationSetPublish => "alter_publication_set_publish",
        K::AlterPublicationSetViaRoot => "alter_publication_set_via_root",
        K::CommentOnPublication => "comment_on_publication",
        K::CreateSubscription => "create_subscription",
        K::DropSubscription => "drop_subscription",
        K::AlterSubscriptionConnection => "alter_subscription_connection",
        K::AlterSubscriptionAddPublication => "alter_subscription_add_publication",
        K::AlterSubscriptionDropPublication => "alter_subscription_drop_publication",
        K::AlterSubscriptionSetOptions => "alter_subscription_set_options",
        K::CommentOnSubscription => "comment_on_subscription",
        K::CreateStatistic => "create_statistic",
        K::DropStatistic => "drop_statistic",
        K::ReplaceStatistic => "replace_statistic",
        K::AlterStatisticSetTarget => "alter_statistic_set_target",
        K::CommentOnStatistic => "comment_on_statistic",
        K::CreateCollation => "create_collation",
        K::DropCollation => "drop_collation",
        K::RenameCollation => "rename_collation",
        K::ReplaceCollation => "replace_collation",
        K::CommentOnCollation => "comment_on_collation",
        K::CreateEventTrigger => "create_event_trigger",
        K::DropEventTrigger => "drop_event_trigger",
        K::AlterEventTriggerEnable => "alter_event_trigger_enable",
        K::AlterEventTriggerOwner => "alter_event_trigger_owner",
        K::CommentOnEventTrigger => "comment_on_event_trigger",
        K::CreateAggregate => "create_aggregate",
        K::DropAggregate => "drop_aggregate",
        K::AlterAggregateOwner => "alter_aggregate_owner",
        K::CommentOnAggregate => "comment_on_aggregate",
        K::CreateCast => "create_cast",
        K::DropCast => "drop_cast",
        K::CommentOnCast => "comment_on_cast",
    }
}

/// Parse [`kind_name`]'s output back into [`crate::plan::StepKind`].
#[allow(clippy::too_many_lines)] // One arm per StepKind variant — extraction would obscure intent.
pub fn parse_kind_name(s: &str) -> Option<crate::plan::raw_step::StepKind> {
    use crate::plan::raw_step::StepKind as K;
    Some(match s {
        "create_schema" => K::CreateSchema,
        "drop_schema" => K::DropSchema,
        "alter_schema_comment" => K::AlterSchemaComment,
        "create_table" => K::CreateTable,
        "drop_table" => K::DropTable,
        "alter_table_set_comment" => K::AlterTableSetComment,
        "alter_table_set_tablespace" => K::AlterTableSetTablespace,
        "add_column" => K::AddColumn,
        "drop_column" => K::DropColumn,
        "alter_column_type" => K::AlterColumnType,
        "set_column_nullable" => K::SetColumnNullable,
        "set_column_default" => K::SetColumnDefault,
        "set_column_comment" => K::SetColumnComment,
        "set_column_identity" => K::SetColumnIdentity,
        "set_column_generated" => K::SetColumnGenerated,
        "set_column_storage" => K::SetColumnStorage,
        "set_column_compression" => K::SetColumnCompression,
        "add_constraint" => K::AddConstraint,
        "add_constraint_not_valid" => K::AddConstraintNotValid,
        "validate_constraint" => K::ValidateConstraint,
        "drop_constraint" => K::DropConstraint,
        "set_constraint_comment" => K::SetConstraintComment,
        "create_index" => K::CreateIndex,
        "create_index_concurrent" => K::CreateIndexConcurrent,
        "drop_index" => K::DropIndex,
        "drop_index_concurrent" => K::DropIndexConcurrent,
        "create_sequence" => K::CreateSequence,
        "drop_sequence" => K::DropSequence,
        "alter_sequence" => K::AlterSequence,
        "add_check_for_not_null" => K::AddCheckForNotNull,
        "create_view" => K::CreateView,
        "drop_view" => K::DropView,
        "alter_view_set_check_option" => K::AlterViewSetCheckOption,
        "create_materialized_view" => K::CreateMaterializedView,
        "drop_materialized_view" => K::DropMaterializedView,
        "refresh_materialized_view" => K::RefreshMaterializedView,
        "alter_view_set_reloption" => K::AlterViewSetReloption,
        "comment_on_view" => K::CommentOnView,
        "create_type" => K::CreateType,
        "drop_type" => K::DropType,
        "alter_type_add_value" => K::AlterTypeAddValue,
        "alter_type_rename_value" => K::AlterTypeRenameValue,
        "alter_domain_add_constraint" => K::AlterDomainAddConstraint,
        "alter_domain_drop_constraint" => K::AlterDomainDropConstraint,
        "alter_domain_set_default" => K::AlterDomainSetDefault,
        "alter_domain_set_not_null" => K::AlterDomainSetNotNull,
        "alter_type_add_attribute" => K::AlterTypeAddAttribute,
        "alter_type_drop_attribute" => K::AlterTypeDropAttribute,
        "alter_type_alter_attribute_type" => K::AlterTypeAlterAttributeType,
        "comment_on_type" => K::CommentOnType,
        "create_or_replace_function" => K::CreateOrReplaceFunction,
        "drop_function" => K::DropFunction,
        "comment_on_function" => K::CommentOnFunction,
        "create_or_replace_procedure" => K::CreateOrReplaceProcedure,
        "drop_procedure" => K::DropProcedure,
        "comment_on_procedure" => K::CommentOnProcedure,
        "create_extension" => K::CreateExtension,
        "drop_extension" => K::DropExtension,
        "alter_extension_update" => K::AlterExtensionUpdate,
        "comment_on_extension" => K::CommentOnExtension,
        "create_trigger" => K::CreateTrigger,
        "drop_trigger" => K::DropTrigger,
        "comment_on_trigger" => K::CommentOnTrigger,
        "attach_partition" => K::AttachPartition,
        "detach_partition" => K::DetachPartition,
        "create_role" => K::CreateRole,
        "drop_role" => K::DropRole,
        "alter_role" => K::AlterRole,
        "grant_role_membership" => K::GrantRoleMembership,
        "revoke_role_membership" => K::RevokeRoleMembership,
        "comment_on_role" => K::CommentOnRole,
        "create_tablespace" => K::CreateTablespace,
        "drop_tablespace" => K::DropTablespace,
        "alter_tablespace_owner" => K::AlterTablespaceOwner,
        "set_tablespace_options" => K::SetTablespaceOptions,
        "comment_on_tablespace" => K::CommentOnTablespace,
        "alter_object_owner" => K::AlterObjectOwner,
        "grant_object_privilege" => K::GrantObjectPrivilege,
        "revoke_object_privilege" => K::RevokeObjectPrivilege,
        "grant_column_privilege" => K::GrantColumnPrivilege,
        "revoke_column_privilege" => K::RevokeColumnPrivilege,
        "alter_default_privileges" => K::AlterDefaultPrivileges,
        "create_policy" => K::CreatePolicy,
        "drop_policy" => K::DropPolicy,
        "alter_policy" => K::AlterPolicy,
        "set_table_row_security" => K::SetTableRowSecurity,
        "set_table_force_row_security" => K::SetTableForceRowSecurity,
        "set_table_storage" => K::SetTableStorage,
        "set_index_storage" => K::SetIndexStorage,
        "set_materialized_view_storage" => K::SetMaterializedViewStorage,
        "create_publication" => K::CreatePublication,
        "drop_publication" => K::DropPublication,
        "replace_publication" => K::ReplacePublication,
        "alter_publication_add_table" => K::AlterPublicationAddTable,
        "alter_publication_drop_table" => K::AlterPublicationDropTable,
        "alter_publication_set_table" => K::AlterPublicationSetTable,
        "alter_publication_add_schema" => K::AlterPublicationAddSchema,
        "alter_publication_drop_schema" => K::AlterPublicationDropSchema,
        "alter_publication_set_publish" => K::AlterPublicationSetPublish,
        "alter_publication_set_via_root" => K::AlterPublicationSetViaRoot,
        "comment_on_publication" => K::CommentOnPublication,
        "create_subscription" => K::CreateSubscription,
        "drop_subscription" => K::DropSubscription,
        "alter_subscription_connection" => K::AlterSubscriptionConnection,
        "alter_subscription_add_publication" => K::AlterSubscriptionAddPublication,
        "alter_subscription_drop_publication" => K::AlterSubscriptionDropPublication,
        "alter_subscription_set_options" => K::AlterSubscriptionSetOptions,
        "comment_on_subscription" => K::CommentOnSubscription,
        "create_statistic" => K::CreateStatistic,
        "drop_statistic" => K::DropStatistic,
        "replace_statistic" => K::ReplaceStatistic,
        "alter_statistic_set_target" => K::AlterStatisticSetTarget,
        "comment_on_statistic" => K::CommentOnStatistic,
        "create_collation" => K::CreateCollation,
        "drop_collation" => K::DropCollation,
        "rename_collation" => K::RenameCollation,
        "replace_collation" => K::ReplaceCollation,
        "comment_on_collation" => K::CommentOnCollation,
        "create_event_trigger" => K::CreateEventTrigger,
        "drop_event_trigger" => K::DropEventTrigger,
        "alter_event_trigger_enable" => K::AlterEventTriggerEnable,
        "alter_event_trigger_owner" => K::AlterEventTriggerOwner,
        "comment_on_event_trigger" => K::CommentOnEventTrigger,
        "create_aggregate" => K::CreateAggregate,
        "drop_aggregate" => K::DropAggregate,
        "alter_aggregate_owner" => K::AlterAggregateOwner,
        "comment_on_aggregate" => K::CommentOnAggregate,
        "create_cast" => K::CreateCast,
        "drop_cast" => K::DropCast,
        "comment_on_cast" => K::CommentOnCast,
        _ => return None,
    })
}

/// Render a step's `targets` list as a comma-separated string of qnames.
fn render_targets(targets: &[crate::identifier::QualifiedName]) -> String {
    let mut s = String::new();
    for (i, t) in targets.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&t.render_sql());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::schema::Schema;

    fn id_id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn cat_with_schema(name: &str) -> Catalog {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id_id(name)));
        c
    }

    #[test]
    fn plan_id_is_deterministic_across_calls() {
        let s = cat_with_schema("app");
        let t = Catalog::empty();
        let a = PlanId::compute(&s, &t, "0.1.0", 1).unwrap();
        let b = PlanId::compute(&s, &t, "0.1.0", 1).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn plan_id_differs_when_target_differs() {
        let s = cat_with_schema("app");
        let a = PlanId::compute(&s, &Catalog::empty(), "0.1.0", 1).unwrap();
        let b = PlanId::compute(&s, &cat_with_schema("legacy"), "0.1.0", 1).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn plan_id_differs_when_version_differs() {
        let s = cat_with_schema("app");
        let t = Catalog::empty();
        let a = PlanId::compute(&s, &t, "0.1.0", 1).unwrap();
        let b = PlanId::compute(&s, &t, "0.2.0", 1).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn plan_id_differs_when_ruleset_differs() {
        let s = cat_with_schema("app");
        let t = Catalog::empty();
        let a = PlanId::compute(&s, &t, "0.1.0", 1).unwrap();
        let b = PlanId::compute(&s, &t, "0.1.0", 2).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn plan_id_short_is_sixteen_hex_chars() {
        let id = PlanId::compute(&Catalog::empty(), &Catalog::empty(), "0.1.0", 1).unwrap();
        let short = id.short();
        assert_eq!(short.len(), 16);
        assert!(short.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn plan_id_full_hex_round_trips() {
        let id = PlanId::compute(&Catalog::empty(), &Catalog::empty(), "0.1.0", 1).unwrap();
        let hex = id.to_hex();
        assert_eq!(hex.len(), 64);
        let back = PlanId::from_full_hex(&hex).unwrap();
        assert_eq!(id, back);
    }

    // ---- Plan::from_grouped (Task 7.2) ----

    use crate::identifier::QualifiedName;
    use crate::plan::grouping::TransactionGroup;
    use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id_id(schema), id_id(name))
    }

    fn step(kind: StepKind, destructive: bool, targets: Vec<QualifiedName>) -> RawStep {
        RawStep {
            step_no: 0,
            kind,
            destructive,
            destructive_reason: destructive.then(|| "test".to_string()),
            intent_id: None,
            targets,
            sql: String::new(),
            transactional: TransactionConstraint::InTransaction,
        }
    }

    fn group(id: u32, steps: Vec<RawStep>) -> TransactionGroup {
        TransactionGroup {
            id,
            transactional: true,
            steps,
        }
    }

    #[test]
    fn from_grouped_assigns_step_numbers_contiguously() {
        let groups = vec![
            group(
                1,
                vec![
                    step(StepKind::CreateSchema, false, vec![qn("app", "app")]),
                    step(StepKind::CreateTable, false, vec![qn("app", "users")]),
                ],
            ),
            group(
                2,
                vec![step(StepKind::DropColumn, true, vec![qn("app", "users")])],
            ),
        ];
        let plan = Plan::from_grouped(
            groups,
            &Catalog::empty(),
            &Catalog::empty(),
            "tid".into(),
            None,
            "0.1.0",
            1,
        )
        .unwrap();
        let nos: Vec<u32> = plan
            .groups
            .iter()
            .flat_map(|g| g.steps.iter().map(|s| s.step_no))
            .collect();
        assert_eq!(nos, vec![1, 2, 3]);
    }

    #[test]
    fn from_grouped_allocates_one_intent_per_destructive_step() {
        let groups = vec![group(
            1,
            vec![
                step(StepKind::CreateTable, false, vec![qn("app", "x")]),
                step(StepKind::DropColumn, true, vec![qn("app", "x")]),
                step(StepKind::DropTable, true, vec![qn("app", "y")]),
            ],
        )];
        let plan = Plan::from_grouped(
            groups,
            &Catalog::empty(),
            &Catalog::empty(),
            "tid".into(),
            None,
            "0.1.0",
            1,
        )
        .unwrap();
        assert_eq!(plan.intents.len(), 2);
        assert_eq!(plan.intents[0].id, 1);
        assert_eq!(plan.intents[0].step, 2);
        assert_eq!(plan.intents[0].kind, "drop_column");
        assert_eq!(plan.intents[1].id, 2);
        assert_eq!(plan.intents[1].step, 3);
        assert_eq!(plan.intents[1].kind, "drop_table");
        // The destructive steps carry back their intent ids.
        let intent_ids: Vec<Option<u32>> = plan
            .groups
            .iter()
            .flat_map(|g| g.steps.iter().map(|s| s.intent_id))
            .collect();
        assert_eq!(intent_ids, vec![None, Some(1), Some(2)]);
    }

    #[test]
    fn from_grouped_metadata_captures_target_snapshot() {
        let target = cat_with_schema("legacy");
        let plan = Plan::from_grouped(
            Vec::new(),
            &Catalog::empty(),
            &target,
            "tid".into(),
            Some("git:abc".into()),
            "0.1.0",
            1,
        )
        .unwrap();
        assert_eq!(plan.metadata.target_snapshot, target);
        assert_eq!(plan.metadata.source_rev.as_deref(), Some("git:abc"));
        assert_eq!(plan.metadata.target_identity, "tid");
    }

    #[test]
    fn kind_name_round_trips_via_parse() {
        for k in [
            StepKind::CreateSchema,
            StepKind::DropColumn,
            StepKind::CreateIndexConcurrent,
            StepKind::AddCheckForNotNull,
        ] {
            assert_eq!(parse_kind_name(kind_name(k)), Some(k));
        }
    }

    #[test]
    fn user_type_step_kinds_round_trip_through_kind_name() {
        for k in [
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
        ] {
            let n = kind_name(k);
            let parsed = parse_kind_name(n).unwrap();
            assert_eq!(parsed, k, "round-trip failed for {n}");
        }
    }

    #[test]
    fn routine_step_kinds_round_trip_through_kind_name() {
        for k in [
            StepKind::CreateOrReplaceFunction,
            StepKind::DropFunction,
            StepKind::CommentOnFunction,
            StepKind::CreateOrReplaceProcedure,
            StepKind::DropProcedure,
            StepKind::CommentOnProcedure,
        ] {
            let n = kind_name(k);
            let parsed = parse_kind_name(n).unwrap();
            assert_eq!(parsed, k, "round-trip failed for {n}");
        }
    }

    #[test]
    fn tablespace_step_kinds_round_trip_through_kind_name() {
        for k in [
            StepKind::CreateTablespace,
            StepKind::DropTablespace,
            StepKind::AlterTablespaceOwner,
            StepKind::SetTablespaceOptions,
            StepKind::CommentOnTablespace,
        ] {
            let n = kind_name(k);
            let parsed = parse_kind_name(n).unwrap();
            assert_eq!(parsed, k, "round-trip failed for {n}");
        }
    }

    #[test]
    fn plan_id_from_invalid_hex_errors() {
        assert!(PlanId::from_full_hex("not-hex").is_err());
        assert!(PlanId::from_full_hex(&"ab".repeat(10)).is_err()); // wrong length
    }

    // ---- StepOverride round-trip (Task 9) ----

    #[test]
    fn step_override_round_trips() {
        let override_ = StepOverride {
            kind: "refresh_materialized_view".to_string(),
            target: "app.daily_revenue".to_string(),
            suppress: true,
        };
        // Serialize a single StepOverride as TOML and confirm it parses back equal.
        let toml_text = toml::to_string(&override_).unwrap();
        let back: StepOverride = toml::from_str(&toml_text).unwrap();
        assert_eq!(back, override_);
    }

    #[test]
    fn step_override_suppress_defaults_to_false() {
        let toml_text = r#"kind = "refresh_materialized_view"
target = "app.daily_revenue"
"#;
        let back: StepOverride = toml::from_str(toml_text).unwrap();
        assert!(!back.suppress);
    }

    #[test]
    fn step_override_round_trips_inside_intent_doc() {
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct Doc {
            plan_id: String,
            #[serde(default, rename = "step_override")]
            step_overrides: Vec<StepOverride>,
        }

        let toml_text = r#"
plan_id = "abc1234567890abc"

[[step_override]]
kind = "refresh_materialized_view"
target = "app.daily_revenue"
suppress = true
"#;
        let doc: Doc = toml::from_str(toml_text).unwrap();
        assert_eq!(doc.step_overrides.len(), 1);
        assert_eq!(doc.step_overrides[0].kind, "refresh_materialized_view");
        assert_eq!(doc.step_overrides[0].target, "app.daily_revenue");
        assert!(doc.step_overrides[0].suppress);
    }

    // ---- LintWaiver round-trip (Task 8) ----

    #[test]
    fn lint_waiver_round_trips() {
        let waiver = LintWaiver {
            rule: "column-position-drift".to_string(),
            target: "app.users".to_string(),
            reason: "applied via rewrite-table; see PR #234".to_string(),
        };

        // Serialize a single waiver as TOML and confirm it parses back equal.
        let toml_text = toml::to_string(&waiver).unwrap();
        let back: LintWaiver = toml::from_str(&toml_text).unwrap();
        assert_eq!(back, waiver);
    }

    #[test]
    fn lint_waiver_round_trips_inside_intent_doc() {
        // The deserializer must accept the full intent.toml shape (including
        // [[intent]] rows) alongside [[lint_waiver]] rows. We use local structs
        // that mirror the real IntentDocDe shape. Declared before any `let`
        // statements to satisfy the `items_after_statements` lint.
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct IntentRow {
            id: u32,
            step: u32,
            kind: String,
            target: String,
            reason: String,
            #[serde(default)]
            approved: bool,
        }
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct Doc {
            plan_id: String,
            #[serde(default, rename = "intent")]
            intents: Vec<IntentRow>,
            #[serde(default, rename = "lint_waiver")]
            lint_waivers: Vec<LintWaiver>,
        }

        // Simulate the shape that intent.toml produces: a document with a
        // `plan_id` key and one or more `[[lint_waiver]]` array rows.
        let toml_text = r#"
plan_id = "abc1234567890abc"

[[intent]]
id = 1
step = 2
kind = "drop_table"
target = "app.legacy"
reason = "drop old table"
approved = false

[[lint_waiver]]
rule = "column-position-drift"
target = "app.users"
reason = "rewrite-table applied; PR #234"
"#;
        let doc: Doc = toml::from_str(toml_text).unwrap();
        assert_eq!(doc.lint_waivers.len(), 1);
        assert_eq!(doc.lint_waivers[0].rule, "column-position-drift");
        assert_eq!(doc.lint_waivers[0].target, "app.users");
    }

    // ---- Plan::from_grouped_with_id (Task 1) ----

    #[test]
    fn from_grouped_with_id_uses_provided_plan_id() {
        // PlanId::from_hex does not exist yet (deferred to Task 6).
        // Fallback: compute a known id via PlanId::compute and verify
        // round-trip equality through from_grouped_with_id.
        let pre_id =
            PlanId::compute(&Catalog::empty(), &Catalog::empty(), "0.0.0-test", 99).unwrap();
        let plan = Plan::from_grouped_with_id(
            vec![],
            pre_id,
            "test-cluster-id".into(),
            None,
            "0.0.0-test",
            99,
        )
        .expect("build empty cluster-style plan");
        assert_eq!(plan.id, pre_id);
        assert_eq!(plan.metadata.target_identity, "test-cluster-id");
        assert_eq!(plan.metadata.planner_ruleset_version, 99);
        assert!(plan.groups.is_empty());
        assert!(plan.intents.is_empty());
    }

    #[test]
    fn from_grouped_with_id_target_snapshot_is_empty() {
        // from_grouped_with_id should always leave target_snapshot empty;
        // only from_grouped populates it (for per-DB drift recheck).
        let pre_id =
            PlanId::compute(&Catalog::empty(), &Catalog::empty(), "0.0.0-test", 0).unwrap();
        let plan =
            Plan::from_grouped_with_id(vec![], pre_id, "cluster:abc".into(), None, "0.0.0-test", 0)
                .unwrap();
        assert_eq!(plan.metadata.target_snapshot, Catalog::empty());
    }

    #[test]
    fn from_grouped_still_populates_target_snapshot() {
        // Regression guard: from_grouped must still override target_snapshot
        // after delegating to from_grouped_with_id.
        let target = cat_with_schema("legacy");
        let plan = Plan::from_grouped(
            Vec::new(),
            &Catalog::empty(),
            &target,
            "tid".into(),
            None,
            "0.1.0",
            1,
        )
        .unwrap();
        assert_eq!(plan.metadata.target_snapshot, target);
    }

    #[test]
    fn approve_all_intents_flips_every_intent_to_approved() {
        let mut plan = sample_plan_with_two_unapproved_intents();
        assert!(!plan.intents[0].approved);
        assert!(!plan.intents[1].approved);
        plan.approve_all_intents();
        assert!(plan.intents[0].approved);
        assert!(plan.intents[1].approved);
    }

    fn sample_plan_with_two_unapproved_intents() -> Plan {
        Plan {
            id: PlanId::compute(&Catalog::empty(), &Catalog::empty(), "0.1.0", 1).unwrap(),
            groups: Vec::new(),
            intents: vec![
                DestructiveIntent {
                    id: 1,
                    step: 1,
                    kind: "drop_column".into(),
                    target: "app.users.legacy_email".into(),
                    reason: "test".into(),
                    approved: false,
                },
                DestructiveIntent {
                    id: 2,
                    step: 2,
                    kind: "drop_table".into(),
                    target: "app.old_users".into(),
                    reason: "test".into(),
                    approved: false,
                },
            ],
            lint_waivers: Vec::new(),
            step_overrides: Vec::new(),
            metadata: PlanMetadata {
                pgevolve_version: crate::VERSION.to_string(),
                planner_ruleset_version: 1,
                source_rev: None,
                target_identity: "test-identity".into(),
                target_snapshot: Catalog::empty(),
                created_at: OffsetDateTime::UNIX_EPOCH,
                lint_at_plan_findings: Vec::new(),
            },
            advisory_findings: Vec::new(),
        }
    }
}
