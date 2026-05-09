//! Structured representation of one or more differences between two IR values.

use serde::{Deserialize, Serialize};

/// A single named difference between two IR values.
///
/// Fields capture the path to the differing position (e.g., a column name)
/// and a JSON-encoded representation of the two values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Difference {
    /// Dotted path to the differing field (e.g., `columns.email.ty`).
    pub path: String,
    /// Old value, as displayed (`Display` impl, not `Debug`).
    pub from: String,
    /// New value, as displayed.
    pub to: String,
}

impl Difference {
    /// Construct a `Difference` from displayable values.
    pub fn new<F: std::fmt::Display, T: std::fmt::Display>(
        path: impl Into<String>,
        from: F,
        to: T,
    ) -> Self {
        Self {
            path: path.into(),
            from: from.to_string(),
            to: to.to_string(),
        }
    }

    /// Prefix the path of this entry with the given prefix.
    #[must_use]
    pub fn prefix_path(self, prefix: &str) -> Self {
        Self {
            path: if self.path.is_empty() {
                prefix.into()
            } else {
                format!("{prefix}.{}", self.path)
            },
            ..self
        }
    }
}
