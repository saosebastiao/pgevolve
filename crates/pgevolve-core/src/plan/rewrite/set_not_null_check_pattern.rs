//! `SET NOT NULL` via CHECK pattern (spec §6.5).
//!
//! `ALTER COLUMN ... SET NOT NULL` rewrites a populated table to verify the
//! constraint, holding `ACCESS EXCLUSIVE` for the duration of the scan. Doing
//! it as a CHECK + validate avoids the long lock:
//!
//! 1. `ADD CONSTRAINT __pgevolve_chk_<col> CHECK (col IS NOT NULL) NOT VALID;`
//! 2. `VALIDATE CONSTRAINT __pgevolve_chk_<col>;`
//! 3. `ALTER COLUMN col SET NOT NULL;` — cheap once Postgres can prove via the
//!    validated CHECK that no row violates it.
//! 4. `DROP CONSTRAINT __pgevolve_chk_<col>;`
//!
//! v0.1 trigger condition is "the column exists in the target catalog" — that
//! distinguishes "make this existing column NOT NULL" from "I'm adding a
//! NOT-NULL column" (the latter rides inline with `ADD COLUMN`). A future
//! refinement using a non-zero row-count estimate can skip the rewrite for
//! known-empty tables.

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::default_expr::NormalizedExpr;
use crate::plan::policy::PlannerPolicy;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::sql;

/// Synthesized constraint name used for the intermediate CHECK.
fn synth_check_name(col: &Identifier) -> Identifier {
    Identifier::from_unquoted(&format!("__pgevolve_chk_{}", col.as_str()))
        .expect("synthesized name uses only ASCII identifier characters")
}

/// Should `SetColumnNullable { nullable: false }` use the CHECK pattern?
///
/// Yes if the policy allows it and the column already exists in the target
/// table — i.e., this is "make an existing column NOT NULL," not "add a
/// brand-new NOT-NULL column."
pub fn should_rewrite(
    qname: &QualifiedName,
    col: &Identifier,
    target: &Catalog,
    policy: &PlannerPolicy,
) -> bool {
    if !policy.not_null_via_check_pattern() {
        return false;
    }
    target
        .tables
        .iter()
        .find(|t| &t.qname == qname)
        .is_some_and(|t| t.columns.iter().any(|c| &c.name == col))
}

/// Build the four-step pattern.
///
/// Caller must have first verified [`should_rewrite`].
pub fn rewrite_steps(
    qname: &QualifiedName,
    col: &Identifier,
    destructive: bool,
    destructive_reason: Option<String>,
) -> [RawStep; 4] {
    let chk_name = synth_check_name(col);

    // Synthesize a Constraint just for SQL rendering of step 1. The IR
    // representation is incidental — the rewrite never persists this object.
    let synth = crate::ir::constraint::Constraint {
        qname: QualifiedName::new(qname.schema.clone(), chk_name.clone()),
        kind: crate::ir::constraint::ConstraintKind::Check {
            expression: NormalizedExpr::from_text(format!("{} IS NOT NULL", col.render_sql())),
            no_inherit: false,
        },
        deferrable: crate::ir::constraint::Deferrable::NotDeferrable,
        comment: None,
    };

    [
        RawStep {
            step_no: 0,
            kind: StepKind::AddCheckForNotNull,
            destructive,
            destructive_reason: destructive_reason.clone(),
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_table_add_constraint_not_valid(qname, &synth),
            transactional: TransactionConstraint::InTransaction,
        },
        RawStep {
            step_no: 0,
            kind: StepKind::ValidateConstraint,
            destructive,
            destructive_reason: destructive_reason.clone(),
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_table_validate_constraint(qname, &chk_name),
            transactional: TransactionConstraint::InTransaction,
        },
        RawStep {
            step_no: 0,
            kind: StepKind::SetColumnNullable,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_set_nullable(qname, col, false),
            transactional: TransactionConstraint::InTransaction,
        },
        RawStep {
            step_no: 0,
            kind: StepKind::DropConstraint,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_table_drop_constraint(qname, &chk_name),
            transactional: TransactionConstraint::InTransaction,
        },
    ]
}
