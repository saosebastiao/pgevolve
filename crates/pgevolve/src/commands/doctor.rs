//! `pgevolve doctor` — project health check.
//!
//! Read-only command. Reports the v0.1 surface health of the connected
//! database: bootstrap status, catalog drift (NOT VALID constraints / INVALID
//! indexes), source-vs-catalog object counts, and recent failed applies (when
//! bootstrapped). Decision 21 in the arch spec.

use anyhow::Result;
use pgevolve_core::catalog::{read_catalog, CatalogFilter};

use crate::config::PgevolveConfig;
use crate::connection::{connect, resolve_db};
use crate::pg_querier::PgCatalogQuerier;

/// Run `pgevolve doctor`.
pub async fn run(cfg: &PgevolveConfig, env: &str, url: Option<&str>) -> Result<i32> {
    let opts = resolve_db(cfg, env, url)?;
    let client = connect(&opts).await?;

    println!("pgevolve doctor — env {env}");

    // --- Bootstrap status -----------------------------------------------
    // Probe for the `pgevolve` schema; avoid calling bootstrap_metadata which
    // would create it. A pg_namespace query is sufficient.
    let bootstrap_ok = client
        .query_opt(
            "SELECT 1 FROM pg_namespace WHERE nspname = 'pgevolve'",
            &[],
        )
        .await
        .is_ok_and(|r| r.is_some());

    if bootstrap_ok {
        println!("  bootstrap: ok");
    } else {
        println!("  bootstrap: NOT installed — run `pgevolve bootstrap --db {env}`");
    }

    // --- Catalog drift report -------------------------------------------
    let filter = CatalogFilter::new(opts.managed_schemas.clone(), opts.ignore_objects.clone())?;
    let querier = PgCatalogQuerier::new(client)?;
    report_drift(cfg, querier, filter).await;

    // --- Recent apply failures (only when bootstrapped) -----------------
    if bootstrap_ok {
        report_recent_failures(cfg, env, url).await;
    }

    Ok(0)
}

/// Print the catalog drift section of the health report.
async fn report_drift(cfg: &PgevolveConfig, querier: PgCatalogQuerier, filter: CatalogFilter) {
    let result = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await
        .map_err(|e| anyhow::anyhow!("join error: {e}"));

    match result {
        Ok(Ok((catalog, drift))) => {
            if !drift.pending_validation.is_empty() {
                println!(
                    "  warning: {} constraint(s) are NOT VALID:",
                    drift.pending_validation.len()
                );
                for (table, name) in &drift.pending_validation {
                    println!("    - {table}.{}", name.as_str());
                }
            }
            if !drift.invalid_indexes.is_empty() {
                println!(
                    "  warning: {} index(es) are INVALID:",
                    drift.invalid_indexes.len()
                );
                for q in &drift.invalid_indexes {
                    println!("    - {q}");
                }
            }
            if drift.pending_validation.is_empty() && drift.invalid_indexes.is_empty() {
                println!("  drift: none");
            }
            print_object_counts(cfg, &catalog);
        }
        Ok(Err(e)) => println!("  catalog read failed: {e}"),
        Err(e) => println!("  catalog read failed: {e}"),
    }
}

/// Print source-vs-catalog object count summary.
fn print_object_counts(cfg: &PgevolveConfig, catalog: &pgevolve_core::ir::catalog::Catalog) {
    match pgevolve_core::parse::parse_directory(&cfg.project.schema_dir, &[]) {
        Ok(source) => {
            println!(
                "  source:  {} schemas, {} tables, {} indexes, {} sequences",
                source.schemas.len(),
                source.tables.len(),
                source.indexes.len(),
                source.sequences.len()
            );
        }
        Err(e) => {
            println!("  source: could not parse schema dir — {e}");
        }
    }
    println!(
        "  catalog: {} schemas, {} tables, {} indexes, {} sequences",
        catalog.schemas.len(),
        catalog.tables.len(),
        catalog.indexes.len(),
        catalog.sequences.len()
    );
}

/// Print recent apply failures section.
async fn report_recent_failures(cfg: &PgevolveConfig, env: &str, url: Option<&str>) {
    // Re-connect because the original client was moved into spawn_blocking.
    let Ok(opts2) = resolve_db(cfg, env, url) else { return };
    let Ok(client2) = connect(&opts2).await else { return };
    let rows = client2
        .query(
            "SELECT apply_id::text, started_at::text \
             FROM pgevolve.apply_log WHERE status = 'failed' \
             ORDER BY started_at DESC LIMIT 5",
            &[],
        )
        .await;
    match rows {
        Ok(rows) if !rows.is_empty() => {
            println!("  warning: recent failed applies:");
            for r in &rows {
                let apply_id: String = r.get(0);
                let started: String = r.get(1);
                println!("    - {apply_id} at {started}");
            }
        }
        Ok(_) => println!("  recent applies: no failures"),
        Err(e) => {
            // apply_log may not exist on very old bootstrap versions; degrade
            // gracefully rather than aborting the whole health check.
            println!("  recent applies: could not query — {e}");
        }
    }
}
