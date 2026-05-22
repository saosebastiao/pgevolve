//! Rewrite pass: turn an [`OrderedChangeSet`] into a flat `Vec<RawStep>`.
//!
//! Each [`Change`] / [`crate::diff::TableOp`] / [`crate::diff::SequenceOp`] is dispatched to an emitter
//! that produces one or more `RawStep`s. Most emitters produce a single step;
//! the four documented online rewrites (spec §6.5) produce multiple. Online
//! rewrites are gated on [`PlannerPolicy`] switches so atomic mode is a single
//! transaction with no rewriting.

// The dispatcher is inherently long (one arm per IR variant) and its work is
// almost entirely SQL string assembly. Pedantic clippy lints that fight
// straight-line emitter code are silenced at module scope rather than peppered
// through individual arms.
#![allow(clippy::too_many_lines)]
#![allow(clippy::option_if_let_else)]
#![allow(clippy::format_push_string)]
#![allow(clippy::needless_pass_by_value)]

pub mod check_not_valid_validate;
pub mod concurrent_index;
pub mod emit;
pub mod extensions;
pub mod fk_not_valid_validate;
pub mod functions;
pub mod partitions;
pub mod refresh_mv_concurrently;
pub mod set_not_null_check_pattern;
pub mod sql;
pub mod triggers;
pub mod types;
pub mod views;

use crate::diff::change::{Change, ChangeEntry};
use crate::diff::destructiveness::Destructiveness;
use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::plan::ordered::OrderedChangeSet;
use crate::plan::policy::PlannerPolicy;
use crate::plan::raw_step::RawStep;

/// Context passed to every emitter — read-only.
pub(super) struct Ctx<'a> {
    pub(super) target: &'a Catalog,
    pub(super) source: &'a Catalog,
    pub(super) policy: &'a PlannerPolicy,
}

/// Apply policy-gated rewrites and emit the flat step list.
///
/// Steps are emitted in this order: creates → modifies → drops → deferred FKs.
/// Within each bucket the dependency order from [`OrderedChangeSet`] is preserved.
pub fn rewrite(
    ordered: OrderedChangeSet,
    target: &Catalog,
    policy: &PlannerPolicy,
) -> Vec<RawStep> {
    rewrite_with_source(ordered, target, &Catalog::empty(), policy)
}

/// Like [`rewrite`] but also accepts the source catalog for drift-recovery
/// changes that need to look up source-side IR (e.g., `RecreateIndex`).
///
/// After all changes are emitted, the `refresh_mv_concurrently` rewrite pass
/// upgrades `REFRESH MATERIALIZED VIEW` steps to `CONCURRENTLY` when the target
/// MV has a unique index (spec §6.5). Lint findings from that pass are
/// currently discarded here (T10 wires them into the plan's lint output).
pub fn rewrite_with_source(
    ordered: OrderedChangeSet,
    target: &Catalog,
    source: &Catalog,
    policy: &PlannerPolicy,
) -> Vec<RawStep> {
    let ctx = Ctx {
        target,
        source,
        policy,
    };
    let mut out = Vec::new();
    for entry in ordered.creates_and_adds {
        emit_change(entry, &ctx, &mut out);
    }
    for entry in ordered.modifies {
        emit_change(entry, &ctx, &mut out);
    }
    for entry in ordered.drops {
        emit_change(entry, &ctx, &mut out);
    }
    for fk in ordered.deferred_fks {
        emit::deferred_fk::emit(&fk, &ctx, &mut out);
    }
    // Post-emit: upgrade REFRESH MATERIALIZED VIEW → CONCURRENTLY where eligible.
    // Check indexes from the *source* catalog (desired state), not target, so
    // that newly-created unique indexes on new MVs are included in the check.
    // Lint findings from this pass are discarded here; T10 wires them into the
    // plan's lint output.
    let mut findings_sink: Vec<crate::lint::Finding> = Vec::new();
    refresh_mv_concurrently::rewrite(&mut out, source, policy, &mut findings_sink);
    out
}

