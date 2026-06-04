//! Three-phase ordering with FK cycle extraction.
//!
//! `order(target, source, changes)` partitions an unordered [`ChangeSet`]
//! into [`OrderedChangeSet`]'s three buckets and sorts each by the appropriate
//! dependency graph. FK cycles in the create graph are broken by removing
//! offending FK constraints into [`OrderedChangeSet::deferred_fks`].

use std::collections::{HashMap, HashSet};

use crate::diff::ChangeSet;
use crate::diff::change::{
    Change, ChangeEntry, CollationChange, FunctionChange, MvChange, ProcedureChange,
    PublicationChange, StatisticChange, SubscriptionChange, TableChange, TriggerChange,
    UserTypeChange, ViewChange,
};
use crate::diff::destructiveness::Destructiveness;
use crate::diff::table_op::TableOp;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::constraint::{Constraint, ConstraintKind};
use crate::ir::index::IndexColumnExpr;
use crate::plan::edges::{NodeId, build_create_graph, build_drop_graph};
use crate::plan::error::PlanError;
use crate::plan::ordered::{DeferredFkAdd, OrderedChangeSet};
use crate::plan::policy::PlannerPolicy;
use crate::plan::recreate_views;

/// Order a `ChangeSet` into an [`OrderedChangeSet`] for plan emission.
///
/// `target` is the live database catalog; `source` is the desired one. The
/// create / modify graphs are built from `source`; the drop graph from `target`.
///
/// `policy` gates the dependent-view recreation walk: when
/// `policy.view_drop_create_dependents()` is `false` and any change would
/// force dependent views to be recreated, this function returns
/// [`PlanError::DependentViewsBlocked`] naming the affected views.
pub fn order(
    target: &Catalog,
    source: &Catalog,
    changes: ChangeSet,
    policy: &PlannerPolicy,
) -> Result<OrderedChangeSet, PlanError> {
    // Elide DropIndex changes whose target index will be cascade-dropped by
    // an upstream `ALTER TABLE ... DROP COLUMN` in the same plan. Postgres
    // implicitly drops any index that references a dropped column (in its
    // key list or `INCLUDE` list); leaving the explicit `DROP INDEX` in the
    // plan causes the executor to fail with SQLSTATE 42704.
    let changes = elide_cascaded_index_drops(target, changes);

    // Extend the changeset with explicit DROP + CREATE steps for every
    // transitively-affected view (never CASCADE). This must happen before
    // partitioning so the new ReplaceBody entries flow through the normal
    // ordering pipeline.
    //
    // `extend_with_dependent_recreations` only appends to `raw_changes`; it
    // never modifies existing entries. We preserve the original
    // `Destructiveness` values for all original entries and assign `Safe` to
    // any newly-appended ones (dependent view recreation is not destructive —
    // the view body itself is unchanged).
    let original_entries: Vec<crate::diff::change::ChangeEntry> = changes.entries;
    let mut raw_changes: Vec<Change> = original_entries.iter().map(|e| e.change.clone()).collect();
    recreate_views::extend_with_dependent_recreations(&mut raw_changes, target, policy)
        .map_err(|views| PlanError::DependentViewsBlocked { views })?;
    // Re-assemble ChangeSet preserving original destructiveness for original
    // entries, and using Safe for any newly-added recreation entries.
    let entries: Vec<crate::diff::change::ChangeEntry> = raw_changes
        .into_iter()
        .enumerate()
        .map(|(i, change)| {
            let destructiveness = original_entries
                .get(i)
                .map_or(Destructiveness::Safe, |e| e.destructiveness.clone());
            crate::diff::change::ChangeEntry {
                change,
                destructiveness,
            }
        })
        .collect();
    let changes = ChangeSet {
        entries,
        ..ChangeSet::new()
    };

    // 1. Bucket entries by phase. Returns Err if any UnsupportedDiff is present.
    let (creates, modifies, drops) = partition(changes)?;

    // 2. Try to topo-sort the create graph; extract FK cycles if needed.
    let mut working_source: Option<Catalog> = None;
    let create_graph = build_create_graph(source);
    let (sorted_create_nodes, deferred_fks) = match create_graph.topological_sort() {
        Ok(order) => (order, Vec::new()),
        Err(cycle) => {
            let (reduced, deferred) = extract_fk_cycles(source, &cycle.nodes);
            let g = build_create_graph(&reduced);
            let order = g.topological_sort().map_err(|c| {
                PlanError::UnbreakableCycle(c.nodes.iter().map(render_node).collect())
            })?;
            working_source = Some(reduced);
            (order, deferred)
        }
    };

    // The graph used for modify ordering is the same as the (possibly reduced)
    // create graph — modify ops live on already-existing objects whose
    // structural dependencies match the source-side picture.
    let modify_graph = working_source
        .as_ref()
        .map_or(create_graph, build_create_graph);
    let sorted_modify_nodes = modify_graph.topological_sort().map_err(|c| {
        PlanError::UnexpectedCycleAfterFkExtraction(c.nodes.iter().map(render_node).collect())
    })?;

    let drop_graph = build_drop_graph(target);
    let sorted_drop_nodes = drop_graph
        .reverse_topological_sort()
        .map_err(|c| PlanError::UnexpectedDropCycle(c.nodes.iter().map(render_node).collect()))?;

    // Strip deferred FKs from any CreateTable change so they aren't emitted
    // both inline and as a post-pass `ADD CONSTRAINT`.
    let creates = strip_deferred_fks(creates, &deferred_fks);

    let creates = sort_changes_by_order(creates, &sorted_create_nodes);
    let modifies = sort_changes_by_order(modifies, &sorted_modify_nodes);
    let drops = sort_changes_by_order(drops, &sorted_drop_nodes);

    // Tier rule for subscriptions: subscriptions cross-reference publications
    // in a *different* cluster — no local dep edges anchor them. To minimize
    // the window where a referenced object might be missing:
    //   - In the creates bucket: subscription creates land after all other creates.
    //   - In the drops bucket:   subscription drops land before all other drops.
    let creates = tier_subscriptions_last(creates);
    let drops = tier_subscriptions_first(drops);

    Ok(OrderedChangeSet {
        creates_and_adds: creates,
        modifies,
        drops,
        deferred_fks,
    })
}

/// Remove every constraint named in `deferred` from any matching `CreateTable`
/// change. The deferred FK will be emitted as a post-pass `ADD CONSTRAINT`.
fn strip_deferred_fks(creates: Vec<ChangeEntry>, deferred: &[DeferredFkAdd]) -> Vec<ChangeEntry> {
    if deferred.is_empty() {
        return creates;
    }
    creates
        .into_iter()
        .map(|mut entry| {
            if let Change::CreateTable(table) = &mut entry.change {
                table.constraints.retain(|c| {
                    !deferred
                        .iter()
                        .any(|d| d.table == table.qname && d.constraint.qname == c.qname)
                });
            }
            entry
        })
        .collect()
}

