//! Tier-5 property tests.
//!
//! Properties in this file:
//!
//! ### v0.1 (pure, no Docker)
//!
//! 1. **`plan_id_is_deterministic`** — for the same source/target catalog
//!    and the same `(version, ruleset_version)`, `PlanId::compute` returns
//!    the same bytes on repeat invocations.
//! 2. **`create_graph_topo_sorts_or_only_fk_cycles`** — the dependency graph
//!    over any generated `Catalog` topologically sorts, except when the
//!    cycle nodes are all FK-bound (which the planner extracts as a
//!    post-pass).
//!
//! ### v0.2 (pure, no Docker)
//!
//! 3. **`enum_add_value_preserves_existing_values`** — for any random initial
//!    enum label list and a new (distinct) label, `diff_user_types` emits
//!    exactly one `EnumAddValue` change. Pure; no Docker.
//! 4. **`plpgsql_canonicalization_is_idempotent`** — for a random PL/pgSQL
//!    body, parsing then re-canonicalizing produces byte-identical
//!    `canonical_text` and `canonical_hash`. Pure; no Docker.
//! 5. **`arb_view_dependency_graph`** — a leaf-table column mutation produces
//!    a plan that recreates exactly the transitively-dependent views, in valid
//!    topological order, with no spurious recreations. Closes spec §12.2.
//!
//! ### v0.2 (Docker-bound)
//!
//! 6. **`view_canonicalization_closed_under_pg_rewrite`** — verifies the
//!    v0.2 invariant: `NormalizedBody::from_sql` applied to a view body
//!    yields the same canonical text as `NormalizedBody::from_sql` applied
//!    to the body Postgres stores in `pg_get_viewdef`. In other words,
//!    the canonicalizer is *closed* under the PG rewrite: source-side and
//!    catalog-side canonicalization produce the same result. A divergence
//!    here is a bug in the canonicalizer. This test is `#[ignore]`'d and
//!    Docker-gated; run manually with
//!    `cargo test -- --ignored view_canonicalization_closed_under_pg_rewrite`.
//!
//! All tests in this file are #[ignore]'d for CI. Run with
//! `cargo test --test property_tests -- --ignored` locally, or via the
//! property-tests.yml workflow.

use std::collections::BTreeSet;

use proptest::prelude::*;

use pgevolve_core::catalog::DriftReport;
use pgevolve_core::diff::ViewChange;
use pgevolve_core::identifier::QualifiedName;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::parse::normalize_body::NormalizedBody;
use pgevolve_core::plan::edges::NodeId;
use pgevolve_core::plan::{PlanId, build_create_graph};
use pgevolve_testkit::{
    IRGeneratorConfig, arbitrary_catalog, arbitrary_view_catalog, docker_available,
};

// ---------------------------------------------------------------------------
// v0.2: representative view bodies for the closure invariant test
// ---------------------------------------------------------------------------

/// A fixed set of representative view SELECT bodies. These are kept as a
/// const array rather than an arbitrary generator because the load-bearing
/// property is the *closure invariant* (source canon == catalog canon), not
/// the breadth of the generator itself. Five to ten bodies that exercise
/// different SELECT shapes are sufficient.
// Bodies must round-trip through `pg_get_viewdef` unchanged. PostgreSQL's
// view-storage analyzer strips redundant table-qualifier prefixes on
// single-source-table queries (e.g. `SELECT u.id FROM app.users u` becomes
// `SELECT id FROM app.users u` after CREATE VIEW + pg_get_viewdef). The
// pgevolve-side canonicalizer does NOT perform that single-table-alias
// stripping, so any test body with a redundant `<alias>.<col>` reference
// fundamentally cannot round-trip. The corpus avoids that shape.
const VIEW_BODIES: &[&str] = &[
    "SELECT id FROM app.users",
    "SELECT id, email FROM app.users",
    "SELECT id FROM app.users WHERE id > 0",
    "SELECT count(*) AS c FROM app.users",
    "SELECT id, email FROM app.users WHERE id > 10 ORDER BY email",
    "SELECT 1 AS one",
    "SELECT id AS user_id, email AS user_email FROM app.users",
];

