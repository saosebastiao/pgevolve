//! Pre-flight checks run before any DDL touches the live database:
//! identity match, drift recheck, intent enforcement, and lint-waiver recheck.

use tokio::runtime::Handle;
use tokio_postgres::Client;

use pgevolve_core::catalog::{
    CatalogError, CatalogFilter, CatalogQuerier, CatalogQuery, DriftReport, Row, Value,
};
use pgevolve_core::lint::{LINT_AT_PLAN_RULES, Severity};
use pgevolve_core::plan::Plan;

use super::error::ApplyError;
use crate::target_identity::compute_target_identity;

/// Toggles for each preflight check. Defaults are "all checks enforced."
// Mirrors ApplyOverrides's boolean flag pattern; excessive-bools suppressed
// for the same reason.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PreflightOverrides {
    /// Skip the target-identity match check.
    pub allow_different_target: bool,
    /// Skip the drift recheck.
    pub allow_drift: bool,
    /// When true, bypass `check_lint_waivers`. See [`super::ApplyOverrides::allow_unwaived_lint`].
    pub allow_unwaived_lint: bool,
    /// When true, bypass `check_intent_approval`. Used by test harnesses and
    /// shadow-validate paths whose plans are never routed through a real
    /// `intent.toml` approval flow. See [`super::ApplyOverrides::allow_unapproved_intents`].
    pub allow_unapproved_intents: bool,
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
        let (live_catalog, live_drift) = read_live_catalog(client, filter)?;
        let drift =
            pgevolve_core::diff::diff(&plan.metadata.target_snapshot, &live_catalog, &live_drift);
        if !drift.is_empty() {
            return Err(ApplyError::DriftDetected(drift.len()));
        }
    }

    // 3. Lint-waiver recheck (arch spec Decision 15).
    //
    // Validates that `[[lint_waiver]]` rows in the plan are structurally
    // well-formed AND — when the plan carries persisted `lint_at_plan_findings`
    // — that each recorded finding still has a matching waiver. This detects
    // the case where a user removes a waiver from `intent.toml` between plan
    // and apply.
    if !overrides.allow_unwaived_lint {
        check_lint_waivers(plan)?;
    }

    // 4. Intent approval enforcement.
    //
    // `read_plan_dir` populates `plan.intents` from `intent.toml`; each
    // `DestructiveIntent` now carries an `approved` flag parsed from the
    // `approved = true/false` field. Any unapproved intent fails preflight.
    if !overrides.allow_unapproved_intents {
        check_intent_approval(plan)?;
    }

    Ok(())
}