/// Remove `DropIndex` changes whose target index will be implicitly dropped
/// by an upstream `ALTER TABLE ... DROP COLUMN` on a column the index
/// references (in its key column list or `INCLUDE` list).
///
/// Postgres cascade-drops such indexes as part of the column drop, so the
/// explicit `DROP INDEX` in a later step would fail with
/// `42704 (undefined_object)`. The elision keeps the audit trail attached
/// to the column-drop step that does the actual work.
///
/// Limitations: expression-key indexes and partial-index predicates are not
/// analyzed; their `DropIndex` is retained even if the predicate or
/// expression references the dropped column. The executor will surface
/// those (still rare) cases as the same 42704 until expression analysis is
/// added.
fn elide_cascaded_index_drops(target: &Catalog, mut changes: ChangeSet) -> ChangeSet {
    let dropped_columns = collect_dropped_columns(&changes);
    if dropped_columns.is_empty() {
        return changes;
    }
    let target_indexes: HashMap<&QualifiedName, &crate::ir::index::Index> =
        target.indexes.iter().map(|i| (&i.qname, i)).collect();
    changes.entries.retain(|entry| {
        let Change::DropIndex(qname) = &entry.change else {
            return true;
        };
        let Some(idx) = target_indexes.get(qname) else {
            return true;
        };
        let cascades = idx
            .columns
            .iter()
            .filter_map(|ic| match &ic.expr {
                IndexColumnExpr::Column(name) => Some(name),
                IndexColumnExpr::Expression(_) => None,
            })
            .chain(idx.include.iter())
            .any(|col| dropped_columns.contains(&(idx.on.qname().clone(), col.clone())));
        !cascades
    });
    changes
}

fn collect_dropped_columns(changes: &ChangeSet) -> HashSet<(QualifiedName, Identifier)> {
    let mut out = HashSet::new();
    for entry in &changes.entries {
        let Change::AlterTable { qname, ops } = &entry.change else {
            continue;
        };
        for op_entry in ops {
            if let TableOp::DropColumn { name, .. } = &op_entry.op {
                out.insert((qname.clone(), name.clone()));
            }
        }
    }
    out
}

/// Three-bucket output of [`partition`]: (creates, modifies, drops).
type PartitionResult = Result<(Vec<ChangeEntry>, Vec<ChangeEntry>, Vec<ChangeEntry>), PlanError>;

/// Split a `ChangeSet` into (creates, modifies, drops) buckets.
///
/// Returns `Err(PlanError::Internal)` immediately if any entry is a
/// `Change::UnsupportedDiff` — the plan cannot proceed.
#[allow(clippy::too_many_lines)] // One arm per Change variant; extraction would obscure intent.
fn partition(changes: ChangeSet) -> PartitionResult {
    let mut creates = Vec::new();
    let mut modifies = Vec::new();
    let mut drops = Vec::new();
    for entry in changes.entries {
        match &entry.change {
            // Creates: structural objects that need to be ordered dependencies-first.
            Change::CreateSchema(_)
            | Change::CreateTable(_)
            | Change::CreateIndex(_)
            | Change::CreateSequence(_)
            | Change::View(ViewChange::Create(_))
            | Change::Mv(MvChange::Create(_))
            | Change::Publication(PublicationChange::Create(_))
            | Change::Subscription(SubscriptionChange::Create(_))
            | Change::Statistic(StatisticChange::Create(_))
            | Change::Collation(CollationChange::Create(_)) => creates.push(entry),
            // Drops: ordered by reverse-dependency (deepest dependents first).
            Change::DropSchema(_)
            | Change::DropTable { .. }
            | Change::DropIndex(_)
            | Change::DropSequence(_)
            | Change::View(ViewChange::Drop(_))
            | Change::Mv(MvChange::Drop(_))
            | Change::Subscription(SubscriptionChange::Drop { .. })
            | Change::Statistic(
                StatisticChange::Drop { .. } | StatisticChange::Replace { .. },
            )
            | Change::Collation(
                CollationChange::Drop { .. } | CollationChange::Replace { .. },
            ) => drops.push(entry),
            // Modifies: ALTER / REPLACE / COMMENT / drift-recovery / grant / owner.
            Change::AlterTable { .. }
            | Change::AlterSchema { .. }
            | Change::AlterSequence { .. }
            | Change::ReplaceIndex { .. }
            | Change::ValidateConstraint { .. }
            | Change::RecreateIndex { .. }
            | Change::View(
                ViewChange::ReplaceBody { .. }
                | ViewChange::SetReloption { .. }
                | ViewChange::SetComment { .. }
                | ViewChange::SetColumnComment { .. },
            )
            | Change::Mv(
                MvChange::ReplaceBody { .. }
                | MvChange::SetComment { .. }
                | MvChange::SetColumnComment { .. },
            )
            // Grant / revoke / owner changes: always non-destructive modifications.
            | Change::GrantObjectPrivilege { .. }
            | Change::RevokeObjectPrivilege { .. }
            | Change::GrantColumnPrivilege { .. }
            | Change::RevokeColumnPrivilege { .. }
            | Change::AlterObjectOwner(_)
            | Change::AlterDefaultPrivileges { .. } => modifies.push(entry),
            // UserType changes: bucket by lifecycle phase.
            Change::UserType(utc) => match utc {
                UserTypeChange::Create(_) => creates.push(entry),
                UserTypeChange::Drop(_) | UserTypeChange::ReplaceWithCascade { .. } => {
                    drops.push(entry);
                }
                UserTypeChange::EnumAddValue { .. }
                | UserTypeChange::EnumRenameValue { .. }
                | UserTypeChange::DomainAddCheck { .. }
                | UserTypeChange::DomainDropCheck { .. }
                | UserTypeChange::DomainSetDefault { .. }
                | UserTypeChange::DomainSetNotNull { .. }
                | UserTypeChange::CompositeAddAttribute { .. }
                | UserTypeChange::CompositeDropAttribute { .. }
                | UserTypeChange::CompositeAlterAttributeType { .. }
                | UserTypeChange::SetComment { .. } => modifies.push(entry),
            },
            // Function changes: bucket by lifecycle phase.
            Change::Function(fc) => match fc {
                FunctionChange::Create(_) => creates.push(entry),
                FunctionChange::Drop { .. } | FunctionChange::ReplaceWithCascade { .. } => {
                    drops.push(entry);
                }
                FunctionChange::CreateOrReplace(_) | FunctionChange::SetComment { .. } => {
                    modifies.push(entry);
                }
            },
            // Procedure changes: bucket by lifecycle phase.
            Change::Procedure(pc) => match pc {
                ProcedureChange::Create(_) => creates.push(entry),
                ProcedureChange::Drop(_) => drops.push(entry),
                ProcedureChange::CreateOrReplace(_) | ProcedureChange::SetComment { .. } => {
                    modifies.push(entry);
                }
            },
            // Extension changes: bucket by lifecycle phase (NodeId::Extension added in EXT6).
            Change::Extension(ec) => match ec {
                crate::diff::change::ExtensionChange::Create(_) => creates.push(entry),
                crate::diff::change::ExtensionChange::Drop(_)
                | crate::diff::change::ExtensionChange::ReplaceWithCascade(_) => {
                    drops.push(entry);
                }
                crate::diff::change::ExtensionChange::AlterUpdate { .. }
                | crate::diff::change::ExtensionChange::CommentOn { .. } => modifies.push(entry),
            },
            // Trigger changes: bucket by lifecycle phase.
            Change::Trigger(tc) => match tc {
                TriggerChange::Create(_) => creates.push(entry),
                // Replace emits drop+create; both land in drops so the emitter
                // can sequence them correctly.
                TriggerChange::Drop { .. } | TriggerChange::Replace(_) => drops.push(entry),
                TriggerChange::CommentOn { .. } => modifies.push(entry),
            },
            // Partition changes: alter partition membership, not the table's existence.
            // Policy + RLS changes: metadata-only, always non-destructive modifications.
            // Storage reloption changes: ALTER TABLE/INDEX/MV SET (...) — always modifies.
            Change::Table(
                TableChange::AttachPartition { .. } | TableChange::DetachPartition { .. },
            )
            | Change::CreatePolicy { .. }
            | Change::DropPolicy { .. }
            | Change::AlterPolicy { .. }
            | Change::SetTableRowSecurity { .. }
            | Change::SetTableForceRowSecurity { .. }
            | Change::SetTableStorage { .. }
            | Change::SetIndexStorage { .. }
            | Change::SetMaterializedViewStorage { .. }
            // AlterViewSetCheckOption: non-destructive, emits CREATE OR REPLACE VIEW.
            | Change::AlterViewSetCheckOption { .. }
            // Publication alter/comment changes: metadata-only, always modifies.
            // Subscription alter/comment changes: same bucket.
            | Change::Publication(
                PublicationChange::AddTable { .. }
                | PublicationChange::DropTable { .. }
                | PublicationChange::SetTable { .. }
                | PublicationChange::AddSchema { .. }
                | PublicationChange::DropSchema { .. }
                | PublicationChange::SetPublish { .. }
                | PublicationChange::SetViaRoot { .. }
                | PublicationChange::CommentOn { .. },
            )
            | Change::Subscription(
                SubscriptionChange::AlterConnection { .. }
                | SubscriptionChange::AddPublication { .. }
                | SubscriptionChange::DropPublication { .. }
                | SubscriptionChange::SetOptions { .. }
                | SubscriptionChange::CommentOn { .. },
            )
            | Change::Statistic(
                StatisticChange::AlterSetTarget { .. } | StatisticChange::CommentOn { .. },
            )
            | Change::Collation(
                CollationChange::Rename { .. } | CollationChange::CommentOn { .. },
            ) => {
                modifies.push(entry);
            }
            // Publication drops/replaces: destructive, goes in drops bucket.
            Change::Publication(
                PublicationChange::Drop { .. } | PublicationChange::Replace { .. },
            ) => {
                drops.push(entry);
            }
            // UnsupportedDiff: abort the plan immediately.
            Change::UnsupportedDiff { reason } => {
                return Err(PlanError::Internal(reason.clone()));
            }
        }
    }
    Ok((creates, modifies, drops))
}