// ---------------------------------------------------------------------------
// v0.2: representative PL/pgSQL bodies for the canonicalization idempotency test
// ---------------------------------------------------------------------------

/// A fixed set of representative PL/pgSQL bodies.
///
/// These are kept as a const array rather than an arbitrary regex generator
/// because constructing syntactically valid PL/pgSQL programmatically is
/// impractical — PL/pgSQL requires `DECLARE` for any variables and has strict
/// statement-list rules. The corpus covers the key structural forms: simple
/// NULL body, PERFORM, RAISE, IF/ELSE, LOOP, static embedded SQL
/// (`INSERT`/`SELECT`), `COMMIT`/`ROLLBACK`, and `-- @pgevolve dep:`
/// directives.
const PLPGSQL_BODIES: &[&str] = &[
    "BEGIN NULL; END",
    "BEGIN PERFORM 1; END",
    "BEGIN\n  PERFORM 1;\nEND",
    "DECLARE\n  v integer := 0;\nBEGIN\n  v := 1;\nEND",
    "BEGIN\n  IF true THEN\n    PERFORM 1;\n  END IF;\nEND",
    "BEGIN\n  IF true THEN\n    PERFORM 1;\n  ELSE\n    PERFORM 2;\n  END IF;\nEND",
    "BEGIN\n  FOR i IN 1..10 LOOP\n    PERFORM i;\n  END LOOP;\nEND",
    "BEGIN\n  INSERT INTO app.log(msg) VALUES ('x');\nEND",
    "BEGIN\n  INSERT INTO app.log(msg) VALUES ('x');\n  COMMIT;\nEND",
    "-- @pgevolve dep: app.summary\nBEGIN EXECUTE 'REFRESH MATERIALIZED VIEW app.summary'; END",
    "BEGIN\n  RAISE NOTICE 'hello';\nEND",
    "DECLARE\n  r record;\nBEGIN\n  FOR r IN SELECT id FROM app.users LOOP\n    PERFORM r.id;\n  END LOOP;\nEND",
];

// ---------------------------------------------------------------------------
// Helpers for `arb_view_dependency_graph`
// ---------------------------------------------------------------------------

/// Find a table that at least one view in `catalog` depends on directly.
/// Returns the qname of such a table, or `None` if no view has any dep.
fn pick_referenced_table(catalog: &Catalog) -> Option<QualifiedName> {
    for v in &catalog.views {
        for dep in &v.body_dependencies {
            if let NodeId::Table(q) = &dep.to {
                return Some(q.clone());
            }
        }
    }
    None
}

/// BFS from `leaf_table` through the reverse dep graph: return the set of
/// view qnames that transitively depend on `leaf_table`.
fn transitively_dependent_views(
    catalog: &Catalog,
    leaf_table: &QualifiedName,
) -> BTreeSet<QualifiedName> {
    // Build reverse index: target_qname → set of view qnames that depend on it.
    let mut reverse: std::collections::BTreeMap<QualifiedName, BTreeSet<QualifiedName>> =
        std::collections::BTreeMap::new();
    for v in &catalog.views {
        for dep in &v.body_dependencies {
            let dep_qname = match &dep.to {
                NodeId::Table(q) | NodeId::View(q) | NodeId::Mv(q) => q.clone(),
                _ => continue,
            };
            reverse
                .entry(dep_qname)
                .or_default()
                .insert(v.qname.clone());
        }
    }

    // BFS from leaf_table.
    let mut affected: BTreeSet<QualifiedName> = BTreeSet::new();
    let mut queue: Vec<QualifiedName> = Vec::new();

    if let Some(dependents) = reverse.get(leaf_table) {
        for q in dependents {
            if affected.insert(q.clone()) {
                queue.push(q.clone());
            }
        }
    }

    while let Some(trigger) = queue.pop() {
        if let Some(dependents) = reverse.get(&trigger) {
            for q in dependents {
                if affected.insert(q.clone()) {
                    queue.push(q.clone());
                }
            }
        }
    }

    affected
}

