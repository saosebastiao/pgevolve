//! Stub — replaced in task 5.3.
#![allow(missing_docs)]

use crate::diff::change::ChangeEntry;
use crate::identifier::QualifiedName;
use crate::ir::constraint::Constraint;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OrderedChangeSet {
    pub creates_and_adds: Vec<ChangeEntry>,
    pub modifies: Vec<ChangeEntry>,
    pub drops: Vec<ChangeEntry>,
    pub deferred_fks: Vec<DeferredFkAdd>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeferredFkAdd {
    pub table: QualifiedName,
    pub constraint: Constraint,
}
