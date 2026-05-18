//! Rewrite pass: turn an [`OrderedChangeSet`] into a flat `Vec<RawStep>`.
//!
//! Each [`Change`] / [`TableOp`] / [`SequenceOp`] is dispatched to an emitter
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
pub mod fk_not_valid_validate;
pub mod refresh_mv_concurrently;
pub mod set_not_null_check_pattern;
pub mod sql;
pub mod views;

use crate::diff::change::{Change, ChangeEntry, MvChange, ViewChange};
use crate::diff::destructiveness::Destructiveness;
use crate::diff::sequence_op::{SequenceOp, SequenceOpEntry};
use crate::diff::table_op::{TableOp, TableOpEntry};
use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::plan::ordered::{DeferredFkAdd, OrderedChangeSet};
use crate::plan::policy::PlannerPolicy;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

/// Context passed to every emitter — read-only.
struct Ctx<'a> {
    target: &'a Catalog,
    source: &'a Catalog,
    policy: &'a PlannerPolicy,
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
        emit_deferred_fk(&fk, &ctx, &mut out);
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
        Change::CreateSchema(s) => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateSchema,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![schema_target(&s.name)],
                sql: sql::create_schema(&s),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(c) = &s.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::AlterSchemaComment,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![schema_target(&s.name)],
                    sql: sql::comment_on_schema(&s.name, Some(c)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        Change::DropSchema(name) => out.push(RawStep {
            step_no: 0,
            kind: StepKind::DropSchema,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![schema_target(&name)],
            sql: sql::drop_schema(&name),
            transactional: TransactionConstraint::InTransaction,
        }),
        Change::AlterSchema { name, comment } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterSchemaComment,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![schema_target(&name)],
            sql: sql::comment_on_schema(&name, comment.as_deref()),
            transactional: TransactionConstraint::InTransaction,
        }),

        Change::CreateTable(t) => {
            let qname = t.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateTable,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: sql::create_table(&t),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(c) = &t.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::AlterTableSetComment,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: sql::comment_on_table(&qname, Some(c)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
            for col in &t.columns {
                if let Some(comment) = &col.comment {
                    out.push(RawStep {
                        step_no: 0,
                        kind: StepKind::SetColumnComment,
                        destructive: false,
                        destructive_reason: None,
                        intent_id: None,
                        targets: vec![qname.clone()],
                        sql: sql::comment_on_column(&qname, &col.name, Some(comment)),
                        transactional: TransactionConstraint::InTransaction,
                    });
                }
            }
            for c in &t.constraints {
                if let Some(cm) = &c.comment {
                    out.push(RawStep {
                        step_no: 0,
                        kind: StepKind::SetConstraintComment,
                        destructive: false,
                        destructive_reason: None,
                        intent_id: None,
                        targets: vec![qname.clone()],
                        sql: sql::comment_on_constraint(&qname, &c.qname.name, Some(cm)),
                        transactional: TransactionConstraint::InTransaction,
                    });
                }
            }
        }
        Change::DropTable { qname, .. } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::DropTable,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::drop_table(&qname),
            transactional: TransactionConstraint::InTransaction,
        }),
        Change::AlterTable { qname, ops } => {
            for op in ops {
                emit_table_op(&qname, op, ctx, out);
            }
        }

        Change::CreateIndex(idx) => {
            let qname = idx.qname.clone();
            let step = if concurrent_index::should_rewrite_create(&idx, ctx.target, ctx.policy) {
                concurrent_index::create_step(&idx, destructive, destructive_reason)
            } else {
                RawStep {
                    step_no: 0,
                    kind: StepKind::CreateIndex,
                    destructive,
                    destructive_reason,
                    intent_id: None,
                    targets: vec![qname.clone(), idx.on.qname().clone()],
                    sql: sql::create_index(&idx, false),
                    transactional: TransactionConstraint::InTransaction,
                }
            };
            out.push(step);
            if let Some(c) = &idx.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::SetColumnComment,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: format!(
                        "COMMENT ON INDEX {} IS '{}';",
                        qname.render_sql(),
                        c.replace('\'', "''"),
                    ),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        Change::DropIndex(qname) => {
            let step = if concurrent_index::should_rewrite_drop(&qname, ctx.target, ctx.policy) {
                concurrent_index::drop_step(&qname, destructive, destructive_reason)
            } else {
                RawStep {
                    step_no: 0,
                    kind: StepKind::DropIndex,
                    destructive,
                    destructive_reason,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: sql::drop_index(&qname, false),
                    transactional: TransactionConstraint::InTransaction,
                }
            };
            out.push(step);
        }
        Change::ReplaceIndex { from, to } => {
            // Drop the old index, then create the new one. Each side runs
            // through the same concurrent-rewrite check as a top-level
            // Create/Drop so policy applies uniformly.
            let drop_step =
                if concurrent_index::should_rewrite_drop(&from.qname, ctx.target, ctx.policy) {
                    concurrent_index::drop_step(&from.qname, false, None)
                } else {
                    RawStep {
                        step_no: 0,
                        kind: StepKind::DropIndex,
                        destructive: false,
                        destructive_reason: None,
                        intent_id: None,
                        targets: vec![from.qname.clone()],
                        sql: sql::drop_index(&from.qname, false),
                        transactional: TransactionConstraint::InTransaction,
                    }
                };
            let create_step =
                if concurrent_index::should_rewrite_create(&to, ctx.target, ctx.policy) {
                    concurrent_index::create_step(&to, false, None)
                } else {
                    RawStep {
                        step_no: 0,
                        kind: StepKind::CreateIndex,
                        destructive: false,
                        destructive_reason: None,
                        intent_id: None,
                        targets: vec![to.qname.clone(), to.on.qname().clone()],
                        sql: sql::create_index(&to, false),
                        transactional: TransactionConstraint::InTransaction,
                    }
                };
            out.push(drop_step);
            out.push(create_step);
        }

        Change::CreateSequence(s) => {
            let qname = s.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateSequence,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: sql::create_sequence(&s),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(c) = &s.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::AlterSequence,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: format!(
                        "COMMENT ON SEQUENCE {} IS '{}';",
                        qname.render_sql(),
                        c.replace('\'', "''"),
                    ),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        Change::DropSequence(qname) => out.push(RawStep {
            step_no: 0,
            kind: StepKind::DropSequence,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::drop_sequence(&qname),
            transactional: TransactionConstraint::InTransaction,
        }),
        Change::AlterSequence { qname, ops } => {
            for op in ops {
                emit_sequence_op(&qname, op, out);
            }
        }

        // Drift-recovery changes emitted from the DriftReport.
        Change::ValidateConstraint { table, constraint } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::ValidateConstraint,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![table.clone()],
                sql: sql::alter_table_validate_constraint(&table, &constraint),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::RecreateIndex { qname } => {
            // Drop the invalid index then re-create it from the source IR.
            // If the index is unknown in the source (e.g., it was dropped in the
            // same migration), emit only the drop so we don't fail the plan.
            let source_idx = ctx.source.indexes.iter().find(|i| i.qname == qname);
            // Drop step — use concurrent rewrite if policy allows and target
            // index exists (we know it does because the drift report saw it).
            let drop_step = if concurrent_index::should_rewrite_drop(&qname, ctx.target, ctx.policy)
            {
                concurrent_index::drop_step(&qname, false, None)
            } else {
                RawStep {
                    step_no: 0,
                    kind: StepKind::DropIndex,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: sql::drop_index(&qname, false),
                    transactional: TransactionConstraint::InTransaction,
                }
            };
            out.push(drop_step);
            if let Some(idx) = source_idx {
                let create_step =
                    if concurrent_index::should_rewrite_create(idx, ctx.target, ctx.policy) {
                        concurrent_index::create_step(idx, false, None)
                    } else {
                        RawStep {
                            step_no: 0,
                            kind: StepKind::CreateIndex,
                            destructive: false,
                            destructive_reason: None,
                            intent_id: None,
                            targets: vec![idx.qname.clone(), idx.on.qname().clone()],
                            sql: sql::create_index(idx, false),
                            transactional: TransactionConstraint::InTransaction,
                        }
                    };
                out.push(create_step);
            }
        }

        Change::View(vc) => emit_view_change(vc, destructive, destructive_reason, out),
        Change::Mv(mc) => emit_mv_change(mc, destructive, destructive_reason, out),
        // UserType SQL emission is wired in Task 9.
        Change::UserType(_) => {
            unimplemented!("Task 9 emits SQL for user-type changes")
        }
    }
}

