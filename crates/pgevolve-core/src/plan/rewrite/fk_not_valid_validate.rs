//! Add-foreign-key rewrite (spec §6.5).
//!
//! When an `AddConstraint(ForeignKey)` lands on an existing table, validating
//! the FK in one shot takes a `SHARE ROW EXCLUSIVE` lock and scans the table.
//! Splitting that work into two steps:
//!
//! 1. `ALTER TABLE ... ADD CONSTRAINT ... NOT VALID` — cheap, in-tx.
//! 2. `ALTER TABLE ... VALIDATE CONSTRAINT ...` — slow, in its own group.
//!
//! …lets the user observe step 1 before committing to the long-running step 2.
//! Both steps are still transactional individually, but they live in different
//! transaction groups so failures of step 2 leave step 1 intact and visible.

use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::ir::constraint::{Constraint, ConstraintKind};
use crate::plan::policy::PlannerPolicy;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::sql;

/// Should this `AddConstraint` be rewritten as `NOT VALID` + `VALIDATE`?
///
/// Three conditions:
/// 1. the policy enables `fk_not_valid_then_validate`,
/// 2. the constraint is a foreign key, and
/// 3. the target table already exists (i.e., not being created in this plan).
pub fn should_rewrite(qname: &QualifiedName, c: &Constraint, target: &Catalog, policy: &PlannerPolicy) -> bool {
    policy.fk_not_valid_then_validate()
        && matches!(c.kind, ConstraintKind::ForeignKey(_))
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
            step_no: 0,
            kind: StepKind::AddConstraintNotValid,
            destructive,
            destructive_reason: destructive_reason.clone(),
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_table_add_constraint_not_valid(qname, c),
            transactional: TransactionConstraint::InTransaction,
        },
        RawStep {
            step_no: 0,
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
