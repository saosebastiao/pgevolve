//! Docker-gated tests for `--shadow-validate` covering views.
//!
//! These tests call `shadow::validate::cross_check` directly (no CLI
//! subprocess) with a constructed `Catalog` containing one table and one
//! view. Skipped automatically when Docker is unavailable.

use pgevolve::shadow::validate::cross_check;
use pgevolve_core::catalog::PgVersion;
use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::column::Column;
use pgevolve_core::ir::column_type::ColumnType;
use pgevolve_core::ir::constraint::{Constraint, ConstraintKind, Deferrable};
use pgevolve_core::ir::schema::Schema;
use pgevolve_core::ir::table::Table;
use pgevolve_core::ir::view::{View, ViewColumn};
use pgevolve_core::parse::normalize_body::NormalizedBody;
use pgevolve_core::plan::edges::{DepEdge, DepSource, NodeId};
use pgevolve_testkit::ephemeral_pg::{default_pg_version, docker_available};

fn id(s: &str) -> Identifier {
    Identifier::from_unquoted(s).unwrap()
}

fn qn(schema: &str, name: &str) -> QualifiedName {
    QualifiedName::new(id(schema), id(name))
}

fn body(sql: &str) -> NormalizedBody {
    NormalizedBody::from_sql(sql).unwrap()
}

const fn pg_major(version: PgVersion) -> u32 {
    match version {
        PgVersion::Pg14 => 14,
        PgVersion::Pg15 => 15,
        PgVersion::Pg16 => 16,
        PgVersion::Pg17 => 17,
        PgVersion::Pg18 => 18,
    }
}

fn make_shadow_backend(version: PgVersion) -> Box<dyn pgevolve::shadow::ShadowBackend> {
    let pg_version_str = match version {
        PgVersion::Pg14 => "14",
        PgVersion::Pg15 => "15",
        PgVersion::Pg16 => "16",
        PgVersion::Pg17 => "17",
        PgVersion::Pg18 => "18",
    };
    let cfg = pgevolve::config::ShadowConfig {
        backend: Some("testcontainers".to_string()),
        postgres_version: Some(pg_version_str.to_string()),
        url: None,
        url_env: None,
        reset: None,
        extensions: vec![],
    };
    pgevolve::shadow::resolve(&cfg).expect("resolve backend")
}

/// Build a small catalog: `app` schema, `app.users` table, `app.active_users` view.
fn small_catalog_with_view() -> Catalog {
    let mut cat = Catalog::empty();

    cat.schemas.push(Schema::new(id("app")));

    cat.tables.push(Table {
        qname: qn("app", "users"),
        columns: vec![
            Column {
                name: id("id"),
                ty: ColumnType::BigInt,
                nullable: false,
                default: None,
                identity: None,
                generated: None,
                collation: None,
                storage: None,
                compression: None,
                comment: None,
            },
            Column {
                name: id("active"),
                ty: ColumnType::Boolean,
                nullable: false,
                default: None,
                identity: None,
                generated: None,
                collation: None,
                storage: None,
                compression: None,
                comment: None,
            },
        ],
        constraints: vec![Constraint {
            qname: qn("app", "users_pkey"),
            kind: ConstraintKind::PrimaryKey {
                columns: vec![id("id")],
                include: vec![],
            },
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }],
        partition_by: None,
        partition_of: None,
        comment: None,
        owner: None,
        grants: vec![],
        rls_enabled: false,
        rls_forced: false,
        policies: vec![],
        storage: pgevolve_core::ir::reloptions::TableStorageOptions::default(),
    });

    // A view that selects from app.users.
    // The body_dependencies include an AstExtracted edge view → table.
    cat.views.push(View {
        qname: qn("app", "active_users"),
        columns: vec![ViewColumn {
            name: id("id"),
            column_type: ColumnType::BigInt,
            comment: None,
        }],
        body_canonical: body("SELECT id FROM app.users WHERE active"),
        body_dependencies: vec![DepEdge {
            from: NodeId::View(qn("app", "active_users")),
            to: NodeId::Table(qn("app", "users")),
            source: DepSource::AstExtracted,
        }],
        security_barrier: None,
        security_invoker: None,
        check_option: None,
        comment: None,
        raw_body: String::new(),
        owner: None,
        grants: vec![],
    });

    cat
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shadow_validate_views_happy_path() {
    if !docker_available() {
        eprintln!("Docker unavailable; skipping shadow_validate_views_happy_path");
        return;
    }

    let version = default_pg_version();
    let backend = make_shadow_backend(version);
    let source = small_catalog_with_view();

    let report = cross_check(backend.as_ref(), &source, pg_major(version), false)
        .await
        .expect("cross_check must succeed on happy-path catalog");

    assert_eq!(
        report.canonical_mismatches.len(),
        0,
        "expected 0 canonical mismatches: {:?}",
        report.canonical_mismatches
    );
    assert_eq!(
        report.extra_ast_edges.len(),
        0,
        "expected 0 extra AST edges: {:?}",
        report.extra_ast_edges
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shadow_validate_strict_passes_on_clean_catalog() {
    if !docker_available() {
        eprintln!("Docker unavailable; skipping shadow_validate_strict_passes_on_clean_catalog");
        return;
    }

    let version = default_pg_version();
    let backend = make_shadow_backend(version);
    let source = small_catalog_with_view();

    // strict=true should not bail when everything matches.
    let result = cross_check(backend.as_ref(), &source, pg_major(version), true).await;
    assert!(
        result.is_ok(),
        "strict cross_check should succeed on clean catalog: {result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shadow_validate_strict_fails_on_missing_ast_edge() {
    if !docker_available() {
        eprintln!("Docker unavailable; skipping shadow_validate_strict_fails_on_missing_ast_edge");
        return;
    }

    let version = default_pg_version();
    let backend = make_shadow_backend(version);

    // Build a view that actually depends on app.users in SQL, but declares
    // a fake dependency so that pg_depend sees the real dep while the AST set
    // claims a non-existent one → extra_ast_edges is non-empty.
    let mut cat = Catalog::empty();
    cat.schemas.push(Schema::new(id("app")));
    cat.tables.push(Table {
        qname: qn("app", "users"),
        columns: vec![Column {
            name: id("id"),
            ty: ColumnType::BigInt,
            nullable: false,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        }],
        constraints: vec![],
        partition_by: None,
        partition_of: None,
        comment: None,
        owner: None,
        grants: vec![],
        rls_enabled: false,
        rls_forced: false,
        policies: vec![],
        storage: pgevolve_core::ir::reloptions::TableStorageOptions::default(),
    });
    // View body references app.users but body_dependencies claims a
    // non-existent table — this produces extra_ast_edges.
    cat.views.push(View {
        qname: qn("app", "v"),
        columns: vec![],
        body_canonical: body("SELECT id FROM app.users"),
        body_dependencies: vec![DepEdge {
            from: NodeId::View(qn("app", "v")),
            to: NodeId::Table(qn("app", "no_such_table")), // intentionally wrong
            source: DepSource::AstExtracted,
        }],
        security_barrier: None,
        security_invoker: None,
        check_option: None,
        comment: None,
        raw_body: String::new(),
        owner: None,
        grants: vec![],
    });

    let result = cross_check(backend.as_ref(), &cat, pg_major(version), true).await;
    assert!(
        result.is_err(),
        "strict cross_check should fail when extra/missing AST edges exist"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("shadow-strict"),
        "error message should mention shadow-strict: {msg}"
    );
}
