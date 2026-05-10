//! Stub — replaced in task 5.4.

use crate::diff::ChangeSet;
use crate::ir::catalog::Catalog;
use crate::plan::error::PlanError;
use crate::plan::ordered::OrderedChangeSet;

/// Order an unordered [`ChangeSet`] (stub — full impl in task 5.4).
pub fn order(
    _target: &Catalog,
    _source: &Catalog,
    _changes: ChangeSet,
) -> Result<OrderedChangeSet, PlanError> {
    Ok(OrderedChangeSet::default())
}
