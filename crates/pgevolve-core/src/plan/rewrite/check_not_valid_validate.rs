//! Add-CHECK rewrite (spec §6.5).
//!
//! Same shape as the FK rewrite ([`super::fk_not_valid_validate`]): adding a
//! CHECK to an existing table normally takes a `SHARE ROW EXCLUSIVE` lock and
//! scans the table. Splitting into `NOT VALID` + `VALIDATE` lets the user
//! commit the cheap step before the expensive one.

use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::ir::constraint::{Constraint, ConstraintKind};
use crate::plan::policy::PlannerPolicy;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::sql;

/// Should this `AddConstraint` be rewritten as `NOT VALID` + `VALIDATE`?
///
/// Three conditions:
/// 1. the policy enables `check_not_valid_then_validate`,
/// 2. the constraint is a `CHECK`, and
/// 3. the target table already exists (i.e., not being created in this plan).
pub fn should_rewrite(qname: &QualifiedName, c: &Constraint, target: &Catalog, policy: &PlannerPolicy) -> bool {
    policy.check_not_valid_then_validate()
        && matches!(c.kind, ConstraintKind::Check { .. })
        && target.table_exists(qname)
}

/// Build the two-step pattern.
///
/// Caller must have first verified [`should_rewrite`].
pub fn rewrite_steps(
    qname: &QualifiedName,
    c: &Constraint,
    destructive: bool,
    destructive_reason: Option<String>,
) -> [RawStep; 2] {
    [
        RawStep {
            kind: StepKind::AddConstraintNotValid,
            destructive,
            destructive_reason: destructive_reason.clone(),
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_table_add_constraint_not_valid(qname, c),
            transactional: TransactionConstraint::InTransaction,
        },
        RawStep {
            kind: StepKind::ValidateConstraint,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_table_validate_constraint(qname, &c.qname.name),
            transactional: TransactionConstraint::InTransaction,
        },
    ]
}
