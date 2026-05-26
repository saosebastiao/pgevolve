//! `pgevolve diff` — print the change set from `source` against a live DB.

use anyhow::Result;

use pgevolve_core::catalog::CatalogFilter;
use pgevolve_core::catalog::read_catalog;
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
    let (target, drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await
        .map_err(|e| anyhow::anyhow!("join error: {e}"))??;

    let changes = diff(&target, &source, &drift);
    match format {
        OutputFormat::Human => print_human(&changes),
        OutputFormat::Json => print_json(&changes)?,
        OutputFormat::Sql => print_sql(&changes),
    }

    if args.shadow_validate {
        let shadow_cfg = cfg.shadow.as_ref().ok_or_else(|| {
            anyhow::anyhow!("--shadow-validate requires a [shadow] section in pgevolve.toml")
        })?;
        let backend = crate::shadow::resolve(shadow_cfg)?;
        // v0.1: default to PG 17. v0.2 will thread the real major from the
        // live DB connection or from [shadow].postgres_version.
        let major = shadow_cfg
            .postgres_version
            .as_deref()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(17);
        let report = crate::shadow::validate::cross_check(
            backend.as_ref(),
            &source,
            major,
            args.shadow_strict,
        )
        .await?;
        let mismatch_count = report.canonical_mismatches.len()
            + report.extra_ast_edges.len()
            + report.missing_ast_edges.len();
        if mismatch_count > 0 {
            for m in &report.canonical_mismatches {
                eprintln!(
                    "  canonical mismatch {}: source={:?} catalog={:?}",
                    m.view_qname, m.source_canonical, m.catalog_canonical
                );
            }
            for e in &report.extra_ast_edges {
                eprintln!("  extra AST edge {}: {}", e.view_qname, e.dep_node);
            }
            for m in &report.missing_ast_edges {
                eprintln!(
                    "  missing AST edge {}: {}.{}",
                    m.view_qname, m.ref_schema, m.ref_name
                );
            }
            if args.shadow_strict {
                anyhow::bail!("shadow-validate --strict: {mismatch_count} mismatch(es)");
            }
        }
        let n_edges = report.structural_edges_checked;
        eprintln!(
            "shadow-validate: ok ({n_edges} structural edge(s), {mismatch_count} canonical mismatch(es))"
        );
    }

    // Spec §10.1: `diff` is informational — always exit 0 regardless of change count.
    Ok(0)
}

