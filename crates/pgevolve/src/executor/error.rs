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

    /// The executor aborted cleanly after a step because the caller passed
    /// `ApplyOverrides::abort_after_step`. Used by the testkit's chaos
    /// harness to validate the re-plan-and-resume recovery path.
    #[error("aborted cleanly after step {step_no}")]
    AbortedAfterStep {
        /// Step at which the abort fired.
        step_no: u32,
    },

    /// One or more `LintAtPlan` findings lack a matching `[[lint_waiver]]` in
    /// `intent.toml`. The plan cannot be applied until waivers are added or the
    /// source is corrected.
    #[error("unwaived LintAtPlan findings at apply time: {count} finding(s)")]
    LintWaiverMissing {
        /// Number of unwaived findings.
        count: usize,
        /// Brief listing for diagnostics: `(rule, target)` per finding.
        details: Vec<(String, String)>,
    },

    /// A plan step's SQL referenced an env var that wasn't set.
    #[error(
        "missing env var ${{{0}}} referenced by step {1}; required for subscription CONNECTION resolution"
    )]
    MissingEnvVar(String, u32),
}
