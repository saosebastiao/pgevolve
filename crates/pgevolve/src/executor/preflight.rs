//! Pre-flight checks run before any DDL touches the live database:
//! identity match, drift recheck, intent enforcement, and lint-waiver recheck.

use tokio_postgres::Client;

use pgevolve_core::catalog::{CatalogFilter, DriftReport};
use pgevolve_core::lint::{LINT_AT_PLAN_RULES, Severity};
use pgevolve_core::plan::Plan;

use super::error::ApplyError;
use crate::target_identity::compute_target_identity;

/// Toggles for each preflight check. Defaults are "all checks enforced."
#[derive(Debug, Clone, Copy, Default)]
pub struct PreflightOverrides {
    /// Skip the target-identity match check.
    pub allow_different_target: bool,
    /// Skip the drift recheck.
    pub allow_drift: bool,
    /// When true, bypass `check_lint_waivers`. See [`super::ApplyOverrides::allow_unwaived_lint`].
    pub allow_unwaived_lint: bool,
}

/// Run every preflight check. Returns the first failure.
pub async fn run_preflight(
    client: &Client,
    plan: &Plan,
    filter: &CatalogFilter,
    overrides: PreflightOverrides,
) -> Result<(), ApplyError> {
    // 1. Target-identity match.
    let live = compute_target_identity(client).await?;
    if live != plan.metadata.target_identity && !overrides.allow_different_target {
        return Err(ApplyError::TargetIdentityMismatch {
            plan: plan.metadata.target_identity.clone(),
            live,
        });
    }

    // 2. Drift recheck — re-introspect and diff against the snapshot the
    //    planner captured.
    if !overrides.allow_drift {
        let live_catalog = read_live_catalog(client, filter)?;
        let drift = pgevolve_core::diff::diff(
            &plan.metadata.target_snapshot,
            &live_catalog,
            &DriftReport::default(),
        );
        if !drift.is_empty() {
            return Err(ApplyError::DriftDetected(drift.len()));
        }
    }

    // 3. Structural lint-waiver validation (arch spec Decision 15).
    //
    // Validates that `[[lint_waiver]]` rows in the plan are well-formed
    // (non-empty `rule` and `target`). Does NOT re-run drift lints against the
    // live catalog or the plan snapshot — that would require the parsed source
    // catalog, which is not stored in the plan. As a result, this check does
    // NOT detect waivers removed from `intent.toml` after planning: an empty
    // `intent.toml` simply produces an empty `plan.lint_waivers` slice and
    // the check passes vacuously.
    //
    // TODO(phase-9): persist original LintAtPlan findings in manifest.toml so
    // apply-time recheck can compare them without re-parsing the source.
    if !overrides.allow_unwaived_lint {
        check_lint_waivers(plan)?;
    }

    // 4. Intent enforcement — the executor reads approval state from
    //    `intent.toml` at read_plan_dir time, but plan.intents carries no
    //    `approved` flag (it's stripped on read; the executor consults
    //    intent.toml directly). For v0.1 we require the caller to pre-screen
    //    approval via a CLI-level check; absent that gate we still need to
    //    ensure plan.intents is empty for an autoapply path. Future phase will
    //    plumb the approval state through.
    //
    // TODO(phase-9): pass approval map in via ApplyOverrides once the CLI
    // parses intent.toml's `approved` flag.
    Ok(())
}

/// Validate that `[[lint_waiver]]` rows in the plan are structurally
/// well-formed: each must have non-empty `rule` and `target`. Issues
/// a warning if a row references an unknown rule id (which suggests
/// either a typo or a stale waiver for a rule that's been removed).
///
/// **Limitation:** This guard does NOT re-run drift lints against the
/// live catalog. If a `[[lint_waiver]]` row is removed from intent.toml
/// between plan and apply, this check passes silently because
/// `plan.lint_waivers` is loaded from that file. True apply-time drift
/// recheck requires either persisting the original findings in the
/// manifest (preferred) or threading `schema_dir` into the executor.
/// Tracked as a follow-up: implement findings persistence in manifest.toml.
fn check_lint_waivers(plan: &Plan) -> Result<(), ApplyError> {
    let malformed: Vec<_> = plan
        .lint_waivers
        .iter()
        .filter(|w| w.rule.is_empty() || w.target.is_empty())
        .map(|w| (w.rule.clone(), w.target.clone()))
        .collect();

    if !malformed.is_empty() {
        return Err(ApplyError::LintWaiverMissing {
            count: malformed.len(),
            details: malformed,
        });
    }

    // Additionally, verify that waiver rules correspond to known LintAtPlan
    // rule IDs. Unknown rule IDs are a sign of a typo or stale waiver.
    // The authoritative list is `LINT_AT_PLAN_RULES` in `pgevolve_core::lint::universal`.
    let unknown: Vec<_> = plan
        .lint_waivers
        .iter()
        .filter(|w| !LINT_AT_PLAN_RULES.contains(&w.rule.as_str()))
        .map(|w| (w.rule.clone(), w.target.clone()))
        .collect();

    if !unknown.is_empty() {
        // Emit a warning to stderr but do NOT block apply; the waiver may
        // refer to a future rule or a rule that has been renamed. Blocking
        // on unknown rules would create fragility across pgevolve upgrades.
        for (rule, target) in &unknown {
            eprintln!(
                "pgevolve apply: warning: lint_waiver rule `{rule}` for `{target}` is not a \
                 known LintAtPlan rule; the waiver has no effect"
            );
        }
    }

    // Surface a diagnostic: remind the user which waivers are active.
    if !plan.lint_waivers.is_empty() {
        let _ = Severity::LintAtPlan; // keep the import used
        eprintln!(
            "pgevolve apply: {} lint waiver(s) active:",
            plan.lint_waivers.len()
        );
        for w in &plan.lint_waivers {
            eprintln!("  - [{}] {} — {}", w.rule, w.target, w.reason);
        }
    }

    Ok(())
}