/// Validate lint waivers and, when the plan carries persisted
/// `lint_at_plan_findings`, check that each recorded finding still has a
/// matching `[[lint_waiver]]` row.
///
/// Two checks:
/// 1. Structural well-formedness: each `[[lint_waiver]]` must have non-empty
///    `rule` and `target`.
/// 2. Recorded-findings recheck: for every `RecordedFinding` stored in
///    `plan.metadata.lint_at_plan_findings` (populated at plan time), there
///    must be a matching `[[lint_waiver]]` row whose `rule` equals the finding's
///    `rule` and whose `target` appears as a substring of the finding's
///    `message`. If any recorded finding is unmatched, apply fails — this
///    catches the case where a user removes a waiver from `intent.toml` between
///    plan and apply.
fn check_lint_waivers(plan: &Plan) -> Result<(), ApplyError> {
    // ---- 1. Structural well-formedness ----
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

    // ---- 2. Unknown-rule warning ----
    // Verify that waiver rules correspond to known LintAtPlan rule IDs. Unknown
    // rule IDs are a sign of a typo or stale waiver. Emit a warning but do NOT
    // block apply — blocking on unknown rules would create fragility across
    // pgevolve upgrades.
    let unknown: Vec<_> = plan
        .lint_waivers
        .iter()
        .filter(|w| !LINT_AT_PLAN_RULES.contains(&w.rule.as_str()))
        .map(|w| (w.rule.clone(), w.target.clone()))
        .collect();

    if !unknown.is_empty() {
        for (rule, target) in &unknown {
            eprintln!(
                "pgevolve apply: warning: lint_waiver rule `{rule}` for `{target}` is not a \
                 known LintAtPlan rule; the waiver has no effect"
            );
        }
    }

    // ---- 3. Recorded-findings recheck ----
    // If the plan carries findings persisted at plan time, every one of them
    // must have a matching waiver now. A missing match means the user removed
    // a waiver from intent.toml after planning — fail hard.
    let unmatched: Vec<_> = plan
        .metadata
        .lint_at_plan_findings
        .iter()
        .filter(|f| {
            !plan
                .lint_waivers
                .iter()
                .any(|w| w.rule == f.rule && f.message.contains(&w.target))
        })
        .map(|f| (f.rule.clone(), f.target.clone()))
        .collect();

    if !unmatched.is_empty() {
        return Err(ApplyError::LintWaiverMissing {
            count: unmatched.len(),
            details: unmatched,
        });
    }

    // ---- 4. Diagnostic: list active waivers ----
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

/// Enforce that every `DestructiveIntent` in the plan has been approved by the
/// user (i.e., `approved = true` in `intent.toml`). Returns
/// [`ApplyError::UnapprovedIntents`] when any intent is still `approved = false`.
fn check_intent_approval(plan: &Plan) -> Result<(), ApplyError> {
    let unapproved: Vec<_> = plan
        .intents
        .iter()
        .filter(|i| !i.approved)
        .map(|i| (i.id, i.target.clone(), i.reason.clone()))
        .collect();

    if !unapproved.is_empty() {
        return Err(ApplyError::UnapprovedIntents {
            count: unapproved.len(),
            details: unapproved,
        });
    }

    Ok(())
}

/// Read the live catalog from the Postgres connection.
///
/// Uses a lightweight inline [`CatalogQuerier`] adapter that holds `&Client`
/// and dispatches each catalog query via [`tokio::task::block_in_place`].
/// Because `block_in_place` does not require its closure to be `Send`, we can
/// safely borrow `client` here without any `unsafe` code.
///
/// The caller must be on a multi-threaded Tokio runtime.
fn read_live_catalog(
    client: &Client,
    filter: &CatalogFilter,
) -> Result<(pgevolve_core::ir::catalog::Catalog, DriftReport), CatalogError> {
    struct BorrowedQuerier<'a> {
        client: &'a Client,
        runtime: Handle,
        version: std::cell::Cell<Option<pgevolve_core::catalog::PgVersion>>,
    }

    impl CatalogQuerier for BorrowedQuerier<'_> {
        fn fetch(
            &self,
            query: CatalogQuery,
            managed_schemas: &[&str],
        ) -> Result<Vec<Row>, CatalogError> {
            use pgevolve_core::catalog::queries::query_for;

            // Determine PG version, caching after the first call.
            let version = if matches!(query, CatalogQuery::PgVersion) {
                pgevolve_core::catalog::PgVersion::Pg16
            } else if let Some(v) = self.version.get() {
                v
            } else {
                let v = pgevolve_core::catalog::PgVersion::detect(self)?;
                self.version.set(Some(v));
                v
            };

            let sql = query_for(version, query);
            let client = self.client;
            let owned: Vec<String> = managed_schemas.iter().map(ToString::to_string).collect();

            // `block_in_place` blocks the current thread without requiring
            // `Send` on the closure, so capturing `&Client` is safe here.
            let pg_rows: Vec<tokio_postgres::Row> = tokio::task::block_in_place(|| {
                self.runtime.block_on(async move {
                    if matches!(query, CatalogQuery::PgVersion) {
                        client.query(sql, &[]).await
                    } else {
                        client.query(sql, &[&owned]).await
                    }
                })
            })
            .map_err(|e| CatalogError::QueryFailed {
                query,
                message: e.to_string(),
            })?;

            pg_rows
                .iter()
                .map(|r| pg_row_to_catalog_row(r, query))
                .collect()
        }
    }

    let runtime = Handle::try_current().map_err(|e| CatalogError::QueryFailed {
        query: CatalogQuery::PgVersion,
        message: format!("not inside a Tokio runtime: {e}"),
    })?;

    let querier = BorrowedQuerier {
        client,
        runtime,
        version: std::cell::Cell::new(None),
    };

    pgevolve_core::catalog::read_catalog(&querier, filter)
}