/// Map a `Change` to the [`NodeId`] that represents it in the dependency graph.
///
/// Returns the schema/table/index/sequence node for top-level operations.
/// `AlterTable` maps to its target table node; per-op constraint changes
/// inside it are not separately ordered (they ride with the table).
#[allow(clippy::match_same_arms)] // View and Mv arms share the body shape but not the inner type.
#[allow(clippy::too_many_lines)] // one arm per Change variant mapping to its NodeId; extraction would obscure the table.
fn change_node(change: &Change) -> NodeId {
    match change {
        Change::CreateSchema(s) => NodeId::Schema(s.name.clone()),
        Change::DropSchema(name) | Change::AlterSchema { name, .. } => NodeId::Schema(name.clone()),
        Change::CreateTable(t) => NodeId::Table(t.qname.clone()),
        Change::DropTable { qname, .. } | Change::AlterTable { qname, .. } => {
            NodeId::Table(qname.clone())
        }
        Change::CreateIndex(i) => NodeId::Index(i.qname.clone()),
        // RecreateIndex maps to the same index node as DropIndex.
        Change::DropIndex(qname) | Change::RecreateIndex { qname } => NodeId::Index(qname.clone()),
        Change::ReplaceIndex { to, .. } => NodeId::Index(to.qname.clone()),
        Change::CreateSequence(s) => NodeId::Sequence(s.qname.clone()),
        Change::DropSequence(qname) | Change::AlterSequence { qname, .. } => {
            NodeId::Sequence(qname.clone())
        }
        // Drift-recovery changes: map to the table they affect.
        Change::ValidateConstraint { table, .. } => NodeId::Table(table.clone()),
        // View changes: use NodeId::View for correct topological ordering.
        Change::View(ViewChange::Create(v)) => NodeId::View(v.qname.clone()),
        Change::View(ViewChange::ReplaceBody { source, .. }) => NodeId::View(source.qname.clone()),
        Change::View(
            ViewChange::Drop(qname)
            | ViewChange::SetReloption { qname, .. }
            | ViewChange::SetComment { qname, .. }
            | ViewChange::SetColumnComment { qname, .. },
        ) => NodeId::View(qname.clone()),
        Change::AlterViewSetCheckOption { qname, .. } => NodeId::View(qname.clone()),
        // MV changes: use NodeId::Mv for correct topological ordering.
        Change::Mv(MvChange::Create(mv)) => NodeId::Mv(mv.qname.clone()),
        Change::Mv(MvChange::ReplaceBody { source, .. }) => NodeId::Mv(source.qname.clone()),
        Change::Mv(
            MvChange::Drop(qname)
            | MvChange::SetComment { qname, .. }
            | MvChange::SetColumnComment { qname, .. },
        ) => NodeId::Mv(qname.clone()),
        // UserType changes: extract the qualified name and return NodeId::Type.
        Change::UserType(utc) => {
            let qname = match utc {
                UserTypeChange::Create(ut) => &ut.qname,
                UserTypeChange::Drop(q) => q,
                UserTypeChange::ReplaceWithCascade { source, .. } => &source.qname,
                UserTypeChange::EnumAddValue { qname: q, .. }
                | UserTypeChange::EnumRenameValue { qname: q, .. }
                | UserTypeChange::DomainAddCheck { qname: q, .. }
                | UserTypeChange::DomainDropCheck { qname: q, .. }
                | UserTypeChange::DomainSetDefault { qname: q, .. }
                | UserTypeChange::DomainSetNotNull { qname: q, .. }
                | UserTypeChange::CompositeAddAttribute { qname: q, .. }
                | UserTypeChange::CompositeDropAttribute { qname: q, .. }
                | UserTypeChange::CompositeAlterAttributeType { qname: q, .. }
                | UserTypeChange::SetComment { qname: q, .. } => q,
            };
            NodeId::Type(qname.clone())
        }
        // Function node mapping.
        Change::Function(fc) => match fc {
            FunctionChange::Create(f) | FunctionChange::CreateOrReplace(f) => {
                NodeId::Function(f.qname.clone(), f.arg_types_normalized.clone())
            }
            FunctionChange::ReplaceWithCascade { source: f, .. } => {
                NodeId::Function(f.qname.clone(), f.arg_types_normalized.clone())
            }
            FunctionChange::Drop { qname, args } => NodeId::Function(qname.clone(), args.clone()),
            FunctionChange::SetComment { qname, args, .. } => {
                NodeId::Function(qname.clone(), args.clone())
            }
        },
        // Procedure node mapping.
        Change::Procedure(pc) => {
            let qname = match pc {
                ProcedureChange::Create(p) | ProcedureChange::CreateOrReplace(p) => &p.qname,
                ProcedureChange::Drop(q) | ProcedureChange::SetComment { qname: q, .. } => q,
            };
            NodeId::Procedure(qname.clone())
        }
        // Extension node mapping.
        Change::Extension(ec) => {
            use crate::diff::change::ExtensionChange;
            let name = match ec {
                ExtensionChange::Create(e) | ExtensionChange::ReplaceWithCascade(e) => {
                    e.name.clone()
                }
                ExtensionChange::Drop(n)
                | ExtensionChange::AlterUpdate { name: n, .. }
                | ExtensionChange::CommentOn { name: n, .. } => n.clone(),
            };
            NodeId::Extension(name)
        }
        // Trigger node mapping.
        Change::Trigger(tc) => match tc {
            TriggerChange::Create(t) | TriggerChange::Replace(t) => {
                NodeId::Trigger(t.qname.clone())
            }
            TriggerChange::Drop { qname, .. } | TriggerChange::CommentOn { qname, .. } => {
                NodeId::Trigger(qname.clone())
            }
        },
        // Partition change node mapping: use the child partition table.
        Change::Table(
            TableChange::AttachPartition { child, .. } | TableChange::DetachPartition { child, .. },
        ) => NodeId::Table(child.clone()),
        // Grant / revoke / owner: map to the object's primary node.
        Change::GrantObjectPrivilege { qname, .. }
        | Change::RevokeObjectPrivilege { qname, .. }
        | Change::GrantColumnPrivilege { qname, .. }
        | Change::RevokeColumnPrivilege { qname, .. } => NodeId::Table(qname.clone()),
        Change::AlterObjectOwner(op) => match &op.id {
            crate::diff::owner_op::OwnedObjectId::Qualified(q) => NodeId::Table(q.clone()),
            crate::diff::owner_op::OwnedObjectId::Schema(name) => NodeId::Schema(name.clone()),
            crate::diff::owner_op::OwnedObjectId::Cluster(name) => {
                use crate::diff::owner_op::OwnerObjectKind;
                match op.kind {
                    OwnerObjectKind::Publication => NodeId::Publication(name.clone()),
                    OwnerObjectKind::Subscription => NodeId::Subscription(name.clone()),
                    _ => NodeId::Table(QualifiedName::new(name.clone(), name.clone())),
                }
            }
        },
        Change::AlterDefaultPrivileges { target_role, .. } => {
            // Default-privilege changes have no natural node; use a Schema node
            // keyed by the target_role name as a stable ordering anchor.
            NodeId::Schema(target_role.clone())
        }
        // Policy + RLS changes: scoped to the owning table.
        Change::CreatePolicy { table, .. }
        | Change::DropPolicy { table, .. }
        | Change::AlterPolicy { table, .. } => NodeId::Table(table.clone()),
        Change::SetTableRowSecurity { qname, .. }
        | Change::SetTableForceRowSecurity { qname, .. } => NodeId::Table(qname.clone()),
        // Storage reloption changes: scoped to the named object.
        Change::SetTableStorage { qname, .. } => NodeId::Table(qname.clone()),
        Change::SetIndexStorage { qname, .. } => NodeId::Index(qname.clone()),
        Change::SetMaterializedViewStorage { qname, .. } => NodeId::Mv(qname.clone()),
        // Publication changes: use NodeId::Publication keyed by publication name.
        Change::Publication(PublicationChange::Create(p)) => NodeId::Publication(p.name.clone()),
        Change::Publication(
            PublicationChange::Drop { name } | PublicationChange::CommentOn { name, .. },
        ) => NodeId::Publication(name.clone()),
        Change::Publication(PublicationChange::Replace { to, .. }) => {
            NodeId::Publication(to.name.clone())
        }
        Change::Publication(
            PublicationChange::AddTable { publication, .. }
            | PublicationChange::DropTable { publication, .. }
            | PublicationChange::SetTable { publication, .. }
            | PublicationChange::AddSchema { publication, .. }
            | PublicationChange::DropSchema { publication, .. }
            | PublicationChange::SetPublish { publication, .. }
            | PublicationChange::SetViaRoot { publication, .. },
        ) => NodeId::Publication(publication.clone()),
        // Subscription changes: use NodeId::Subscription keyed by subscription name.
        // Subscriptions have no local dep edges (they cross-reference a remote cluster);
        // the tier rule in `order` schedules them create-last, drop-first.
        Change::Subscription(SubscriptionChange::Create(s)) => NodeId::Subscription(s.name.clone()),
        Change::Subscription(SubscriptionChange::Drop { name }) => {
            NodeId::Subscription(name.clone())
        }
        Change::Subscription(
            SubscriptionChange::AlterConnection { name, .. }
            | SubscriptionChange::AddPublication { name, .. }
            | SubscriptionChange::DropPublication { name, .. }
            | SubscriptionChange::SetOptions { name, .. }
            | SubscriptionChange::CommentOn { name, .. },
        ) => NodeId::Subscription(name.clone()),
        // Statistic changes: use NodeId::Statistic for correct topological ordering.
        // Statistics depend on their target table, so they are created after it and
        // dropped before it.
        Change::Statistic(StatisticChange::Create(s)) => NodeId::Statistic(s.qname.clone()),
        Change::Statistic(StatisticChange::Replace { to, .. }) => {
            NodeId::Statistic(to.qname.clone())
        }
        Change::Statistic(
            StatisticChange::Drop { qname }
            | StatisticChange::AlterSetTarget { qname, .. }
            | StatisticChange::CommentOn { qname, .. },
        ) => NodeId::Statistic(qname.clone()),
        // Collation changes: route to the dedicated NodeId::Collation
        // variant. Graph edges (column / domain / range / composite-attribute
        // → collation) are wired in a follow-up stage; the node already
        // exists so future edges have a stable identity to attach to.
        Change::Collation(CollationChange::Create(c)) => NodeId::Collation(c.qname.clone()),
        Change::Collation(CollationChange::Replace { to, .. }) => {
            NodeId::Collation(to.qname.clone())
        }
        Change::Collation(CollationChange::Rename { from: qname, .. }) => {
            NodeId::Collation(qname.clone())
        }
        Change::Collation(
            CollationChange::Drop { qname } | CollationChange::CommentOn { qname, .. },
        ) => NodeId::Collation(qname.clone()),
        // UnsupportedDiff is intercepted in `partition()` before `change_node` is called.
        Change::UnsupportedDiff { .. } => {
            unreachable!("UnsupportedDiff must never reach change_node")
        }
    }
}

