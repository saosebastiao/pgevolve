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
}
