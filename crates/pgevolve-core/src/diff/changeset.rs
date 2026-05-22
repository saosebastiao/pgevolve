//! `ChangeSet` — an *unordered* collection of [`ChangeEntry`] values, plus
//! two observation accumulators for downstream lint rules.
//!
//! Ordering and dependency analysis are the planner's job (phase 5); the differ
//! emits changes in whatever order is convenient.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::grant::GrantTarget;

use super::change::{Change, ChangeEntry};
use super::destructiveness::Destructiveness;

/// A grant that exists in the live catalog but whose grantee role is not
/// mentioned anywhere in the source catalog (unmanaged role).
///
/// The differ does not emit a REVOKE for unmanaged grantees (lenient policy);
/// it surfaces them here so Stage 11 lint rules can flag them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnmanagedGrantObservation {
    /// Human-readable label for the object (e.g., `"schema app"`).
    pub object_label: String,
    /// SQL privilege keyword (e.g., `"SELECT"`).
    pub privilege_label: String,
    /// The unmanaged role name.
    pub role_name: Identifier,
}

/// A REVOKE step that is being emitted against a role that is the same as the
/// object's owner.
///
/// Revoking from the owner is typically a no-op in Postgres (owners always
/// have implicit privileges). Stage 11 lint rules can warn about this pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokeWithOwnerObservation {
    /// Human-readable label for the object (e.g., `"table app.users"`).
    pub object_label: String,
    /// SQL privilege keyword (e.g., `"SELECT"`).
    pub privilege_label: String,
    /// The grantee being revoked.
    pub grantee: GrantTarget,
    /// The declared owner of the object.
    pub owner: Identifier,
}

/// An unordered set of [`ChangeEntry`] values.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangeSet {
    /// The change entries.
    pub entries: Vec<ChangeEntry>,
    /// Grants observed in the live catalog whose grantee role is not tracked
    /// in the source catalog (lenient-policy observations).
    ///
    /// Stage 11 lint rules read this to produce `grants-to-unmanaged-role`
    /// warnings.
    #[serde(default)]
    pub unmanaged_grants: Vec<UnmanagedGrantObservation>,
    /// REVOKE steps being emitted against the declared owner of the object.
    ///
    /// Stage 11 lint rules read this to produce `revoke-from-owner` warnings.
    #[serde(default)]
    pub revokes_with_owner: Vec<RevokeWithOwnerObservation>,
}

impl ChangeSet {
    /// Construct an empty changeset.
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            unmanaged_grants: Vec::new(),
            revokes_with_owner: Vec::new(),
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

    /// Move every entry from `other` into `self`, including observation vecs.
    pub fn extend(&mut self, other: Self) {
        self.entries.extend(other.entries);
        self.unmanaged_grants.extend(other.unmanaged_grants);
        self.revokes_with_owner.extend(other.revokes_with_owner);
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