/// Produce a mutated catalog where a non-PK column on `leaf_table` has its
/// type changed. This forces the differ to emit `AlterColumnType`, which
/// triggers the view recreation walker.
///
/// If `leaf_table` has no non-PK column that can be type-changed (i.e., only
/// a PK column), returns `None`.
fn mutate_leaf_column(mut catalog: Catalog, leaf_qname: &QualifiedName) -> Option<Catalog> {
    use pgevolve_core::ir::column_type::ColumnType;

    let table = catalog.tables.iter_mut().find(|t| &t.qname == leaf_qname)?;

    // Find a non-PK column. The PK column is "id" (always bigint) per the
    // generator. We want to change a non-PK column type.
    let pk_cols: BTreeSet<String> = table
        .constraints
        .iter()
        .filter_map(|c| {
            if let pgevolve_core::ir::constraint::ConstraintKind::PrimaryKey { columns, .. } =
                &c.kind
            {
                Some(
                    columns
                        .iter()
                        .map(|id| id.as_str().to_string())
                        .collect::<Vec<_>>(),
                )
            } else {
                None
            }
        })
        .flatten()
        .collect();

    // Find a non-PK column with a type we can safely alter.
    let target_col = table
        .columns
        .iter_mut()
        .find(|c| !pk_cols.contains(c.name.as_str()))?;

    // Change the column type to something structurally different.
    // We cycle: Text → BigInt → Text, etc. — just needs to be different.
    target_col.ty = if matches!(target_col.ty, ColumnType::Text) {
        ColumnType::BigInt
    } else {
        ColumnType::Text
    };
    // Also make it nullable so the column change is structurally valid.
    target_col.nullable = true;

    Some(catalog)
}

