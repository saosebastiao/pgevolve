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
//!
//! ### v0.2 (Docker-bound)
//!
//! 5. **`view_canonicalization_closed_under_pg_rewrite`** — verifies the
//!    v0.2 invariant: `NormalizedBody::from_sql` applied to a view body
//!    yields the same canonical text as `NormalizedBody::from_sql` applied
//!    to the body Postgres stores in `pg_get_viewdef`. In other words,
//!    the canonicalizer is *closed* under the PG rewrite: source-side and
//!    catalog-side canonicalization produce the same result. A divergence
//!    here is a bug in the canonicalizer. This test is `#[ignore]`'d and
//!    Docker-gated; run manually with
//!    `cargo test -- --ignored view_canonicalization_closed_under_pg_rewrite`.
//!    `arb_view_dependency_graph` (spec §12 step 12.2) is deferred — it
//!    requires a non-trivial proptest generator for arbitrary dep graphs.
//!
//! All tests in this file are #[ignore]'d for CI. Run with
//! `cargo test --test property_tests -- --ignored` locally, or via the
//! property-tests.yml workflow.

use proptest::prelude::*;

use pgevolve_core::parse::normalize_body::NormalizedBody;
use pgevolve_core::plan::{NodeId, PlanId, build_create_graph};
use pgevolve_testkit::{IRGeneratorConfig, arbitrary_catalog, docker_available};

// ---------------------------------------------------------------------------
// v0.2: representative view bodies for the closure invariant test
// ---------------------------------------------------------------------------

/// A fixed set of representative view SELECT bodies. These are kept as a
/// const array rather than an arbitrary generator because the load-bearing
/// property is the *closure invariant* (source canon == catalog canon), not
/// the breadth of the generator itself. Five to ten bodies that exercise
/// different SELECT shapes are sufficient.
const VIEW_BODIES: &[&str] = &[
    "SELECT id FROM app.users",
    "SELECT id, email FROM app.users",
    "SELECT u.id AS user_id, u.email FROM app.users u",
    "SELECT id FROM app.users WHERE id > 0",
    "SELECT count(*) AS c FROM app.users",
    "SELECT id, email FROM app.users WHERE id > 10 ORDER BY email",
    "SELECT u.id, u.email FROM app.users u WHERE u.id IS NOT NULL",
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

        let catalog_body: String = rt.block_on(async {
            use pgevolve_testkit::EphemeralPostgres;
            let pg = EphemeralPostgres::start(pgevolve_testkit::default_pg_version())
                .await
                .expect("ephemeral PG");
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
            raw
        });

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
}
