//! Online rewrite: upgrade plain `REFRESH MATERIALIZED VIEW` steps to
//! `REFRESH MATERIALIZED VIEW CONCURRENTLY` when the target MV has at least
//! one unique index (spec §6.5).
//!
//! # Gate conditions
//!
//! The rewrite fires only when **all** of the following hold:
//! 1. `policy.refresh_mv_concurrently()` returns `true`.
//! 2. `policy.is_online()` is `true` (i.e., the strategy is `Online`).
//! 3. The `RefreshMaterializedView` step targets an MV that has at least one
//!    unique index in `catalog`.
//!
//! When condition 3 fails (no unique index), a `Warning`-severity lint finding
//! is emitted instead; the REFRESH step is left as a plain non-concurrent
//! refresh (which blocks concurrent reads but is still safe).
//!
//! # Correctness note on `concurrently=false` + `OutsideTransaction`
//!
//! `REFRESH MATERIALIZED VIEW CONCURRENTLY` must run outside a transaction.
//! We update `step.transactional` to `OutsideTransaction` when upgrading.
//! Plain `REFRESH MATERIALIZED VIEW` runs inside a transaction and we leave
//! `transactional` unchanged when no upgrade is made.

use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::ir::index::IndexParent;
use crate::lint::Finding;
use crate::plan::policy::PlannerPolicy;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::views::emit_refresh_mv;

/// Apply the REFRESH CONCURRENTLY upgrade pass over `steps`.
///
/// Mutates steps in-place; appends lint `findings` when an MV lacks a unique
/// index and the concurrent refresh therefore cannot fire.
pub(crate) fn rewrite(
    steps: &mut [RawStep],
    catalog: &Catalog,
    policy: &PlannerPolicy,
    findings: &mut Vec<Finding>,
) {
    if !policy.refresh_mv_concurrently() {
        return;
    }
    if !policy.is_online() {
        return;
    }

    // Collect MVs being CREATEd in this same plan. CREATE MATERIALIZED VIEW
    // emits `WITH NO DATA`, so the REFRESH that follows is the first populate
    // — and PG rejects `REFRESH CONCURRENTLY` on a not-yet-populated MV with
    // `[0A000] CONCURRENTLY cannot be used when the materialized view is not
    // populated`. The first-populate REFRESH must stay non-concurrent even
    // when a unique index already exists.
    let created_in_plan: std::collections::BTreeSet<QualifiedName> = steps
        .iter()
        .filter(|s| s.kind == StepKind::CreateMaterializedView)
        .filter_map(|s| s.targets.first().cloned())
        .collect();

    for step in steps.iter_mut() {
        if step.kind != StepKind::RefreshMaterializedView {
            continue;
        }
        let Some(target_qname) = step.targets.first().cloned() else {
            continue;
        };

        if created_in_plan.contains(&target_qname) {
            // First-populate REFRESH for a fresh MV — must stay non-concurrent.
            continue;
        }

        if mv_has_unique_index(catalog, &target_qname) {
            step.sql = emit_refresh_mv(&target_qname, true);
            // CONCURRENTLY cannot run inside a transaction.
            step.transactional = TransactionConstraint::OutsideTransaction;
        } else {
            findings.push(Finding::warning(
                "refresh-concurrently-needs-unique-index",
                format!(
                    "MV {target_qname} has no unique index; plain REFRESH issued (blocks reads). \
                     Add a unique index and re-plan to enable CONCURRENTLY refresh."
                ),
            ));
        }
    }
}

