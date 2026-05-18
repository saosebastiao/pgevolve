//! [`PlannerPolicy`] — feature switches that gate the rewrite pass.
//!
//! Spec §6.5: each rewrite is gated on a policy switch so per-environment
//! overrides plug in cleanly later. v0.1 ships two strategies:
//!
//! - [`Strategy::Online`] — apply every enabled rewrite (default).
//! - [`Strategy::Atomic`] — short-circuit every online switch to `false`,
//!   producing one in-transaction step per change with no online rewrites.
//!
//! Atomic mode is "single transaction, no rewrites." Use [`PlannerPolicy::is_online`]
//! to read the effective switch values; do not read [`OnlineRewrites`] directly,
//! because `Atomic` makes the individual switches inert.

/// Top-level rewrite strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Strategy {
    /// All operations run inside one transaction; no online rewrites apply.
    Atomic,
    /// Apply each enabled rewrite from [`OnlineRewrites`].
    Online,
}

/// Per-rewrite enable switches. Only consulted when [`Strategy::Online`].
//
// Each bool is an independent on/off switch addressing a distinct rewrite —
// they are not a hidden state machine, so `struct_excessive_bools` doesn't apply.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnlineRewrites {
    /// `CreateIndex` (non-unique, on existing table) → `CREATE INDEX CONCURRENTLY`.
    pub create_index_concurrent: bool,
    /// `AddConstraint(ForeignKey)` on existing table → `NOT VALID` + `VALIDATE`.
    pub fk_not_valid_then_validate: bool,
    /// `AddConstraint(Check)` on existing table → `NOT VALID` + `VALIDATE`.
    pub check_not_valid_then_validate: bool,
    /// `SetColumnNullable { nullable: false }` on a populated column →
    /// `ADD CHECK NOT VALID` + `VALIDATE` + `SET NOT NULL` + `DROP CONSTRAINT`.
    pub not_null_via_check_pattern: bool,
    /// Upgrade `REFRESH MATERIALIZED VIEW` to `REFRESH MATERIALIZED VIEW
    /// CONCURRENTLY` when the MV has at least one unique index. Emits a lint
    /// warning when the MV has no unique index. Default `true`.
    pub refresh_mv_concurrently: bool,
    /// Walk transitively-affected views and emit explicit DROP + CREATE steps
    /// for them instead of relying on `CASCADE`. When `false`, the planner
    /// errors (naming every affected view) if any change would cascade
    /// dependent recreations. Default `true`.
    pub view_drop_create_dependents: bool,
}

impl OnlineRewrites {
    /// All rewrites enabled — default.
    pub const fn all_enabled() -> Self {
        Self {
            create_index_concurrent: true,
            fk_not_valid_then_validate: true,
            check_not_valid_then_validate: true,
            not_null_via_check_pattern: true,
            refresh_mv_concurrently: true,
            view_drop_create_dependents: true,
        }
    }

    /// All rewrites disabled.
    pub const fn all_disabled() -> Self {
        Self {
            create_index_concurrent: false,
            fk_not_valid_then_validate: false,
            check_not_valid_then_validate: false,
            not_null_via_check_pattern: false,
            refresh_mv_concurrently: false,
            view_drop_create_dependents: false,
        }
    }
}

/// Top-level planner policy: strategy + per-rewrite switches + ruleset version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannerPolicy {
    /// Strategy. [`Strategy::Atomic`] forces every online switch to `false`.
    pub strategy: Strategy,
    /// Per-rewrite switches; honored iff `strategy == Online`.
    pub online: OnlineRewrites,
    /// Ruleset version included in the plan id hash (spec §6.6).
    pub planner_ruleset_version: u32,
}

impl PlannerPolicy {
    /// Is the `create_index_concurrent` rewrite effectively enabled?
    pub const fn create_index_concurrent(&self) -> bool {
        matches!(self.strategy, Strategy::Online) && self.online.create_index_concurrent
    }

    /// Is the FK `NOT VALID` + `VALIDATE` rewrite effectively enabled?
    pub const fn fk_not_valid_then_validate(&self) -> bool {
        matches!(self.strategy, Strategy::Online) && self.online.fk_not_valid_then_validate
    }