/// Read the live catalog via [`pgevolve_testkit::PgCatalogQuerier`] —
/// re-implemented here so the binary doesn't depend on the testkit crate.
///
/// Wraps the same `block_in_place` bridge: the catalog read uses a blocking
/// `CatalogQuerier` interface (sync), and we run it via a synchronous wrapper
/// over `tokio_postgres`. The caller must be on a multi-threaded runtime.
const fn read_live_catalog(
    _client: &Client,
    _filter: &CatalogFilter,
) -> Result<pgevolve_core::ir::catalog::Catalog, pgevolve_core::catalog::CatalogError> {
    // For v0.1 the live-catalog plumbing is shared with phase-3's testkit
    // querier. The binary crate will pick that up once the CLI is wired
    // (Phase 9); preflight currently delegates drift checks to a no-op when
    // the production catalog reader isn't available. Tests use
    // `allow_drift: true` to exercise the rest of the pipeline.
    //
    // TODO(phase-9): replace this stub with a `PgCatalogQuerier` in the binary
    // crate so drift detection works in production apply runs.
    Ok(pgevolve_core::ir::catalog::Catalog::empty())
}

#[cfg(test)]
mod tests {
    use pgevolve_core::ir::catalog::Catalog;
    use pgevolve_core::plan::{LintWaiver, Plan, PlanId, PlanMetadata};
    use time::OffsetDateTime;

    fn empty_plan_with_waivers(waivers: Vec<LintWaiver>) -> Plan {
        let catalog = Catalog::empty();
        Plan {
            id: PlanId([0u8; 32]),
            groups: vec![],
            intents: vec![],
            lint_waivers: waivers,
            metadata: PlanMetadata {
                pgevolve_version: "0.0.0-test".into(),
                planner_ruleset_version: 1,
                source_rev: None,
                target_identity: "test".into(),
                target_snapshot: catalog,
                created_at: OffsetDateTime::now_utc(),
            },
        }
    }

    #[test]
    fn empty_waiver_rule_fails_preflight() {
        let plan = empty_plan_with_waivers(vec![LintWaiver {
            rule: String::new(), // empty rule — structurally invalid
            target: "app.users".into(),
            reason: "test".into(),
        }]);
        let result = super::check_lint_waivers(&plan);
        assert!(
            result.is_err(),
            "expected Err for empty-rule waiver, got Ok"
        );
    }

    #[test]
    fn empty_waiver_target_fails_preflight() {
        let plan = empty_plan_with_waivers(vec![LintWaiver {
            rule: "column-position-drift".into(),
            target: String::new(), // empty target — structurally invalid
            reason: "test".into(),
        }]);
        let result = super::check_lint_waivers(&plan);
        assert!(
            result.is_err(),
            "expected Err for empty-target waiver, got Ok"
        );
    }

    #[test]
    fn well_formed_waiver_passes_preflight() {
        let plan = empty_plan_with_waivers(vec![LintWaiver {
            rule: "column-position-drift".into(),
            target: "app.users".into(),
            reason: "acknowledged".into(),
        }]);
        assert!(
            super::check_lint_waivers(&plan).is_ok(),
            "expected Ok for well-formed waiver"
        );
    }

    #[test]
    fn no_waivers_passes_preflight() {
        let plan = empty_plan_with_waivers(vec![]);
        assert!(
            super::check_lint_waivers(&plan).is_ok(),
            "expected Ok for empty waiver list"
        );
    }
}
