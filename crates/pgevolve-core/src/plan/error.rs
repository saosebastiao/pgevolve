//! Stub — replaced in task 5.5.

use thiserror::Error;

/// Errors raised by the plan-ordering phase.
#[derive(Debug, Error)]
pub enum PlanError {
    /// A dependency cycle could not be broken by FK extraction.
    #[error("unbreakable dependency cycle: {0:?}")]
    UnbreakableCycle(Vec<String>),
    /// Internal invariant violation.
    #[error("internal error: {0}")]
    Internal(String),
}
