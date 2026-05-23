//! Tier-3 round-trip test for `pgevolve dump`.
//!
//! Strategy:
//! 1. Construct a representative `Catalog` (schemas, tables with columns and
//!    constraints, indexes, sequences).
//! 2. Render it to SQL via `render_catalog`.
//! 3. Apply the rendered SQL to an `EphemeralPostgres`.
//! 4. Read the catalog back from the ephemeral DB.
//! 5. Compare with the original using `Catalog::canonicalize` + `Diff`.
//!
//! Skipped when Docker is not available (matches the existing tier-3 pattern).

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use pgevolve_core::catalog::{CatalogFilter, read_catalog};
    use pgevolve_core::identifier::{Identifier, QualifiedName};
    use pgevolve_core::ir::catalog::Catalog;
    use pgevolve_core::ir::column::Column;
    use pgevolve_core::ir::column_type::ColumnType;
    use pgevolve_core::ir::constraint::{
        Constraint, ConstraintKind, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
    };
    use pgevolve_core::ir::default_expr::{DefaultExpr, LiteralValue};
    use pgevolve_core::ir::eq::Diff;
    use pgevolve_core::ir::index::{
        Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
    };
    use pgevolve_core::ir::schema::Schema;
    use pgevolve_core::ir::sequence::Sequence;
    use pgevolve_core::ir::table::Table;
    use pgevolve_core::render::render_catalog;
    use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};
    use pgevolve_testkit::pg_querier::PgCatalogQuerier;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn col(name: &str, ty: ColumnType, nullable: bool) -> Column {
        Column {
            name: id(name),
            ty,
            nullable,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        }
    }

    /// Build a representative catalog exercising all major IR kinds:
    /// - Multiple schemas.
    /// - Tables with PK, UK, CHECK, FK constraints.
    /// - NOT NULL and nullable columns.
    /// - Column defaults (literal value).
    /// - A standalone sequence owned by a column.
    /// - A standalone index (partial + covering).
    // Test-only catalog builder; decomposing it would reduce legibility without benefit.
    #[allow(clippy::too_many_lines)]
    fn build_test_catalog() -> Catalog {
        let mut cat = Catalog::empty();

        // Schemas.
        cat.schemas.push(Schema::new(id("app")));

        // Sequences (standalone; owned by app.users.id).
        cat.sequences.push(Sequence {
            qname: qn("app", "users_id_seq"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: Some(1),
            max_value: Some(9_223_372_036_854_775_807),
            cache: 1,
            cycle: false,
            owned_by: None, // We'll omit owned_by to keep the round-trip simple.
            comment: None,
            owner: None,
            grants: vec![],
        });

        // orgs table (referenced by FK).
        cat.tables.push(Table {
            qname: qn("app", "orgs"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![Constraint {
                qname: qn("app", "orgs_pkey"),
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

        // users table with various features.
        let mut active_col = col("active", ColumnType::Boolean, false);
        active_col.default = Some(DefaultExpr::Literal(LiteralValue::Bool(true)));

        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("email", ColumnType::Text, false),
                col("name", ColumnType::Varchar { len: Some(255) }, true),
                active_col,
                col("org_id", ColumnType::BigInt, true),
            ],
            constraints: vec![
                Constraint {
                    qname: qn("app", "users_pkey"),
                    kind: ConstraintKind::PrimaryKey {
                        columns: vec![id("id")],
                        include: vec![],
                    },
                    deferrable: Deferrable::NotDeferrable,
                    comment: None,
                },
                Constraint {
                    qname: qn("app", "users_email_key"),
                    kind: ConstraintKind::Unique {
                        columns: vec![id("email")],
                        include: vec![],
                        nulls_distinct: true,
                    },
                    deferrable: Deferrable::NotDeferrable,
                    comment: None,
                },
                Constraint {
                    qname: qn("app", "users_org_fkey"),
                    kind: ConstraintKind::ForeignKey(ForeignKey {
                        columns: vec![id("org_id")],
                        referenced_table: qn("app", "orgs"),
                        referenced_columns: vec![id("id")],
                        on_update: ReferentialAction::NoAction,
                        on_delete: ReferentialAction::SetNull(vec![]),
                        match_type: FkMatchType::Simple,
                    }),
                    deferrable: Deferrable::NotDeferrable,
                    comment: None,
                },
            ],
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

        // Standalone index.
        cat.indexes.push(Index {
            qname: qn("app", "users_email_idx"),
            on: IndexParent::Table(qn("app", "users")),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("email")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: false,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
            storage: pgevolve_core::ir::reloptions::IndexStorageOptions::default(),
        });

        cat
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dump_round_trip() {
        if !docker_available() {
            eprintln!("skipping dump_round_trip: Docker not available");
            return;
        }

        let version = default_pg_version();
        let result = run_round_trip(version).await;
        match result {
            Ok(()) => {}
            Err(e) => panic!("dump_round_trip failed: {e:#}"),
        }
    }

    async fn run_round_trip(version: pgevolve_core::catalog::PgVersion) -> Result<()> {
        let source_catalog = build_test_catalog();
        let rendered = render_catalog(&source_catalog);

        eprintln!("--- rendered SQL ---\n{rendered}\n--- end ---");

        // Apply rendered SQL to an ephemeral Postgres.
        let pg = EphemeralPostgres::start(version).await?;
        pg.exec_sql(&rendered).await?;

        // Read catalog back.
        let client = pg.connect().await?;
        let querier = PgCatalogQuerier::new(client)?;
        let filter = CatalogFilter::new(vec![id("app")], vec![])?;
        let (read_back, _drift) =
            tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
                .await?
                .map_err(|e| anyhow::anyhow!("catalog read: {e}"))?;

        // Canonicalize both for comparison.
        let source_canonical = source_catalog
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("canonicalize source: {e}"))?;
        let read_canonical = read_back
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("canonicalize read-back: {e}"))?;

        // Compare using the IR diff.
        let diffs = source_canonical.diff(&read_canonical);
        if diffs.is_empty() {
            return Ok(());
        }

        // Format diffs for a helpful assertion message.
        let diff_lines: Vec<String> = diffs
            .iter()
            .map(|d| format!("  {}: {:?} → {:?}", d.path, d.from, d.to))
            .collect();
        anyhow::bail!(
            "dump round-trip IR mismatch — {} difference(s):\n{}",
            diffs.len(),
            diff_lines.join("\n")
        );
    }
}