/// Move all `CreateSubscription` entries to the end of the creates bucket.
///
/// Subscriptions create last so that any local publications they reference
/// have already been created. (The remote-cluster pubs are beyond our control,
/// but this minimizes the local-ref gap for same-cluster setups.)
fn tier_subscriptions_last(entries: Vec<ChangeEntry>) -> Vec<ChangeEntry> {
    let (mut rest, mut subs): (Vec<_>, Vec<_>) = entries.into_iter().partition(|e| {
        !matches!(
            e.change,
            Change::Subscription(SubscriptionChange::Create(_))
        )
    });
    rest.append(&mut subs);
    rest
}

/// Move all `DropSubscription` entries to the front of the drops bucket.
///
/// Subscriptions drop first so they are torn down before local publications
/// (reducing the risk of network errors if a remote subscriber tries to read
/// a publication we're about to drop).
fn tier_subscriptions_first(entries: Vec<ChangeEntry>) -> Vec<ChangeEntry> {
    let (mut subs, mut rest): (Vec<_>, Vec<_>) = entries.into_iter().partition(|e| {
        matches!(
            e.change,
            Change::Subscription(SubscriptionChange::Drop { .. })
        )
    });
    subs.append(&mut rest);
    subs
}

/// Sort `entries` by the position of their associated `NodeId` in `order`.
///
/// Entries whose node is missing from `order` (which would indicate a bug
/// in graph construction) are placed at the end in their original order.
fn sort_changes_by_order(entries: Vec<ChangeEntry>, order: &[NodeId]) -> Vec<ChangeEntry> {
    let position: HashMap<&NodeId, usize> = order.iter().enumerate().map(|(i, n)| (n, i)).collect();
    let mut indexed: Vec<(usize, ChangeEntry)> = entries
        .into_iter()
        .map(|e| {
            let node = change_node(&e.change);
            let pos = position.get(&node).copied().unwrap_or(usize::MAX);
            (pos, e)
        })
        .collect();
    // Stable sort preserves tie-broken input order; primary key is graph index.
    indexed.sort_by_key(|(p, _)| *p);
    indexed.into_iter().map(|(_, e)| e).collect()
}

