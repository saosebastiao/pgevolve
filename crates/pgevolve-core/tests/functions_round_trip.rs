//! Docker-gated round-trip tests for function and procedure catalog reads.
//!
//! Each test creates an ephemeral Postgres 17 container, applies a short DDL
//! snippet, introspects via [`pgevolve_core::catalog::read_catalog`], and
//! asserts that the resulting `catalog.functions` / `catalog.procedures` have
//! the expected shape.
//!
//! All tests skip cleanly when Docker is unavailable (or when
//! `PGEVOLVE_DISABLE_DOCKER_TESTS` is set) — they call [`docker_available`]
//! at the top and return early with a log line.

use anyhow::{Result, anyhow};
use pgevolve_core::catalog::{CatalogFilter, DriftReport, PgVersion, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::function::{FunctionLanguage, Volatility};
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, docker_available};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;

/// Helper: spin up PG 17, apply `sql`, introspect, and return `(catalog, drift)`.
async fn read_catalog_and_drift_from_sql(sql: &str) -> Result<(Catalog, DriftReport)> {
    let pg = EphemeralPostgres::start(PgVersion::Pg17).await?;
    pg.exec_sql(sql).await?;
    let client = pg.connect().await?;
    let querier = PgCatalogQuerier::new(client)?;
    let managed = vec![Identifier::from_unquoted("app").map_err(|e| anyhow!(e))?];
    let filter = CatalogFilter::new(managed, vec![]).map_err(|e| anyhow!(e.to_string()))?;
    let (catalog, drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await?
        .map_err(|e| anyhow!(e.to_string()))?;
    let catalog = catalog.canonicalize().map_err(|e| anyhow!(e.to_string()))?;
    Ok((catalog, drift))
}

// ---------------------------------------------------------------------------
// Test 1: SQL function round-trip
// ---------------------------------------------------------------------------

/// `CREATE FUNCTION … LANGUAGE sql IMMUTABLE STRICT` surfaces with the correct
/// qname, language, volatility, and strict flag.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_sql_function() {
    if !docker_available() {
        eprintln!("skipping catalog_reads_sql_function: Docker not available");
        return;
    }
    let sql = r"
        CREATE SCHEMA app;
        CREATE FUNCTION app.double(x integer) RETURNS integer
            LANGUAGE sql IMMUTABLE STRICT
            AS $$ SELECT x * 2 $$;
    ";
    let (catalog, _drift) = read_catalog_and_drift_from_sql(sql)
        .await
        .expect("catalog read");

    assert_eq!(catalog.functions.len(), 1, "expected 1 function");
    let f = &catalog.functions[0];
    assert_eq!(f.qname.to_string(), "app.double");
    assert_eq!(f.language, FunctionLanguage::Sql);
    assert!(
        matches!(f.volatility, Volatility::Immutable),
        "expected Immutable, got {:?}",
        f.volatility
    );
    assert!(f.strict, "expected strict=true");
    assert_eq!(f.args.len(), 1, "expected 1 arg");
    assert_eq!(
        f.args[0]
            .name
            .as_ref()
            .map(pgevolve_core::identifier::Identifier::as_str),
        Some("x"),
        "expected arg name 'x'"
    );
}

// ---------------------------------------------------------------------------
// Test 2: PL/pgSQL function round-trip
// ---------------------------------------------------------------------------

/// A PL/pgSQL function surfaces as `FunctionLanguage::PlPgSql`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_plpgsql_function() {
    if !docker_available() {
        eprintln!("skipping catalog_reads_plpgsql_function: Docker not available");
        return;
    }
    let sql = r"
        CREATE SCHEMA app;
        CREATE FUNCTION app.greet(name text) RETURNS text
            LANGUAGE plpgsql
            AS $$
            BEGIN
                RETURN 'hello, ' || name;
            END
            $$;
    ";
    let (catalog, _drift) = read_catalog_and_drift_from_sql(sql)
        .await
        .expect("catalog read");

    assert_eq!(catalog.functions.len(), 1, "expected 1 function");
    let f = &catalog.functions[0];
    assert_eq!(f.qname.to_string(), "app.greet");
    assert_eq!(
        f.language,
        FunctionLanguage::PlPgSql,
        "expected PlPgSql language"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Procedure with COMMIT in body
// ---------------------------------------------------------------------------

/// A procedure with `COMMIT` in the body surfaces with `commits_in_body=true`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_procedure_with_commit() {
    if !docker_available() {
        eprintln!("skipping catalog_reads_procedure_with_commit: Docker not available");
        return;
    }
    let sql = r"
        CREATE SCHEMA app;
        CREATE PROCEDURE app.do_work()
            LANGUAGE plpgsql
            AS $$
            BEGIN
                -- do some work
                COMMIT;
            END
            $$;
    ";
    let (catalog, _drift) = read_catalog_and_drift_from_sql(sql)
        .await
        .expect("catalog read");

    assert_eq!(catalog.procedures.len(), 1, "expected 1 procedure");
    let p = &catalog.procedures[0];
    assert_eq!(p.qname.to_string(), "app.do_work");
    assert!(
        p.commits_in_body,
        "expected commits_in_body=true for procedure with COMMIT"
    );
}