fn emit_view_change(
    vc: ViewChange,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    use crate::diff::change::ViewChange as V;
    use views::{
        emit_alter_view_set_reloption, emit_comment_on_view, emit_comment_on_view_column,
        emit_create_view, emit_drop_view,
    };

    match vc {
        V::Create(v) => {
            let qname = v.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_create_view(&v, false),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(c) = &v.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnView,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: emit_comment_on_view(&qname, Some(c)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
            for col in &v.columns {
                if let Some(comment) = &col.comment {
                    out.push(RawStep {
                        step_no: 0,
                        kind: StepKind::CommentOnView,
                        destructive: false,
                        destructive_reason: None,
                        intent_id: None,
                        targets: vec![qname.clone()],
                        sql: emit_comment_on_view_column(&qname, &col.name, Some(comment)),
                        transactional: TransactionConstraint::InTransaction,
                    });
                }
            }
        }
        V::Drop(qname) => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropView,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_drop_view(&qname),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        V::ReplaceBody {
            source,
            catalog: _,
            compatible: true,
        } => {
            let qname = source.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname],
                sql: emit_create_view(&source, true),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        V::ReplaceBody {
            source,
            catalog,
            compatible: false,
        } => {
            let qname = source.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropView,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_drop_view(&catalog.qname),
                transactional: TransactionConstraint::InTransaction,
            });
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname],
                sql: emit_create_view(&source, false),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        V::SetReloption {
            qname,
            security_barrier,
            security_invoker,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterViewSetReloption,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_alter_view_set_reloption(&qname, security_barrier, security_invoker),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        V::SetComment { qname, comment } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_comment_on_view(&qname, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        V::SetColumnComment {
            qname,
            column,
            comment,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_comment_on_view_column(&qname, &column, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}

fn emit_mv_change(
    mc: MvChange,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    use crate::diff::change::MvChange as M;
    use views::{
        emit_comment_on_materialized_view, emit_comment_on_mv_column,
        emit_create_materialized_view, emit_drop_materialized_view, emit_refresh_mv,
    };

    match mc {
        M::Create(mv) => {
            let qname = mv.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_create_materialized_view(&mv),
                transactional: TransactionConstraint::InTransaction,
            });
            // Always follow up with a REFRESH; concurrently=false here — T8 flips it.
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::RefreshMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_refresh_mv(&qname, false),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(c) = &mv.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnView,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: emit_comment_on_materialized_view(&qname, Some(c)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
            for col in &mv.columns {
                if let Some(comment) = &col.comment {
                    out.push(RawStep {
                        step_no: 0,
                        kind: StepKind::CommentOnView,
                        destructive: false,
                        destructive_reason: None,
                        intent_id: None,
                        targets: vec![qname.clone()],
                        sql: emit_comment_on_mv_column(&qname, &col.name, Some(comment)),
                        transactional: TransactionConstraint::InTransaction,
                    });
                }
            }
        }
        M::Drop(qname) => {
            // MV drops are NOT destructive — materialized views are derived data.
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_drop_materialized_view(&qname),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        M::ReplaceBody { source, catalog } => {
            let qname = source.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_drop_materialized_view(&catalog.qname),
                transactional: TransactionConstraint::InTransaction,
            });
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_create_materialized_view(&source),
                transactional: TransactionConstraint::InTransaction,
            });
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::RefreshMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_refresh_mv(&qname, false),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        M::SetComment { qname, comment } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_comment_on_materialized_view(&qname, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        M::SetColumnComment {
            qname,
            column,
            comment,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_comment_on_mv_column(&qname, &column, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
    // MV drops always use destructive=false regardless of what the change entry
    // says; suppress "unused variable" lint for the two parameters.
    let _ = (destructive, destructive_reason);
}

fn emit_table_op(
    qname: &QualifiedName,
    entry: TableOpEntry,
    ctx: &Ctx<'_>,
    out: &mut Vec<RawStep>,
) {
    let destructive = entry.destructiveness.requires_approval();
    let destructive_reason = destructive_reason(&entry.destructiveness);
    match entry.op {
        TableOp::AddColumn(c) => out.push(RawStep {
            step_no: 0,
            kind: StepKind::AddColumn,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_table_add_column(qname, &c),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::DropColumn { name, .. } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::DropColumn,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_table_drop_column(qname, &name),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::AlterColumnType {
            name,
            from: _,
            to,
            using,
        } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterColumnType,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_type(qname, &name, &to, using.as_ref()),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetColumnNullable { name, nullable } => {
            // Only `SET NOT NULL` is eligible for the CHECK pattern; flipping a
            // column back to nullable is always a single cheap step.
            if !nullable
                && set_not_null_check_pattern::should_rewrite(qname, &name, ctx.target, ctx.policy)
            {
                for step in set_not_null_check_pattern::rewrite_steps(
                    qname,
                    &name,
                    destructive,
                    destructive_reason,
                ) {
                    out.push(step);
                }
            } else {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::SetColumnNullable,
                    destructive,
                    destructive_reason,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: sql::alter_column_set_nullable(qname, &name, nullable),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        TableOp::SetColumnDefault { name, default } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnDefault,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_set_default(qname, &name, default.as_ref()),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetColumnIdentity { name, identity } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnIdentity,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_set_identity(qname, &name, identity.as_ref()),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetColumnGenerated { name, generated } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnGenerated,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_set_generated(qname, &name, generated.as_ref()),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetColumnComment { name, comment } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnComment,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::comment_on_column(qname, &name, comment.as_deref()),
            transactional: TransactionConstraint::InTransaction,
        }),

        TableOp::AddConstraint(c) => {
            if fk_not_valid_validate::should_rewrite(qname, &c, ctx.target, ctx.policy) {
                let [a, b] = fk_not_valid_validate::rewrite_steps(
                    qname,
                    &c,
                    destructive,
                    destructive_reason,
                );
                out.push(a);
                out.push(b);
            } else if check_not_valid_validate::should_rewrite(qname, &c, ctx.target, ctx.policy) {
                let [a, b] = check_not_valid_validate::rewrite_steps(
                    qname,
                    &c,
                    destructive,
                    destructive_reason,
                );
                out.push(a);
                out.push(b);
            } else {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::AddConstraint,
                    destructive,
                    destructive_reason,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: sql::alter_table_add_constraint(qname, &c),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        TableOp::DropConstraint { name } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::DropConstraint,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_table_drop_constraint(qname, &name),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetConstraintComment { name, comment } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetConstraintComment,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::comment_on_constraint(qname, &name, comment.as_deref()),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetTableComment { comment } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterTableSetComment,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::comment_on_table(qname, comment.as_deref()),
            transactional: TransactionConstraint::InTransaction,
        }),
    }
}

fn emit_sequence_op(qname: &QualifiedName, entry: SequenceOpEntry, out: &mut Vec<RawStep>) {
    let destructive = entry.destructiveness.requires_approval();
    let destructive_reason = destructive_reason(&entry.destructiveness);
    let sql = match &entry.op {
        SequenceOp::SetIncrement(n) => sql::alter_sequence_increment(qname, *n),
        SequenceOp::SetMinValue(v) => sql::alter_sequence_min_value(qname, *v),
        SequenceOp::SetMaxValue(v) => sql::alter_sequence_max_value(qname, *v),
        SequenceOp::SetCache(n) => sql::alter_sequence_cache(qname, *n),
        SequenceOp::SetCycle(b) => sql::alter_sequence_cycle(qname, *b),
        SequenceOp::SetDataType(t) => sql::alter_sequence_data_type(qname, t),
        SequenceOp::SetOwnedBy(o) => sql::alter_sequence_owned_by(qname, o.as_ref()),
    };
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::AlterSequence,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: vec![qname.clone()],
        sql,
        transactional: TransactionConstraint::InTransaction,
    });
}

fn emit_deferred_fk(fk: &DeferredFkAdd, _ctx: &Ctx<'_>, out: &mut Vec<RawStep>) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::AddConstraint,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![fk.table.clone()],
        sql: sql::alter_table_add_constraint(&fk.table, &fk.constraint),
        transactional: TransactionConstraint::InTransaction,
    });
}

// `Schema` is identified by an `Identifier`, but `RawStep::targets` carries
// `QualifiedName`s. Promote the schema name to a `QualifiedName` whose schema
// half equals its name — same convention used for ordering in the planner's
// Phase 5 helpers.
fn schema_target(name: &crate::identifier::Identifier) -> QualifiedName {
    QualifiedName::new(name.clone(), name.clone())
}

fn destructive_reason(d: &Destructiveness) -> Option<String> {
    match d {
        Destructiveness::Safe => None,
        Destructiveness::RequiresApproval { reason }
        | Destructiveness::RequiresApprovalAndDataLossWarning { reason } => Some(reason.clone()),
    }
}

#[cfg(test)]
#[allow(clippy::too_many_lines)]
mod tests {
    use super::*;
    use crate::diff::change::Change;
    use crate::diff::changeset::ChangeSet;
    use crate::diff::destructiveness::Destructiveness;
    use crate::diff::table_op::TableOp;
    use crate::identifier::Identifier;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::constraint::{
        Constraint, ConstraintKind, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
    };
    use crate::ir::index::{
        Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
    };
    use crate::ir::schema::Schema;
    use crate::ir::sequence::Sequence;
    use crate::ir::table::Table;
    use crate::plan::ordered::OrderedChangeSet;
    use crate::plan::ordering::order;
    use crate::plan::policy::PlannerPolicy;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn col(name: &str, ty: ColumnType, nullable: bool) -> Column {
        Column {
            name: id(name),
            ty,
            nullable,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            comment: None,
        }
    }

    fn pk(name: &str, cols: &[&str]) -> Constraint {
        Constraint {
            qname: qn("app", name),
            kind: ConstraintKind::PrimaryKey {
                columns: cols.iter().map(|c| id(c)).collect(),
                include: vec![],
            },
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    fn make_index(name: &str, table: QualifiedName, unique: bool) -> Index {
        Index {
            qname: qn("app", name),
            on: IndexParent::Table(table),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("id")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
        }
    }

    fn rewrite_with_default(
        target: &Catalog,
        source: &Catalog,
        changes: ChangeSet,
    ) -> Vec<RawStep> {
        let policy = PlannerPolicy::default();
        let ordered = order(target, source, changes, &policy).unwrap();
        rewrite(ordered, target, &policy)
    }

    fn rewrite_changeset_only(changes: ChangeSet) -> Vec<RawStep> {
        rewrite(
            OrderedChangeSet {
                creates_and_adds: changes.entries,
                modifies: vec![],
                drops: vec![],
                deferred_fks: vec![],
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        )
    }

    #[test]
    fn empty_ordered_set_yields_no_steps() {
        let policy = PlannerPolicy::default();
        let steps = rewrite(OrderedChangeSet::default(), &Catalog::empty(), &policy);
        assert!(steps.is_empty());
    }

    #[test]
    fn create_schema_emits_single_step() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::CreateSchema(Schema::new(id("app"))),
            Destructiveness::Safe,
        );
        let mut source = Catalog::empty();
        source.schemas.push(Schema::new(id("app")));
        let steps = rewrite_with_default(&Catalog::empty(), &source, cs);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::CreateSchema);
        assert_eq!(steps[0].sql, "CREATE SCHEMA app;");
        assert_eq!(steps[0].transactional, TransactionConstraint::InTransaction);
        assert!(!steps[0].destructive);
    }

    #[test]
    fn create_schema_with_comment_emits_two_steps() {
        let s = Schema {
            name: id("app"),
            comment: Some("the app".into()),
        };
        let mut cs = ChangeSet::new();
        cs.push(Change::CreateSchema(s), Destructiveness::Safe);
        let steps = rewrite_changeset_only(cs);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].kind, StepKind::CreateSchema);
        assert_eq!(steps[1].kind, StepKind::AlterSchemaComment);
        assert_eq!(steps[1].sql, "COMMENT ON SCHEMA app IS 'the app';");
    }

    #[test]
    fn drop_schema_marks_destructive() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::DropSchema(id("legacy")),
            Destructiveness::RequiresApproval {
                reason: "drop schema".into(),
            },
        );
        let steps = rewrite(
            OrderedChangeSet {
                drops: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps[0].kind, StepKind::DropSchema);
        assert!(steps[0].destructive);
        assert_eq!(steps[0].sql, "DROP SCHEMA legacy;");
    }

    #[test]
    fn alter_schema_emits_comment_step() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterSchema {
                name: id("app"),
                comment: Some("v2".into()),
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::AlterSchemaComment);
        assert_eq!(steps[0].sql, "COMMENT ON SCHEMA app IS 'v2';");
    }

    #[test]
    fn create_table_emits_full_create_with_columns_and_pk() {
        let t = Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("email", ColumnType::Text, true),
            ],
            constraints: vec![pk("users_pkey", &["id"])],
            comment: None,
        };
        let mut cs = ChangeSet::new();
        cs.push(Change::CreateTable(t), Destructiveness::Safe);
        let steps = rewrite_changeset_only(cs);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::CreateTable);
        assert!(steps[0].sql.starts_with("CREATE TABLE app.users ("));
        assert!(steps[0].sql.contains("id bigint NOT NULL"));
        assert!(steps[0].sql.contains("email text"));
        assert!(
            steps[0]
                .sql
                .contains("CONSTRAINT users_pkey PRIMARY KEY (id)")
        );
    }

    #[test]
    fn drop_table_marks_destructive() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::DropTable {
                qname: qn("app", "old"),
                row_count_estimate: Some(100),
            },
            Destructiveness::RequiresApprovalAndDataLossWarning {
                reason: "drops table".into(),
            },
        );
        let steps = rewrite(
            OrderedChangeSet {
                drops: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps[0].kind, StepKind::DropTable);
        assert!(steps[0].destructive);
        assert_eq!(steps[0].sql, "DROP TABLE app.old;");
    }

    #[test]
    fn create_index_emits_basic_create() {
        // Index on a fresh table → not eligible for concurrent rewrite.
        let mut cs = ChangeSet::new();
        cs.push(
            Change::CreateIndex(make_index("users_idx", qn("app", "users"), false)),
            Destructiveness::Safe,
        );
        let steps = rewrite_changeset_only(cs);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::CreateIndex);
        assert!(
            steps[0]
                .sql
                .starts_with("CREATE INDEX users_idx ON app.users USING btree (id)")
        );
        assert_eq!(steps[0].transactional, TransactionConstraint::InTransaction);
    }

    #[test]
    fn drop_index_emits_plain_drop_in_default_path() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::DropIndex(qn("app", "users_idx")),
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                drops: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps[0].kind, StepKind::DropIndex);
        assert_eq!(steps[0].sql, "DROP INDEX app.users_idx;");
    }

    #[test]
    fn replace_index_emits_drop_then_create() {
        let from = make_index("users_idx", qn("app", "users"), false);
        let to = make_index("users_idx", qn("app", "users"), true);
        let mut cs = ChangeSet::new();
        cs.push(Change::ReplaceIndex { from, to }, Destructiveness::Safe);
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].kind, StepKind::DropIndex);
        assert_eq!(steps[1].kind, StepKind::CreateIndex);
        assert!(steps[1].sql.contains("UNIQUE INDEX"));
    }

    #[test]
    fn create_sequence_emits_full_create() {
        let s = Sequence {
            qname: qn("app", "id_seq"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cache: 1,
            cycle: false,
            owned_by: None,
            comment: None,
        };
        let mut cs = ChangeSet::new();
        cs.push(Change::CreateSequence(s), Destructiveness::Safe);
        let steps = rewrite_changeset_only(cs);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::CreateSequence);
        assert!(
            steps[0]
                .sql
                .starts_with("CREATE SEQUENCE app.id_seq AS bigint")
        );
        assert!(steps[0].sql.contains("INCREMENT BY 1"));
        assert!(steps[0].sql.contains("START WITH 1"));
        assert!(steps[0].sql.contains("NO CYCLE"));
    }

    #[test]
    fn alter_table_add_column_emits_step() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::AddColumn(col("email", ColumnType::Text, true)),
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::AddColumn);
        assert_eq!(steps[0].sql, "ALTER TABLE app.users ADD COLUMN email text;");
    }

    #[test]
    fn alter_table_drop_column_marks_destructive() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::DropColumn {
                        name: id("email"),
                        is_populated: true,
                    },
                    destructiveness: Destructiveness::RequiresApprovalAndDataLossWarning {
                        reason: "drop col".into(),
                    },
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert!(steps[0].destructive);
        assert_eq!(steps[0].sql, "ALTER TABLE app.users DROP COLUMN email;");
    }

    #[test]
    fn alter_column_type_emits_using_clause_when_present() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::AlterColumnType {
                        name: id("count"),
                        from: ColumnType::Integer,
                        to: ColumnType::BigInt,
                        using: Some(crate::ir::default_expr::NormalizedExpr::from_text(
                            "count::bigint",
                        )),
                    },
                    destructiveness: Destructiveness::RequiresApproval {
                        reason: "type change".into(),
                    },
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(
            steps[0].sql,
            "ALTER TABLE app.users ALTER COLUMN count TYPE bigint USING count::bigint;",
        );
    }

    #[test]
    fn set_column_nullable_distinguishes_directions() {
        for (nullable, expected) in [
            (true, "ALTER TABLE app.users ALTER COLUMN c DROP NOT NULL;"),
            (false, "ALTER TABLE app.users ALTER COLUMN c SET NOT NULL;"),
        ] {
            let mut cs = ChangeSet::new();
            cs.push(
                Change::AlterTable {
                    qname: qn("app", "users"),
                    ops: vec![TableOpEntry {
                        op: TableOp::SetColumnNullable {
                            name: id("c"),
                            nullable,
                        },
                        destructiveness: Destructiveness::Safe,
                    }],
                },
                Destructiveness::Safe,
            );
            let steps = rewrite(
                OrderedChangeSet {
                    modifies: cs.entries,
                    ..Default::default()
                },
                &Catalog::empty(),
                &PlannerPolicy::default(),
            );
            assert_eq!(steps[0].sql, expected);
        }
    }

    #[test]
    fn add_constraint_emits_single_step_in_default_path() {
        // Non-FK, non-CHECK constraint → no rewrite ever applies, even with
        // online policy. (Unique constraints stay simple.)
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::AddConstraint(Constraint {
                        qname: qn("app", "users_email_uq"),
                        kind: ConstraintKind::Unique {
                            columns: vec![id("email")],
                            include: vec![],
                            nulls_distinct: true,
                        },
                        deferrable: Deferrable::NotDeferrable,
                        comment: None,
                    }),
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::AddConstraint);
        assert!(steps[0].sql.contains("UNIQUE"));
    }

    #[test]
    fn drop_constraint_emits_step() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::DropConstraint {
                        name: id("users_email_uq"),
                    },
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps[0].kind, StepKind::DropConstraint);
        assert_eq!(
            steps[0].sql,
            "ALTER TABLE app.users DROP CONSTRAINT users_email_uq;",
        );
    }

    #[test]
    fn alter_sequence_set_increment_emits_step() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterSequence {
                qname: qn("app", "s1"),
                ops: vec![SequenceOpEntry {
                    op: SequenceOp::SetIncrement(2),
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps[0].kind, StepKind::AlterSequence);
        assert_eq!(steps[0].sql, "ALTER SEQUENCE app.s1 INCREMENT BY 2;");
    }

    #[test]
    fn alter_sequence_set_owned_by_renders_qualified_owner() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterSequence {
                qname: qn("app", "s1"),
                ops: vec![SequenceOpEntry {
                    op: SequenceOp::SetOwnedBy(Some(crate::ir::sequence::SequenceOwner {
                        table: qn("app", "users"),
                        column: id("id"),
                    })),
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps[0].sql, "ALTER SEQUENCE app.s1 OWNED BY app.users.id;");
    }

    #[test]
    fn drop_sequence_marks_destructive() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::DropSequence(qn("app", "s1")),
            Destructiveness::RequiresApproval {
                reason: "drop seq".into(),
            },
        );
        let steps = rewrite(
            OrderedChangeSet {
                drops: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert!(steps[0].destructive);
        assert_eq!(steps[0].sql, "DROP SEQUENCE app.s1;");
    }

    #[test]
    fn deferred_fk_emits_alter_table_add_constraint() {
        let fk = DeferredFkAdd {
            table: qn("app", "a"),
            constraint: Constraint {
                qname: qn("app", "a_b_fk"),
                kind: ConstraintKind::ForeignKey(ForeignKey {
                    columns: vec![id("ref_id")],
                    referenced_table: qn("app", "b"),
                    referenced_columns: vec![id("id")],
                    on_update: ReferentialAction::NoAction,
                    on_delete: ReferentialAction::NoAction,
                    match_type: FkMatchType::Simple,
                }),
                deferrable: Deferrable::NotDeferrable,
                comment: None,
            },
        };
        let steps = rewrite(
            OrderedChangeSet {
                deferred_fks: vec![fk],
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::AddConstraint);
        assert!(
            steps[0]
                .sql
                .contains("ADD CONSTRAINT a_b_fk FOREIGN KEY (ref_id) REFERENCES app.b (id)")
        );
    }

    // ---- concurrent-index rewrite (Task 6.4) ----

    #[test]
    fn create_index_on_existing_table_rewrites_to_concurrent() {
        let mut target = Catalog::empty();
        target.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            comment: None,
        });

        let idx = make_index("users_idx", qn("app", "users"), false);
        let mut cs = ChangeSet::new();
        cs.push(Change::CreateIndex(idx), Destructiveness::Safe);

        let steps = rewrite(
            OrderedChangeSet {
                creates_and_adds: cs.entries,
                ..Default::default()
            },
            &target,
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::CreateIndexConcurrent);
        assert_eq!(
            steps[0].transactional,
            TransactionConstraint::OutsideTransaction,
        );
        assert!(steps[0].sql.contains("CONCURRENTLY"));
    }

    #[test]
    fn create_index_on_new_table_stays_inline() {
        // Empty target ⇒ users is being created in this plan ⇒ no concurrent rewrite.
        let idx = make_index("users_idx", qn("app", "users"), false);
        let mut cs = ChangeSet::new();
        cs.push(Change::CreateIndex(idx), Destructiveness::Safe);

        let steps = rewrite(
            OrderedChangeSet {
                creates_and_adds: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps[0].kind, StepKind::CreateIndex);
        assert_eq!(steps[0].transactional, TransactionConstraint::InTransaction);
        assert!(!steps[0].sql.contains("CONCURRENTLY"));
    }

    #[test]
    fn unique_create_index_does_not_rewrite_to_concurrent() {
        let mut target = Catalog::empty();
        target.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            comment: None,
        });

        let idx = make_index("users_email_idx", qn("app", "users"), true);
        let mut cs = ChangeSet::new();
        cs.push(Change::CreateIndex(idx), Destructiveness::Safe);

        let steps = rewrite(
            OrderedChangeSet {
                creates_and_adds: cs.entries,
                ..Default::default()
            },
            &target,
            &PlannerPolicy::default(),
        );
        assert_eq!(steps[0].kind, StepKind::CreateIndex);
        assert!(steps[0].sql.contains("UNIQUE INDEX"));
        assert!(!steps[0].sql.contains("CONCURRENTLY"));
    }

    #[test]
    fn atomic_policy_disables_concurrent_index_rewrite() {
        let mut target = Catalog::empty();
        target.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            comment: None,
        });

        let idx = make_index("users_idx", qn("app", "users"), false);
        let mut cs = ChangeSet::new();
        cs.push(Change::CreateIndex(idx), Destructiveness::Safe);

        let policy = PlannerPolicy {
            strategy: crate::plan::policy::Strategy::Atomic,
            ..PlannerPolicy::default()
        };
        let steps = rewrite(
            OrderedChangeSet {
                creates_and_adds: cs.entries,
                ..Default::default()
            },
            &target,
            &policy,
        );
        assert_eq!(steps[0].kind, StepKind::CreateIndex);
        assert!(!steps[0].sql.contains("CONCURRENTLY"));
    }

    #[test]
    fn drop_index_on_existing_non_unique_rewrites_to_concurrent() {
        let mut target = Catalog::empty();
        target
            .indexes
            .push(make_index("users_idx", qn("app", "users"), false));

        let mut cs = ChangeSet::new();
        cs.push(
            Change::DropIndex(qn("app", "users_idx")),
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                drops: cs.entries,
                ..Default::default()
            },
            &target,
            &PlannerPolicy::default(),
        );
        assert_eq!(steps[0].kind, StepKind::DropIndexConcurrent);
        assert_eq!(
            steps[0].transactional,
            TransactionConstraint::OutsideTransaction
        );
    }

    #[test]
    fn drop_unique_index_stays_inline() {
        let mut target = Catalog::empty();
        target
            .indexes
            .push(make_index("users_idx", qn("app", "users"), true));

        let mut cs = ChangeSet::new();
        cs.push(
            Change::DropIndex(qn("app", "users_idx")),
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                drops: cs.entries,
                ..Default::default()
            },
            &target,
            &PlannerPolicy::default(),
        );
        assert_eq!(steps[0].kind, StepKind::DropIndex);
        assert_eq!(steps[0].transactional, TransactionConstraint::InTransaction);
    }

    #[test]
    fn drop_index_unknown_in_target_stays_inline() {
        // If the index isn't in the target catalog, we can't tell whether
        // it's unique. Default to the safe inline form.
        let mut cs = ChangeSet::new();
        cs.push(
            Change::DropIndex(qn("app", "users_idx")),
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                drops: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps[0].kind, StepKind::DropIndex);
    }

    // ---- FK NOT VALID + VALIDATE rewrite (Task 6.5) ----

    fn fk(name: &str, ref_table: QualifiedName) -> Constraint {
        Constraint {
            qname: qn("app", name),
            kind: ConstraintKind::ForeignKey(ForeignKey {
                columns: vec![id("ref_id")],
                referenced_table: ref_table,
                referenced_columns: vec![id("id")],
                on_update: ReferentialAction::NoAction,
                on_delete: ReferentialAction::NoAction,
                match_type: FkMatchType::Simple,
            }),
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    #[test]
    fn add_fk_on_existing_table_emits_two_steps() {
        let mut target = Catalog::empty();
        target.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("ref_id", ColumnType::BigInt, false),
            ],
            constraints: vec![],
            comment: None,
        });
        target.tables.push(Table {
            qname: qn("app", "orgs"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            comment: None,
        });

        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::AddConstraint(fk("users_orgs_fk", qn("app", "orgs"))),
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &target,
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].kind, StepKind::AddConstraintNotValid);
        assert!(steps[0].sql.contains("NOT VALID"));
        assert_eq!(steps[1].kind, StepKind::ValidateConstraint);
        assert_eq!(
            steps[1].sql,
            "ALTER TABLE app.users VALIDATE CONSTRAINT users_orgs_fk;",
        );
    }

    #[test]
    fn add_fk_on_new_table_via_alter_stays_inline_when_target_missing() {
        // Target is empty, so users does not yet exist ⇒ no rewrite.
        // (In practice an FK on a brand-new table would ride inside the
        // CREATE TABLE — we exercise the alter-path edge case here.)
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::AddConstraint(fk("users_orgs_fk", qn("app", "orgs"))),
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::AddConstraint);
    }

    #[test]
    fn add_fk_with_atomic_policy_stays_inline() {
        let mut target = Catalog::empty();
        target.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("ref_id", ColumnType::BigInt, false),
            ],
            constraints: vec![],
            comment: None,
        });

        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::AddConstraint(fk("users_orgs_fk", qn("app", "orgs"))),
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let policy = PlannerPolicy {
            strategy: crate::plan::policy::Strategy::Atomic,
            ..PlannerPolicy::default()
        };
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &target,
            &policy,
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::AddConstraint);
    }

    #[test]
    fn add_unique_constraint_on_existing_table_does_not_trigger_fk_rewrite() {
        let mut target = Catalog::empty();
        target.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("email", ColumnType::Text, true),
            ],
            constraints: vec![],
            comment: None,
        });

        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::AddConstraint(Constraint {
                        qname: qn("app", "users_email_uq"),
                        kind: ConstraintKind::Unique {
                            columns: vec![id("email")],
                            include: vec![],
                            nulls_distinct: true,
                        },
                        deferrable: Deferrable::NotDeferrable,
                        comment: None,
                    }),
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &target,
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::AddConstraint);
    }

    // ---- CHECK NOT VALID + VALIDATE rewrite (Task 6.6) ----

    fn check(name: &str, expr: &str) -> Constraint {
        Constraint {
            qname: qn("app", name),
            kind: ConstraintKind::Check {
                expression: crate::ir::default_expr::NormalizedExpr::from_text(expr),
                no_inherit: false,
            },
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    #[test]
    fn add_check_on_existing_table_emits_two_steps() {
        let mut target = Catalog::empty();
        target.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col("age", ColumnType::Integer, true)],
            constraints: vec![],
            comment: None,
        });

        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::AddConstraint(check("users_age_chk", "age >= 0")),
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &target,
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].kind, StepKind::AddConstraintNotValid);
        assert!(steps[0].sql.contains("CHECK (age >= 0)"));
        assert!(steps[0].sql.contains("NOT VALID"));
        assert_eq!(steps[1].kind, StepKind::ValidateConstraint);
    }

    #[test]
    fn add_check_on_new_table_via_alter_stays_inline_when_target_missing() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::AddConstraint(check("users_age_chk", "age >= 0")),
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::AddConstraint);
    }

    #[test]
    fn add_check_with_atomic_policy_stays_inline() {
        let mut target = Catalog::empty();
        target.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col("age", ColumnType::Integer, true)],
            constraints: vec![],
            comment: None,
        });

        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::AddConstraint(check("users_age_chk", "age >= 0")),
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let policy = PlannerPolicy {
            strategy: crate::plan::policy::Strategy::Atomic,
            ..PlannerPolicy::default()
        };
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &target,
            &policy,
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::AddConstraint);
    }

    // ---- SET NOT NULL via CHECK pattern (Task 6.7) ----

    fn target_with_users_and_email() -> Catalog {
        let mut target = Catalog::empty();
        target.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("email", ColumnType::Text, true),
            ],
            constraints: vec![],
            comment: None,
        });
        target
    }

    #[test]
    fn set_not_null_on_existing_column_emits_four_steps() {
        let target = target_with_users_and_email();
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::SetColumnNullable {
                        name: id("email"),
                        nullable: false,
                    },
                    destructiveness: Destructiveness::RequiresApproval {
                        reason: "set not null".into(),
                    },
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &target,
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0].kind, StepKind::AddCheckForNotNull);
        assert!(steps[0].sql.contains("__pgevolve_chk_email"));
        assert!(steps[0].sql.contains("CHECK (email IS NOT NULL)"));
        assert!(steps[0].sql.contains("NOT VALID"));
        assert_eq!(steps[1].kind, StepKind::ValidateConstraint);
        assert!(steps[1].sql.contains("__pgevolve_chk_email"));
        assert_eq!(steps[2].kind, StepKind::SetColumnNullable);
        assert_eq!(
            steps[2].sql,
            "ALTER TABLE app.users ALTER COLUMN email SET NOT NULL;",
        );
        assert_eq!(steps[3].kind, StepKind::DropConstraint);
        assert!(steps[3].sql.contains("__pgevolve_chk_email"));
    }

    #[test]
    fn set_not_null_on_unknown_column_stays_single_step() {
        // email isn't in the (empty) target ⇒ this is a new column path; the
        // existing AddColumn would carry NOT NULL inline, but if the differ
        // happens to emit a bare SetColumnNullable it should remain one step.
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::SetColumnNullable {
                        name: id("email"),
                        nullable: false,
                    },
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::SetColumnNullable);
    }

    #[test]
    fn set_not_null_with_atomic_policy_stays_single_step() {
        let target = target_with_users_and_email();
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::SetColumnNullable {
                        name: id("email"),
                        nullable: false,
                    },
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let policy = PlannerPolicy {
            strategy: crate::plan::policy::Strategy::Atomic,
            ..PlannerPolicy::default()
        };
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &target,
            &policy,
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::SetColumnNullable);
    }

    #[test]
    fn drop_not_null_is_always_single_step() {
        // Going from NOT NULL to nullable never needs the CHECK pattern.
        let target = target_with_users_and_email();
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::SetColumnNullable {
                        name: id("email"),
                        nullable: true,
                    },
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &target,
            &PlannerPolicy::default(),
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::SetColumnNullable);
        assert!(steps[0].sql.contains("DROP NOT NULL"));
    }

    #[test]
    fn rewrite_preserves_bucket_order_creates_modifies_drops() {
        let mut source = Catalog::empty();
        source.schemas.push(Schema::new(id("app")));
        source.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            comment: None,
        });

        let mut cs = ChangeSet::new();
        cs.push(
            Change::CreateSchema(Schema::new(id("app"))),
            Destructiveness::Safe,
        );
        cs.push(
            Change::CreateTable(source.tables[0].clone()),
            Destructiveness::Safe,
        );
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![],
            },
            Destructiveness::Safe,
        );
        cs.push(
            Change::DropSchema(id("legacy")),
            Destructiveness::RequiresApproval { reason: "x".into() },
        );

        let mut target = Catalog::empty();
        target.schemas.push(Schema::new(id("legacy")));
        let steps = rewrite_with_default(&target, &source, cs);
        let kinds: Vec<StepKind> = steps.iter().map(|s| s.kind).collect();
        // Creates first (schema, table), then modifies (alter table — produces no
        // child ops for empty `ops`), then drops (drop schema).
        assert_eq!(
            kinds,
            vec![
                StepKind::CreateSchema,
                StepKind::CreateTable,
                StepKind::DropSchema
            ]
        );
    }
}
