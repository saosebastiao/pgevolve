//! `pgevolve diff` — print the change set from `source` against a live DB.

use anyhow::Result;

use pgevolve_core::catalog::read_catalog;
use pgevolve_core::catalog::CatalogFilter;
use pgevolve_core::diff::diff;

use crate::cli::{DiffArgs, OutputFormat};
use crate::config::PgevolveConfig;
use crate::connection::{connect, resolve_db};
use crate::pg_querier::PgCatalogQuerier;

/// Run `pgevolve diff`.
pub async fn run(args: DiffArgs, cfg: &PgevolveConfig, format: OutputFormat) -> Result<i32> {
    let opts = resolve_db(cfg, &args.db, args.url.as_deref())?;
    let source = pgevolve_core::parse::parse_directory(&cfg.project.schema_dir, &[])
        .map_err(|e| anyhow::anyhow!("parse error: {e}"))?;

    let client = connect(&opts).await?;
    let querier = PgCatalogQuerier::new(client)?;
    let filter = CatalogFilter::new(opts.managed_schemas.clone(), opts.ignore_objects.clone())?;
    let target = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await
        .map_err(|e| anyhow::anyhow!("join error: {e}"))??;

    let changes = diff(&target, &source);
    match format {
        OutputFormat::Human => print_human(&changes),
        OutputFormat::Json => print_json(&changes)?,
        OutputFormat::Sql => print_sql(&changes),
    }
    // Spec §10.1: `diff` is informational — always exit 0 regardless of change count.
    Ok(0)
}

fn print_human(changes: &pgevolve_core::diff::ChangeSet) {
    if changes.is_empty() {
        println!("No changes.");
        return;
    }
    println!("{} change(s):", changes.len());
    for e in changes.iter() {
        let kind = std::mem::discriminant(&e.change);
        let destructive = if e.destructiveness.requires_approval() {
            " [destructive]"
        } else {
            ""
        };
        println!("  - {kind:?}{destructive}");
        // Pretty per-variant detail. The diff Change enum lives in core; we
        // emit a one-line form keyed on the variant.
        match &e.change {
            pgevolve_core::diff::change::Change::CreateSchema(s) => {
                println!("      create schema {}", s.name);
            }
            pgevolve_core::diff::change::Change::DropSchema(n) => {
                println!("      drop schema {n}");
            }
            pgevolve_core::diff::change::Change::CreateTable(t) => {
                println!("      create table {}", t.qname);
            }
            pgevolve_core::diff::change::Change::DropTable { qname, .. } => {
                println!("      drop table {qname}");
            }
            pgevolve_core::diff::change::Change::AlterTable { qname, ops } => {
                println!("      alter table {} ({} op(s))", qname, ops.len());
            }
            pgevolve_core::diff::change::Change::CreateIndex(i) => {
                println!("      create index {}", i.qname);
            }
            pgevolve_core::diff::change::Change::DropIndex(q) => {
                println!("      drop index {q}");
            }
            pgevolve_core::diff::change::Change::ReplaceIndex { to, .. } => {
                println!("      replace index {}", to.qname);
            }
            pgevolve_core::diff::change::Change::CreateSequence(s) => {
                println!("      create sequence {}", s.qname);
            }
            pgevolve_core::diff::change::Change::DropSequence(q) => {
                println!("      drop sequence {q}");
            }
            pgevolve_core::diff::change::Change::AlterSequence { qname, ops } => {
                println!("      alter sequence {} ({} op(s))", qname, ops.len());
            }
            pgevolve_core::diff::change::Change::AlterSchema { name, .. } => {
                println!("      alter schema {name}");
            }
        }
    }
}

fn print_json(changes: &pgevolve_core::diff::ChangeSet) -> Result<()> {
    let s = serde_json::to_string_pretty(changes)?;
    println!("{s}");
    Ok(())
}

fn print_sql(changes: &pgevolve_core::diff::ChangeSet) {
    if changes.is_empty() {
        println!("-- no changes");
        return;
    }
    // Naive form per spec §10.1: emit SQL via the rewrite-pass renderer in
    // pgevolve_core (no online rewrites). Not a valid plan; meant for review.
    println!("-- pgevolve diff --format=sql (no online rewrites)");
    println!("-- run `pgevolve plan` for the real applyable form\n");
    for e in changes.iter() {
        println!("-- {} change", change_kind_name(&e.change));
        match &e.change {
            pgevolve_core::diff::change::Change::CreateSchema(s) => {
                println!("{}", pgevolve_core::plan::rewrite::sql::create_schema(s));
            }
            pgevolve_core::diff::change::Change::DropSchema(n) => {
                println!("{}", pgevolve_core::plan::rewrite::sql::drop_schema(n));
            }
            pgevolve_core::diff::change::Change::CreateTable(t) => {
                println!("{}", pgevolve_core::plan::rewrite::sql::create_table(t));
            }
            pgevolve_core::diff::change::Change::DropTable { qname, .. } => {
                println!("{}", pgevolve_core::plan::rewrite::sql::drop_table(qname));
            }
            pgevolve_core::diff::change::Change::CreateIndex(i) => {
                println!(
                    "{}",
                    pgevolve_core::plan::rewrite::sql::create_index(i, false)
                );
            }
            pgevolve_core::diff::change::Change::DropIndex(q) => {
                println!(
                    "{}",
                    pgevolve_core::plan::rewrite::sql::drop_index(q, false)
                );
            }
            pgevolve_core::diff::change::Change::CreateSequence(s) => {
                println!("{}", pgevolve_core::plan::rewrite::sql::create_sequence(s));
            }
            pgevolve_core::diff::change::Change::DropSequence(q) => {
                println!("{}", pgevolve_core::plan::rewrite::sql::drop_sequence(q));
            }
            other => println!("-- (alter/replace not rendered as standalone SQL): {other:?}"),
        }
        println!();
    }
}

const fn change_kind_name(c: &pgevolve_core::diff::change::Change) -> &'static str {
    use pgevolve_core::diff::change::Change;
    match c {
        Change::CreateSchema(_) => "CreateSchema",
        Change::DropSchema(_) => "DropSchema",
        Change::AlterSchema { .. } => "AlterSchema",
        Change::CreateTable(_) => "CreateTable",
        Change::DropTable { .. } => "DropTable",
        Change::AlterTable { .. } => "AlterTable",
        Change::CreateIndex(_) => "CreateIndex",
        Change::DropIndex(_) => "DropIndex",
        Change::ReplaceIndex { .. } => "ReplaceIndex",
        Change::CreateSequence(_) => "CreateSequence",
        Change::DropSequence(_) => "DropSequence",
        Change::AlterSequence { .. } => "AlterSequence",
    }
}
