//! [`PlanError`] — errors raised by the dependency analyzer / planner.

use thiserror::Error;

/// Errors raised by the plan-ordering phase.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlanError {
    /// A dependency cycle remained after the planner attempted to break it
    /// by extracting FK constraints. Carries the rendered node identifiers
    /// participating in the cycle.
    #[error("unbreakable dependency cycle: {0:?}")]
    UnbreakableCycle(Vec<String>),

    /// After FK extraction the modify-graph topo sort still cycled. This
    /// indicates a non-FK cycle that the planner cannot resolve and is
    /// almost certainly a bug in upstream phases.
    #[error("unexpected cycle in modify graph after FK extraction: {0:?}")]
    UnexpectedCycleAfterFkExtraction(Vec<String>),

    /// The drop-graph topo sort cycled. Drops never have legitimate cycles;
    /// this indicates a corrupt target catalog.
    #[error("unexpected cycle in drop graph: {0:?}")]
    UnexpectedDropCycle(Vec<String>),

    /// An internal invariant was violated.
    #[error("internal error: {0}")]
    Internal(String),

    /// Body-derived cycle in the dependency graph.
    ///
    /// FK cycles between tables are auto-extracted into deferred FK adds
    /// (`UnbreakableCycle` is a different error). Body-derived cycles —
    /// view A queries view B that queries view A — have no general
    /// mechanical fix and surface here. User must edit source to break
    /// the cycle.
    #[error("body-derived dependency cycle: {}", format_node_chain(.nodes))]
    BodyCycle {
        /// Nodes participating in the cycle, in graph-walk order.
        nodes: Vec<crate::plan::edges::NodeId>,
    },

    /// An AST resolution error escalated to plan time (e.g., a sub-spec
    /// resolver runs after the initial parse pass).
    #[error("AST resolution failed during planning: {0}")]
    AstResolution(String),

    /// The `view_drop_create_dependents` policy is `false` and at least one
    /// change would force dependent views to be dropped and recreated. Carries
    /// the names of the blocked views.
    ///
    /// Resolution: either enable `view_drop_create_dependents` in the planner
    /// policy, or modify the migration to explicitly DROP + CREATE the listed
    /// views.
    #[error(
        "view_drop_create_dependents is disabled but the following views would be \
         force-recreated: {}", .views.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
    )]
    DependentViewsBlocked {
        /// Views that would need to be recreated.
        views: Vec<crate::identifier::QualifiedName>,
    },
}

fn format_node_chain(nodes: &[crate::plan::edges::NodeId]) -> String {
    nodes
        .iter()
        .map(render_node)
        .collect::<Vec<_>>()
        .join(" \u{2192} ")
}

fn render_node(n: &crate::plan::edges::NodeId) -> String {
    use crate::plan::edges::NodeId::{
        Aggregate, Cast, Collation, Constraint, EventTrigger, Extension, Function, Index, Mv,
        Procedure, Publication, Schema, Sequence, Statistic, Subscription, Table, Trigger, Type,
        View,
    };
    match n {
        Schema(s) | Extension(s) | Publication(s) | Subscription(s) | EventTrigger(s) => {
            s.as_str().to_string()
        }
        Table(q) | Index(q) | Sequence(q) | View(q) | Mv(q) | Type(q) | Procedure(q)
        | Statistic(q) | Collation(q) => q.to_string(),
        Trigger(qname) => format!("trigger {qname}"),
        Constraint { table, name } => format!("{}.{}", table, name.as_str()),
        Function(q, args) | Aggregate(q, args) => format!(
            "{}({})",
            q,
            args.types
                .iter()
                .map(crate::ir::column_type::ColumnType::render_sql)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Cast(src, tgt) => format!("cast({src} as {tgt})"),
    }
}