// ---------------------------------------------------------------------------
// Test 4: Overloaded functions
// ---------------------------------------------------------------------------

/// Two functions with the same qname but different arg types both surface.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_overloaded_functions() {
    if !docker_available() {
        eprintln!("skipping catalog_reads_overloaded_functions: Docker not available");
        return;
    }
    let sql = r"
        CREATE SCHEMA app;
        CREATE FUNCTION app.stringify(x integer) RETURNS text
            LANGUAGE sql IMMUTABLE STRICT
            AS $$ SELECT x::text $$;
        CREATE FUNCTION app.stringify(x boolean) RETURNS text
            LANGUAGE sql IMMUTABLE STRICT
            AS $$ SELECT x::text $$;
    ";
    let (catalog, _drift) = read_catalog_and_drift_from_sql(sql)
        .await
        .expect("catalog read");

    assert_eq!(
        catalog.functions.len(),
        2,
        "expected 2 overloaded functions, got {:?}",
        catalog
            .functions
            .iter()
            .map(|f| f.qname.to_string())
            .collect::<Vec<_>>()
    );
    // Both have the same qname.
    assert!(
        catalog
            .functions
            .iter()
            .all(|f| f.qname.to_string() == "app.stringify")
    );
    // Their arg type hashes must differ.
    assert_ne!(
        catalog.functions[0].arg_types_normalized.canonical_hash,
        catalog.functions[1].arg_types_normalized.canonical_hash,
        "overloads must have distinct arg type hashes"
    );
}

// ---------------------------------------------------------------------------
// Test 5: Unsupported language (plperl) is skipped and reported in drift
// ---------------------------------------------------------------------------

/// A `plperl` function must be skipped by the assembler (`catalog.functions`
/// stays empty) and its `(qname, language)` must appear in
/// `drift.unmanaged_language_routines`.
///
/// Note: `plperl` is not available in the default Postgres Alpine image.
/// We skip this test if loading the language fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_skips_plperl_function_and_reports_drift() {
    if !docker_available() {
        eprintln!("skipping catalog_skips_plperl_function_and_reports_drift: Docker not available");
        return;
    }

    // Attempt to create a plperl function. On Alpine images plperl is not
    // installed, so we catch a failure here and skip gracefully.
    let pg = match EphemeralPostgres::start(PgVersion::Pg17).await {
        Ok(pg) => pg,
        Err(e) => {
            eprintln!("skipping: failed to start container: {e}");
            return;
        }
    };

    let ddl = r"
        CREATE SCHEMA app;
        CREATE EXTENSION plperl;
        CREATE FUNCTION app.perl_hello() RETURNS text
            LANGUAGE plperl
            AS $$ return 'hello'; $$;
    ";
    match pg.exec_sql(ddl).await {
        Ok(()) => {}
        Err(e) => {
            eprintln!(
                "skipping catalog_skips_plperl_function_and_reports_drift: plperl not available ({e})"
            );
            return;
        }
    }

    let client = match pg.connect().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping: connect failed: {e}");
            return;
        }
    };
    let querier = PgCatalogQuerier::new(client).expect("querier");
    let managed = vec![Identifier::from_unquoted("app").unwrap()];
    let filter = CatalogFilter::new(managed, vec![]).unwrap();
    let (catalog, drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await
        .expect("spawn")
        .expect("read");
    let catalog = catalog.canonicalize().expect("canonicalize");

    assert!(
        catalog.functions.is_empty(),
        "plperl function must be skipped; got {:?}",
        catalog.functions
    );
    assert_eq!(
        drift.unmanaged_language_routines.len(),
        1,
        "expected 1 unmanaged language routine; got {:?}",
        drift.unmanaged_language_routines
    );
    let (drift_qname, drift_lang) = &drift.unmanaged_language_routines[0];
    assert_eq!(drift_qname.to_string(), "app.perl_hello");
    assert_eq!(drift_lang.as_str(), "plperl");
}