/// Returns `true` iff `catalog.indexes` contains at least one unique index
/// whose parent is the given MV.
fn mv_has_unique_index(catalog: &Catalog, mv_qname: &QualifiedName) -> bool {
    catalog
        .indexes
        .iter()
        .any(|ix| ix.unique && matches!(&ix.on, IndexParent::Mv(q) if q == mv_qname))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::redundant_clone)] // test helpers use clone for clarity
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::catalog::Catalog;
    use crate::ir::index::{
        Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
    };
    use crate::plan::policy::{OnlineRewrites, PlannerPolicy, Strategy};
    use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn make_refresh_step(mv_qname: QualifiedName) -> RawStep {
        RawStep {
            step_no: 0,
            kind: StepKind::RefreshMaterializedView,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![mv_qname.clone()],
            sql: emit_refresh_mv(&mv_qname, false),
            transactional: TransactionConstraint::InTransaction,
        }
    }

    fn make_unique_mv_index(mv_qname: QualifiedName) -> Index {
        Index {
            qname: qn("app", "mv_uid_idx"),
            on: IndexParent::Mv(mv_qname),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("id")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: true,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
            storage: crate::ir::reloptions::IndexStorageOptions::default(),
        }
    }

    fn online_policy() -> PlannerPolicy {
        PlannerPolicy::default()
    }

    fn atomic_policy() -> PlannerPolicy {
        PlannerPolicy {
            strategy: Strategy::Atomic,
            online: OnlineRewrites::all_enabled(),
            planner_ruleset_version: 1,
        }
    }

    fn disabled_policy() -> PlannerPolicy {
        let mut p = PlannerPolicy::default();
        p.online.refresh_mv_concurrently = false;
        p
    }

    #[test]
    fn mv_with_unique_index_upgrades_to_concurrent() {
        let mv_qname = qn("app", "summary");
        let mut catalog = Catalog::empty();
        catalog.indexes.push(make_unique_mv_index(mv_qname.clone()));

        let mut steps = vec![make_refresh_step(mv_qname.clone())];
        let mut findings = Vec::new();
        rewrite(&mut steps, &catalog, &online_policy(), &mut findings);

        assert!(findings.is_empty(), "no findings expected: {findings:?}");
        assert!(
            steps[0].sql.contains("CONCURRENTLY"),
            "expected CONCURRENTLY: {}",
            steps[0].sql
        );
        assert_eq!(
            steps[0].transactional,
            TransactionConstraint::OutsideTransaction
        );
    }

    #[test]
    fn mv_without_unique_index_stays_plain_and_emits_warning() {
        let mv_qname = qn("app", "summary");
        let catalog = Catalog::empty(); // no indexes

        let mut steps = vec![make_refresh_step(mv_qname.clone())];
        let mut findings = Vec::new();
        rewrite(&mut steps, &catalog, &online_policy(), &mut findings);

        assert_eq!(findings.len(), 1, "expected exactly one warning");
        assert_eq!(findings[0].rule, "refresh-concurrently-needs-unique-index");
        assert!(
            !steps[0].sql.contains("CONCURRENTLY"),
            "expected plain REFRESH: {}",
            steps[0].sql
        );
        assert_eq!(steps[0].transactional, TransactionConstraint::InTransaction);
    }

    #[test]
    fn atomic_strategy_skips_rewrite() {
        let mv_qname = qn("app", "summary");
        let mut catalog = Catalog::empty();
        catalog.indexes.push(make_unique_mv_index(mv_qname.clone()));

        let mut steps = vec![make_refresh_step(mv_qname.clone())];
        let mut findings = Vec::new();
        rewrite(&mut steps, &catalog, &atomic_policy(), &mut findings);

        assert!(findings.is_empty());
        assert!(
            !steps[0].sql.contains("CONCURRENTLY"),
            "atomic mode must not upgrade: {}",
            steps[0].sql
        );
    }

    #[test]
    fn disabled_policy_skips_rewrite() {
        let mv_qname = qn("app", "summary");
        let mut catalog = Catalog::empty();
        catalog.indexes.push(make_unique_mv_index(mv_qname.clone()));

        let mut steps = vec![make_refresh_step(mv_qname.clone())];
        let mut findings = Vec::new();
        rewrite(&mut steps, &catalog, &disabled_policy(), &mut findings);

        assert!(findings.is_empty());
        assert!(
            !steps[0].sql.contains("CONCURRENTLY"),
            "disabled policy must not upgrade: {}",
            steps[0].sql
        );
    }

    #[test]
    fn non_unique_mv_index_does_not_upgrade() {
        let mv_qname = qn("app", "summary");
        let mut catalog = Catalog::empty();
        // Non-unique index on the MV.
        catalog.indexes.push(Index {
            unique: false,
            ..make_unique_mv_index(mv_qname.clone())
        });

        let mut steps = vec![make_refresh_step(mv_qname.clone())];
        let mut findings = Vec::new();
        rewrite(&mut steps, &catalog, &online_policy(), &mut findings);

        assert_eq!(findings.len(), 1, "should warn about missing unique index");
        assert!(
            !steps[0].sql.contains("CONCURRENTLY"),
            "non-unique index must not trigger concurrent: {}",
            steps[0].sql
        );
    }

    #[test]
    fn table_unique_index_does_not_upgrade_mv() {
        // A unique index on a *table* with the same qname should not count.
        let mv_qname = qn("app", "summary");
        let mut catalog = Catalog::empty();
        catalog.indexes.push(Index {
            on: IndexParent::Table(mv_qname.clone()), // table, not MV
            unique: true,
            ..make_unique_mv_index(mv_qname.clone())
        });

        let mut steps = vec![make_refresh_step(mv_qname.clone())];
        let mut findings = Vec::new();
        rewrite(&mut steps, &catalog, &online_policy(), &mut findings);

        assert_eq!(findings.len(), 1, "table index should not count for MV");
        assert!(
            !steps[0].sql.contains("CONCURRENTLY"),
            "table index must not trigger MV concurrent: {}",
            steps[0].sql
        );
    }

    #[test]
    fn first_populate_refresh_after_create_stays_non_concurrent() {
        // CREATE MATERIALIZED VIEW emits WITH NO DATA, so the immediately-
        // following REFRESH is the first populate. PG rejects REFRESH
        // CONCURRENTLY on an unpopulated MV, so the rewrite must skip it
        // even when a unique index exists.
        let mv_qname = qn("app", "summary");
        let mut catalog = Catalog::empty();
        catalog.indexes.push(make_unique_mv_index(mv_qname.clone()));

        let mut steps = vec![
            RawStep {
                step_no: 0,
                kind: StepKind::CreateMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![mv_qname.clone()],
                sql: "CREATE MATERIALIZED VIEW app.summary AS SELECT 1 WITH NO DATA;".to_string(),
                transactional: TransactionConstraint::InTransaction,
            },
            make_refresh_step(mv_qname.clone()),
        ];
        let mut findings = Vec::new();
        rewrite(&mut steps, &catalog, &online_policy(), &mut findings);

        // CREATE untouched.
        assert!(steps[0].sql.contains("CREATE MATERIALIZED VIEW"));
        // First-populate REFRESH stays non-concurrent.
        assert!(
            !steps[1].sql.contains("CONCURRENTLY"),
            "REFRESH following CREATE must stay non-concurrent (first populate); got: {}",
            steps[1].sql
        );
    }

    #[test]
    fn refresh_without_preceding_create_upgrades_to_concurrent() {
        // A standalone REFRESH (no CREATE in the same plan) for an existing
        // MV with a unique index DOES get upgraded to CONCURRENTLY.
        let mv_qname = qn("app", "summary");
        let mut catalog = Catalog::empty();
        catalog.indexes.push(make_unique_mv_index(mv_qname.clone()));

        let mut steps = vec![make_refresh_step(mv_qname.clone())];
        let mut findings = Vec::new();
        rewrite(&mut steps, &catalog, &online_policy(), &mut findings);

        assert!(
            steps[0].sql.contains("CONCURRENTLY"),
            "standalone REFRESH should upgrade to CONCURRENTLY; got: {}",
            steps[0].sql
        );
    }
}
