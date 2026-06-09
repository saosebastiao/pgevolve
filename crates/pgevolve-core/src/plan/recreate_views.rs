//! Identifies every catalog view (transitively) affected by upstream
//! changes in the diff, and emits explicit `DROP + CREATE` steps for them.
//!
//! # Why not CASCADE?
//!
//! pgevolve never issues `DROP ... CASCADE`. The CASCADE path hides which
//! objects were destroyed, making the plan non-auditable. Instead we walk
//! the `body_dependencies` graph to find every transitively-affected view,
//! then emit an explicit `ReplaceBody { compatible: false }` change for each
//! one. The resulting DROP + CREATE chain is deterministic and visible.
//!
//! # Policy gate
//!
//! When [`PlannerPolicy::view_drop_create_dependents`] is `false`, the walk
//! still runs to detect affected views, but instead of emitting recreations
//! the function returns `Err(affected)` so the caller can surface a
//! human-readable error naming every affected view.

use std::collections::{BTreeMap, BTreeSet};

use crate::diff::change::{
    Change, FunctionChange, MvChange, ProcedureChange, UserTypeChange, ViewChange,
};
use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::plan::edges::NodeId;
use crate::plan::policy::PlannerPolicy;

/// A trigger: an upstream object (or column on it) whose change forces every
/// view with a matching `body_dependencies` edge to be recreated.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DepTrigger {
    /// Object whose change was detected.
    qname: QualifiedName,
    /// `None` means "any dep on this object" (table/view/mv drop, view body
    /// replace-incompatible). `Some(col)` means "deps on this specific column"
    /// (column drop, rename, type change).
    column: Option<String>,
}

