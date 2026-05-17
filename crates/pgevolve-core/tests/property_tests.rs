//! Tier-5 property tests.
//!
//! Two properties land in v0.1 — both pure (no Docker required):
//!
//! 1. **`plan_id_is_deterministic`** — for the same source/target catalog
//!    and the same `(version, ruleset_version)`, `PlanId::compute` returns
//!    the same bytes on repeat invocations.
//! 2. **`create_graph_topo_sorts_or_only_fk_cycles`** — the dependency graph
//!    over any generated `Catalog` topologically sorts, except when the
//!    cycle nodes are all FK-bound (which the planner extracts as a
//!    post-pass).
//!
//! Property tests that depend on a live Postgres (round-trip, idempotency,
//! end-to-end equivalence) land in v0.1.x once `IRMutator` exists and the
//! generator's coverage is tight enough to survive PG normalization.
//!
//! All tests in this file are #[ignore]'d for CI. Run with
//! `cargo test --test property_tests -- --ignored` locally, or via the
//! property-tests.yml workflow.

use proptest::prelude::*;

use pgevolve_core::plan::{NodeId, PlanId, build_create_graph};
use pgevolve_testkit::{IRGeneratorConfig, arbitrary_catalog};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Property: planning C → C for any random catalog C produces an
    /// empty change set. Pure; no Docker.
    #[ignore = "property test — run via property-tests workflow or `cargo test -- --ignored`"]
    #[test]
    fn plan_minimality_under_no_op_mutations(
        catalog in arbitrary_catalog(IRGeneratorConfig::default()),
    ) {
        let drift = pgevolve_core::catalog::DriftReport::default();
        let changes = pgevolve_core::diff::diff(&catalog, &catalog, &drift);
        prop_assert!(changes.is_empty(), "C → C produced {:?}", changes);
    }

    /// PlanId is deterministic across re-runs.
    #[ignore = "property test — run via property-tests workflow or `cargo test -- --ignored`"]
    #[test]
    fn plan_id_is_deterministic(
        source in arbitrary_catalog(IRGeneratorConfig::default()),
        target in arbitrary_catalog(IRGeneratorConfig::default()),
    ) {
        let a = PlanId::compute(&source, &target, "0.1.0", 1);
        let b = PlanId::compute(&source, &target, "0.1.0", 1);
        prop_assert_eq!(a, b);
        // And differ when the ruleset version differs.
        let c = PlanId::compute(&source, &target, "0.1.0", 2);
        prop_assert_ne!(a, c);
    }

    /// The dependency graph over any generated `Catalog` topologically
    /// sorts. (The generator does not currently emit FK cycles; if it ever
    /// does, the cycle's nodes must be FK-bound, which the planner's
    /// `extract_fk_cycles_and_resort` resolves.)
    #[ignore = "property test — run via property-tests workflow or `cargo test -- --ignored`"]
    #[test]
    fn create_graph_topo_sorts_or_only_fk_cycles(
        catalog in arbitrary_catalog(IRGeneratorConfig::default()),
    ) {
        let g = build_create_graph(&catalog);
        match g.topological_sort() {
            Ok(_) => {}
            Err(cycle) => {
                // If the generator produced a cycle, every participating
                // node must be a Table or Constraint (those are the only
                // node kinds an FK cycle can involve).
                for node in &cycle.nodes {
                    prop_assert!(
                        matches!(node, NodeId::Table(_) | NodeId::Constraint { .. }),
                        "cycle contains non-FK node: {node:?}",
                    );
                }
            }
        }
    }
}
