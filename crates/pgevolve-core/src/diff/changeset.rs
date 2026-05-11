//! `ChangeSet` — an *unordered* collection of [`ChangeEntry`] values.
//!
//! Ordering and dependency analysis are the planner's job (phase 5); the differ
//! emits changes in whatever order is convenient.

use serde::{Deserialize, Serialize};

use super::change::{Change, ChangeEntry};
use super::destructiveness::Destructiveness;

/// An unordered set of [`ChangeEntry`] values.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangeSet {
    /// The change entries.
    pub entries: Vec<ChangeEntry>,
}

impl ChangeSet {
    /// Construct an empty changeset.
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Append a change with its destructiveness classification.
    pub fn push(&mut self, change: Change, destructiveness: Destructiveness) {
        self.entries.push(ChangeEntry {
            change,
            destructiveness,
        });
    }

    /// True iff there are no entries.
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of entries.
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Iterator over the entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &ChangeEntry> {
        self.entries.iter()
    }

    /// Move every entry from `other` into `self`.
    pub fn extend(&mut self, other: Self) {
        self.entries.extend(other.entries);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn new_is_empty() {
        let cs = ChangeSet::new();
        assert!(cs.is_empty());
        assert_eq!(cs.len(), 0);
        assert!(cs.iter().next().is_none());
    }

    #[test]
    fn push_then_len_is_one() {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::DropSchema(id("old")),
            Destructiveness::RequiresApproval {
                reason: "drops schema".into(),
            },
        );
        assert!(!cs.is_empty());
        assert_eq!(cs.len(), 1);
    }

    #[test]
    fn extend_concatenates() {
        let mut a = ChangeSet::new();
        a.push(
            Change::DropSchema(id("a")),
            Destructiveness::RequiresApproval { reason: "x".into() },
        );
        let mut b = ChangeSet::new();
        b.push(
            Change::DropSchema(id("b")),
            Destructiveness::RequiresApproval { reason: "x".into() },
        );
        a.extend(b);
        assert_eq!(a.len(), 2);
    }
}
