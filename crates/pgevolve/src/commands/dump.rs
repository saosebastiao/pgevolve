//! `pgevolve dump` — introspect a live DB and write source SQL.
//!
//! ## What it does
//!
//! 1. Connects to the live database via `[environments.<env>]`.
//! 2. Runs the existing `read_catalog` → `(Catalog, DriftReport)` flow.
//! 3. Renders each Catalog object as a CREATE-style SQL statement via
//!    [`pgevolve_core::render::render_catalog`].
//! 4. Writes the rendered SQL to `<output_dir>/schema.sql`.
//!
//! ## v0.1.1 scope
//!
//! The entire catalog is written to a single `schema.sql` file.  A multi-file
//! layout following the project's `layout_profile` is deferred to v0.1.2+.
//!
//! ## Output and `parse_directory`
//!
//! The written SQL does NOT contain pgevolve source directives
//! (`-- pgevolve: intent = ...` etc.), so it cannot be consumed by
//! `pgevolve lint` or `parse_directory` without first adding directives.
//! After `pgevolve dump`, users should either:
//! - Run `pgevolve init` on the output directory and add directives manually.
//! - Use a future `pgevolve annotate` helper (not yet implemented).

use std::path::Path;

use anyhow::Result;
use pgevolve_core::catalog::CatalogFilter;
use pgevolve_core::catalog::read_catalog;
use pgevolve_core::render::render_catalog;

use crate::cli::DumpArgs;
use crate::config::PgevolveConfig;
use crate::connection::{connect, resolve_db};
use crate::pg_querier::PgCatalogQuerier;

/// Run `pgevolve dump`.
pub async fn run(args: DumpArgs, cfg: &PgevolveConfig) -> Result<i32> {
    let opts = resolve_db(cfg, &args.db, args.url.as_deref())?;
    let out_dir: &Path = &args.output;

    let client = connect(&opts).await?;
    let querier = PgCatalogQuerier::new(client)?;
    let filter = CatalogFilter::new(opts.managed_schemas.clone(), opts.ignore_objects.clone())?;

    let (catalog, drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await
        .map_err(|e| anyhow::anyhow!("join error: {e}"))??;

    // Warn about drift (NOT VALID constraints / INVALID indexes).
    if !drift.pending_validation.is_empty() {
        eprintln!(
            "warning: {} constraint(s) are NOT VALID — they will be rendered as validated in the dump",
            drift.pending_validation.len()
        );
        for (table, cname) in &drift.pending_validation {
            eprintln!("  - {table}.{}", cname.as_str());
        }
    }
    if !drift.invalid_indexes.is_empty() {
        eprintln!(
            "warning: {} index(es) are INVALID — they will be included in the dump but may need attention",
            drift.invalid_indexes.len()
        );
        for q in &drift.invalid_indexes {
            eprintln!("  - {q}");
        }
    }

    let rendered = render_catalog(&catalog);

    std::fs::create_dir_all(out_dir)
        .map_err(|e| anyhow::anyhow!("failed to create output dir {}: {e}", out_dir.display()))?;

    let out_path = out_dir.join("schema.sql");
    std::fs::write(&out_path, &rendered)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", out_path.display()))?;

    eprintln!("wrote {} bytes to {}", rendered.len(), out_path.display());
    eprintln!(
        "note: output does not include pgevolve directives; add them before running `pgevolve lint`"
    );

    Ok(0)
}