#[allow(clippy::too_many_lines)]
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
            pgevolve_core::diff::change::Change::ValidateConstraint { table, constraint } => {
                println!("      validate constraint {constraint} on {table}");
            }
            pgevolve_core::diff::change::Change::RecreateIndex { qname } => {
                println!("      recreate invalid index {qname}");
            }
            pgevolve_core::diff::change::Change::View(vc) => {
                use pgevolve_core::diff::change::ViewChange;
                match vc {
                    ViewChange::Create(v) => println!("      create view {}", v.qname),
                    ViewChange::Drop(q) => println!("      drop view {q}"),
                    ViewChange::ReplaceBody {
                        source, compatible, ..
                    } => {
                        let compat = if *compatible {
                            "compatible"
                        } else {
                            "incompatible"
                        };
                        println!("      replace view body {} ({compat})", source.qname);
                    }
                    ViewChange::SetReloption { qname, .. } => {
                        println!("      set view reloption {qname}");
                    }
                    ViewChange::SetComment { qname, .. } => {
                        println!("      set view comment {qname}");
                    }
                    ViewChange::SetColumnComment { qname, column, .. } => {
                        println!("      set column comment {qname}.{column}");
                    }
                }
            }
            pgevolve_core::diff::change::Change::Mv(mc) => {
                use pgevolve_core::diff::change::MvChange;
                match mc {
                    MvChange::Create(mv) => println!("      create materialized view {}", mv.qname),
                    MvChange::Drop(q) => println!("      drop materialized view {q}"),
                    MvChange::ReplaceBody { source, .. } => {
                        println!("      replace mv body {}", source.qname);
                    }
                    MvChange::SetComment { qname, .. } => {
                        println!("      set mv comment {qname}");
                    }
                    MvChange::SetColumnComment { qname, column, .. } => {
                        println!("      set mv column comment {qname}.{column}");
                    }
                }
            }
            pgevolve_core::diff::change::Change::UserType(utc) => {
                use pgevolve_core::diff::change::UserTypeChange;
                match utc {
                    UserTypeChange::Create(t) => println!("      create type {}", t.qname),
                    UserTypeChange::Drop(q) => println!("      drop type {q}"),
                    UserTypeChange::EnumAddValue { qname, value, .. } => {
                        println!("      enum {qname}: add value {value}");
                    }
                    UserTypeChange::EnumRenameValue { qname, from, to } => {
                        println!("      enum {qname}: rename {from} -> {to}");
                    }
                    UserTypeChange::DomainAddCheck { qname, .. } => {
                        println!("      domain {qname}: add check");
                    }
                    UserTypeChange::DomainDropCheck { qname, name } => {
                        println!("      domain {qname}: drop check {name}");
                    }
                    UserTypeChange::DomainSetDefault { qname, .. } => {
                        println!("      domain {qname}: set default");
                    }
                    UserTypeChange::DomainSetNotNull { qname, not_null } => {
                        println!("      domain {qname}: set not null = {not_null}");
                    }
                    UserTypeChange::CompositeAddAttribute { qname, attribute } => {
                        println!("      composite {qname}: add attribute {}", attribute.name);
                    }
                    UserTypeChange::CompositeDropAttribute { qname, name } => {
                        println!("      composite {qname}: drop attribute {name}");
                    }
                    UserTypeChange::CompositeAlterAttributeType {
                        qname, attribute, ..
                    } => {
                        println!("      composite {qname}: alter attribute type {attribute}");
                    }
                    UserTypeChange::SetComment { qname, .. } => {
                        println!("      set type comment {qname}");
                    }
                    UserTypeChange::ReplaceWithCascade { source, .. } => {
                        println!("      replace type {} with cascade", source.qname);
                    }
                }
            }
            pgevolve_core::diff::change::Change::Function(fc) => {
                use pgevolve_core::diff::change::FunctionChange;
                match fc {
                    FunctionChange::Create(f) => {
                        println!("      function {}: create", f.qname);
                    }
                    FunctionChange::Drop { qname, args } => {
                        let arg_sig = args
                            .types
                            .iter()
                            .map(pgevolve_core::ir::column_type::ColumnType::render_sql)
                            .collect::<Vec<_>>()
                            .join(", ");
                        println!("      function {qname}({arg_sig}): drop");
                    }
                    FunctionChange::CreateOrReplace(f) => {
                        println!("      function {}: create or replace", f.qname);
                    }
                    FunctionChange::ReplaceWithCascade { source, .. } => {
                        println!("      function {}: drop cascade + recreate", source.qname);
                    }
                    FunctionChange::SetComment { qname, comment, .. } => {
                        if comment.is_some() {
                            println!("      function {qname}: set comment");
                        } else {
                            println!("      function {qname}: clear comment");
                        }
                    }
                }
            }
            pgevolve_core::diff::change::Change::Procedure(pc) => {
                use pgevolve_core::diff::change::ProcedureChange;
                match pc {
                    ProcedureChange::Create(p) => {
                        println!("      procedure {}: create", p.qname);
                    }
                    ProcedureChange::Drop(q) => {
                        println!("      procedure {q}: drop");
                    }
                    ProcedureChange::CreateOrReplace(p) => {
                        println!("      procedure {}: create or replace", p.qname);
                    }
                    ProcedureChange::SetComment { qname, comment } => {
                        if comment.is_some() {
                            println!("      procedure {qname}: set comment");
                        } else {
                            println!("      procedure {qname}: clear comment");
                        }
                    }
                }
            }
            pgevolve_core::diff::change::Change::Extension(ec) => {
                use pgevolve_core::diff::change::ExtensionChange;
                match ec {
                    ExtensionChange::Create(e) => {
                        println!("      create extension {}", e.name);
                    }
                    ExtensionChange::Drop(n) => {
                        println!("      drop extension {n}");
                    }
                    ExtensionChange::AlterUpdate { name, to_version } => {
                        println!("      alter extension {name} update to {to_version}");
                    }
                    ExtensionChange::ReplaceWithCascade(e) => {
                        println!("      replace extension {} with cascade", e.name);
                    }
                    ExtensionChange::CommentOn { name, comment } => {
                        if comment.is_some() {
                            println!("      extension {name}: set comment");
                        } else {
                            println!("      extension {name}: clear comment");
                        }
                    }
                }
            }
            pgevolve_core::diff::change::Change::Trigger(tc) => {
                use pgevolve_core::diff::change::TriggerChange;
                match tc {
                    TriggerChange::Create(t) => {
                        println!("      create trigger {} on {}", t.qname, t.table);
                    }
                    TriggerChange::Drop { qname, table } => {
                        println!("      drop trigger {qname} on {table}");
                    }
                    TriggerChange::Replace(t) => {
                        println!(
                            "      replace trigger {} on {} (drop + recreate)",
                            t.qname, t.table
                        );
                    }
                    TriggerChange::CommentOn {
                        qname,
                        table,
                        comment,
                    } => {
                        if comment.is_some() {
                            println!("      trigger {qname} on {table}: set comment");
                        } else {
                            println!("      trigger {qname} on {table}: clear comment");
                        }
                    }
                }
            }
            pgevolve_core::diff::change::Change::Table(tc) => {
                use pgevolve_core::diff::change::TableChange;
                match tc {
                    TableChange::AttachPartition { parent, child, .. } => {
                        println!("      attach partition {child} to {parent}");
                    }
                    TableChange::DetachPartition { parent, child } => {
                        println!("      detach partition {child} from {parent}");
                    }
                }
            }
            pgevolve_core::diff::change::Change::GrantObjectPrivilege {
                qname,
                kind,
                signature,
                grant,
            } => {
                println!(
                    "      grant {} on {:?} {qname}{signature} to {:?}",
                    grant.privilege.sql_keyword(),
                    kind,
                    grant.grantee
                );
            }
            pgevolve_core::diff::change::Change::RevokeObjectPrivilege {
                qname,
                kind,
                signature,
                grant,
            } => {
                println!(
                    "      revoke {} on {:?} {qname}{signature} from {:?}",
                    grant.privilege.sql_keyword(),
                    kind,
                    grant.grantee
                );
            }
            pgevolve_core::diff::change::Change::GrantColumnPrivilege { qname, grant } => {
                println!(
                    "      grant column privilege on {qname} to {:?}",
                    grant.grantee
                );
            }
            pgevolve_core::diff::change::Change::RevokeColumnPrivilege { qname, grant } => {
                println!(
                    "      revoke column privilege on {qname} from {:?}",
                    grant.grantee
                );
            }
            pgevolve_core::diff::change::Change::AlterObjectOwner(op) => {
                println!(
                    "      alter owner of {:?} {} from {} to {}",
                    op.kind, op.qname, op.from, op.to
                );
            }
            pgevolve_core::diff::change::Change::AlterDefaultPrivileges {
                target_role,
                schema,
                object_type,
                is_grant,
                grant,
            } => {
                let action = if *is_grant { "grant" } else { "revoke" };
                let in_schema = schema
                    .as_ref()
                    .map_or_else(String::new, |s| format!(" in schema {s}"));
                println!(
                    "      alter default privileges for role {target_role}{in_schema}: {action} {:?} {:?} to {:?}",
                    object_type, grant.privilege, grant.grantee
                );
            }
            // Stage 6 will wire these into proper display.
            pgevolve_core::diff::change::Change::CreatePolicy { table, policy } => {
                println!("      create policy {} on {table}", policy.name);
            }
            pgevolve_core::diff::change::Change::DropPolicy { table, name } => {
                println!("      drop policy {name} on {table}");
            }
            pgevolve_core::diff::change::Change::AlterPolicy { table, policy } => {
                println!("      alter policy {} on {table}", policy.name);
            }
            pgevolve_core::diff::change::Change::SetTableRowSecurity { qname, enable } => {
                let verb = if *enable { "enable" } else { "disable" };
                println!("      {verb} row level security on {qname}");
            }
            pgevolve_core::diff::change::Change::SetTableForceRowSecurity { qname, force } => {
                let verb = if *force { "force" } else { "no force" };
                println!("      {verb} row level security on {qname}");
            }
            pgevolve_core::diff::change::Change::SetTableStorage { qname, .. } => {
                println!("      ~ ALTER TABLE {qname} SET (...)");
            }
            pgevolve_core::diff::change::Change::SetIndexStorage { qname, .. } => {
                println!("      ~ ALTER INDEX {qname} SET (...)");
            }
            pgevolve_core::diff::change::Change::SetMaterializedViewStorage { qname, .. } => {
                println!("      ~ ALTER MATERIALIZED VIEW {qname} SET (...)");
            }
            pgevolve_core::diff::change::Change::UnsupportedDiff { reason } => {
                println!("      unsupported diff: {reason}");
            }
            // Publication changes: real display lands in Stage 8.
            pgevolve_core::diff::change::Change::CreatePublication(p) => {
                println!("      + CREATE PUBLICATION {} (stub)", p.name);
            }
            pgevolve_core::diff::change::Change::DropPublication { name } => {
                println!("      - DROP PUBLICATION {name} (stub)");
            }
            pgevolve_core::diff::change::Change::ReplacePublication { to, .. } => {
                println!("      ~ REPLACE PUBLICATION {} (stub)", to.name);
            }
            pgevolve_core::diff::change::Change::AlterPublicationAddTable {
                publication,
                table,
            } => {
                println!(
                    "      ~ ALTER PUBLICATION {publication} ADD TABLE {} (stub)",
                    table.qname
                );
            }
            pgevolve_core::diff::change::Change::AlterPublicationDropTable {
                publication,
                qname,
            } => {
                println!("      ~ ALTER PUBLICATION {publication} DROP TABLE {qname} (stub)");
            }
            pgevolve_core::diff::change::Change::AlterPublicationSetTable {
                publication,
                table,
            } => {
                println!(
                    "      ~ ALTER PUBLICATION {publication} SET TABLE {} (stub)",
                    table.qname
                );
            }
            pgevolve_core::diff::change::Change::AlterPublicationAddSchema {
                publication,
                schema,
            } => {
                println!(
                    "      ~ ALTER PUBLICATION {publication} ADD TABLES IN SCHEMA {schema} (stub)"
                );
            }
            pgevolve_core::diff::change::Change::AlterPublicationDropSchema {
                publication,
                schema,
            } => {
                println!(
                    "      ~ ALTER PUBLICATION {publication} DROP TABLES IN SCHEMA {schema} (stub)"
                );
            }
            pgevolve_core::diff::change::Change::AlterPublicationSetPublish {
                publication, ..
            } => {
                println!("      ~ ALTER PUBLICATION {publication} SET (publish=...) (stub)");
            }
            pgevolve_core::diff::change::Change::AlterPublicationSetViaRoot {
                publication,
                value,
            } => {
                println!(
                    "      ~ ALTER PUBLICATION {publication} SET (publish_via_partition_root={value}) (stub)"
                );
            }
            pgevolve_core::diff::change::Change::CommentOnPublication { name, .. } => {
                println!("      ~ COMMENT ON PUBLICATION {name} (stub)");
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

#[allow(clippy::too_many_lines)]
const fn change_kind_name(c: &pgevolve_core::diff::change::Change) -> &'static str {
    use pgevolve_core::diff::change::{Change, MvChange, ViewChange};
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
        Change::ValidateConstraint { .. } => "ValidateConstraint",
        Change::RecreateIndex { .. } => "RecreateIndex",
        Change::View(ViewChange::Create(_)) => "CreateView",
        Change::View(ViewChange::Drop(_)) => "DropView",
        Change::View(ViewChange::ReplaceBody { .. }) => "ReplaceViewBody",
        Change::View(ViewChange::SetReloption { .. }) => "SetViewReloption",
        Change::View(ViewChange::SetComment { .. }) => "SetViewComment",
        Change::View(ViewChange::SetColumnComment { .. }) => "SetViewColumnComment",
        Change::Mv(MvChange::Create(_)) => "CreateMv",
        Change::Mv(MvChange::Drop(_)) => "DropMv",
        Change::Mv(MvChange::ReplaceBody { .. }) => "ReplaceMvBody",
        Change::Mv(MvChange::SetComment { .. }) => "SetMvComment",
        Change::Mv(MvChange::SetColumnComment { .. }) => "SetMvColumnComment",
        Change::UserType(pgevolve_core::diff::change::UserTypeChange::Create(_)) => "CreateType",
        Change::UserType(pgevolve_core::diff::change::UserTypeChange::Drop(_)) => "DropType",
        Change::UserType(pgevolve_core::diff::change::UserTypeChange::EnumAddValue { .. }) => {
            "EnumAddValue"
        }
        Change::UserType(pgevolve_core::diff::change::UserTypeChange::EnumRenameValue {
            ..
        }) => "EnumRenameValue",
        Change::UserType(pgevolve_core::diff::change::UserTypeChange::DomainAddCheck {
            ..
        }) => "DomainAddCheck",
        Change::UserType(pgevolve_core::diff::change::UserTypeChange::DomainDropCheck {
            ..
        }) => "DomainDropCheck",
        Change::UserType(pgevolve_core::diff::change::UserTypeChange::DomainSetDefault {
            ..
        }) => "DomainSetDefault",
        Change::UserType(pgevolve_core::diff::change::UserTypeChange::DomainSetNotNull {
            ..
        }) => "DomainSetNotNull",
        Change::UserType(pgevolve_core::diff::change::UserTypeChange::CompositeAddAttribute {
            ..
        }) => "CompositeAddAttribute",
        Change::UserType(pgevolve_core::diff::change::UserTypeChange::CompositeDropAttribute {
            ..
        }) => "CompositeDropAttribute",
        Change::UserType(
            pgevolve_core::diff::change::UserTypeChange::CompositeAlterAttributeType { .. },
        ) => "CompositeAlterAttributeType",
        Change::UserType(pgevolve_core::diff::change::UserTypeChange::SetComment { .. }) => {
            "SetTypeComment"
        }
        Change::UserType(pgevolve_core::diff::change::UserTypeChange::ReplaceWithCascade {
            ..
        }) => "ReplaceTypeWithCascade",
        Change::Function(pgevolve_core::diff::change::FunctionChange::Create(_)) => {
            "CreateFunction"
        }
        Change::Function(pgevolve_core::diff::change::FunctionChange::Drop { .. }) => {
            "DropFunction"
        }
        Change::Function(pgevolve_core::diff::change::FunctionChange::CreateOrReplace(_)) => {
            "CreateOrReplaceFunction"
        }
        Change::Function(pgevolve_core::diff::change::FunctionChange::ReplaceWithCascade {
            ..
        }) => "ReplaceFunctionWithCascade",
        Change::Function(pgevolve_core::diff::change::FunctionChange::SetComment { .. }) => {
            "SetFunctionComment"
        }
        Change::Procedure(pgevolve_core::diff::change::ProcedureChange::Create(_)) => {
            "CreateProcedure"
        }
        Change::Procedure(pgevolve_core::diff::change::ProcedureChange::Drop(_)) => "DropProcedure",
        Change::Procedure(pgevolve_core::diff::change::ProcedureChange::CreateOrReplace(_)) => {
            "CreateOrReplaceProcedure"
        }
        Change::Procedure(pgevolve_core::diff::change::ProcedureChange::SetComment { .. }) => {
            "SetProcedureComment"
        }
        Change::Extension(pgevolve_core::diff::change::ExtensionChange::Create(_)) => {
            "CreateExtension"
        }
        Change::Extension(pgevolve_core::diff::change::ExtensionChange::Drop(_)) => "DropExtension",
        Change::Extension(pgevolve_core::diff::change::ExtensionChange::AlterUpdate { .. }) => {
            "AlterExtensionUpdate"
        }
        Change::Extension(pgevolve_core::diff::change::ExtensionChange::ReplaceWithCascade(_)) => {
            "ReplaceExtensionWithCascade"
        }
        Change::Extension(pgevolve_core::diff::change::ExtensionChange::CommentOn { .. }) => {
            "CommentOnExtension"
        }
        Change::Trigger(pgevolve_core::diff::change::TriggerChange::Create(_)) => "CreateTrigger",
        Change::Trigger(pgevolve_core::diff::change::TriggerChange::Drop { .. }) => "DropTrigger",
        Change::Trigger(pgevolve_core::diff::change::TriggerChange::Replace(_)) => "ReplaceTrigger",
        Change::Trigger(pgevolve_core::diff::change::TriggerChange::CommentOn { .. }) => {
            "CommentOnTrigger"
        }
        Change::Table(pgevolve_core::diff::change::TableChange::AttachPartition { .. }) => {
            "AttachPartition"
        }
        Change::Table(pgevolve_core::diff::change::TableChange::DetachPartition { .. }) => {
            "DetachPartition"
        }
        Change::GrantObjectPrivilege { .. } => "GrantObjectPrivilege",
        Change::RevokeObjectPrivilege { .. } => "RevokeObjectPrivilege",
        Change::GrantColumnPrivilege { .. } => "GrantColumnPrivilege",
        Change::RevokeColumnPrivilege { .. } => "RevokeColumnPrivilege",
        Change::AlterObjectOwner(_) => "AlterObjectOwner",
        Change::AlterDefaultPrivileges { .. } => "AlterDefaultPrivileges",
        Change::CreatePolicy { .. } => "CreatePolicy",
        Change::DropPolicy { .. } => "DropPolicy",
        Change::AlterPolicy { .. } => "AlterPolicy",
        Change::SetTableRowSecurity { .. } => "SetTableRowSecurity",
        Change::SetTableForceRowSecurity { .. } => "SetTableForceRowSecurity",
        Change::SetTableStorage { .. } => "SetTableStorage",
        Change::SetIndexStorage { .. } => "SetIndexStorage",
        Change::SetMaterializedViewStorage { .. } => "SetMaterializedViewStorage",
        Change::UnsupportedDiff { .. } => "UnsupportedDiff",
        Change::CreatePublication(_) => "CreatePublication",
        Change::DropPublication { .. } => "DropPublication",
        Change::ReplacePublication { .. } => "ReplacePublication",
        Change::AlterPublicationAddTable { .. } => "AlterPublicationAddTable",
        Change::AlterPublicationDropTable { .. } => "AlterPublicationDropTable",
        Change::AlterPublicationSetTable { .. } => "AlterPublicationSetTable",
        Change::AlterPublicationAddSchema { .. } => "AlterPublicationAddSchema",
        Change::AlterPublicationDropSchema { .. } => "AlterPublicationDropSchema",
        Change::AlterPublicationSetPublish { .. } => "AlterPublicationSetPublish",
        Change::AlterPublicationSetViaRoot { .. } => "AlterPublicationSetViaRoot",
        Change::CommentOnPublication { .. } => "CommentOnPublication",
    }
}