/// Convert a single `tokio_postgres::Row` to a `pgevolve_core::catalog::Row`.
///
/// Mirrors the conversion in [`crate::pg_querier`] — duplicated here to avoid
/// a circular dependency between the executor and `pg_querier` modules.
fn pg_row_to_catalog_row(
    row: &tokio_postgres::Row,
    query: CatalogQuery,
) -> Result<Row, CatalogError> {
    use tokio_postgres::types::Type;

    let bad = |col: &str, msg: String| CatalogError::BadColumnType {
        query,
        column: col.to_string(),
        message: msg,
    };

    macro_rules! get_opt {
        ($row:expr, $idx:expr, $col:expr, $t:ty, $ty:expr) => {
            $row.try_get::<_, Option<$t>>($idx).map_err(|e| {
                bad(
                    $col,
                    format!("decode {} as {}: {e}", $ty.name(), stringify!($t)),
                )
            })?
        };
    }

    let mut out = Row::new();
    for (i, col) in row.columns().iter().enumerate() {
        let name = col.name();
        let ty = col.type_();
        let value = match *ty {
            Type::BOOL => get_opt!(row, i, name, bool, ty).map_or(Value::Null, Value::Bool),
            Type::INT2 => get_opt!(row, i, name, i16, ty).map_or(Value::Null, Value::SmallInt),
            Type::INT4 => get_opt!(row, i, name, i32, ty)
                .map_or(Value::Null, |v| Value::Integer(i64::from(v))),
            Type::INT8 => get_opt!(row, i, name, i64, ty).map_or(Value::Null, Value::Integer),
            Type::OID => get_opt!(row, i, name, u32, ty)
                .map_or(Value::Null, |v| Value::Integer(i64::from(v))),
            Type::TEXT | Type::VARCHAR | Type::NAME | Type::BPCHAR => {
                get_opt!(row, i, name, String, ty).map_or(Value::Null, Value::Text)
            }
            Type::CHAR => {
                let v = get_opt!(row, i, name, i8, ty);
                #[allow(clippy::cast_sign_loss)]
                v.map_or(Value::Null, |b| Value::Char(b as u8 as char))
            }
            Type::INT2_ARRAY => get_opt!(row, i, name, Vec<i16>, ty).map_or(Value::Null, |v| {
                Value::IntegerArray(v.into_iter().map(i64::from).collect())
            }),
            Type::INT4_ARRAY => get_opt!(row, i, name, Vec<i32>, ty).map_or(Value::Null, |v| {
                Value::IntegerArray(v.into_iter().map(i64::from).collect())
            }),
            Type::INT8_ARRAY => {
                get_opt!(row, i, name, Vec<i64>, ty).map_or(Value::Null, Value::IntegerArray)
            }
            Type::TEXT_ARRAY | Type::NAME_ARRAY | Type::VARCHAR_ARRAY => {
                get_opt!(row, i, name, Vec<String>, ty).map_or(Value::Null, Value::TextArray)
            }
            Type::BYTEA => get_opt!(row, i, name, Vec<u8>, ty).map_or(Value::Null, Value::Bytes),
            _ => row
                .try_get::<_, Option<String>>(i)
                .map(|v| v.map_or(Value::Null, Value::Text))
                .map_err(|e| bad(name, format!("unsupported type {} ({e})", ty.name())))?,
        };
        out.insert(name, value);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use pgevolve_core::ir::catalog::Catalog;
    use pgevolve_core::plan::{
        DestructiveIntent, LintWaiver, Plan, PlanId, PlanMetadata, RecordedFinding,
    };
    use time::OffsetDateTime;

    fn empty_plan_with_waivers(waivers: Vec<LintWaiver>) -> Plan {
        empty_plan_with_waivers_and_findings(waivers, vec![])
    }

    fn empty_plan_with_waivers_and_findings(
        waivers: Vec<LintWaiver>,
        findings: Vec<RecordedFinding>,
    ) -> Plan {
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
                lint_at_plan_findings: findings,
            },
        }
    }

    fn plan_with_intents(intents: Vec<DestructiveIntent>) -> Plan {
        let catalog = Catalog::empty();
        Plan {
            id: PlanId([0u8; 32]),
            groups: vec![],
            intents,
            lint_waivers: vec![],
            metadata: PlanMetadata {
                pgevolve_version: "0.0.0-test".into(),
                planner_ruleset_version: 1,
                source_rev: None,
                target_identity: "test".into(),
                target_snapshot: catalog,
                created_at: OffsetDateTime::now_utc(),
                lint_at_plan_findings: vec![],
            },
        }
    }

    // ---- Structural well-formedness tests ----

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

    // ---- Recorded-findings recheck tests (TODO 2) ----

    #[test]
    fn recorded_finding_with_matching_waiver_passes() {
        // A recorded finding + a matching waiver → Ok.
        let plan = empty_plan_with_waivers_and_findings(
            vec![LintWaiver {
                rule: "column-position-drift".into(),
                target: "app.users".into(),
                reason: "acknowledged".into(),
            }],
            vec![RecordedFinding {
                rule: "column-position-drift".into(),
                target: "app.users".into(),
                message: "app.users: column position drift. source order [id, created_at, email] \
                          vs catalog order [id, email, created_at]"
                    .into(),
            }],
        );
        assert!(
            super::check_lint_waivers(&plan).is_ok(),
            "expected Ok when recorded finding has a matching waiver"
        );
    }

    #[test]
    fn recorded_finding_with_no_waiver_fails() {
        // A recorded finding + no waivers → Err.
        let plan = empty_plan_with_waivers_and_findings(
            vec![],
            vec![RecordedFinding {
                rule: "column-position-drift".into(),
                target: "app.users".into(),
                message: "app.users: column position drift.".into(),
            }],
        );
        let result = super::check_lint_waivers(&plan);
        assert!(
            result.is_err(),
            "expected Err when recorded finding has no waiver"
        );
    }

    #[test]
    fn removed_waiver_fails_preflight() {
        // A recorded finding + a waiver for a *different* target → Err.
        // Simulates the user having removed the correct waiver after planning.
        let plan = empty_plan_with_waivers_and_findings(
            vec![LintWaiver {
                rule: "column-position-drift".into(),
                target: "app.orders".into(), // different target — does NOT match
                reason: "for a different table".into(),
            }],
            vec![RecordedFinding {
                rule: "column-position-drift".into(),
                target: "app.users".into(),
                message: "app.users: column position drift.".into(),
            }],
        );
        let result = super::check_lint_waivers(&plan);
        assert!(
            result.is_err(),
            "expected Err when the correct waiver was removed (replaced with a non-matching one)"
        );
    }

    // ---- Intent approval tests (TODO 3) ----

    #[test]
    fn unapproved_intent_fails_preflight() {
        let plan = plan_with_intents(vec![DestructiveIntent {
            id: 1,
            step: 1,
            kind: "drop_column".into(),
            target: "app.users.legacy_email".into(),
            reason: "removing deprecated column".into(),
            approved: false,
        }]);
        let result = super::check_intent_approval(&plan);
        assert!(
            result.is_err(),
            "expected Err for unapproved destructive intent"
        );
    }

    #[test]
    fn approved_intent_passes_preflight() {
        let plan = plan_with_intents(vec![DestructiveIntent {
            id: 1,
            step: 1,
            kind: "drop_column".into(),
            target: "app.users.legacy_email".into(),
            reason: "removing deprecated column".into(),
            approved: true,
        }]);
        assert!(
            super::check_intent_approval(&plan).is_ok(),
            "expected Ok when intent is approved"
        );
    }

    #[test]
    fn no_intents_passes_preflight() {
        let plan = plan_with_intents(vec![]);
        assert!(
            super::check_intent_approval(&plan).is_ok(),
            "expected Ok when there are no intents"
        );
    }

    #[test]
    fn allow_unapproved_intents_override_bypasses_check() {
        // The check itself is bypassed at the `run_preflight` level via
        // `PreflightOverrides::allow_unapproved_intents`. Verify that the
        // check function itself would fail so the override is meaningful.
        let plan = plan_with_intents(vec![DestructiveIntent {
            id: 1,
            step: 1,
            kind: "drop_table".into(),
            target: "app.legacy".into(),
            reason: "old table".into(),
            approved: false,
        }]);
        // Direct call to check_intent_approval must fail.
        assert!(
            super::check_intent_approval(&plan).is_err(),
            "check_intent_approval must fail for an unapproved intent (override tested at run_preflight level)"
        );
        // The override is enforced in run_preflight, which skips the call
        // when allow_unapproved_intents = true. That path is exercised by
        // the integration tests in tests/common/mod.rs.
    }
}
