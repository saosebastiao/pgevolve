//! `CREATE INDEX [CONCURRENTLY]` / `DROP INDEX [CONCURRENTLY]` rewrite (spec §6.5).
//!
//! When the target table already exists in the live catalog, building the
//! index concurrently avoids holding `ACCESS EXCLUSIVE` long enough to block
//! writes. CONCURRENTLY can't run inside a transaction, so the resulting step
//! is emitted with [`TransactionConstraint::OutsideTransaction`] and ends up
//! in its own group.
//!
//! Unique indexes are NOT concurrent-rewritten in v0.1: a failed
//! `CREATE UNIQUE INDEX CONCURRENTLY` leaves an INVALID index that must be
//! cleaned up out-of-band, and v0.1 plays it safe. Document as a known
//! limitation; a future opt-in switch can lift the restriction.

use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::ir::index::Index;
use crate::plan::policy::PlannerPolicy;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::sql;

/// Should this `CreateIndex` be rewritten as `CREATE INDEX CONCURRENTLY`?
///
/// Three conditions, all required:
/// 1. the policy enables `create_index_concurrent`,
/// 2. the index targets a table that already exists in `target` (i.e., not
///    being created in the same plan), and
/// 3. the index is not `UNIQUE` (see module comment).
pub fn should_rewrite_create(idx: &Index, target: &Catalog, policy: &PlannerPolicy) -> bool {
    // Note: for `IndexParent::Mv`, `target.table_exists()` returns false
    // because `table_exists` only consults `catalog.tables`. Skipping the
    // concurrent rewrite for MV indexes is correct: PostgreSQL does NOT
    // support `CREATE INDEX CONCURRENTLY` on materialized views (PG emits
    // "CREATE INDEX CONCURRENTLY cannot be executed within a pipeline" or
    // equivalent depending on context). MV indexes go through the inline
    // `CREATE INDEX` path. T7 may want to add a sibling
    // `should_rewrite_concurrent_mv_refresh` for the REFRESH CONCURRENTLY
    // pattern, which IS supported.
    policy.create_index_concurrent() && target.table_exists(idx.on.qname()) && !idx.unique
}

/// Should this `DropIndex` be rewritten as `DROP INDEX CONCURRENTLY`?
///
/// Mirrors `should_rewrite_create`: enabled by the same policy switch, and
/// only valid for non-unique indexes that exist in the target catalog (so we
/// can read `unique` from there).
pub fn should_rewrite_drop(
    qname: &QualifiedName,
    target: &Catalog,
    policy: &PlannerPolicy,
) -> bool {
    if !policy.create_index_concurrent() {
        return false;
    }
    target
        .indexes
        .iter()
        .find(|i| &i.qname == qname)
        .is_some_and(|i| !i.unique)
}

/// Build the `CREATE INDEX CONCURRENTLY` step.
///
/// Caller must have first verified [`should_rewrite_create`].
pub fn create_step(idx: &Index, destructive: bool, destructive_reason: Option<String>) -> RawStep {
    RawStep {
        step_no: 0,
        kind: StepKind::CreateIndexConcurrent,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: vec![idx.qname.clone(), idx.on.qname().clone()],
        sql: sql::create_index(idx, true),
        transactional: TransactionConstraint::OutsideTransaction,
    }
}

/// Build the `DROP INDEX CONCURRENTLY` step.
///
/// Caller must have first verified [`should_rewrite_drop`].
pub fn drop_step(
    qname: &QualifiedName,
    destructive: bool,
    destructive_reason: Option<String>,
) -> RawStep {
    RawStep {
        step_no: 0,
        kind: StepKind::DropIndexConcurrent,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: vec![qname.clone()],
        sql: sql::drop_index(qname, true),
        transactional: TransactionConstraint::OutsideTransaction,
    }
}
