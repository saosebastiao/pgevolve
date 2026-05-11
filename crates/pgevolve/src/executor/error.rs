//! [`ApplyError`] — errors raised by every stage of `apply()`.

use thiserror::Error;

/// Errors raised by [`apply`](super::apply) and its sub-stages.
#[derive(Debug, Error)]
pub enum ApplyError {
    /// `tokio_postgres` reported an error.
    #[error("postgres error: {0}")]
    Postgres(#[from] tokio_postgres::Error),

    /// Reading the plan directory failed.
    #[error("plan i/o: {0}")]
    PlanIo(#[from] pgevolve_core::plan::PlanIoError),

    /// Catalog read or assemble failed during preflight.
    #[error("catalog: {0}")]
    Catalog(#[from] pgevolve_core::catalog::CatalogError),

    /// The advisory lock is already held by another session.
    #[error("pgevolve advisory lock is held by another session")]
    LockHeld,

    /// The live target's identity hash does not match the plan's.
    #[error("target identity mismatch: plan={plan} live={live}")]
    TargetIdentityMismatch {
        /// Plan-time target identity.
        plan: String,
        /// Live identity computed at apply time.
        live: String,
    },

    /// The live catalog has drifted from the plan's `target_snapshot` since
    /// planning. The list of changes is rendered for the user.
    #[error("drift detected since planning: {0} change(s)")]
    DriftDetected(usize),

    /// One or more destructive intents were not approved.
    #[error("unapproved destructive intents: {count}")]
    UnapprovedIntents {
        /// Number of unapproved intents.
        count: usize,
        /// Brief listing for diagnostics: `(id, target, reason)` per intent.
        details: Vec<(u32, String, String)>,
    },

    /// A single step failed; the executor rolled back the enclosing group
    /// (if transactional) or stopped (if not).
    #[error("step {step_no} (group {group_no}) failed: {error}")]
    StepFailed {
        /// Failing step number.
        step_no: u32,
        /// Enclosing group id.
        group_no: u32,
        /// Postgres error message.
        error: String,
    },
}
