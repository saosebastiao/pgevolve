//! Docker-gated round-trip tests for user-defined type catalog reads.
//!
//! Each test creates an ephemeral Postgres 17 container, applies a short DDL
//! snippet, introspects via [`pgevolve_core::catalog::read_catalog`], and
//! asserts that the resulting `catalog.types` has the expected shape.
//!
//! All four tests skip cleanly when Docker is unavailable (or when
//! `PGEVOLVE_DISABLE_DOCKER_TESTS` is set) — they call [`docker_available`]
//! at the top and return early with a log line.

use anyhow::{Result, anyhow};
use pgevolve_core::catalog::{CatalogFilter, PgVersion, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::user_type::UserTypeKind;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, docker_available};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;

/// Helper: spin up PG 17, apply `sql`, introspect, and canonicalize.
async fn read_catalog_from_sql(sql: &str) -> Result<Catalog> {
    let pg = EphemeralPostgres::start(PgVersion::Pg17).await?;
    pg.exec_sql(sql).await?;
    let client = pg.connect().await?;
    let querier = PgCatalogQuerier::new(client)?;
    let managed = vec![Identifier::from_unquoted("app").map_err(|e| anyhow!(e))?];
    let filter = CatalogFilter::new(managed, vec![]).map_err(|e| anyhow!(e.to_string()))?;
    let (catalog, _drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await?
        .map_err(|e| anyhow!(e.to_string()))?;
    catalog.canonicalize().map_err(|e| anyhow!(e.to_string()))
}

// ---------------------------------------------------------------------------
// Test 1: Enum type round-trip
// ---------------------------------------------------------------------------

/// `CREATE TYPE … AS ENUM` surfaces as `UserTypeKind::Enum` with the correct
/// labels in sort-order sequence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_enum_type() {
    if !docker_available() {
        eprintln!("skipping catalog_reads_enum_type: Docker not available");
        return;
    }
    let sql = r"
        CREATE SCHEMA app;
        CREATE TYPE app.order_status AS ENUM ('pending', 'shipped', 'delivered');
    ";
    let catalog = read_catalog_from_sql(sql).await.expect("catalog read");

    assert_eq!(catalog.types.len(), 1, "expected 1 user type");
    let ut = &catalog.types[0];
    assert_eq!(ut.qname.to_string(), "app.order_status");

    let UserTypeKind::Enum { values } = &ut.kind else {
        panic!("expected Enum, got {:?}", ut.kind);
    };
    assert_eq!(values.len(), 3, "expected 3 enum values");
    assert_eq!(values[0].name, "pending");
    assert_eq!(values[1].name, "shipped");
    assert_eq!(values[2].name, "delivered");
    // sort_order must be strictly increasing.
    assert!(
        values[0].sort_order < values[1].sort_order,
        "sort_order must be strictly increasing"
    );
    assert!(
        values[1].sort_order < values[2].sort_order,
        "sort_order must be strictly increasing"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Domain type round-trip
// ---------------------------------------------------------------------------

/// `CREATE DOMAIN … AS …` with a named CHECK constraint surfaces as
/// `UserTypeKind::Domain` with the correct base type, nullability, and check.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_domain_type() {
    if !docker_available() {
        eprintln!("skipping catalog_reads_domain_type: Docker not available");
        return;
    }
    let sql = r"
        CREATE SCHEMA app;
        CREATE DOMAIN app.positive_int AS integer NOT NULL DEFAULT 1
            CONSTRAINT chk_positive CHECK (VALUE > 0);
    ";
    let catalog = read_catalog_from_sql(sql).await.expect("catalog read");

    assert_eq!(catalog.types.len(), 1, "expected 1 user type");
    let ut = &catalog.types[0];
    assert_eq!(ut.qname.to_string(), "app.positive_int");

    let UserTypeKind::Domain {
        base,
        nullable,
        default,
        check_constraints,
        ..
    } = &ut.kind
    else {
        panic!("expected Domain, got {:?}", ut.kind);
    };

    assert!(
        default.is_some(),
        "DEFAULT 1 must round-trip as Some(NormalizedExpr)",
    );

    // Base type must be integer.
    let base_str = format!("{base:?}");
    assert!(
        base_str.contains("Integer") || base_str.contains("integer"),
        "expected integer base type, got {base_str}"
    );
    assert!(!nullable, "NOT NULL domain must have nullable=false");
    assert_eq!(check_constraints.len(), 1, "expected 1 CHECK constraint");
    let check = &check_constraints[0];
    assert_eq!(check.name.as_str(), "chk_positive");
    // The canonical check expression must reference 'value' (lowercased by the normalizer)
    // and the literal 0.
    let expr_text = &check.expression.canonical_text;
    assert!(
        expr_text.contains("value") || expr_text.contains("VALUE"),
        "check expression must reference VALUE: {expr_text}"
    );
    assert!(
        expr_text.contains('0') || expr_text.contains("> 0"),
        "check expression must contain 0: {expr_text}"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Composite type round-trip
// ---------------------------------------------------------------------------

/// `CREATE TYPE … AS (…)` surfaces as `UserTypeKind::Composite` with the
/// correct attributes in declaration order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_composite_type() {
    if !docker_available() {
        eprintln!("skipping catalog_reads_composite_type: Docker not available");
        return;
    }
    let sql = r"
        CREATE SCHEMA app;
        CREATE TYPE app.address AS (
            street  text,
            city    text,
            zip     varchar(10)
        );
    ";
    let catalog = read_catalog_from_sql(sql).await.expect("catalog read");

    assert_eq!(catalog.types.len(), 1, "expected 1 user type");
    let ut = &catalog.types[0];
    assert_eq!(ut.qname.to_string(), "app.address");

    let UserTypeKind::Composite { attributes } = &ut.kind else {
        panic!("expected Composite, got {:?}", ut.kind);
    };
    assert_eq!(attributes.len(), 3, "expected 3 composite attributes");
    assert_eq!(attributes[0].name.as_str(), "street");
    assert_eq!(attributes[1].name.as_str(), "city");
    assert_eq!(attributes[2].name.as_str(), "zip");
}

// ---------------------------------------------------------------------------
// Test 4: Table row types do NOT appear in catalog.types
// ---------------------------------------------------------------------------

/// The auto-generated row type backing an ordinary table (`relkind='r'`) must
/// NOT appear in `catalog.types`. The exclusion guard in `SELECT_USER_TYPES`
/// checks `NOT (typtype='c' AND EXISTS (SELECT 1 FROM pg_class c WHERE
/// c.oid = t.typrelid AND c.relkind <> 'c'))`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_excludes_table_row_types() {
    if !docker_available() {
        eprintln!("skipping catalog_excludes_table_row_types: Docker not available");
        return;
    }
    let sql = r"
        CREATE SCHEMA app;
        -- An ordinary table: PG auto-creates a composite row type for it, but
        -- it must NOT appear in catalog.types.
        CREATE TABLE app.orders (
            id      bigint PRIMARY KEY,
            amount  numeric NOT NULL
        );
        -- An explicit composite type: this MUST appear in catalog.types.
        CREATE TYPE app.money_pair AS (
            amount   numeric,
            currency text
        );
    ";
    let catalog = read_catalog_from_sql(sql).await.expect("catalog read");

    // Only the explicit composite type must be present.
    assert_eq!(
        catalog.types.len(),
        1,
        "only the explicit composite must appear; got {:?}",
        catalog
            .types
            .iter()
            .map(|t| t.qname.to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(catalog.types[0].qname.to_string(), "app.money_pair");
    // And the table itself must appear as a table, not as a type.
    assert_eq!(catalog.tables.len(), 1);
    assert_eq!(catalog.tables[0].qname.to_string(), "app.orders");
}
