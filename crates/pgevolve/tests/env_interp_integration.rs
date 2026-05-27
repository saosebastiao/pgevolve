//! Preflight env-var resolution: missing var must error before any DB connection.
//!
//! These tests exercise `check_env_vars_resolvable` against a minimal in-memory
//! `Plan` whose step SQL carries `${VAR}` references. All assertions are based
//! on the process environment as-is (no mutation), using:
//!
//! - A sentinel name guaranteed not to be in env to test the failure path.
//! - The `references()` helper from `env_interp` to verify detection directly.
//!
//! Note: `StepKind::CreateSubscription` is added in Stage 8. For now we use
//! `StepKind::CreatePublication` as a stand-in; the env-var interpolator is
//! SQL-agnostic and does not inspect the step kind.

use pgevolve::executor::ApplyError;
use pgevolve::executor::env_interp;
use pgevolve::executor::preflight::check_env_vars_resolvable;
use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use pgevolve_core::plan::{Plan, PlanId, PlanMetadata, TransactionGroup};
use time::OffsetDateTime;

fn id(s: &str) -> Identifier {
    Identifier::from_unquoted(s).unwrap()
}

fn qn(schema: &str, name: &str) -> QualifiedName {
    QualifiedName::new(id(schema), id(name))
}

/// Build a minimal `Plan` with a single step whose SQL is `sql`.
///
/// Uses `StepKind::CreatePublication` as a placeholder; the env-var
/// resolution logic is step-kind-agnostic.
fn plan_with_sql(sql: &str) -> Plan {
    let step = RawStep {
        step_no: 1,
        kind: StepKind::CreatePublication,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![qn("public", "my_sub")],
        sql: sql.to_string(),
        transactional: TransactionConstraint::OutsideTransaction,
    };
    let group = TransactionGroup {
        id: 1,
        transactional: false,
        steps: vec![step],
    };
    Plan {
        id: PlanId([0u8; 32]),
        groups: vec![group],
        intents: vec![],
        lint_waivers: vec![],
        step_overrides: vec![],
        metadata: PlanMetadata {
            pgevolve_version: "0.0.0-test".into(),
            planner_ruleset_version: 1,
            source_rev: None,
            target_identity: "test".into(),
            target_snapshot: Catalog::empty(),
            created_at: OffsetDateTime::now_utc(),
            lint_at_plan_findings: vec![],
        },
        advisory_findings: vec![],
    }
}

/// The sentinel var name used in tests expecting a missing-var failure.
///
/// Chosen to be astronomically unlikely to exist in any real env.
const ABSENT_VAR: &str = "PGEVOLVE_STAGE4_INTEGRATION_SENTINEL_VAR_DEFINITELY_NOT_SET_XQZW9B";

/// A plan step whose SQL references a var that is not in process env must fail
/// `check_env_vars_resolvable` with `MissingEnvVar`.
#[test]
fn missing_env_var_fails_preflight() {
    // Verify the sentinel is genuinely absent before relying on it.
    assert!(
        std::env::var(ABSENT_VAR).is_err(),
        "sentinel env var {ABSENT_VAR} is unexpectedly set; choose a different name"
    );

    let sql = format!(
        "CREATE SUBSCRIPTION my_sub CONNECTION 'host=db password=${{{ABSENT_VAR}}}' PUBLICATION pub;"
    );
    let plan = plan_with_sql(&sql);

    let result = check_env_vars_resolvable(&plan);
    match result {
        Err(ApplyError::MissingEnvVar(name, step_no)) => {
            assert_eq!(name, ABSENT_VAR, "wrong var name in error");
            assert_eq!(step_no, 1, "wrong step number in error");
        }
        other => panic!("expected MissingEnvVar, got: {other:?}"),
    }
}

/// A plan step with no `${VAR}` references must always pass.
#[test]
fn no_refs_always_passes() {
    let plan = plan_with_sql("CREATE SUBSCRIPTION my_sub CONNECTION 'host=db' PUBLICATION pub;");
    assert!(
        check_env_vars_resolvable(&plan).is_ok(),
        "expected Ok for SQL with no env-var references"
    );
}

/// The `references()` helper correctly detects `${VAR}` names in SQL that
/// resembles a SUBSCRIPTION CONNECTION string.
#[test]
fn references_detects_subscription_connection_vars() {
    let sql = "CREATE SUBSCRIPTION my_sub CONNECTION \
               'host=db password=${DB_PASSWORD} user=${DB_USER}' PUBLICATION pub;";
    let refs = env_interp::references(sql);
    assert_eq!(refs, vec!["DB_PASSWORD", "DB_USER"]);
}

/// The `references()` helper must not flag lowercase or PL/pgSQL `$1` syntax.
#[test]
fn references_ignores_invalid_syntax() {
    let sql = "SELECT $1, ${lowercase}, ${} FROM t;";
    let refs = env_interp::references(sql);
    assert!(
        refs.is_empty(),
        "expected no references from invalid syntax, got: {refs:?}"
    );
}
