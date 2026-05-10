//! [`group_steps`] — partition a flat `Vec<RawStep>` into `TransactionGroup`s.
//!
//! Adjacent steps with the same [`TransactionConstraint`] coalesce into one
//! group. The resulting list is the unit the executor consumes:
//!
//! - Transactional groups run inside a single `BEGIN; ... COMMIT;`.
//! - Non-transactional groups are conceptually a sequence of singletons; each
//!   step runs autocommit. Coalescing them into a "group" is purely
//!   organizational — it keeps the directive-comment structure clean (spec §7.1).

use crate::plan::raw_step::{RawStep, TransactionConstraint};

/// A run of consecutive [`RawStep`]s that share a transactional / non-transactional
/// classification. Group ids are 1-indexed in emission order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionGroup {
    /// 1-indexed group id (assigned by [`group_steps`]).
    pub id: u32,
    /// True if every step in the group can run inside a single
    /// `BEGIN; ... COMMIT;`. Non-transactional groups (`CONCURRENTLY`, etc.)
    /// run as autocommit singletons even when the group has multiple steps.
    pub transactional: bool,
    /// The steps in the group, in emission order.
    pub steps: Vec<RawStep>,
}

/// Partition `steps` into groups by transactional boundary.
///
/// Adjacent steps with the same [`TransactionConstraint`] go in one group;
/// each transition between in-tx and out-of-tx starts a new group. Empty
/// input produces an empty output. Group ids are 1-indexed.
pub fn group_steps(steps: Vec<RawStep>) -> Vec<TransactionGroup> {
    let mut groups: Vec<TransactionGroup> = Vec::new();
    let mut current: Vec<RawStep> = Vec::new();
    let mut current_kind: Option<TransactionConstraint> = None;

    for step in steps {
        match current_kind {
            None => {
                current_kind = Some(step.transactional);
                current.push(step);
            }
            Some(prev) if prev == step.transactional => {
                current.push(step);
            }
            Some(prev) => {
                let id = u32::try_from(groups.len() + 1).unwrap_or(u32::MAX);
                groups.push(TransactionGroup {
                    id,
                    transactional: matches!(prev, TransactionConstraint::InTransaction),
                    steps: std::mem::take(&mut current),
                });
                current_kind = Some(step.transactional);
                current.push(step);
            }
        }
    }

    if let Some(prev) = current_kind {
        let id = u32::try_from(groups.len() + 1).unwrap_or(u32::MAX);
        groups.push(TransactionGroup {
            id,
            transactional: matches!(prev, TransactionConstraint::InTransaction),
            steps: current,
        });
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::raw_step::StepKind;

    fn step(kind: StepKind, c: TransactionConstraint) -> RawStep {
        RawStep {
            step_no: 0,
            kind,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: String::new(),
            transactional: c,
        }
    }

    #[test]
    fn empty_input_yields_no_groups() {
        let out = group_steps(vec![]);
        assert!(out.is_empty());
    }

    #[test]
    fn single_in_tx_step_yields_one_group() {
        let out = group_steps(vec![step(
            StepKind::CreateTable,
            TransactionConstraint::InTransaction,
        )]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, 1);
        assert!(out[0].transactional);
        assert_eq!(out[0].steps.len(), 1);
    }

    #[test]
    fn single_out_of_tx_step_yields_one_group() {
        let out = group_steps(vec![step(
            StepKind::CreateIndexConcurrent,
            TransactionConstraint::OutsideTransaction,
        )]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, 1);
        assert!(!out[0].transactional);
    }

    #[test]
    fn all_in_tx_steps_coalesce_into_one_group() {
        let out = group_steps(vec![
            step(StepKind::CreateSchema, TransactionConstraint::InTransaction),
            step(StepKind::CreateTable, TransactionConstraint::InTransaction),
            step(StepKind::CreateIndex, TransactionConstraint::InTransaction),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].steps.len(), 3);
        assert!(out[0].transactional);
    }

    #[test]
    fn transition_creates_new_group() {
        let out = group_steps(vec![
            step(StepKind::CreateTable, TransactionConstraint::InTransaction),
            step(
                StepKind::CreateIndexConcurrent,
                TransactionConstraint::OutsideTransaction,
            ),
            step(StepKind::ValidateConstraint, TransactionConstraint::InTransaction),
        ]);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].id, 1);
        assert!(out[0].transactional);
        assert_eq!(out[1].id, 2);
        assert!(!out[1].transactional);
        assert_eq!(out[2].id, 3);
        assert!(out[2].transactional);
    }

    #[test]
    fn consecutive_out_of_tx_steps_coalesce() {
        let out = group_steps(vec![
            step(
                StepKind::CreateIndexConcurrent,
                TransactionConstraint::OutsideTransaction,
            ),
            step(
                StepKind::DropIndexConcurrent,
                TransactionConstraint::OutsideTransaction,
            ),
        ]);
        assert_eq!(out.len(), 1);
        assert!(!out[0].transactional);
        assert_eq!(out[0].steps.len(), 2);
    }

    #[test]
    fn group_ids_are_one_indexed_and_sequential() {
        let out = group_steps(vec![
            step(StepKind::CreateTable, TransactionConstraint::InTransaction),
            step(
                StepKind::CreateIndexConcurrent,
                TransactionConstraint::OutsideTransaction,
            ),
            step(StepKind::CreateTable, TransactionConstraint::InTransaction),
            step(
                StepKind::CreateIndexConcurrent,
                TransactionConstraint::OutsideTransaction,
            ),
        ]);
        let ids: Vec<u32> = out.iter().map(|g| g.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
    }
}