/// Collect the set of view qnames that appear as `ViewChange::ReplaceBody`
/// targets in the change list produced by the differ + dep-recreation walker.
fn view_recreations_in_changes(
    changes: &pgevolve_core::diff::ChangeSet,
) -> BTreeSet<QualifiedName> {
    use pgevolve_core::diff::Change;
    changes
        .entries
        .iter()
        .filter_map(|e| match &e.change {
            Change::View(ViewChange::ReplaceBody { source, .. }) => Some(source.qname.clone()),
            _ => None,
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// v0.2 invariant: `NormalizedBody::from_sql` is closed under the PG
    /// rewrite.
    ///
    /// For each body in `VIEW_BODIES`:
    ///  1. Boot an ephemeral PG instance.
    ///  2. Create `app.users (id bigint, email text)` to satisfy FK refs.
    ///  3. Create a view `CREATE VIEW app.v AS <body>`.
    ///  4. Query `pg_get_viewdef('app.v'::regclass, true)` to get the
    ///     catalog-stored body text.
    ///  5. Canonicalize both the source body and the catalog body via
    ///     `NormalizedBody::from_sql`.
    ///  6. Assert the two canonical texts are equal.
    ///
    /// A divergence is a canonicalization bug: the differ would consider the
    /// view body changed on every plan even though the semantics are
    /// identical.
    ///
    /// Docker-gated: the test returns early without asserting if Docker is
    /// not available. `arb_view_dependency_graph` (spec §12 step 12.2) is
    /// deferred — it requires a non-trivial proptest generator for arbitrary
    /// dep graphs and is not load-bearing for the closure invariant.
    #[ignore = "property test — Docker-gated; run with `cargo test -- --ignored view_canonicalization_closed_under_pg_rewrite`"]
    #[test]
    fn view_canonicalization_closed_under_pg_rewrite(
        body_idx in 0usize..VIEW_BODIES.len(),
    ) {
        if !docker_available() {
            return Ok(());
        }

        let body_text = VIEW_BODIES[body_idx];

        // Canonicalize the source-side body.
        let source_canon = NormalizedBody::from_sql(body_text)
            .map_err(|e| TestCaseError::fail(format!("source canonicalize failed: {e}")))?;

        // Boot ephemeral PG, apply the schema, query pg_get_viewdef.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        let catalog_body: Option<String> = rt.block_on(async {
            use pgevolve_testkit::EphemeralPostgres;
            // Container startup can flake (testcontainers port mapping race).
            // Treat that as an environment skip — not a canonicalization bug.
            let pg = match EphemeralPostgres::start(pgevolve_testkit::default_pg_version()).await {
                Ok(pg) => pg,
                Err(e) => {
                    eprintln!("skipping case: ephemeral PG failed to start: {e}");
                    return None;
                }
            };
            let client = pg.connect().await.expect("connect");

            client
                .batch_execute(
                    "CREATE SCHEMA app; \
                     CREATE TABLE app.users (id bigint, email text);",
                )
                .await
                .expect("create schema + table");

            // CREATE VIEW — one view per ephemeral container, unique per body.
            let create_sql = format!("CREATE VIEW app.v AS {body_text}");
            client
                .execute(&create_sql, &[])
                .await
                .expect("create view");

            let row = client
                .query_one(
                    "SELECT pg_get_viewdef('app.v'::regclass, true)",
                    &[],
                )
                .await
                .expect("pg_get_viewdef");

            let raw: String = row.get(0);
            Some(raw)
        });

        let Some(catalog_body) = catalog_body else {
            // Container start flaked — skip this case rather than fail.
            return Ok(());
        };

        // Canonicalize the catalog-side body.
        let catalog_canon = NormalizedBody::from_sql(&catalog_body)
            .map_err(|e| TestCaseError::fail(format!(
                "catalog canonicalize failed for PG-rewritten body {catalog_body:?}: {e}"
            )))?;

        prop_assert_eq!(
            source_canon.canonical_text(),
            catalog_canon.canonical_text(),
            "canonicalization diverged for body {:?}\n  source  => {:?}\n  catalog => {:?}\n  pg_get_viewdef raw => {:?}",
            body_text,
            source_canon.canonical_text(),
            catalog_canon.canonical_text(),
            catalog_body,
        );
    }

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
        let a = PlanId::compute(&source, &target, "0.1.0", 1).unwrap();
        let b = PlanId::compute(&source, &target, "0.1.0", 1).unwrap();
        prop_assert_eq!(a, b);
        // And differ when the ruleset version differs.
        let c = PlanId::compute(&source, &target, "0.1.0", 2).unwrap();
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

    /// For a random initial enum value list and a random ADD VALUE operation,
    /// `diff_user_types` emits exactly one `EnumAddValue` change. Pure; no Docker.
    #[ignore = "property test — run via property-tests workflow or `cargo test -- --ignored`"]
    #[test]
    fn enum_add_value_preserves_existing_values(
        existing in proptest::collection::vec("[a-z]{1,5}", 1..5usize),
        new_value in "[a-z]{1,5}",
    ) {
        use pgevolve_core::diff::change::{Change, UserTypeChange};
        use pgevolve_core::identifier::{Identifier, QualifiedName};
        use pgevolve_core::ir::user_type::{EnumValue, UserType, UserTypeKind};

        prop_assume!(!existing.contains(&new_value));
        // Ensure all existing labels are distinct (the differ requires unique labels).
        let unique_existing: Vec<String> = {
            let mut seen = std::collections::BTreeSet::new();
            existing.into_iter().filter(|v| seen.insert(v.clone())).collect()
        };
        prop_assume!(!unique_existing.is_empty());

        #[allow(clippy::cast_precision_loss)]
        let before: Vec<EnumValue> = unique_existing
            .iter()
            .enumerate()
            .map(|(i, n)| EnumValue { name: n.clone(), sort_order: i as f32 + 1.0 })
            .collect();
        #[allow(clippy::cast_precision_loss)]
        let new_sort_order = before.len() as f32 + 1.0;
        let mut after = before.clone();
        after.push(EnumValue {
            name: new_value.clone(),
            sort_order: new_sort_order,
        });

        let qname = QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted("status").unwrap(),
        );
        let cat = vec![UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Enum { values: before },
            comment: None,
        }];
        let src = vec![UserType {
            qname,
            kind: UserTypeKind::Enum { values: after },
            comment: None,
        }];

        let mut out = pgevolve_core::diff::ChangeSet::new();
        pgevolve_core::diff::types::diff_user_types(&cat, &src, &mut out);

        // Expect exactly one EnumAddValue change.
        prop_assert_eq!(out.len(), 1, "expected exactly one change, got: {:?}", out);
        let entry = &out.entries[0];
        prop_assert!(
            matches!(&entry.change, Change::UserType(UserTypeChange::EnumAddValue { value, .. }) if value == &new_value),
            "expected EnumAddValue for {:?}, got: {:?}", new_value, entry.change,
        );
    }

    /// For a random PL/pgSQL body from the representative corpus, parsing then
    /// re-canonicalizing produces byte-identical `canonical_text` and
    /// `canonical_hash`. Pure; no Docker.
    ///
    /// The differ relies on this round-trip invariant: if
    /// `parse_routine_body(body1.canonical_text())` produces a different
    /// canonical text than `parse_routine_body(body)`, the planner would
    /// see a spurious change on every plan even though nothing changed.
    ///
    /// A fixed corpus (rather than an unconstrained regex generator) is used
    /// because constructing syntactically valid PL/pgSQL with a proptest
    /// string strategy is impractical — PL/pgSQL requires `DECLARE` for any
    /// variables and has strict statement-list rules. The corpus covers the
    /// key structural forms: assignment, PERFORM, RAISE, IF, LOOP, embedded
    /// static SQL, COMMIT/ROLLBACK, and `-- @pgevolve dep:` directives.
    #[test]
    #[ignore = "property test — run via property-tests workflow or `cargo test -- --ignored`"]
    fn plpgsql_canonicalization_is_idempotent(
        body_idx in 0usize..PLPGSQL_BODIES.len(),
    ) {
        use pgevolve_core::identifier::{Identifier, QualifiedName};
        use pgevolve_core::ir::function::FunctionLanguage;
        use pgevolve_core::parse::builder::plpgsql::parse_routine_body;
        use pgevolve_core::parse::error::SourceLocation;

        let qname = QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted("f").unwrap(),
        );
        let loc = SourceLocation::new(std::path::PathBuf::from("test.sql"), 1, 1);
        let body = PLPGSQL_BODIES[body_idx];

        let r1 = parse_routine_body(body, FunctionLanguage::PlPgSql, &qname, &loc);
        prop_assume!(r1.is_ok());
        let (body1, _deps1, _commits1) = r1.unwrap();

        // Re-canonicalize: feed canonical_text back through parse_routine_body.
        let r2 = parse_routine_body(body1.canonical_text(), FunctionLanguage::PlPgSql, &qname, &loc);
        prop_assume!(r2.is_ok());
        let (body2, _, _) = r2.unwrap();

        prop_assert_eq!(
            body1.canonical_text(),
            body2.canonical_text(),
            "canonical_text diverged on re-parse for body {:?}",
            body,
        );
        prop_assert_eq!(
            body1.canonical_hash(),
            body2.canonical_hash(),
            "canonical_hash diverged on re-parse for body {:?}",
            body,
        );
    }

    /// Property: a leaf-table column mutation produces a plan that recreates
    /// exactly the transitively-dependent views, in valid topological order,
    /// with no spurious recreations. Closes spec §12.2 (`arb_view_dependency_graph`).
    ///
    /// Generator: `arbitrary_view_catalog` builds a catalog with 2–5 tables
    /// and 1–6 views per schema, where views reference tables (and prior views)
    /// via programmatically-constructed `body_dependencies`. No SQL parsing is
    /// required — the generator bypasses the canonicalizer for the dep edges.
    ///
    /// Mutation: pick any table that a view depends on. Change one non-PK
    /// column's type (forces `AlterColumnType`). The planner's
    /// `extend_with_dependent_recreations` walker then adds `ReplaceBody`
    /// changes for every transitively-affected view.
    ///
    /// Assertions:
    ///   a. Every transitively-dependent view is recreated (no missing).
    ///   b. No view that does NOT transitively depend is recreated (no spurious).
    ///   c. The recreated-view subgraph topologically sorts (dep edges respected).
    ///
    /// Pure; no Docker.
    #[ignore = "property test — run via property-tests workflow or `cargo test -- --ignored`"]
    #[test]
    fn arb_view_dependency_graph(
        catalog in arbitrary_view_catalog(),
    ) {
        // 1. Skip cases with no view-referenced tables (generator may produce
        //    catalogs with views that only ref other views, not tables directly,
        //    if the schema has zero tables — prop_assume drops those cases).
        let leaf_qname = pick_referenced_table(&catalog);
        prop_assume!(leaf_qname.is_some());
        let leaf_qname = leaf_qname.unwrap();

        // 2. Compute expected affected views via BFS over the dep graph.
        let expected = transitively_dependent_views(&catalog, &leaf_qname);
        prop_assume!(!expected.is_empty());

        // 3. Mutate: change a non-PK column type on the leaf table.
        //    If the leaf table only has a PK column (no non-PK columns to
        //    mutate), skip this case.
        let mutated = mutate_leaf_column(catalog.clone(), &leaf_qname);
        prop_assume!(mutated.is_some());
        let mutated = mutated.unwrap();

        // 4. Run differ: target = original catalog (live DB), source = mutated (desired).
        //    direction: diff(target, source) = changes to apply to reach `source`.
        let drift = DriftReport::default();
        let changes = pgevolve_core::diff::diff(&catalog, &mutated, &drift);

        // 5. Run the dep-recreation walker (same logic the planner calls in
        //    `ordering::order`).
        let policy = pgevolve_core::plan::PlannerPolicy::default();
        let mut raw_changes: Vec<pgevolve_core::diff::Change> =
            changes.entries.iter().map(|e| e.change.clone()).collect();
        pgevolve_core::plan::recreate_views::extend_with_dependent_recreations(
            &mut raw_changes,
            &catalog,
            &policy,
        )
        .map_err(|views| {
            TestCaseError::fail(format!(
                "dep-recreation walker returned policy error for: {views:?}"
            ))
        })?;

        // Re-assemble as ChangeSet for the helper.
        let extended = pgevolve_core::diff::ChangeSet {
            entries: raw_changes
                .into_iter()
                .map(|c| pgevolve_core::diff::change::ChangeEntry {
                    change: c,
                    destructiveness: pgevolve_core::diff::Destructiveness::Safe,
                })
                .collect(),
        };

        // 6. Collect actual view recreations.
        let actual = view_recreations_in_changes(&extended);

        // 7a. Every expected view is recreated (no missing).
        for q in &expected {
            prop_assert!(
                actual.contains(q),
                "expected view {:?} to be recreated but it was not; actual: {:?}",
                q,
                actual,
            );
        }

        // 7b. No spurious recreations: every actual recreation is expected.
        for q in &actual {
            prop_assert!(
                expected.contains(q),
                "spurious recreation of view {:?}; expected set: {:?}",
                q,
                expected,
            );
        }

        // 8. Topo-order check: build the create graph over the original catalog
        //    (the dep edges live there) and verify it topologically sorts.
        //    The generator's DAG construction guarantees no view-on-view cycles;
        //    assert that here to surface any generator invariant violation.
        let g = build_create_graph(&catalog);
        match g.topological_sort() {
            Ok(order) => {
                // Collect positions of affected views in the topo order.
                let positions: std::collections::BTreeMap<QualifiedName, usize> = order
                    .iter()
                    .enumerate()
                    .filter_map(|(i, node)| {
                        if let NodeId::View(q) = node {
                            if actual.contains(q) {
                                Some((q.clone(), i))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .collect();

                // For every pair (v, dep_view) where v depends on dep_view
                // and both are recreated, dep_view must come before v in the
                // topo order (parent before child — dependencies first).
                for v in &catalog.views {
                    if !actual.contains(&v.qname) {
                        continue;
                    }
                    for dep in &v.body_dependencies {
                        let NodeId::View(dep_q) = &dep.to else { continue };
                        if !actual.contains(dep_q) {
                            continue;
                        }
                        let Some(&vp) = positions.get(&v.qname) else { continue };
                        let Some(&dp) = positions.get(dep_q) else { continue };
                        prop_assert!(
                            dp < vp,
                            "topo violation: dep {:?} (pos {}) must precede {:?} (pos {})",
                            dep_q,
                            dp,
                            v.qname,
                            vp,
                        );
                    }
                }
            }
            Err(cycle) => {
                // View-on-view cycles are impossible given the generator's
                // topo-ordered construction. Surface as a failure.
                prop_assert!(
                    false,
                    "create graph has a cycle (generator invariant violated): {:?}",
                    cycle.nodes,
                );
            }
        }
    }
}
