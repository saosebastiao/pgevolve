//! [`PlanIoError`] — errors raised by plan-directory I/O (read/write).

use std::path::PathBuf;

use thiserror::Error;

use crate::plan::plan::InvalidPlanHash;

/// Errors raised when reading or writing a plan directory
/// (`plan.sql` + `intent.toml` + `manifest.toml`).
#[derive(Debug, Error)]
pub enum PlanIoError {
    /// I/O error on a specific path.
    #[error("i/o error on {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),

    /// A `-- @pgevolve` directive line failed to parse.
    #[error("malformed directive: {0}")]
    MalformedDirective(String),

    /// The plan id encoded in `plan.sql`, `intent.toml`, and `manifest.toml`
    /// did not agree.
    #[error("plan id mismatch: sql={sql} intent={intent} manifest={manifest}")]
    PlanIdMismatch {
        /// Short plan id parsed from `plan.sql`.
        sql: String,
        /// Short plan id parsed from `intent.toml`.
        intent: String,
        /// Short plan id parsed from `manifest.toml`.
        manifest: String,
    },

    /// TOML parse failure.
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),

    /// TOML serialize failure (when writing).
    #[error("toml serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),

    /// YAML parse failure.
    #[error("yaml parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    /// `manifest.toml`'s `plan_hash` field is not a valid 64-char hex string.
    #[error(transparent)]
    InvalidPlanHash(#[from] InvalidPlanHash),
}

impl PlanIoError {
    /// Construct an [`PlanIoError::Io`] from a path and an error.
    pub fn io(path: impl Into<PathBuf>, err: std::io::Error) -> Self {
        Self::Io(path.into(), err)
    }
}