/// Extend `changes` with explicit `Drop + Create` steps for every transitively-
/// affected view in `target_catalog`.
///
/// Returns `Ok(())` when the policy permits dependent recreation (or when no
/// views are affected).  Returns `Err(affected)` — a list of view qnames —
/// when the policy has `view_drop_create_dependents = false` and at least one
/// view would need recreation (the caller surfaces an appropriate error).
///
/// The function appends [`Change::View(ViewChange::ReplaceBody { compatible: false })`]
/// for every affected view **not already present** in `changes` (to avoid
/// duplicate recreation of a view that the differ itself already marked for
/// replacement). Views are appended in topological order relative to each
/// other (dependencies first, so the DROP + CREATE sequence is correct).
pub fn extend_with_dependent_recreations(
    changes: &mut Vec<Change>,
    target_catalog: &Catalog,
    policy: &PlannerPolicy,
) -> Result<(), Vec<QualifiedName>> {
    let triggers = collect_upstream_triggers(changes);
    if triggers.is_empty() {
        return Ok(());
    }

    let affected = walk_transitive(&triggers, target_catalog);
    if affected.is_empty() {
        return Ok(());
    }

    // Collect the set of view/MV qnames already present in changes (those the
    // differ already produced a ReplaceBody for) so we don't double-emit.
    #[allow(clippy::match_same_arms)] // source fields differ by type (View vs MaterializedView)
    let already_in_changes: BTreeSet<QualifiedName> = changes
        .iter()
        .filter_map(|c| match c {
            Change::View(ViewChange::ReplaceBody { source, .. }) => Some(source.qname.clone()),
            Change::View(ViewChange::Drop(q)) => Some(q.clone()),
            Change::Mv(MvChange::ReplaceBody { source, .. }) => Some(source.qname.clone()),
            Change::Mv(MvChange::Drop(q)) => Some(q.clone()),
            _ => None,
        })
        .collect();

    let new_affected: Vec<QualifiedName> = affected
        .into_iter()
        .filter(|q| !already_in_changes.contains(q))
        .collect();

    if new_affected.is_empty() {
        return Ok(());
    }

    if !policy.view_drop_create_dependents() {
        return Err(new_affected);
    }

    for obj_qname in new_affected {
        if let Some(cat) = target_catalog.views.iter().find(|v| v.qname == obj_qname) {
            changes.push(Change::View(ViewChange::ReplaceBody {
                // source = the desired state; for recreations triggered by
                // upstream changes, the target catalog's version *is* the
                // desired version (the view body itself is unchanged — only its
                // dependencies changed).
                source: cat.clone(),
                catalog: cat.clone(),
                compatible: false,
            }));
        } else if let Some(cat_mv) = target_catalog
            .materialized_views
            .iter()
            .find(|mv| mv.qname == obj_qname)
        {
            // MVs do not support CREATE OR REPLACE — always DROP + CREATE + REFRESH.
            changes.push(Change::Mv(MvChange::ReplaceBody {
                source: cat_mv.clone(),
                catalog: cat_mv.clone(),
            }));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Trigger collection
// ---------------------------------------------------------------------------

/// Scan the already-computed `changes` list for events that force dependent
/// views to be recreated.
fn collect_upstream_triggers(changes: &[Change]) -> Vec<DepTrigger> {
    let mut out = Vec::new();
    for change in changes {
        // Extract the object qname(s) that this change triggers on.
        if let Some(trigger_qname) = object_drop_qname(change) {
            // Table, view, or MV drop — any dep on that object triggers recreation.
            out.push(DepTrigger {
                qname: trigger_qname,
                column: None,
            });
        } else {
            match change {
                // Column drops and type changes trigger views that reference
                // the specific column.
                Change::AlterTable { qname, ops } => {
                    for op_entry in ops {
                        match &op_entry.op {
                            crate::diff::table_op::TableOp::DropColumn { name, .. }
                            | crate::diff::table_op::TableOp::AlterColumnType { name, .. } => {
                                out.push(DepTrigger {
                                    qname: qname.clone(),
                                    column: Some(name.as_str().to_string()),
                                });
                            }
                            _ => {}
                        }
                    }
                }
                // Incompatible view body replaces trigger dependents.
                Change::View(ViewChange::ReplaceBody {
                    catalog,
                    compatible: false,
                    ..
                }) => out.push(DepTrigger {
                    qname: catalog.qname.clone(),
                    column: None,
                }),
                // Compatible view body replaces use `CREATE OR REPLACE VIEW` and
                // do NOT break dependents — that is the point of OR REPLACE.

                // MV body replaces also trigger dependent recreation.
                Change::Mv(MvChange::ReplaceBody { catalog, .. }) => out.push(DepTrigger {
                    qname: catalog.qname.clone(),
                    column: None,
                }),

                _ => {}
            }
        }
    }
    out
}

/// If `change` is a drop (or destructive replace) of a top-level object
/// (table, view, MV, user-defined type, function, or procedure), return its qname.
///
/// For `UserTypeChange::ReplaceWithCascade` and
/// `FunctionChange::ReplaceWithCascade`, the object is effectively dropped and
/// re-created — any view that depends on it must be recreated.
fn object_drop_qname(change: &Change) -> Option<QualifiedName> {
    match change {
        Change::DropTable { qname, .. }
        | Change::View(ViewChange::Drop(qname))
        | Change::Mv(MvChange::Drop(qname))
        | Change::UserType(UserTypeChange::Drop(qname))
        | Change::Function(FunctionChange::Drop { qname, .. })
        | Change::Procedure(ProcedureChange::Drop(qname)) => Some(qname.clone()),
        Change::UserType(UserTypeChange::ReplaceWithCascade { source, .. }) => {
            Some(source.qname.clone())
        }
        // Function cascade-replace: effectively a drop-and-recreate.
        Change::Function(FunctionChange::ReplaceWithCascade { source, .. }) => {
            Some(source.qname.clone())
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Transitive walk
// ---------------------------------------------------------------------------

/// Build an index: qname → list of `(dep_qname, column_or_none)` for quick lookup.
///
/// For each view *and* materialized view, we record every (object, column) pair
/// that it depends on, derived from its `body_dependencies`.
fn build_dep_index(
    catalog: &Catalog,
) -> BTreeMap<QualifiedName, Vec<(QualifiedName, Option<String>)>> {
    let mut index: BTreeMap<QualifiedName, Vec<(QualifiedName, Option<String>)>> = BTreeMap::new();
    for v in &catalog.views {
        let deps: Vec<(QualifiedName, Option<String>)> = v
            .body_dependencies
            .iter()
            .filter_map(|dep| match &dep.to {
                NodeId::Table(q)
                | NodeId::View(q)
                | NodeId::Mv(q)
                | NodeId::Type(q)
                | NodeId::Procedure(q)
                // Functions are identified by (qname, args) but cascade
                // matching uses qname only (conservative: any overload drop
                // triggers dependent recreation).
                | NodeId::Function(q, _) => Some((q.clone(), None)),
                // We don't have column-level NodeId granularity yet, so
                // column-specific triggers fall back to object-level matching.
                _ => None,
            })
            .collect();
        if !deps.is_empty() {
            index.insert(v.qname.clone(), deps);
        }
    }
    // Materialized views are also dependency sinks: if their upstream changes,
    // they must be recreated (DROP + CREATE + REFRESH).
    for mv in &catalog.materialized_views {
        let deps: Vec<(QualifiedName, Option<String>)> = mv
            .body_dependencies
            .iter()
            .filter_map(|dep| match &dep.to {
                NodeId::Table(q)
                | NodeId::View(q)
                | NodeId::Mv(q)
                | NodeId::Type(q)
                | NodeId::Procedure(q)
                | NodeId::Function(q, _) => Some((q.clone(), None)),
                _ => None,
            })
            .collect();
        if !deps.is_empty() {
            index.insert(mv.qname.clone(), deps);
        }
    }
    index
}

/// Compute the transitive closure of views affected by `triggers`.
///
/// Returns a `Vec` in topological order: if view B depends on view A, A comes
/// first (so its DROP precedes B's DROP when we later emit DROP + CREATE).
fn walk_transitive(triggers: &[DepTrigger], catalog: &Catalog) -> Vec<QualifiedName> {
    // Build quick lookup: object qname → set of views directly depending on it.
    let dep_index = build_dep_index(catalog);

    // Reverse index: for each object, which views reference it?
    let mut reverse: BTreeMap<QualifiedName, BTreeSet<QualifiedName>> = BTreeMap::new();
    for (view_qname, deps) in &dep_index {
        for (dep_qname, _col) in deps {
            reverse
                .entry(dep_qname.clone())
                .or_default()
                .insert(view_qname.clone());
        }
    }

    // Start with the initial set of affected views from direct trigger hits.
    let mut affected: BTreeSet<QualifiedName> = BTreeSet::new();
    let mut work_queue: Vec<QualifiedName> = Vec::new();

    for trigger in triggers {
        // Find views that reference the trigger's object.
        // For column-level triggers, we only match dep edges that reference
        // the same object (at object granularity — column granularity not yet
        // available in the edge model).
        if let Some(dependent_views) = reverse.get(&trigger.qname) {
            for view_qname in dependent_views {
                if affected.insert(view_qname.clone()) {
                    work_queue.push(view_qname.clone());
                }
            }
        }
    }

    // Iteratively expand: newly-affected views may themselves be depended on
    // by further views.
    while let Some(trigger_qname) = work_queue.pop() {
        if let Some(dependent_views) = reverse.get(&trigger_qname) {
            for view_qname in dependent_views {
                if affected.insert(view_qname.clone()) {
                    work_queue.push(view_qname.clone());
                }
            }
        }
    }

    if affected.is_empty() {
        return Vec::new();
    }

    // Topological sort: dependencies first.
    // We use Kahn's algorithm over the subgraph induced by `affected`.
    topological_sort_affected(&affected, &dep_index)
}

/// Kahn's algorithm over the set of affected views to produce a topological
/// ordering (dependency-first). Views with no edges within `affected` come
/// first; views that depend on other affected views come later.
fn topological_sort_affected(
    affected: &BTreeSet<QualifiedName>,
    dep_index: &BTreeMap<QualifiedName, Vec<(QualifiedName, Option<String>)>>,
) -> Vec<QualifiedName> {
    // Build in-degree map for affected views (counting only edges within the
    // affected set — cross-edges to non-affected objects are irrelevant here).
    let mut in_degree: BTreeMap<&QualifiedName, usize> = affected.iter().map(|q| (q, 0)).collect();
    // adjacency: from -> list of views that depend on `from` (within affected).
    let mut adj: BTreeMap<&QualifiedName, Vec<&QualifiedName>> = BTreeMap::new();

    for view_qname in affected {
        if let Some(deps) = dep_index.get(view_qname) {
            for (dep_qname, _) in deps {
                if affected.contains(dep_qname) {
                    // view_qname depends on dep_qname => dep_qname -> view_qname edge.
                    adj.entry(dep_qname).or_default().push(view_qname);
                    *in_degree.entry(view_qname).or_insert(0) += 1;
                }
            }
        }
    }

    // Start with nodes of in-degree 0 (sorted for determinism).
    let mut queue: std::collections::BinaryHeap<std::cmp::Reverse<&QualifiedName>> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(q, _)| std::cmp::Reverse(*q))
        .collect();

    let mut result = Vec::with_capacity(affected.len());
    while let Some(std::cmp::Reverse(node)) = queue.pop() {
        result.push(node.clone());
        if let Some(dependents) = adj.get(node) {
            for dep in dependents {
                let entry = in_degree.entry(dep).or_insert(0);
                if *entry > 0 {
                    *entry -= 1;
                    if *entry == 0 {
                        queue.push(std::cmp::Reverse(dep));
                    }
                }
            }
        }
    }

    // If result.len() < affected.len() there's a cycle within views; emit
    // remaining in name-sorted order as a safe fallback.
    if result.len() < affected.len() {
        let emitted: BTreeSet<QualifiedName> = result.iter().cloned().collect();
        for q in affected {
            if !emitted.contains(q) {
                result.push(q.clone());
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::redundant_clone)] // test helpers use clone for clarity
mod tests {
    use super::*;
    use crate::diff::destructiveness::Destructiveness;
    use crate::diff::table_op::{TableOp, TableOpEntry};
    use crate::identifier::Identifier;
    use crate::ir::catalog::Catalog;
    use crate::ir::column_type::ColumnType;
    use crate::ir::view::{View, ViewColumn};
    use crate::parse::normalize_body::NormalizedBody;
    use crate::plan::edges::{DepEdge, DepSource, NodeId};
    use crate::plan::policy::PlannerPolicy;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn body(sql: &str) -> NormalizedBody {
        NormalizedBody::from_sql(sql).unwrap()
    }

    fn simple_view(schema: &str, name: &str, deps: Vec<DepEdge>) -> View {
        View {
            qname: qn(schema, name),
            columns: vec![ViewColumn {
                name: id("id"),
                column_type: Some(ColumnType::BigInt),
                comment: None,
            }],
            body_canonical: body("SELECT 1"),
            body_dependencies: deps,
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        }
    }

    fn dep_edge(from_view: &QualifiedName, to_table: &QualifiedName) -> DepEdge {
        DepEdge {
            from: NodeId::View(from_view.clone()),
            to: NodeId::Table(to_table.clone()),
            source: DepSource::AstExtracted,
        }
    }

    fn view_to_view_edge(from_view: &QualifiedName, to_view: &QualifiedName) -> DepEdge {
        DepEdge {
            from: NodeId::View(from_view.clone()),
            to: NodeId::View(to_view.clone()),
            source: DepSource::AstExtracted,
        }
    }

    fn drop_table_change(qname: QualifiedName) -> Change {
        Change::DropTable {
            qname,
            row_count_estimate: None,
        }
    }

    fn alter_drop_col(table: QualifiedName, col: &str) -> Change {
        Change::AlterTable {
            qname: table,
            ops: vec![TableOpEntry {
                op: TableOp::DropColumn {
                    name: id(col),
                    is_populated: false,
                },
                destructiveness: Destructiveness::Safe,
            }],
        }
    }

    fn alter_col_type(table: QualifiedName, col: &str) -> Change {
        Change::AlterTable {
            qname: table,
            ops: vec![TableOpEntry {
                op: TableOp::AlterColumnType {
                    name: id(col),
                    from: ColumnType::BigInt,
                    to: ColumnType::Text,
                    using: None,
                },
                destructiveness: Destructiveness::Safe,
            }],
        }
    }

    fn policy_enabled() -> PlannerPolicy {
        PlannerPolicy::default()
    }

    fn policy_disabled() -> PlannerPolicy {
        let mut p = PlannerPolicy::default();
        p.online.view_drop_create_dependents = false;
        p
    }

    // --- collect_upstream_triggers ---

    #[test]
    fn drop_table_produces_trigger() {
        let changes = vec![drop_table_change(qn("app", "users"))];
        let triggers = collect_upstream_triggers(&changes);
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].qname, qn("app", "users"));
        assert!(triggers[0].column.is_none());
    }

    #[test]
    fn drop_column_produces_column_trigger() {
        let changes = vec![alter_drop_col(qn("app", "users"), "email")];
        let triggers = collect_upstream_triggers(&changes);
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].qname, qn("app", "users"));
        assert_eq!(triggers[0].column.as_deref(), Some("email"));
    }

    #[test]
    fn alter_column_type_produces_trigger() {
        let changes = vec![alter_col_type(qn("app", "users"), "name")];
        let triggers = collect_upstream_triggers(&changes);
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].column.as_deref(), Some("name"));
    }

    #[test]
    fn compatible_body_replace_does_not_trigger() {
        let view = simple_view("app", "v", vec![]);
        let changes = vec![Change::View(ViewChange::ReplaceBody {
            source: view.clone(),
            catalog: view,
            compatible: true,
        })];
        let triggers = collect_upstream_triggers(&changes);
        assert!(triggers.is_empty(), "compatible replace must not trigger");
    }

    #[test]
    fn incompatible_body_replace_triggers() {
        let view = simple_view("app", "v", vec![]);
        let changes = vec![Change::View(ViewChange::ReplaceBody {
            source: view.clone(),
            catalog: view,
            compatible: false,
        })];
        let triggers = collect_upstream_triggers(&changes);
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].qname, qn("app", "v"));
    }

    #[test]
    fn mv_drop_triggers() {
        let changes = vec![Change::Mv(MvChange::Drop(qn("app", "mv")))];
        let triggers = collect_upstream_triggers(&changes);
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].qname, qn("app", "mv"));
    }

    // --- walk_transitive ---

    #[test]
    fn single_dependent_view_is_found() {
        // view "v" depends on table "users". Drop "users" → recreate "v".
        let v_qname = qn("app", "v");
        let users = qn("app", "users");
        let v = simple_view("app", "v", vec![dep_edge(&v_qname, &users)]);
        let mut catalog = Catalog::empty();
        catalog.views.push(v);

        let triggers = vec![DepTrigger {
            qname: users.clone(),
            column: None,
        }];
        let affected = walk_transitive(&triggers, &catalog);
        assert_eq!(affected, vec![v_qname]);
    }

    #[test]
    fn chain_a_b_c_all_affected() {
        // a: depends on table T
        // b: depends on view a
        // c: depends on view b
        // Trigger on T → affects a, b, c (topological: a before b before c)
        let t = qn("app", "t");
        let a = qn("app", "a");
        let b = qn("app", "b");
        let c = qn("app", "c");

        let view_a = simple_view("app", "a", vec![dep_edge(&a, &t)]);
        let view_b = simple_view("app", "b", vec![view_to_view_edge(&b, &a)]);
        let view_c = simple_view("app", "c", vec![view_to_view_edge(&c, &b)]);

        let mut catalog = Catalog::empty();
        catalog.views.push(view_a);
        catalog.views.push(view_b);
        catalog.views.push(view_c);

        let triggers = vec![DepTrigger {
            qname: t.clone(),
            column: None,
        }];
        let affected = walk_transitive(&triggers, &catalog);
        // All three must be present; a must come before b, b before c.
        assert!(
            affected.contains(&a),
            "view a must be in affected: {affected:?}"
        );
        assert!(
            affected.contains(&b),
            "view b must be in affected: {affected:?}"
        );
        assert!(
            affected.contains(&c),
            "view c must be in affected: {affected:?}"
        );
        let pos_a = affected.iter().position(|q| q == &a).unwrap();
        let pos_b = affected.iter().position(|q| q == &b).unwrap();
        let pos_c = affected.iter().position(|q| q == &c).unwrap();
        assert!(pos_a < pos_b, "a must come before b: {affected:?}");
        assert!(pos_b < pos_c, "b must come before c: {affected:?}");
    }

    #[test]
    fn unrelated_view_not_included() {
        // view "v1" depends on table "users"
        // view "v2" depends on table "orders"
        // Trigger on "users" → only "v1" affected.
        let users = qn("app", "users");
        let orders = qn("app", "orders");
        let v1 = qn("app", "v1");
        let v2 = qn("app", "v2");

        let view_v1 = simple_view("app", "v1", vec![dep_edge(&v1, &users)]);
        let view_v2 = simple_view("app", "v2", vec![dep_edge(&v2, &orders)]);
        let mut catalog = Catalog::empty();
        catalog.views.push(view_v1);
        catalog.views.push(view_v2);

        let triggers = vec![DepTrigger {
            qname: users.clone(),
            column: None,
        }];
        let affected = walk_transitive(&triggers, &catalog);
        assert_eq!(affected, vec![v1]);
    }

    #[test]
    fn no_triggers_no_affected() {
        let v = simple_view("app", "v", vec![]);
        let mut catalog = Catalog::empty();
        catalog.views.push(v);
        let affected = walk_transitive(&[], &catalog);
        assert!(affected.is_empty());
    }

    // --- extend_with_dependent_recreations ---

    #[test]
    fn extend_adds_replace_body_for_affected_view() {
        let users = qn("app", "users");
        let v_qname = qn("app", "v");
        let view = simple_view("app", "v", vec![dep_edge(&v_qname, &users)]);

        let mut catalog = Catalog::empty();
        catalog.views.push(view);

        let mut changes = vec![drop_table_change(users.clone())];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_enabled());
        assert!(result.is_ok());
        // changes[0] = DropTable, changes[1] = ReplaceBody for "v"
        assert_eq!(changes.len(), 2);
        assert!(
            matches!(
                &changes[1],
                Change::View(ViewChange::ReplaceBody { source, compatible: false, .. })
                if source.qname == v_qname
            ),
            "expected ReplaceBody for v, got: {:?}",
            changes[1]
        );
    }

    #[test]
    fn extend_does_not_duplicate_already_replaced_view() {
        let users = qn("app", "users");
        let v_qname = qn("app", "v");
        let view = simple_view("app", "v", vec![dep_edge(&v_qname, &users)]);
        let mut catalog = Catalog::empty();
        catalog.views.push(view.clone());

        // Differ already produced a ReplaceBody for "v".
        let mut changes = vec![
            drop_table_change(users.clone()),
            Change::View(ViewChange::ReplaceBody {
                source: view.clone(),
                catalog: view,
                compatible: false,
            }),
        ];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_enabled());
        assert!(result.is_ok());
        // Still 2 changes — no duplicate.
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn policy_disabled_returns_error_with_names() {
        let users = qn("app", "users");
        let v_qname = qn("app", "v");
        let view = simple_view("app", "v", vec![dep_edge(&v_qname, &users)]);
        let mut catalog = Catalog::empty();
        catalog.views.push(view);

        let mut changes = vec![drop_table_change(users.clone())];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_disabled());
        assert!(result.is_err());
        let affected = result.unwrap_err();
        assert!(
            affected.contains(&v_qname),
            "expected v in error list: {affected:?}"
        );
        // Changes should NOT be modified when policy blocks.
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn no_changes_no_recreation() {
        let view = simple_view("app", "v", vec![]);
        let mut catalog = Catalog::empty();
        catalog.views.push(view);

        let mut changes = vec![];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_enabled());
        assert!(result.is_ok());
        assert!(changes.is_empty());
    }

    // ── MV-specific tests ────────────────────────────────────────────────────

    fn simple_mv(
        schema: &str,
        name: &str,
        deps: Vec<DepEdge>,
    ) -> crate::ir::view::MaterializedView {
        crate::ir::view::MaterializedView {
            qname: qn(schema, name),
            columns: vec![],
            body_canonical: body("SELECT 1"),
            body_dependencies: deps,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        }
    }

    fn mv_dep_edge(from_mv: &QualifiedName, to_table: &QualifiedName) -> DepEdge {
        DepEdge {
            from: NodeId::Mv(from_mv.clone()),
            to: NodeId::Table(to_table.clone()),
            source: DepSource::AstExtracted,
        }
    }

    /// A single MV with a `body_dependency` on a table; a table-drop trigger
    /// should produce an `MvChange::ReplaceBody` for that MV.
    #[test]
    fn mv_with_body_dep_on_table_is_recreated_on_table_drop() {
        let orders = qn("app", "orders");
        let mv_qname = qn("app", "totals");
        let mv = simple_mv("app", "totals", vec![mv_dep_edge(&mv_qname, &orders)]);
        let mut catalog = Catalog::empty();
        catalog.materialized_views.push(mv);

        let mut changes = vec![drop_table_change(orders.clone())];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_enabled());
        assert!(result.is_ok());
        // changes[0] = DropTable, changes[1] = ReplaceBody for "totals"
        assert_eq!(
            changes.len(),
            2,
            "expected DropTable + MvChange::ReplaceBody"
        );
        assert!(
            matches!(
                &changes[1],
                Change::Mv(MvChange::ReplaceBody { source, .. })
                if source.qname == mv_qname
            ),
            "expected MvChange::ReplaceBody for totals, got: {:?}",
            changes[1]
        );
    }

    /// An MV `ReplaceBody` that is already in changes is not double-emitted.
    #[test]
    fn mv_replace_body_not_duplicated() {
        let orders = qn("app", "orders");
        let mv_qname = qn("app", "totals");
        let mv = simple_mv("app", "totals", vec![mv_dep_edge(&mv_qname, &orders)]);
        let mut catalog = Catalog::empty();
        catalog.materialized_views.push(mv.clone());

        let mut changes = vec![
            drop_table_change(orders.clone()),
            Change::Mv(MvChange::ReplaceBody {
                source: mv.clone(),
                catalog: mv,
            }),
        ];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_enabled());
        assert!(result.is_ok());
        // Still 2 changes — no duplicate.
        assert_eq!(
            changes.len(),
            2,
            "should not duplicate MvChange::ReplaceBody"
        );
    }

    /// When policy blocks recreations, affected MV qnames appear in the error.
    #[test]
    fn mv_recreation_blocked_by_policy_returns_error() {
        let orders = qn("app", "orders");
        let mv_qname = qn("app", "totals");
        let mv = simple_mv("app", "totals", vec![mv_dep_edge(&mv_qname, &orders)]);
        let mut catalog = Catalog::empty();
        catalog.materialized_views.push(mv);

        let mut changes = vec![drop_table_change(orders.clone())];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_disabled());
        assert!(result.is_err(), "policy disabled must return Err");
        let affected = result.unwrap_err();
        assert!(
            affected.contains(&mv_qname),
            "expected totals in error list: {affected:?}"
        );
        assert_eq!(
            changes.len(),
            1,
            "changes must not be modified when policy blocks"
        );
    }

    // ── User-type cascade tests ───────────────────────────────────────────────

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

    fn type_dep_edge(from_view: &QualifiedName, to_type: &QualifiedName) -> DepEdge {
        DepEdge {
            from: NodeId::View(from_view.clone()),
            to: NodeId::Type(to_type.clone()),
            source: DepSource::AstExtracted,
        }
    }

    fn mv_type_dep_edge(from_mv: &QualifiedName, to_type: &QualifiedName) -> DepEdge {
        DepEdge {
            from: NodeId::Mv(from_mv.clone()),
            to: NodeId::Type(to_type.clone()),
            source: DepSource::AstExtracted,
        }
    }

    /// Dropping a type that a view depends on (via `body_dependencies`) causes
    /// the walker to append a `ReplaceBody` for that view.
    #[test]
    fn type_drop_cascades_to_dependent_view() {
        let status_type = qn("app", "status");
        let v_qname = qn("app", "order_view");
        let view = simple_view(
            "app",
            "order_view",
            vec![type_dep_edge(&v_qname, &status_type)],
        );

        let mut catalog = Catalog::empty();
        catalog.views.push(view);

        let mut changes = vec![Change::UserType(UserTypeChange::Drop(status_type.clone()))];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_enabled());
        assert!(result.is_ok());
        // changes[0] = Drop type, changes[1] = ReplaceBody for order_view
        assert_eq!(
            changes.len(),
            2,
            "expected UserType::Drop + ReplaceBody for dependent view"
        );
        assert!(
            matches!(
                &changes[1],
                Change::View(ViewChange::ReplaceBody { source, compatible: false, .. })
                if source.qname == v_qname
            ),
            "expected ReplaceBody for order_view, got: {:?}",
            changes[1]
        );
    }

    /// Replacing a type with cascade also triggers dependent view recreation.
    #[test]
    fn type_replace_with_cascade_cascades_to_dependent_view() {
        let status_type = qn("app", "status");
        let v_qname = qn("app", "order_view");
        let view = simple_view(
            "app",
            "order_view",
            vec![type_dep_edge(&v_qname, &status_type)],
        );
        let ut = make_enum_type("app", "status");

        let mut catalog = Catalog::empty();
        catalog.views.push(view);

        let mut changes = vec![Change::UserType(UserTypeChange::ReplaceWithCascade {
            source: ut.clone(),
            catalog: ut.clone(),
        })];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_enabled());
        assert!(result.is_ok());
        assert_eq!(
            changes.len(),
            2,
            "expected ReplaceWithCascade + ReplaceBody for dependent view"
        );
        assert!(
            matches!(
                &changes[1],
                Change::View(ViewChange::ReplaceBody { source, compatible: false, .. })
                if source.qname == v_qname
            ),
            "expected ReplaceBody for order_view on cascade replace, got: {:?}",
            changes[1]
        );
    }

    /// Dropping a type that a materialized view depends on triggers MV recreation.
    #[test]
    fn type_drop_cascades_to_dependent_mv() {
        let status_type = qn("app", "status");
        let mv_qname = qn("app", "type_summary");
        let mv = simple_mv(
            "app",
            "type_summary",
            vec![mv_type_dep_edge(&mv_qname, &status_type)],
        );

        let mut catalog = Catalog::empty();
        catalog.materialized_views.push(mv);

        let mut changes = vec![Change::UserType(UserTypeChange::Drop(status_type.clone()))];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_enabled());
        assert!(result.is_ok());
        assert_eq!(
            changes.len(),
            2,
            "expected UserType::Drop + MvChange::ReplaceBody"
        );
        assert!(
            matches!(
                &changes[1],
                Change::Mv(MvChange::ReplaceBody { source, .. })
                if source.qname == mv_qname
            ),
            "expected MvChange::ReplaceBody for type_summary, got: {:?}",
            changes[1]
        );
    }

    /// When policy blocks recreations and a type drop would force view recreation,
    /// the error list includes the affected view name.
    #[test]
    fn type_drop_recreation_blocked_by_policy() {
        let status_type = qn("app", "status");
        let v_qname = qn("app", "order_view");
        let view = simple_view(
            "app",
            "order_view",
            vec![type_dep_edge(&v_qname, &status_type)],
        );

        let mut catalog = Catalog::empty();
        catalog.views.push(view);

        let mut changes = vec![Change::UserType(UserTypeChange::Drop(status_type.clone()))];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_disabled());
        assert!(result.is_err(), "policy must block recreation");
        let affected = result.unwrap_err();
        assert!(
            affected.contains(&v_qname),
            "expected order_view in error list: {affected:?}"
        );
        // Changes must not be modified when policy blocks.
        assert_eq!(changes.len(), 1);
    }

    // ── Function / Procedure cascade tests ───────────────────────────────────

    use crate::diff::change::FunctionChange;
    use crate::ir::function::{
        Function, FunctionLanguage, NormalizedArgTypes, ParallelSafety, ReturnType, SecurityMode,
        Volatility,
    };
    use crate::ir::procedure::Procedure;

    fn make_function(schema: &str, name: &str) -> Function {
        let args = vec![];
        let arg_types_normalized = NormalizedArgTypes::from_args(&args);
        Function {
            qname: qn(schema, name),
            args,
            arg_types_normalized,
            return_type: ReturnType::Scalar {
                ty: ColumnType::BigInt,
            },
            language: FunctionLanguage::Sql,
            body: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            volatility: Volatility::Volatile,
            strict: false,
            security: SecurityMode::Invoker,
            parallel: ParallelSafety::Unsafe,
            leakproof: false,
            cost: None,
            rows: None,
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn make_procedure(schema: &str, name: &str) -> Procedure {
        Procedure {
            qname: qn(schema, name),
            args: vec![],
            language: FunctionLanguage::PlPgSql,
            body: NormalizedBody::empty(),
            body_dependencies: vec![],
            security: SecurityMode::Invoker,
            commits_in_body: false,
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn function_dep_edge(from_view: &QualifiedName, to_fn: &QualifiedName) -> DepEdge {
        // We use NormalizedArgTypes::from_args with an empty arg list for testing.
        let nat = NormalizedArgTypes::from_args(&[]);
        DepEdge {
            from: NodeId::View(from_view.clone()),
            to: NodeId::Function(to_fn.clone(), nat),
            source: DepSource::AstExtracted,
        }
    }

    fn procedure_dep_edge(from_view: &QualifiedName, to_proc: &QualifiedName) -> DepEdge {
        DepEdge {
            from: NodeId::View(from_view.clone()),
            to: NodeId::Procedure(to_proc.clone()),
            source: DepSource::AstExtracted,
        }
    }

    /// Dropping a function that a view calls (via `body_dependencies`) causes
    /// the walker to append a `ReplaceBody` for that view.
    #[test]
    fn function_drop_cascades_to_dependent_view() {
        let fn_qname = qn("app", "f");
        let v_qname = qn("app", "v_using_f");
        let view = simple_view(
            "app",
            "v_using_f",
            vec![function_dep_edge(&v_qname, &fn_qname)],
        );
        let f = make_function("app", "f");

        let mut catalog = Catalog::empty();
        catalog.views.push(view);
        catalog.functions.push(f.clone());

        let mut changes = vec![Change::Function(FunctionChange::Drop {
            qname: fn_qname.clone(),
            args: f.arg_types_normalized.clone(),
        })];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_enabled());
        assert!(result.is_ok());
        // changes[0] = FunctionChange::Drop, changes[1] = ReplaceBody for v_using_f
        assert_eq!(
            changes.len(),
            2,
            "expected Function::Drop + ReplaceBody for dependent view, got: {changes:?}"
        );
        assert!(
            matches!(
                &changes[1],
                Change::View(ViewChange::ReplaceBody { source, compatible: false, .. })
                if source.qname == v_qname
            ),
            "expected ReplaceBody for v_using_f, got: {:?}",
            changes[1]
        );
    }

    /// `FunctionChange::ReplaceWithCascade` also triggers dependent view recreation.
    #[test]
    fn function_replace_with_cascade_propagates_to_dependent_view() {
        let fn_qname = qn("app", "f");
        let v_qname = qn("app", "v_using_f");
        let view = simple_view(
            "app",
            "v_using_f",
            vec![function_dep_edge(&v_qname, &fn_qname)],
        );
        let f = make_function("app", "f");

        let mut catalog = Catalog::empty();
        catalog.views.push(view);
        catalog.functions.push(f.clone());

        let mut changes = vec![Change::Function(FunctionChange::ReplaceWithCascade {
            source: f.clone(),
            catalog: f,
        })];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_enabled());
        assert!(result.is_ok());
        assert_eq!(
            changes.len(),
            2,
            "expected ReplaceWithCascade + ReplaceBody for dependent view, got: {changes:?}"
        );
        assert!(
            matches!(
                &changes[1],
                Change::View(ViewChange::ReplaceBody { source, compatible: false, .. })
                if source.qname == v_qname
            ),
            "expected ReplaceBody for v_using_f on cascade replace, got: {:?}",
            changes[1]
        );
    }

    /// A procedure with no dependents drops cleanly — no cascade recreation is
    /// triggered and the change list stays the same length.
    #[test]
    fn procedure_drop_does_not_cascade_unnecessarily() {
        let proc_qname = qn("app", "do_thing");
        // No views depend on this procedure.
        let mut catalog = Catalog::empty();
        catalog.procedures.push(make_procedure("app", "do_thing"));

        let mut changes = vec![Change::Procedure(ProcedureChange::Drop(proc_qname.clone()))];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_enabled());
        assert!(result.is_ok());
        assert_eq!(
            changes.len(),
            1,
            "a procedure drop with no dependents must not append extra changes; got: {changes:?}"
        );
    }

    /// Dropping a procedure that a view calls triggers view recreation.
    #[test]
    fn procedure_drop_cascades_to_dependent_view() {
        let proc_qname = qn("app", "do_thing");
        let v_qname = qn("app", "v_using_proc");
        let view = simple_view(
            "app",
            "v_using_proc",
            vec![procedure_dep_edge(&v_qname, &proc_qname)],
        );

        let mut catalog = Catalog::empty();
        catalog.views.push(view);
        catalog.procedures.push(make_procedure("app", "do_thing"));

        let mut changes = vec![Change::Procedure(ProcedureChange::Drop(proc_qname.clone()))];
        let result = extend_with_dependent_recreations(&mut changes, &catalog, &policy_enabled());
        assert!(result.is_ok());
        assert_eq!(
            changes.len(),
            2,
            "expected Procedure::Drop + ReplaceBody for dependent view; got: {changes:?}"
        );
        assert!(
            matches!(
                &changes[1],
                Change::View(ViewChange::ReplaceBody { source, compatible: false, .. })
                if source.qname == v_qname
            ),
            "expected ReplaceBody for v_using_proc, got: {:?}",
            changes[1]
        );
    }
}