    /// Is the CHECK `NOT VALID` + `VALIDATE` rewrite effectively enabled?
    pub const fn check_not_valid_then_validate(&self) -> bool {
        matches!(self.strategy, Strategy::Online) && self.online.check_not_valid_then_validate
    }

    /// Is the `SET NOT NULL` via CHECK pattern effectively enabled?
    pub const fn not_null_via_check_pattern(&self) -> bool {
        matches!(self.strategy, Strategy::Online) && self.online.not_null_via_check_pattern
    }

    /// Is the `REFRESH MATERIALIZED VIEW CONCURRENTLY` upgrade effectively enabled?
    pub const fn refresh_mv_concurrently(&self) -> bool {
        matches!(self.strategy, Strategy::Online) && self.online.refresh_mv_concurrently
    }

    /// Is the dependent-view DROP + CREATE walk effectively enabled?
    ///
    /// When `false`, the planner errors when any change would force dependent
    /// view recreations (instead of walking and emitting them silently).
    pub const fn view_drop_create_dependents(&self) -> bool {
        // This switch is consulted even in atomic mode because it controls
        // error-vs-walk behavior (not a pure online-only optimization).
        self.online.view_drop_create_dependents
    }

    /// True iff the strategy is `Online` (i.e., online rewrites may run).
    pub const fn is_online(&self) -> bool {
        matches!(self.strategy, Strategy::Online)
    }
}

impl Default for PlannerPolicy {
    fn default() -> Self {
        Self {
            strategy: Strategy::Online,
            online: OnlineRewrites::all_enabled(),
            planner_ruleset_version: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_online_with_every_rewrite_enabled() {
        let p = PlannerPolicy::default();
        assert!(p.is_online());
        assert!(p.create_index_concurrent());
        assert!(p.fk_not_valid_then_validate());
        assert!(p.check_not_valid_then_validate());
        assert!(p.not_null_via_check_pattern());
        assert!(p.refresh_mv_concurrently());
        assert!(p.view_drop_create_dependents());
        assert_eq!(p.planner_ruleset_version, 1);
    }

    #[test]
    fn atomic_strategy_disables_every_rewrite_regardless_of_switches() {
        let p = PlannerPolicy {
            strategy: Strategy::Atomic,
            online: OnlineRewrites::all_enabled(),
            planner_ruleset_version: 1,
        };
        assert!(!p.is_online());
        assert!(!p.create_index_concurrent());
        assert!(!p.fk_not_valid_then_validate());
        assert!(!p.check_not_valid_then_validate());
        assert!(!p.not_null_via_check_pattern());
        assert!(!p.refresh_mv_concurrently());
        // view_drop_create_dependents is consulted regardless of strategy.
        assert!(p.view_drop_create_dependents());
    }

    #[test]
    fn online_strategy_respects_individual_switches() {
        let p = PlannerPolicy {
            strategy: Strategy::Online,
            online: OnlineRewrites {
                create_index_concurrent: false,
                fk_not_valid_then_validate: true,
                check_not_valid_then_validate: false,
                not_null_via_check_pattern: true,
                refresh_mv_concurrently: false,
                view_drop_create_dependents: true,
            },
            planner_ruleset_version: 1,
        };
        assert!(!p.create_index_concurrent());
        assert!(p.fk_not_valid_then_validate());
        assert!(!p.check_not_valid_then_validate());
        assert!(p.not_null_via_check_pattern());
        assert!(!p.refresh_mv_concurrently());
        assert!(p.view_drop_create_dependents());
    }

    #[test]
    fn online_with_all_disabled_disables_every_rewrite() {
        let p = PlannerPolicy {
            strategy: Strategy::Online,
            online: OnlineRewrites::all_disabled(),
            planner_ruleset_version: 1,
        };
        assert!(p.is_online());
        assert!(!p.create_index_concurrent());
        assert!(!p.fk_not_valid_then_validate());
        assert!(!p.check_not_valid_then_validate());
        assert!(!p.not_null_via_check_pattern());
        assert!(!p.refresh_mv_concurrently());
        assert!(!p.view_drop_create_dependents());
    }
}