fn emit_change(entry: ChangeEntry, ctx: &Ctx<'_>, out: &mut Vec<RawStep>) {
    let destructive_reason = destructive_reason(&entry.destructiveness);
    let destructive = entry.destructiveness.requires_approval();

    match entry.change {
        Change::CreateSchema(s) => emit::schema::create(s, destructive, destructive_reason, out),
        Change::DropSchema(name) => emit::schema::drop_(name, destructive, destructive_reason, out),
        Change::AlterSchema { name, comment } => emit::schema::alter(name, comment, out),

        Change::CreateTable(t) => emit::table::create(t, destructive, destructive_reason, out),
        Change::DropTable { qname, .. } => {
            emit::table::drop_(qname, destructive, destructive_reason, out);
        }
        Change::AlterTable { qname, ops } => emit::table::alter(qname, ops, ctx, out),

        Change::CreateIndex(idx) => {
            emit::index::create(idx, ctx, destructive, destructive_reason, out);
        }
        Change::DropIndex(qname) => {
            emit::index::drop_(qname, ctx, destructive, destructive_reason, out);
        }
        Change::ReplaceIndex { from, to } => {
            emit::index::replace(from, to, ctx, destructive, destructive_reason, out);
        }

        Change::CreateSequence(s) => {
            emit::sequence::create(s, destructive, destructive_reason, out);
        }
        Change::DropSequence(qname) => {
            emit::sequence::drop_(qname, destructive, destructive_reason, out);
        }
        Change::AlterSequence { qname, ops } => emit::sequence::alter(qname, ops, out),

        // Drift-recovery changes emitted from the DriftReport.
        Change::ValidateConstraint { table, constraint } => {
            emit::constraint::validate(table, constraint, destructive, destructive_reason, out);
        }
        Change::RecreateIndex { qname } => {
            emit::index::recreate(qname, ctx, destructive, destructive_reason, out);
        }

        Change::View(vc) => emit::view::emit(vc, destructive, destructive_reason, out),
        Change::Mv(mc) => emit::mv::emit(mc, destructive, destructive_reason, out),
        Change::UserType(utc) => {
            emit::user_type::emit(utc, destructive, destructive_reason, ctx, out);
        }
        Change::Function(fc) => emit::function::emit(fc, destructive, destructive_reason, out),
        Change::Procedure(pc) => emit::procedure::emit(pc, destructive, destructive_reason, out),
        Change::Extension(ec) => emit::extension::emit(ec, destructive, destructive_reason, out),
        Change::Trigger(tc) => emit::trigger::emit(tc, destructive, destructive_reason, out),
        Change::Table(tc) => {
            use crate::diff::change::TableChange;
            match tc {
                TableChange::AttachPartition {
                    parent,
                    child,
                    bounds,
                } => {
                    out.push(RawStep {
                        step_no: 0,
                        kind: crate::plan::raw_step::StepKind::AttachPartition,
                        destructive: false,
                        destructive_reason: None,
                        intent_id: None,
                        targets: vec![child.clone()],
                        sql: crate::plan::rewrite::partitions::attach_partition(
                            &parent, &child, &bounds,
                        ),
                        transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
                    });
                }
                TableChange::DetachPartition { parent, child } => {
                    out.push(RawStep {
                        step_no: 0,
                        kind: crate::plan::raw_step::StepKind::DetachPartition,
                        destructive: false,
                        destructive_reason: None,
                        intent_id: None,
                        targets: vec![child.clone()],
                        sql: crate::plan::rewrite::partitions::detach_partition(&parent, &child),
                        transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
                    });
                }
            }
        }
        // Grant / revoke / owner changes: SQL emission wired up in Stage 9.
        Change::GrantObjectPrivilege { .. }
        | Change::RevokeObjectPrivilege { .. }
        | Change::GrantColumnPrivilege { .. }
        | Change::RevokeColumnPrivilege { .. }
        | Change::AlterObjectOwner(_)
        | Change::AlterDefaultPrivileges { .. } => {
            // Stage 9 will wire these up. For now emit nothing so existing
            // tests and plans that don't exercise grants continue to work.
        }
        // UnsupportedDiff is intercepted by the ordering phase and never reaches here.
        Change::UnsupportedDiff { .. } => {
            unreachable!("UnsupportedDiff must never reach the rewrite phase")
        }
    }
}

// `Schema` is identified by an `Identifier`, but `RawStep::targets` carries
// `QualifiedName`s. Promote the schema name to a `QualifiedName` whose schema
// half equals its name — same convention used for ordering in the planner's
// Phase 5 helpers.
pub(super) fn schema_target(name: &crate::identifier::Identifier) -> QualifiedName {
    QualifiedName::new(name.clone(), name.clone())
}

pub(super) fn destructive_reason(d: &Destructiveness) -> Option<String> {
    match d {
        Destructiveness::Safe => None,
        Destructiveness::RequiresApproval { reason }
        | Destructiveness::RequiresApprovalAndDataLossWarning { reason } => Some(reason.clone()),
    }
}

#[cfg(test)]
mod tests;