/// Identify FK constraints inside a cycle and return a reduced catalog plus
/// the extracted FK list for the planner's post-pass.
///
/// An FK is extracted iff its owning-table and referenced-table nodes both
/// appear in `cycle_nodes` and the two tables are distinct (a self-referential
/// FK never induces a graph-level cycle, by construction in `edges.rs`).
fn extract_fk_cycles(source: &Catalog, cycle_nodes: &[NodeId]) -> (Catalog, Vec<DeferredFkAdd>) {
    let in_cycle: std::collections::HashSet<&NodeId> = cycle_nodes.iter().collect();
    let mut reduced = source.clone();
    let mut deferred = Vec::new();

    for table in &mut reduced.tables {
        let table_node = NodeId::Table(table.qname.clone());
        if !in_cycle.contains(&table_node) {
            continue;
        }
        let owner_qname = table.qname.clone();
        let mut keep = Vec::with_capacity(table.constraints.len());
        for c in std::mem::take(&mut table.constraints) {
            if let Some(ref_table) = fk_referenced_table(&c) {
                let ref_node = NodeId::Table(ref_table.clone());
                if *ref_table != owner_qname && in_cycle.contains(&ref_node) {
                    deferred.push(DeferredFkAdd {
                        table: owner_qname.clone(),
                        constraint: c,
                    });
                    continue;
                }
            }
            keep.push(c);
        }
        table.constraints = keep;
    }
    // Stable, deterministic order: deferred FKs are produced in iteration
    // order over `tables` (which is `Catalog::canonicalize`-sorted upstream).
    (reduced, deferred)
}

const fn fk_referenced_table(c: &Constraint) -> Option<&QualifiedName> {
    match &c.kind {
        ConstraintKind::ForeignKey(fk) => Some(&fk.referenced_table),
        _ => None,
    }
}

