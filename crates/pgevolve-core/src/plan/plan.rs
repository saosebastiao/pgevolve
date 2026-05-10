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
use crate::plan::grouping::TransactionGroup;

/// 32-byte plan identity. See module docs and [`PlanId::compute`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlanId(pub [u8; 32]);

impl PlanId {
    /// Deterministic identity hash over the planner's logical inputs.
    ///
    /// The hash payload is: a domain-separator string, the pgevolve version,
    /// the planner ruleset version, and `bincode`-serialized source and target
    /// catalogs. Bincode's encoding is deterministic — same value, same bytes —
    /// which is the property `PlanId` requires.
    pub fn compute(
        source: &Catalog,
        target: &Catalog,
        pgevolve_version: &str,
        planner_ruleset_version: u32,
    ) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(b"pgevolve-plan-id-v1\n");
        h.update(pgevolve_version.as_bytes());
        h.update(&[0]);
        h.update(&planner_ruleset_version.to_be_bytes());
        h.update(&[0]);
        let cfg = bincode::config::standard();
        let source_bytes = bincode::serde::encode_to_vec(source, cfg)
            .expect("Catalog is bincode-serializable");
        let target_bytes = bincode::serde::encode_to_vec(target, cfg)
            .expect("Catalog is bincode-serializable");
        h.update(&source_bytes);
        h.update(&[0]);
        h.update(&target_bytes);
        Self(*h.finalize().as_bytes())
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
        let arr: [u8; 32] = bytes.try_into().map_err(|_| InvalidPlanHash(s.to_string()))?;
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
    /// Plan metadata.
    pub metadata: PlanMetadata,
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
        let a = PlanId::compute(&s, &t, "0.1.0", 1);
        let b = PlanId::compute(&s, &t, "0.1.0", 1);
        assert_eq!(a, b);
    }

    #[test]
    fn plan_id_differs_when_target_differs() {
        let s = cat_with_schema("app");
        let a = PlanId::compute(&s, &Catalog::empty(), "0.1.0", 1);
        let b = PlanId::compute(&s, &cat_with_schema("legacy"), "0.1.0", 1);
        assert_ne!(a, b);
    }

    #[test]
    fn plan_id_differs_when_version_differs() {
        let s = cat_with_schema("app");
        let t = Catalog::empty();
        let a = PlanId::compute(&s, &t, "0.1.0", 1);
        let b = PlanId::compute(&s, &t, "0.2.0", 1);
        assert_ne!(a, b);
    }

    #[test]
    fn plan_id_differs_when_ruleset_differs() {
        let s = cat_with_schema("app");
        let t = Catalog::empty();
        let a = PlanId::compute(&s, &t, "0.1.0", 1);
        let b = PlanId::compute(&s, &t, "0.1.0", 2);
        assert_ne!(a, b);
    }

    #[test]
    fn plan_id_short_is_sixteen_hex_chars() {
        let id = PlanId::compute(&Catalog::empty(), &Catalog::empty(), "0.1.0", 1);
        let short = id.short();
        assert_eq!(short.len(), 16);
        assert!(short.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn plan_id_full_hex_round_trips() {
        let id = PlanId::compute(&Catalog::empty(), &Catalog::empty(), "0.1.0", 1);
        let hex = id.to_hex();
        assert_eq!(hex.len(), 64);
        let back = PlanId::from_full_hex(&hex).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn plan_id_from_invalid_hex_errors() {
        assert!(PlanId::from_full_hex("not-hex").is_err());
        assert!(PlanId::from_full_hex(&"ab".repeat(10)).is_err()); // wrong length
    }
}
