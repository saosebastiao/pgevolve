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
pub mod collations;
pub mod concurrent_index;
pub mod emit;
pub mod extensions;
pub mod fk_not_valid_validate;
pub mod functions;
pub mod grants;
pub mod partitions;
pub mod policies;
pub mod publications;
pub mod refresh_mv_concurrently;
pub mod reloptions;
pub mod set_not_null_check_pattern;
pub mod sql;
pub mod statistics;
pub mod subscriptions;
pub mod triggers;
pub mod types;
pub mod views;

use crate::diff::change::{
    Change, ChangeEntry, CollationChange, PublicationChange, StatisticChange, SubscriptionChange,
};
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
        Change::AlterViewSetCheckOption { qname, .. } => {
            // Look up the full source View IR to render CREATE OR REPLACE VIEW.
            // The differ only emits this change when the source view is present,
            // so the unwrap is invariant-safe.
            let view = ctx
                .source
                .views
                .iter()
                .find(|v| v.qname == qname)
                .expect("source view must be present when AlterViewSetCheckOption is emitted");
            out.push(crate::plan::raw_step::RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterViewSetCheckOption,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname],
                sql: crate::plan::rewrite::views::emit_alter_view_set_check_option(view),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
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
        Change::AlterObjectOwner(op) => {
            // Audit `targets` historically carried the QualifiedName; for
            // cluster-level (publication/subscription) and schema owners we
            // synthesize a QualifiedName for tracking only.
            let target_qname = match &op.id {
                crate::diff::owner_op::OwnedObjectId::Qualified(q) => q.clone(),
                crate::diff::owner_op::OwnedObjectId::Schema(name)
                | crate::diff::owner_op::OwnedObjectId::Cluster(name) => {
                    QualifiedName::new(name.clone(), name.clone())
                }
            };
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterObjectOwner,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![target_qname],
                sql: grants::alter_object_owner(op.kind, &op.id, &op.signature, &op.to),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::GrantObjectPrivilege {
            kind,
            qname,
            signature,
            grant,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::GrantObjectPrivilege,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: grants::grant_object_privilege(kind, &qname, &signature, &grant),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::RevokeObjectPrivilege {
            kind,
            qname,
            signature,
            grant,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::RevokeObjectPrivilege,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: grants::revoke_object_privilege(kind, &qname, &signature, &grant),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::GrantColumnPrivilege { qname, grant } => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::GrantColumnPrivilege,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: grants::grant_column_privilege(&qname, &grant),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::RevokeColumnPrivilege { qname, grant } => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::RevokeColumnPrivilege,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: grants::revoke_column_privilege(&qname, &grant),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::AlterDefaultPrivileges {
            target_role,
            schema,
            object_type,
            direction,
            grant,
        } => {
            // Default-priv rules are not scoped to a per-object QualifiedName;
            // targets is left empty (same convention as cluster ops in
            // plan/cluster_rewrite/emit.rs).
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterDefaultPrivileges,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: grants::alter_default_privileges(
                    &target_role,
                    schema.as_ref(),
                    object_type,
                    direction,
                    &grant,
                ),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::CreatePolicy { table, policy } => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::CreatePolicy,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![table.clone()],
                sql: policies::create_policy(&table, &policy),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::DropPolicy { table, name } => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::DropPolicy,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![table.clone()],
                sql: policies::drop_policy(&table, &name),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::AlterPolicy { table, policy } => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterPolicy,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![table.clone()],
                sql: policies::alter_policy(&table, &policy),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::SetTableRowSecurity { qname, security } => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::SetTableRowSecurity,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: policies::set_table_row_security(&qname, security),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::SetTableForceRowSecurity { qname, force } => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::SetTableForceRowSecurity,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: policies::set_table_force_row_security(&qname, force),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }

        Change::SetTableStorage { qname, options } => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::SetTableStorage,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: reloptions::alter_table_set_storage(&qname, &options),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::SetIndexStorage { qname, options } => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::SetIndexStorage,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: reloptions::alter_index_set_storage(&qname, &options),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::SetMaterializedViewStorage { qname, options } => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::SetMaterializedViewStorage,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: reloptions::alter_mv_set_storage(&qname, &options),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }

        Change::Publication(PublicationChange::Create(p)) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::CreatePublication,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: publications::create_publication(&p),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
            // Follow-up COMMENT step if a comment is present.
            if let Some(c) = &p.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: crate::plan::raw_step::StepKind::CommentOnPublication,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![],
                    sql: publications::comment_on_publication(&p.name, Some(c)),
                    transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
                });
            }
        }
        Change::Publication(PublicationChange::Drop { name }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::DropPublication,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![],
                sql: publications::drop_publication(&name),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Publication(PublicationChange::Replace { from, to }) => {
            let [drop_sql, create_sql] = publications::replace_publication(&from, &to);
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::ReplacePublication,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![],
                sql: format!("{drop_sql}\n{create_sql}"),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Publication(PublicationChange::AddTable { publication, table }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterPublicationAddTable,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![table.qname.clone()],
                sql: publications::alter_publication_add_table(&publication, &table),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Publication(PublicationChange::DropTable { publication, qname }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterPublicationDropTable,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: publications::alter_publication_drop_table(&publication, &qname),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Publication(PublicationChange::SetTable { publication, table }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterPublicationSetTable,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![table.qname.clone()],
                sql: publications::alter_publication_set_table(&publication, &table),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Publication(PublicationChange::AddSchema {
            publication,
            schema,
        }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterPublicationAddSchema,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: publications::alter_publication_add_schema(&publication, &schema),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Publication(PublicationChange::DropSchema {
            publication,
            schema,
        }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterPublicationDropSchema,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: publications::alter_publication_drop_schema(&publication, &schema),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Publication(PublicationChange::SetPublish { publication, kinds }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterPublicationSetPublish,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: publications::alter_publication_set_publish(&publication, kinds),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Publication(PublicationChange::SetViaRoot { publication, value }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterPublicationSetViaRoot,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: publications::alter_publication_set_via_root(&publication, value),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Publication(PublicationChange::CommentOn { name, comment }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::CommentOnPublication,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: publications::comment_on_publication(&name, comment.as_deref()),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }

        Change::Subscription(SubscriptionChange::Create(s)) => {
            // PG error 25001: CREATE SUBSCRIPTION ... WITH (create_slot = true)
            // cannot run inside a transaction block. The PG default is true, so
            // the only case where InTransaction is safe is `create_slot = Some(false)`.
            let create_tx = if s.options.create_slot == Some(false) {
                crate::plan::raw_step::TransactionConstraint::InTransaction
            } else {
                crate::plan::raw_step::TransactionConstraint::OutsideTransaction
            };
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::CreateSubscription,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::create_subscription(&s),
                transactional: create_tx,
            });
            // Follow-up COMMENT step if a comment is present.
            if let Some(c) = &s.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: crate::plan::raw_step::StepKind::CommentOnSubscription,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![],
                    sql: subscriptions::comment_on_subscription(&s.name, Some(c)),
                    transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
                });
            }
        }
        Change::Subscription(SubscriptionChange::Drop { name }) => {
            // PG error 25001: DROP SUBSCRIPTION cannot run inside a transaction
            // block when the subscription has an attached replication slot. The
            // IR cannot determine at diff time whether the live subscription has
            // a slot (that is runtime state in pg_subscription_rel), so the
            // conservative path is to always run OutsideTransaction.
            //
            // Trade-off: each DROP SUBSCRIPTION becomes its own transaction
            // group. Acceptable cost for correctness.
            //
            // Companion of 4d8ec92 (#11 — CREATE SUBSCRIPTION). Closes #26.
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::DropSubscription,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::drop_subscription(&name),
                transactional: crate::plan::raw_step::TransactionConstraint::OutsideTransaction,
            });
        }
        Change::Subscription(SubscriptionChange::AlterConnection {
            name,
            new_connection,
        }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterSubscriptionConnection,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::alter_subscription_connection(&name, &new_connection),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Subscription(SubscriptionChange::AddPublication { name, publication }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterSubscriptionAddPublication,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::alter_subscription_add_publication(&name, &publication),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Subscription(SubscriptionChange::DropPublication { name, publication }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterSubscriptionDropPublication,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::alter_subscription_drop_publication(&name, &publication),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Subscription(SubscriptionChange::SetOptions { name, options }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterSubscriptionSetOptions,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::alter_subscription_set_options(&name, &options),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Subscription(SubscriptionChange::CommentOn { name, comment }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::CommentOnSubscription,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::comment_on_subscription(&name, comment.as_deref()),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }

        Change::Statistic(StatisticChange::Create(s)) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::CreateStatistic,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![s.qname.clone()],
                sql: statistics::create_statistic(&s),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
            // Follow-up COMMENT if present.
            if let Some(c) = &s.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: crate::plan::raw_step::StepKind::CommentOnStatistic,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![s.qname.clone()],
                    sql: statistics::comment_on_statistic(&s.qname, Some(c)),
                    transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
                });
            }
        }
        Change::Statistic(StatisticChange::Drop { qname }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::DropStatistic,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: statistics::drop_statistic(&qname),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Statistic(StatisticChange::Replace { from, to }) => {
            let [drop_sql, create_sql] = statistics::replace_statistic(&from, &to);
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::ReplaceStatistic,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![to.qname],
                sql: format!("{drop_sql}\n{create_sql}"),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Statistic(StatisticChange::AlterSetTarget { qname, value }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::AlterStatisticSetTarget,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: statistics::alter_statistic_set_target(&qname, value),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }
        Change::Statistic(StatisticChange::CommentOn { qname, comment }) => {
            out.push(RawStep {
                step_no: 0,
                kind: crate::plan::raw_step::StepKind::CommentOnStatistic,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: statistics::comment_on_statistic(&qname, comment.as_deref()),
                transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
            });
        }

        // Collation changes: each variant maps to one or two RawSteps.
        // `Replace` decomposes into DROP + CREATE because PG has no in-place
        // ALTER for provider / locale / deterministic; the audit log keeps
        // the two halves distinct via separate `StepKind`s.
        Change::Collation(cc) => match cc {
            CollationChange::Create(c) => {
                out.push(RawStep {
                    step_no: 0,
                    kind: crate::plan::raw_step::StepKind::CreateCollation,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![c.qname.clone()],
                    sql: collations::create_collation(&c),
                    transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
                });
                // Follow-up COMMENT if present on the IR.
                if let Some(comment) = &c.comment {
                    out.push(RawStep {
                        step_no: 0,
                        kind: crate::plan::raw_step::StepKind::CommentOnCollation,
                        destructive: false,
                        destructive_reason: None,
                        intent_id: None,
                        targets: vec![c.qname.clone()],
                        sql: collations::comment_on_collation(&c.qname, Some(comment)),
                        transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
                    });
                }
            }
            CollationChange::Drop { qname } => {
                out.push(RawStep {
                    step_no: 0,
                    kind: crate::plan::raw_step::StepKind::DropCollation,
                    destructive,
                    destructive_reason,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: collations::drop_collation(&qname),
                    transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
                });
            }
            CollationChange::Rename { from, to } => {
                out.push(RawStep {
                    step_no: 0,
                    kind: crate::plan::raw_step::StepKind::RenameCollation,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![from.clone()],
                    sql: collations::rename_collation(&from, &to),
                    transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
                });
            }
            CollationChange::Replace { from, to } => {
                // Two-step decomposition: DROP old, then CREATE new. The
                // drop step inherits the entry-level destructive flag; the
                // create step is always safe.
                out.push(RawStep {
                    step_no: 0,
                    kind: crate::plan::raw_step::StepKind::DropCollation,
                    destructive,
                    destructive_reason,
                    intent_id: None,
                    targets: vec![from.qname.clone()],
                    sql: collations::drop_collation(&from.qname),
                    transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
                });
                out.push(RawStep {
                    step_no: 0,
                    kind: crate::plan::raw_step::StepKind::CreateCollation,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![to.qname.clone()],
                    sql: collations::create_collation(&to),
                    transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
                });
            }
            CollationChange::CommentOn { qname, comment } => {
                out.push(RawStep {
                    step_no: 0,
                    kind: crate::plan::raw_step::StepKind::CommentOnCollation,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: collations::comment_on_collation(&qname, comment.as_deref()),
                    transactional: crate::plan::raw_step::TransactionConstraint::InTransaction,
                });
            }
        },

        Change::EventTrigger(etc) => {
            emit::event_trigger::emit(etc, destructive, destructive_reason, out);
        }

        Change::Aggregate(agg) => {
            emit::aggregate::emit(agg, destructive, destructive_reason, out);
        }

        Change::Cast(cc) => {
            emit::cast::emit(cc, destructive, destructive_reason, out);
        }

        Change::TsDictionary(dc) => {
            emit::ts_dictionary::emit(dc, destructive, destructive_reason, out);
        }

        Change::TsConfiguration(c) => {
            emit::ts_configuration::emit(c, destructive, destructive_reason, out);
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