fn render_node(n: &NodeId) -> String {
    match n {
        NodeId::Schema(s) => format!("schema:{s}"),
        NodeId::Table(q) => format!("table:{q}"),
        NodeId::Index(q) => format!("index:{q}"),
        NodeId::Sequence(q) => format!("sequence:{q}"),
        NodeId::Constraint { table, name } => format!("constraint:{table}.{name}"),
        NodeId::View(q) => format!("view:{q}"),
        NodeId::Mv(q) => format!("mv:{q}"),
        NodeId::Type(q) => format!("type:{q}"),
        NodeId::Procedure(q) => format!("procedure:{q}"),
        NodeId::Extension(name) => format!("extension:{name}"),
        NodeId::Trigger(q) => format!("trigger:{q}"),
        NodeId::Publication(name) => format!("publication:{name}"),
        NodeId::Subscription(name) => format!("subscription:{name}"),
        NodeId::Statistic(q) => format!("statistic:{q}"),
        NodeId::Collation(q) => format!("collation:{q}"),
        NodeId::Function(q, args) => format!(
            "function:{}({})",
            q,
            args.types
                .iter()
                .map(crate::ir::column_type::ColumnType::render_sql)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::change::Change;
    use crate::diff::destructiveness::Destructiveness;
    use crate::diff::table_op::{TableOp, TableOpEntry};
    use crate::identifier::Identifier;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::constraint::{
        Constraint, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
    };
    use crate::ir::index::{
        Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
    };
    use crate::ir::schema::Schema;
    use crate::ir::table::Table;

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
            storage: None,
            compression: None,
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

    fn fk(name: &str, cols: &[&str], ref_table: QualifiedName, ref_cols: &[&str]) -> Constraint {
        Constraint {
            qname: qn("app", name),
            kind: ConstraintKind::ForeignKey(ForeignKey {
                columns: cols.iter().map(|c| id(c)).collect(),
                referenced_table: ref_table,
                referenced_columns: ref_cols.iter().map(|c| id(c)).collect(),
                on_update: ReferentialAction::NoAction,
                on_delete: ReferentialAction::NoAction,
                match_type: FkMatchType::Simple,
            }),
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    fn make_index(name: &str, table: QualifiedName) -> Index {
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
            unique: false,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
            storage: crate::ir::reloptions::IndexStorageOptions::default(),
        }
    }

    fn safe(change: Change) -> ChangeEntry {
        ChangeEntry {
            change,
            destructiveness: Destructiveness::Safe,
        }
    }

    fn drop(change: Change) -> ChangeEntry {
        ChangeEntry {
            change,
            destructiveness: Destructiveness::RequiresApproval {
                reason: "drop".into(),
            },
        }
    }

    /// Helper: position of an entry's node in a slice of entries.
    fn pos<F: Fn(&Change) -> bool>(entries: &[ChangeEntry], pred: F) -> usize {
        entries
            .iter()
            .position(|e| pred(&e.change))
            .expect("entry not found")
    }

    #[test]
    fn empty_changeset_yields_empty_ordered_set() {
        let target = Catalog::empty();
        let source = Catalog::empty();
        let result = order(
            &target,
            &source,
            ChangeSet::new(),
            &PlannerPolicy::default(),
        )
        .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn linear_schema_table_index_orders_in_dependency_order() {
        // source has schema app, table users in app, index users_idx on users
        let mut source = Catalog::empty();
        source.schemas.push(Schema::new(id("app")));
        source.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        source
            .indexes
            .push(make_index("users_idx", qn("app", "users")));

        let mut cs = ChangeSet::new();
        // Push in deliberately wrong order to confirm the planner sorts.
        cs.push(
            Change::CreateIndex(make_index("users_idx", qn("app", "users"))),
            Destructiveness::Safe,
        );
        cs.push(
            Change::CreateTable(source.tables[0].clone()),
            Destructiveness::Safe,
        );
        cs.push(
            Change::CreateSchema(Schema::new(id("app"))),
            Destructiveness::Safe,
        );

        let result = order(&Catalog::empty(), &source, cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(result.creates_and_adds.len(), 3);

        let schema_pos = pos(&result.creates_and_adds, |c| {
            matches!(c, Change::CreateSchema(_))
        });
        let table_pos = pos(&result.creates_and_adds, |c| {
            matches!(c, Change::CreateTable(_))
        });
        let index_pos = pos(&result.creates_and_adds, |c| {
            matches!(c, Change::CreateIndex(_))
        });
        assert!(schema_pos < table_pos);
        assert!(table_pos < index_pos);
    }

    #[test]
    fn fk_between_independent_tables_orders_referenced_first() {
        // source: orgs (referenced) and users (with FK to orgs).
        let mut source = Catalog::empty();
        source.schemas.push(Schema::new(id("app")));
        source.tables.push(Table {
            qname: qn("app", "orgs"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![pk("orgs_pkey", &["id"])],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        source.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("org_id", ColumnType::BigInt, false),
            ],
            constraints: vec![
                pk("users_pkey", &["id"]),
                fk("users_org_fk", &["org_id"], qn("app", "orgs"), &["id"]),
            ],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });

        let mut cs = ChangeSet::new();
        cs.push(
            Change::CreateTable(source.tables[1].clone()), // users first
            Destructiveness::Safe,
        );
        cs.push(
            Change::CreateTable(source.tables[0].clone()), // orgs second
            Destructiveness::Safe,
        );
        cs.push(
            Change::CreateSchema(Schema::new(id("app"))),
            Destructiveness::Safe,
        );

        let result = order(&Catalog::empty(), &source, cs, &PlannerPolicy::default()).unwrap();
        assert!(result.deferred_fks.is_empty());
        let orgs_pos = result
            .creates_and_adds
            .iter()
            .position(
                |e| matches!(&e.change, Change::CreateTable(t) if t.qname == qn("app", "orgs")),
            )
            .unwrap();
        let users_pos = result
            .creates_and_adds
            .iter()
            .position(
                |e| matches!(&e.change, Change::CreateTable(t) if t.qname == qn("app", "users")),
            )
            .unwrap();
        assert!(orgs_pos < users_pos);
    }

    #[test]
    fn two_table_fk_cycle_extracts_one_or_more_fks() {
        // a.id, a.ref_id (FK -> b); b.id, b.ref_id (FK -> a).
        let mut source = Catalog::empty();
        source.schemas.push(Schema::new(id("app")));
        source.tables.push(Table {
            qname: qn("app", "a"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("ref_id", ColumnType::BigInt, false),
            ],
            constraints: vec![
                pk("a_pk", &["id"]),
                fk("a_to_b", &["ref_id"], qn("app", "b"), &["id"]),
            ],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        source.tables.push(Table {
            qname: qn("app", "b"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("ref_id", ColumnType::BigInt, false),
            ],
            constraints: vec![
                pk("b_pk", &["id"]),
                fk("b_to_a", &["ref_id"], qn("app", "a"), &["id"]),
            ],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
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
            Change::CreateTable(source.tables[1].clone()),
            Destructiveness::Safe,
        );

        let result = order(&Catalog::empty(), &source, cs, &PlannerPolicy::default()).unwrap();
        // Both tables present in creates_and_adds, schema first.
        assert_eq!(result.creates_and_adds.len(), 3);
        // At least one FK was extracted.
        assert!(!result.deferred_fks.is_empty());
        // Each deferred entry is in fact a ForeignKey constraint.
        for d in &result.deferred_fks {
            assert!(matches!(d.constraint.kind, ConstraintKind::ForeignKey(_)));
        }
        // Total deferred + remaining FKs == original FK count (2).
        let remaining_fks: usize = result
            .creates_and_adds
            .iter()
            .map(|e| match &e.change {
                Change::CreateTable(t) => t
                    .constraints
                    .iter()
                    .filter(|c| matches!(c.kind, ConstraintKind::ForeignKey(_)))
                    .count(),
                _ => 0,
            })
            .sum();
        assert_eq!(remaining_fks + result.deferred_fks.len(), 2);
    }

    #[test]
    fn drops_run_in_reverse_dependency_order() {
        // target: schema app, table users, index users_idx
        let mut target = Catalog::empty();
        target.schemas.push(Schema::new(id("app")));
        target.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        target
            .indexes
            .push(make_index("users_idx", qn("app", "users")));

        let mut cs = ChangeSet::new();
        cs.push(
            Change::DropSchema(id("app")),
            Destructiveness::RequiresApproval { reason: "x".into() },
        );
        cs.push(
            Change::DropTable {
                qname: qn("app", "users"),
                row_count_estimate: None,
            },
            Destructiveness::RequiresApprovalAndDataLossWarning {
                reason: "drop users".into(),
            },
        );
        cs.push(
            Change::DropIndex(qn("app", "users_idx")),
            Destructiveness::Safe,
        );

        let result = order(&target, &Catalog::empty(), cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(result.drops.len(), 3);
        let idx_pos = pos(&result.drops, |c| matches!(c, Change::DropIndex(_)));
        let table_pos = pos(&result.drops, |c| matches!(c, Change::DropTable { .. }));
        let schema_pos = pos(&result.drops, |c| matches!(c, Change::DropSchema(_)));
        // Reverse dependency: index dropped before table; table before schema.
        assert!(idx_pos < table_pos);
        assert!(table_pos < schema_pos);
    }

    #[test]
    fn drop_fk_constraint_handled_via_alter_table_modify_bucket() {
        // ALTER TABLE entries land in `modifies`. Confirm modify-bucket
        // ordering follows source-side dependencies.
        let mut source = Catalog::empty();
        source.schemas.push(Schema::new(id("app")));
        source.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![pk("users_pkey", &["id"])],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });

        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![],
            },
            Destructiveness::Safe,
        );

        let result = order(&Catalog::empty(), &source, cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(result.modifies.len(), 1);
        assert!(result.creates_and_adds.is_empty());
        assert!(result.drops.is_empty());
    }

    #[test]
    fn deterministic_under_input_permutation() {
        // Same source, two different changeset orderings; outputs must match.
        let mut source = Catalog::empty();
        source.schemas.push(Schema::new(id("app")));
        source.tables.push(Table {
            qname: qn("app", "orgs"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![pk("orgs_pkey", &["id"])],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        source.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![pk("users_pkey", &["id"])],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });

        let mk_cs = |reversed: bool| {
            let mut cs = ChangeSet::new();
            let entries = [
                Change::CreateSchema(Schema::new(id("app"))),
                Change::CreateTable(source.tables[0].clone()),
                Change::CreateTable(source.tables[1].clone()),
            ];
            let iter: Box<dyn Iterator<Item = &Change>> = if reversed {
                Box::new(entries.iter().rev())
            } else {
                Box::new(entries.iter())
            };
            for c in iter {
                cs.push(c.clone(), Destructiveness::Safe);
            }
            cs
        };

        let policy = PlannerPolicy::default();
        let r1 = order(&Catalog::empty(), &source, mk_cs(false), &policy).unwrap();
        let r2 = order(&Catalog::empty(), &source, mk_cs(true), &policy).unwrap();
        assert_eq!(r1, r2);
    }

    #[test]
    fn replace_index_lands_in_modifies() {
        let mut source = Catalog::empty();
        source.schemas.push(Schema::new(id("app")));
        source.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        source
            .indexes
            .push(make_index("users_idx", qn("app", "users")));

        let mut cs = ChangeSet::new();
        cs.push(
            Change::ReplaceIndex {
                from: make_index("users_idx", qn("app", "users")),
                to: make_index("users_idx", qn("app", "users")),
            },
            Destructiveness::Safe,
        );

        let result = order(&Catalog::empty(), &source, cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(result.modifies.len(), 1);
    }

    #[test]
    fn alter_sequence_lands_in_modifies() {
        use crate::ir::sequence::Sequence;
        let mut source = Catalog::empty();
        source.sequences.push(Sequence {
            qname: qn("app", "s1"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cache: 1,
            cycle: false,
            owned_by: None,
            comment: None,
            owner: None,
            grants: vec![],
        });

        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterSequence {
                qname: qn("app", "s1"),
                ops: vec![],
            },
            Destructiveness::Safe,
        );

        let result = order(&Catalog::empty(), &source, cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(result.modifies.len(), 1);
    }

    #[test]
    fn three_way_fk_cycle_breaks_at_least_one() {
        // a -> b -> c -> a
        let mut source = Catalog::empty();
        source.schemas.push(Schema::new(id("app")));
        for n in ["a", "b", "c"] {
            source.tables.push(Table {
                qname: qn("app", n),
                columns: vec![
                    col("id", ColumnType::BigInt, false),
                    col("ref_id", ColumnType::BigInt, false),
                ],
                constraints: vec![pk(&format!("{n}_pk"), &["id"])],
                partition_by: None,
                partition_of: None,
                comment: None,
                owner: None,
                grants: vec![],
                rls_enabled: false,
                rls_forced: false,
                policies: vec![],
                storage: crate::ir::reloptions::TableStorageOptions::default(),
            });
        }
        // Add FKs forming a cycle: a -> b, b -> c, c -> a.
        let pairs = [("a", "b"), ("b", "c"), ("c", "a")];
        for (from, to) in pairs {
            let table = source
                .tables
                .iter_mut()
                .find(|t| t.qname == qn("app", from))
                .unwrap();
            table.constraints.push(fk(
                &format!("{from}_to_{to}"),
                &["ref_id"],
                qn("app", to),
                &["id"],
            ));
        }

        let mut cs = ChangeSet::new();
        cs.push(
            Change::CreateSchema(Schema::new(id("app"))),
            Destructiveness::Safe,
        );
        for t in &source.tables {
            cs.push(Change::CreateTable(t.clone()), Destructiveness::Safe);
        }

        let result = order(&Catalog::empty(), &source, cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(result.creates_and_adds.len(), 4);
        assert!(!result.deferred_fks.is_empty());
    }

    #[test]
    fn drops_with_independent_objects_use_target_graph() {
        // Target has two independent schemas + tables; drop them all.
        let mut target = Catalog::empty();
        target.schemas.push(Schema::new(id("a")));
        target.schemas.push(Schema::new(id("b")));
        target.tables.push(Table {
            qname: QualifiedName::new(id("a"), id("t1")),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        target.tables.push(Table {
            qname: QualifiedName::new(id("b"), id("t2")),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });

        let mut cs = ChangeSet::new();
        for t in &target.tables {
            cs.push(
                Change::DropTable {
                    qname: t.qname.clone(),
                    row_count_estimate: None,
                },
                Destructiveness::RequiresApprovalAndDataLossWarning {
                    reason: "drop".into(),
                },
            );
        }
        for s in &target.schemas {
            cs.push(
                Change::DropSchema(s.name.clone()),
                Destructiveness::RequiresApproval {
                    reason: "drop".into(),
                },
            );
        }

        let result = order(&target, &Catalog::empty(), cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(result.drops.len(), 4);
        // Every DropTable must precede the corresponding DropSchema.
        for table in &target.tables {
            let table_pos = result
                .drops
                .iter()
                .position(|e| matches!(&e.change, Change::DropTable { qname, .. } if qname == &table.qname))
                .unwrap();
            let schema_pos = result
                .drops
                .iter()
                .position(
                    |e| matches!(&e.change, Change::DropSchema(s) if s == &table.qname.schema),
                )
                .unwrap();
            assert!(table_pos < schema_pos);
        }
    }

    // ---- Suppress dead-code warnings for the `drop` helper used in tests. ----
    #[test]
    fn _drop_helper_is_used() {
        let _ = drop(Change::DropSchema(id("x")));
        let _ = safe(Change::CreateSchema(Schema::new(id("y"))));
    }

    #[test]
    fn drop_index_elided_when_alter_table_drops_indexed_column() {
        // Postgres cascade-drops any index whose column list references a
        // column being dropped by `ALTER TABLE ... DROP COLUMN`. If the
        // planner emits a separate `DROP INDEX` in the drops phase that runs
        // after the column drop, the explicit DROP fails with
        // `42704 (undefined_object): index "..." does not exist`.
        //
        // The planner must therefore elide such DropIndex changes — the
        // cascade is implicit in the column drop.
        let mut target = Catalog::empty();
        target.schemas.push(Schema::new(id("app")));
        target.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("deleted_at", ColumnType::Text, true),
            ],
            constraints: vec![pk("users_pkey", &["id"])],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        target.indexes.push(Index {
            qname: qn("app", "users_deleted_at_idx"),
            on: IndexParent::Table(qn("app", "users")),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("deleted_at")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: false,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
            storage: crate::ir::reloptions::IndexStorageOptions::default(),
        });

        let mut source = Catalog::empty();
        source.schemas.push(Schema::new(id("app")));
        source.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![pk("users_pkey", &["id"])],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });

        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::DropColumn {
                        name: id("deleted_at"),
                        is_populated: false,
                    },
                    destructiveness: Destructiveness::RequiresApproval {
                        reason: "drops column".into(),
                    },
                }],
            },
            Destructiveness::RequiresApproval {
                reason: "drops column".into(),
            },
        );
        cs.push(
            Change::DropIndex(qn("app", "users_deleted_at_idx")),
            Destructiveness::RequiresApproval {
                reason: "drops index".into(),
            },
        );

        let result = order(&target, &source, cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(result.modifies.len(), 1, "AlterTable must stay in modifies");
        assert!(
            result.drops.is_empty(),
            "DropIndex must be elided when the indexed column is dropped in \
             the same plan; got: {:?}",
            result.drops.iter().map(|e| &e.change).collect::<Vec<_>>()
        );
    }

    #[test]
    fn drop_index_retained_when_column_drop_unrelated() {
        // Sanity check on the elision logic: if the column being dropped is
        // not in the index's column list, the DropIndex must NOT be elided.
        let mut target = Catalog::empty();
        target.schemas.push(Schema::new(id("app")));
        target.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("email", ColumnType::Text, true),
                col("unused", ColumnType::Text, true),
            ],
            constraints: vec![pk("users_pkey", &["id"])],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        // Index on `email`, but the column being dropped is `unused`.
        target.indexes.push(Index {
            qname: qn("app", "users_email_idx"),
            on: IndexParent::Table(qn("app", "users")),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("email")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: false,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
            storage: crate::ir::reloptions::IndexStorageOptions::default(),
        });

        let mut source = Catalog::empty();
        source.schemas.push(Schema::new(id("app")));
        source.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("email", ColumnType::Text, true),
            ],
            constraints: vec![pk("users_pkey", &["id"])],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        // Source still has the email index, but it's being dropped from the
        // source for an unrelated reason (simulate user removing it).

        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::DropColumn {
                        name: id("unused"),
                        is_populated: false,
                    },
                    destructiveness: Destructiveness::RequiresApproval {
                        reason: "drops column".into(),
                    },
                }],
            },
            Destructiveness::RequiresApproval {
                reason: "drops column".into(),
            },
        );
        cs.push(
            Change::DropIndex(qn("app", "users_email_idx")),
            Destructiveness::RequiresApproval {
                reason: "drops index".into(),
            },
        );

        let result = order(&target, &source, cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(result.modifies.len(), 1);
        assert_eq!(
            result.drops.len(),
            1,
            "DropIndex must be retained when the dropped column is unrelated"
        );
    }

    #[test]
    fn drop_index_elided_when_indexed_column_is_in_include_list() {
        // INCLUDE columns participate in the index's storage and dropping
        // them also cascades the index.
        let mut target = Catalog::empty();
        target.schemas.push(Schema::new(id("app")));
        target.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("email", ColumnType::Text, true),
                col("payload", ColumnType::Text, true),
            ],
            constraints: vec![pk("users_pkey", &["id"])],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        target.indexes.push(Index {
            qname: qn("app", "users_email_idx"),
            on: IndexParent::Table(qn("app", "users")),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("email")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![id("payload")],
            unique: false,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
            storage: crate::ir::reloptions::IndexStorageOptions::default(),
        });

        let mut source = Catalog::empty();
        source.schemas.push(Schema::new(id("app")));
        source.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("email", ColumnType::Text, true),
            ],
            constraints: vec![pk("users_pkey", &["id"])],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });

        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::DropColumn {
                        name: id("payload"),
                        is_populated: false,
                    },
                    destructiveness: Destructiveness::RequiresApproval {
                        reason: "drops column".into(),
                    },
                }],
            },
            Destructiveness::RequiresApproval {
                reason: "drops column".into(),
            },
        );
        cs.push(
            Change::DropIndex(qn("app", "users_email_idx")),
            Destructiveness::RequiresApproval {
                reason: "drops index".into(),
            },
        );

        let result = order(&target, &source, cs, &PlannerPolicy::default()).unwrap();
        assert!(
            result.drops.is_empty(),
            "DropIndex must be elided when an INCLUDEd column is dropped; got: {:?}",
            result.drops.iter().map(|e| &e.change).collect::<Vec<_>>()
        );
    }

    // ── UserType partition / change_node tests ────────────────────────────────

    use crate::diff::change::UserTypeChange;
    use crate::ir::user_type::{UserType, UserTypeKind};

    fn make_enum_type(schema: &str, name: &str) -> UserType {
        UserType {
            qname: qn(schema, name),
            kind: UserTypeKind::Enum { values: vec![] },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    #[test]
    fn user_type_create_lands_in_creates() {
        let ut = make_enum_type("app", "status");
        let mut source = Catalog::empty();
        source.types.push(ut.clone());

        let mut cs = ChangeSet::new();
        cs.push(
            Change::UserType(UserTypeChange::Create(ut)),
            Destructiveness::Safe,
        );

        let result = order(&Catalog::empty(), &source, cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(
            result.creates_and_adds.len(),
            1,
            "Create must land in creates_and_adds"
        );
        assert!(result.modifies.is_empty());
        assert!(result.drops.is_empty());
    }

    #[test]
    fn user_type_drop_lands_in_drops() {
        let ut = make_enum_type("app", "status");
        let mut target = Catalog::empty();
        target.types.push(ut);

        let mut cs = ChangeSet::new();
        cs.push(
            Change::UserType(UserTypeChange::Drop(qn("app", "status"))),
            Destructiveness::RequiresApproval {
                reason: "drop type".into(),
            },
        );

        let result = order(&target, &Catalog::empty(), cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(result.drops.len(), 1, "Drop must land in drops");
        assert!(result.creates_and_adds.is_empty());
        assert!(result.modifies.is_empty());
    }

    #[test]
    fn user_type_replace_with_cascade_lands_in_drops() {
        let ut = make_enum_type("app", "status");
        let mut target = Catalog::empty();
        target.types.push(ut.clone());

        let mut cs = ChangeSet::new();
        cs.push(
            Change::UserType(UserTypeChange::ReplaceWithCascade {
                source: ut.clone(),
                catalog: ut,
            }),
            Destructiveness::RequiresApproval {
                reason: "cascade replace".into(),
            },
        );

        let result = order(&target, &Catalog::empty(), cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(
            result.drops.len(),
            1,
            "ReplaceWithCascade must land in drops"
        );
        assert!(result.creates_and_adds.is_empty());
        assert!(result.modifies.is_empty());
    }

    #[test]
    fn user_type_enum_add_value_lands_in_modifies() {
        let ut = make_enum_type("app", "status");
        let mut source = Catalog::empty();
        source.types.push(ut);

        let mut cs = ChangeSet::new();
        cs.push(
            Change::UserType(UserTypeChange::EnumAddValue {
                qname: qn("app", "status"),
                value: "archived".into(),
                before: None,
                after: None,
            }),
            Destructiveness::Safe,
        );

        let result = order(&Catalog::empty(), &source, cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(
            result.modifies.len(),
            1,
            "EnumAddValue must land in modifies"
        );
        assert!(result.creates_and_adds.is_empty());
        assert!(result.drops.is_empty());
    }

    #[test]
    fn user_type_create_before_table_using_it() {
        // When both a type create and a table create are in the changeset,
        // the type must come first (table depends on it in the source graph).
        use crate::ir::column::Column;

        let ut = make_enum_type("app", "status");
        let mut source = Catalog::empty();
        source.types.push(ut.clone());
        source.tables.push(Table {
            qname: qn("app", "orders"),
            columns: vec![Column {
                name: id("status"),
                ty: ColumnType::UserDefined(qn("app", "status")),
                nullable: false,
                default: None,
                identity: None,
                generated: None,
                collation: None,
                storage: None,
                compression: None,
                comment: None,
            }],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });

        let mut cs = ChangeSet::new();
        // Deliberately push table first to verify sorting.
        cs.push(
            Change::CreateTable(source.tables[0].clone()),
            Destructiveness::Safe,
        );
        cs.push(
            Change::UserType(UserTypeChange::Create(ut)),
            Destructiveness::Safe,
        );

        let result = order(&Catalog::empty(), &source, cs, &PlannerPolicy::default()).unwrap();
        let type_pos = result
            .creates_and_adds
            .iter()
            .position(|e| matches!(&e.change, Change::UserType(UserTypeChange::Create(_))))
            .expect("type create not found");
        let table_pos = result
            .creates_and_adds
            .iter()
            .position(|e| matches!(&e.change, Change::CreateTable(_)))
            .expect("table create not found");
        assert!(
            type_pos < table_pos,
            "type must be created before the table that uses it"
        );
    }

    /// Regression for issue #38: `DropCollation` must be ordered before
    /// `DropSchema` when both appear in the drops bucket. The drop graph
    /// requires a `Collation → Schema` dependency edge so that the
    /// reverse-topo sort places the collation drop first.
    ///
    /// Without the edge, ordering is non-deterministic and the generated DDL
    /// may try to `DROP SCHEMA X` while collations in X are still live, which
    /// PG rejects with error 2BP01.
    #[test]
    fn drop_collation_ordered_before_drop_schema() {
        use crate::diff::CollationChange;
        use crate::ir::collation::{Collation, CollationProvider};

        fn coll(schema: &str, name: &str) -> Collation {
            Collation {
                qname: qn(schema, name),
                provider: CollationProvider::Libc,
                lc_collate: "C".into(),
                lc_ctype: "C".into(),
                deterministic: true,
                version: None,
                owner: None,
                comment: None,
            }
        }

        let mut target = Catalog::empty();
        target.schemas.push(Schema::new(id("audit")));
        target.collations.push(coll("audit", "coll_a"));
        target.collations.push(coll("audit", "coll_b"));

        let mut cs = ChangeSet::new();
        cs.push(
            Change::DropSchema(id("audit")),
            Destructiveness::RequiresApproval {
                reason: "drops schema audit".into(),
            },
        );
        cs.push(
            Change::Collation(CollationChange::Drop {
                qname: qn("audit", "coll_a"),
            }),
            Destructiveness::RequiresApproval {
                reason: "drops collation audit.coll_a".into(),
            },
        );
        cs.push(
            Change::Collation(CollationChange::Drop {
                qname: qn("audit", "coll_b"),
            }),
            Destructiveness::RequiresApproval {
                reason: "drops collation audit.coll_b".into(),
            },
        );

        let result = order(&target, &Catalog::empty(), cs, &PlannerPolicy::default()).unwrap();
        assert_eq!(result.drops.len(), 3, "expected 3 drop steps");

        let schema_pos = result
            .drops
            .iter()
            .position(|e| matches!(&e.change, Change::DropSchema(n) if n == &id("audit")))
            .expect("DropSchema(audit) not found");
        let pos_coll_a = result
            .drops
            .iter()
            .position(|e| {
                matches!(&e.change, Change::Collation(CollationChange::Drop { qname }) if qname == &qn("audit", "coll_a"))
            })
            .expect("DropCollation(audit.coll_a) not found");
        let pos_coll_b = result
            .drops
            .iter()
            .position(|e| {
                matches!(&e.change, Change::Collation(CollationChange::Drop { qname }) if qname == &qn("audit", "coll_b"))
            })
            .expect("DropCollation(audit.coll_b) not found");

        assert!(
            pos_coll_a < schema_pos,
            "collation coll_a (pos {pos_coll_a}) must be dropped before schema (pos {schema_pos})"
        );
        assert!(
            pos_coll_b < schema_pos,
            "collation coll_b (pos {pos_coll_b}) must be dropped before schema (pos {schema_pos})"
        );
    }
}
