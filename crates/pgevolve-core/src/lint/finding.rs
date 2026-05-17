//! [`Finding`] and [`Severity`] — the unit of lint output.

use crate::parse::SourceLocation;

/// Severity of a single [`Finding`]. Affects exit codes only — both severities
/// are printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// A rule violation that should fail the lint.
    Error,
    /// A best-practice violation; does not fail the lint.
    Warning,
    /// Drift / divergence detected at plan time that pgevolve declines
    /// to act on without explicit user instruction. By default, plan
    /// fails when any `LintAtPlan` finding is present without a matching
    /// `[[lint_waiver]]` row in `intent.toml`. See arch spec Decisions
    /// 13–14.
    LintAtPlan,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::LintAtPlan => "lint-at-plan",
        })
    }
}

/// One lint finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Severity classification.
    pub severity: Severity,
    /// Stable rule identifier (used for filter / deny / explain).
    pub rule: &'static str,
    /// Human-readable message.
    pub message: String,
    /// Optional source location.
    pub location: Option<SourceLocation>,
}

impl Finding {
    /// Build an error-severity finding.
    pub fn error(rule: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            rule,
            message: message.into(),
            location: None,
        }
    }

    /// Build a warning-severity finding.
    pub fn warning(rule: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            rule,
            message: message.into(),
            location: None,
        }
    }

    /// Build a lint-at-plan-severity finding.
    pub fn lint_at_plan(rule: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::LintAtPlan,
            rule,
            message: message.into(),
            location: None,
        }
    }

    /// Attach a source location.
    #[must_use]
    pub fn at(mut self, loc: SourceLocation) -> Self {
        self.location = Some(loc);
        self
    }
}
