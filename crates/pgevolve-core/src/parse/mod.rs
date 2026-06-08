//! Source-side parser: SQL bytes → IR.
//!
//! This module accepts a directory of `CREATE`-style DDL files and produces a
//! [`crate::ir::catalog::Catalog`]. Construction is I/O-free at the type level —
//! the only I/O is performed by [`parse_directory`] on behalf of callers.

pub mod ast_canon;
mod ast_resolution;
pub mod builder;
pub mod cluster;
pub mod directives;
pub mod error;
pub mod normalize_body;
pub mod normalize_expr;
pub mod statement;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

pub use directives::{FileDirectives, extract_file_directives};
pub use error::{ParseError, SourceLocation};
pub use statement::Statement;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::IrError;
use crate::ir::aggregate::Aggregate;
use crate::ir::cast::Cast;
use crate::ir::catalog::Catalog;
use crate::ir::event_trigger::EventTrigger;
use crate::ir::publication::Publication;
use crate::ir::statistic::Statistic;
use crate::ir::subscription::Subscription;

/// Parse every `*.sql` file under `root`, recursively, and produce a fully-
/// populated [`Catalog`]. Files matching any pattern in `ignores` are skipped.
///
/// Walking is deterministic (paths are sorted before processing), and each
/// statement classifies through the v0.1 whitelist; non-MVP DDL kinds raise
/// [`ParseError::UnsupportedObjectKind`] with a phase-2 message.
///
/// After all files have been processed, the catalog is canonicalized (vec-
/// sorted) and duplicate qnames raise [`ParseError::DuplicateObject`].
pub fn parse_directory(root: &Path, ignores: &[glob::Pattern]) -> Result<Catalog, ParseError> {
    parse_directory_with_locations(root, ignores).map(|(c, _)| c)
}

/// Like [`parse_directory`] but also returns the per-qname source-location
/// map built during parsing. Used by the lint engine (Phase 10) to know which
/// file each object was declared in.
///
/// The map keys are qname strings as rendered by `Display`: `"schema_name"`
/// for schemas, `"schema.name"` for tables / indexes / sequences.
pub fn parse_directory_with_locations(
    root: &Path,
    ignores: &[glob::Pattern],
) -> Result<(Catalog, HashMap<String, SourceLocation>), ParseError> {
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
        let entry = entry.map_err(|e| ParseError::Io {
            path: e.path().map(Path::to_path_buf).unwrap_or_default(),
            source: e
                .into_io_error()
                .unwrap_or_else(|| std::io::Error::other("walkdir error")),
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if path.extension().and_then(|e| e.to_str()) != Some("sql") {
            continue;
        }
        if ignores.iter().any(|pat| pat.matches_path(&path)) {
            continue;
        }
        files.push(path);
    }

    let mut catalog = Catalog::default();
    let mut locations: HashMap<String, SourceLocation> = HashMap::new();
    let mut pending_fks: Vec<builder::alter_table_stmt::PendingFk> = Vec::new();
    let mut pending_column_attrs: Vec<builder::alter_table_stmt::PendingColumnAttr> = Vec::new();
    let mut pending_owners: Vec<builder::alter_table_stmt::PendingOwner> = Vec::new();
    let mut pending_rls_toggles: Vec<builder::alter_table_stmt::PendingRlsToggle> = Vec::new();
    let mut pending_rel_options: Vec<builder::alter_table_stmt::PendingRelOptions> = Vec::new();
    let mut deferred_comments: Vec<(
        pg_query::protobuf::CommentStmt,
        SourceLocation,
        Option<crate::identifier::Identifier>,
    )> = Vec::new();
    // Publications are accumulated in insertion order (keyed by name) and
    // folded: CREATE ... WITH (...) then subsequent ALTER ... ADD/DROP/SET
    // all land in the same record.
    let mut publications: BTreeMap<Identifier, Publication> = BTreeMap::new();
    // Subscriptions: same fold-accumulate pattern as publications.
    let mut subscriptions: BTreeMap<Identifier, Subscription> = BTreeMap::new();
    // Statistics: fold CREATE + ALTER SET STATISTICS + COMMENT into one record per qname.
    let mut statistics: BTreeMap<QualifiedName, Statistic> = BTreeMap::new();
    // Event triggers: database-global, accumulated by name. Fold CREATE +
    // ALTER (ENABLE/DISABLE) + COMMENT + OWNER into one record per name.
    let mut event_triggers: BTreeMap<Identifier, EventTrigger> = BTreeMap::new();
    // Aggregates: identity is `(qname, arg_types)` (overloadable), so they are
    // accumulated in a Vec and ALTER OWNER / COMMENT are folded by matching that
    // identity. Duplicate identities are rejected at CREATE time.
    let mut aggregates: Vec<Aggregate> = Vec::new();
    // Casts: identity is `(source, target)`. Not overloadable. COMMENT ON CAST
    // is folded inline. Duplicate identities are rejected at CREATE time.
    let mut casts: Vec<Cast> = Vec::new();

    for path in files {
        let contents = std::fs::read_to_string(&path).map_err(|e| ParseError::Io {
            path: path.clone(),
            source: e,
        })?;
        process_file(
            &path,
            &contents,
            &mut catalog,
            &mut locations,
            &mut pending_fks,
            &mut pending_column_attrs,
            &mut pending_owners,
            &mut pending_rls_toggles,
            &mut pending_rel_options,
            &mut deferred_comments,
            &mut publications,
            &mut subscriptions,
            &mut statistics,
            &mut event_triggers,
            &mut aggregates,
            &mut casts,
        )?;
    }

    // Apply deferred comments (the underlying object may be defined in a later
    // file).
    for (stmt, location, default_schema) in deferred_comments {
        builder::comment_stmt::apply_comment(
            &stmt,
            &mut catalog,
            default_schema.as_ref(),
            &location,
        )?;
    }

    // Merge pending FKs and column-attribute updates from ALTER TABLE statements.
    apply_pending_fks(&mut catalog, pending_fks)?;
    apply_pending_column_attrs(&mut catalog, pending_column_attrs)?;
    apply_pending_owners(&mut catalog, pending_owners)?;
    apply_pending_rls_toggles(&mut catalog, pending_rls_toggles)?;
    apply_pending_rel_options(&mut catalog, pending_rel_options)?;

    // Flush the publications accumulator into the catalog.
    catalog.publications = publications.into_values().collect();

    // Flush the subscriptions accumulator into the catalog.
    catalog.subscriptions = subscriptions.into_values().collect();

    // Flush the statistics accumulator into the catalog.
    catalog.statistics = statistics.into_values().collect();

    // Flush the event-triggers accumulator into the catalog.
    catalog.event_triggers = event_triggers.into_values().collect();

    // Flush the aggregates accumulator into the catalog.
    catalog.aggregates = aggregates;

    // Flush the casts accumulator into the catalog.
    catalog.casts = casts;

    // AST resolution pass: validate that all structural references (FKs,
    // sequence defaults) resolve against the declared IR, before any DB touch.
    ast_resolution::resolve(&catalog, &locations).map_err(ParseError::AstResolution)?;

    // AST canonicalization pass: fill body_canonical, body_dependencies, and
    // (when needed) columns for all views and materialized views. Skipped when
    // the catalog has no views, so v0.1 fixtures pay no overhead.
    if !catalog.views.is_empty() || !catalog.materialized_views.is_empty() {
        ast_canon::canonicalize_view_bodies(&mut catalog).map_err(ParseError::AstCanon)?;
    }

    // MV index parent promotion: source-side `CREATE INDEX ON mv_name (...)` is
    // initially parsed as `IndexParent::Table` because the parser doesn't know
    // whether the relation is a table or an MV. Now that both the indexes and
    // the MVs are in the catalog, promote any `IndexParent::Table(q)` where `q`
    // is actually a materialized view.
    ast_canon::promote_mv_index_parents(&mut catalog);

    let canonical = catalog
        .canonicalize()
        .map_err(|e: IrError| translate_canonicalize_error(e, &locations))?;
    Ok((canonical, locations))
}

/// Apply a `COMMENT ON STATISTICS …` to the in-progress statistics accumulator.
///
/// Called inline during `process_file` (not deferred) because statistics are
/// accumulated in a `BTreeMap<QualifiedName, Statistic>` that is flushed into
/// the catalog *after* all deferred comments are applied.
fn apply_statistics_comment(
    stmt: &pg_query::protobuf::CommentStmt,
    statistics: &mut BTreeMap<QualifiedName, Statistic>,
    location: &SourceLocation,
) -> Result<(), ParseError> {
    use pg_query::NodeEnum;

    // pg_query encodes COMMENT ON STATISTICS as a List of String nodes.
    let obj = stmt
        .object
        .as_ref()
        .and_then(|o| o.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "COMMENT ON STATISTICS missing object reference".into(),
        })?;
    let NodeEnum::List(list) = obj else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "COMMENT ON STATISTICS expected a List node, got {:?}",
                std::mem::discriminant(obj)
            ),
        });
    };
    let parts: Vec<String> = list
        .items
        .iter()
        .filter_map(|n| {
            if let Some(NodeEnum::String(s)) = n.node.as_ref() {
                Some(s.sval.clone())
            } else {
                None
            }
        })
        .collect();
    let qname = match parts.as_slice() {
        [schema, name] => QualifiedName::new(
            builder::shared::ident(schema, location)?,
            builder::shared::ident(name, location)?,
        ),
        [_name] => {
            return Err(ParseError::UnqualifiedName {
                location: location.clone(),
            });
        }
        _ => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "COMMENT ON STATISTICS expected 1-2 qualified components, got {parts:?}"
                ),
            });
        }
    };

    let comment = if stmt.comment.is_empty() {
        None
    } else {
        Some(stmt.comment.clone())
    };

    let statistic = statistics.get_mut(&qname).ok_or_else(|| {
        ParseError::CommentOnStatisticBeforeCreate(qname.clone(), location.clone())
    })?;
    statistic.comment = comment;
    Ok(())
}

/// Merge accumulated pending FKs onto their target tables.
fn apply_pending_fks(
    catalog: &mut Catalog,
    pending_fks: Vec<builder::alter_table_stmt::PendingFk>,
) -> Result<(), ParseError> {
    for pending in pending_fks {
        let table = catalog
            .tables
            .iter_mut()
            .find(|t| t.qname == pending.target)
            .ok_or_else(|| ParseError::Structural {
                location: SourceLocation::new(PathBuf::new(), 0, 0),
                message: format!("ALTER TABLE referenced unknown table {}", pending.target),
            })?;
        table.constraints.push(pending.constraint);
    }
    Ok(())
}

/// Apply accumulated `ALTER COLUMN SET STORAGE / SET COMPRESSION` updates.
fn apply_pending_column_attrs(
    catalog: &mut Catalog,
    pending: Vec<builder::alter_table_stmt::PendingColumnAttr>,
) -> Result<(), ParseError> {
    use builder::alter_table_stmt::PendingColumnAttrKind;
    for attr in pending {
        let table = catalog
            .tables
            .iter_mut()
            .find(|t| t.qname == attr.target)
            .ok_or_else(|| ParseError::Structural {
                location: SourceLocation::new(PathBuf::new(), 0, 0),
                message: format!(
                    "ALTER TABLE ALTER COLUMN referenced unknown table {}",
                    attr.target
                ),
            })?;
        let col = table
            .columns
            .iter_mut()
            .find(|c| c.name == attr.column)
            .ok_or_else(|| ParseError::Structural {
                location: SourceLocation::new(PathBuf::new(), 0, 0),
                message: format!(
                    "ALTER TABLE ALTER COLUMN referenced unknown column {}.{}",
                    attr.target, attr.column
                ),
            })?;
        match attr.kind {
            PendingColumnAttrKind::Storage(s) => {
                col.storage = Some(s);
            }
            PendingColumnAttrKind::Compression(c) => {
                col.compression = c;
            }
        }
    }
    Ok(())
}

/// Apply accumulated `ALTER TABLE/MATERIALIZED VIEW ... SET (...)` reloption
/// updates to the catalog.
///
/// Called after all tables and materialized views are built.
fn apply_pending_rel_options(
    catalog: &mut Catalog,
    pending: Vec<builder::alter_table_stmt::PendingRelOptions>,
) -> Result<(), ParseError> {
    let loc = SourceLocation::new(PathBuf::new(), 0, 0);
    builder::alter_table_stmt::apply_pending_rel_options(catalog, pending, &loc)
}

/// Apply accumulated RLS mode toggles from ALTER TABLE statements.
///
/// Called after all tables are built so that the tables exist in the catalog.
fn apply_pending_rls_toggles(
    catalog: &mut Catalog,
    pending: Vec<builder::alter_table_stmt::PendingRlsToggle>,
) -> Result<(), ParseError> {
    let loc = SourceLocation::new(PathBuf::new(), 0, 0);
    builder::alter_table_stmt::apply_pending_rls_toggles(catalog, pending, &loc)
}

/// Apply accumulated `ALTER TABLE ... OWNER TO` ownership assignments.
///
/// Called after all tables, views, and materialized views are built.
fn apply_pending_owners(
    catalog: &mut Catalog,
    pending: Vec<builder::alter_table_stmt::PendingOwner>,
) -> Result<(), ParseError> {
    let loc = SourceLocation::new(PathBuf::new(), 0, 0);
    for po in pending {
        builder::alter_table_stmt::apply_pending_owners(catalog, vec![po], &loc)?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn process_file(
    path: &Path,
    contents: &str,
    catalog: &mut Catalog,
    locations: &mut HashMap<String, SourceLocation>,
    pending_fks: &mut Vec<builder::alter_table_stmt::PendingFk>,
    pending_column_attrs: &mut Vec<builder::alter_table_stmt::PendingColumnAttr>,
    pending_owners: &mut Vec<builder::alter_table_stmt::PendingOwner>,
    pending_rls_toggles: &mut Vec<builder::alter_table_stmt::PendingRlsToggle>,
    pending_rel_options: &mut Vec<builder::alter_table_stmt::PendingRelOptions>,
    deferred_comments: &mut Vec<(
        pg_query::protobuf::CommentStmt,
        SourceLocation,
        Option<crate::identifier::Identifier>,
    )>,
    publications: &mut BTreeMap<Identifier, Publication>,
    subscriptions: &mut BTreeMap<Identifier, Subscription>,
    statistics: &mut BTreeMap<QualifiedName, Statistic>,
    event_triggers: &mut BTreeMap<Identifier, EventTrigger>,
    aggregates: &mut Vec<Aggregate>,
    casts: &mut Vec<Cast>,
) -> Result<(), ParseError> {
    let directives = directives::extract_file_directives(contents, path)?;
    let parsed = pg_query::parse(contents).map_err(|e| ParseError::PgQuery {
        location: SourceLocation::new(path.to_path_buf(), 1, 1),
        message: e.to_string(),
    })?;

    for raw in parsed.protobuf.stmts {
        let location = stmt_location(path, contents, raw.stmt_location);
        let Some(node) = raw.stmt.and_then(|n| n.node) else {
            continue;
        };
        let stmt = Statement::classify(node, location.clone())?;
        match stmt {
            Statement::CreateSchema(s) => {
                let schema = builder::create_schema_stmt::build_schema(&s, &location)?;
                let schema_qname = QualifiedName::new(schema.name.clone(), schema.name.clone()); // schema has no parent; track by name
                if let Some(prior) = locations.get(&schema.name.to_string()) {
                    return Err(ParseError::DuplicateObject {
                        qname: schema.name.to_string(),
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(schema.name.to_string(), location.clone());
                catalog.schemas.push(schema);
                let _ = schema_qname;
            }
            Statement::CreateTable(s) => {
                let mut table =
                    builder::create_stmt::build_table(&s, directives.schema.as_ref(), &location)?;
                let serial_seqs =
                    builder::desugar_serial::desugar_serials_in_table(&mut table, &location)?;
                if let Some(prior) = locations.get(&table.qname.to_string()) {
                    return Err(ParseError::DuplicateObject {
                        qname: table.qname.to_string(),
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(table.qname.to_string(), location.clone());
                catalog.tables.push(table);
                for seq in serial_seqs {
                    if let Some(prior) = locations.get(&seq.qname.to_string()) {
                        return Err(ParseError::DuplicateObject {
                            qname: seq.qname.to_string(),
                            first: prior.clone(),
                            second: location.clone(),
                        });
                    }
                    locations.insert(seq.qname.to_string(), location.clone());
                    catalog.sequences.push(seq);
                }
            }
            Statement::CreateSequence(s) => {
                let seq = builder::create_seq_stmt::build_sequence(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                )?;
                if let Some(prior) = locations.get(&seq.qname.to_string()) {
                    return Err(ParseError::DuplicateObject {
                        qname: seq.qname.to_string(),
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(seq.qname.to_string(), location.clone());
                catalog.sequences.push(seq);
            }
            Statement::CreateIndex(s) => {
                let idx =
                    builder::index_stmt::build_index(&s, directives.schema.as_ref(), &location)?;
                if let Some(prior) = locations.get(&idx.qname.to_string()) {
                    return Err(ParseError::DuplicateObject {
                        qname: idx.qname.to_string(),
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(idx.qname.to_string(), location.clone());
                catalog.indexes.push(idx);
            }
            Statement::AlterTable(s) => {
                let alter_out = builder::alter_table_stmt::build_alter_table(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                )?;
                pending_fks.extend(alter_out.pending_fks);
                pending_column_attrs.extend(alter_out.pending_column_attrs);
                pending_owners.extend(alter_out.pending_owners);
                pending_rls_toggles.extend(alter_out.pending_rls_toggles);
                pending_rel_options.extend(alter_out.pending_rel_options);
            }
            Statement::Comment(s) => {
                use pg_query::protobuf::ObjectType;
                let kind = ObjectType::try_from(s.objtype).unwrap_or(ObjectType::Undefined);
                if matches!(kind, ObjectType::ObjectStatisticExt) {
                    // COMMENT ON STATISTICS is handled inline against the statistics
                    // accumulator (not deferred), because statistics are not yet in
                    // catalog at the deferred-comment application point.
                    apply_statistics_comment(&s, statistics, &location)?;
                } else if matches!(kind, ObjectType::ObjectEventTrigger) {
                    // COMMENT ON EVENT TRIGGER is handled inline against the
                    // event-trigger accumulator for the same reason.
                    builder::event_trigger_stmt::apply_event_trigger_comment(
                        &s,
                        &location,
                        event_triggers,
                    )?;
                } else if matches!(kind, ObjectType::ObjectAggregate) {
                    // COMMENT ON AGGREGATE is applied inline by `(qname, arg_types)`
                    // identity against the aggregate accumulator.
                    builder::aggregate_stmt::apply_comment(
                        &s,
                        directives.schema.as_ref(),
                        &location,
                        aggregates,
                    )?;
                } else if matches!(kind, ObjectType::ObjectCast) {
                    // COMMENT ON CAST is applied inline by `(source, target)` identity
                    // against the cast accumulator.
                    builder::cast_stmt::apply_comment(
                        &s,
                        directives.schema.as_ref(),
                        &location,
                        casts,
                    )?;
                } else {
                    deferred_comments.push((s, location, directives.schema.clone()));
                }
            }
            Statement::CreateView(s) => {
                let view = builder::create_view_stmt::build_view(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                )?;
                if let Some(prior) = locations.get(&view.qname.to_string()) {
                    return Err(ParseError::DuplicateObject {
                        qname: view.qname.to_string(),
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(view.qname.to_string(), location.clone());
                catalog.views.push(view);
            }
            Statement::CreateMaterializedView(s) => {
                let mv = builder::create_materialized_view_stmt::build_materialized_view(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                )?;
                if let Some(prior) = locations.get(&mv.qname.to_string()) {
                    return Err(ParseError::DuplicateObject {
                        qname: mv.qname.to_string(),
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(mv.qname.to_string(), location.clone());
                catalog.materialized_views.push(mv);
            }
            Statement::CreateEnum(s) => {
                let ut = builder::create_enum_stmt::build_enum(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                )?;
                if let Some(prior) = locations.get(&ut.qname.to_string()) {
                    return Err(ParseError::DuplicateObject {
                        qname: ut.qname.to_string(),
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(ut.qname.to_string(), location.clone());
                catalog.types.push(ut);
            }
            Statement::CreateDomain(s) => {
                let ut = builder::create_domain_stmt::build_domain(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                )?;
                if let Some(prior) = locations.get(&ut.qname.to_string()) {
                    return Err(ParseError::DuplicateObject {
                        qname: ut.qname.to_string(),
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(ut.qname.to_string(), location.clone());
                catalog.types.push(ut);
            }
            Statement::CreateCompositeType(s) => {
                let ut = builder::create_composite_type_stmt::build_composite(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                )?;
                if let Some(prior) = locations.get(&ut.qname.to_string()) {
                    return Err(ParseError::DuplicateObject {
                        qname: ut.qname.to_string(),
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(ut.qname.to_string(), location.clone());
                catalog.types.push(ut);
            }
            Statement::CreateRange(s) => {
                let ut = builder::create_range_stmt::build_range(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                )?;
                if let Some(prior) = locations.get(&ut.qname.to_string()) {
                    return Err(ParseError::DuplicateObject {
                        qname: ut.qname.to_string(),
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(ut.qname.to_string(), location.clone());
                catalog.types.push(ut);
            }
            Statement::CreateFunction(s) => {
                let routine = builder::create_function_stmt::build_function_or_procedure(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                )?;
                let builder::create_function_stmt::Routine::Function(f) = routine else {
                    return Err(ParseError::Structural {
                        location,
                        message: "internal error: expected Function from non-procedure stmt".into(),
                    });
                };
                let arg_sig = f
                    .args
                    .iter()
                    .filter(|a| {
                        matches!(
                            a.mode,
                            crate::ir::function::ArgMode::In
                                | crate::ir::function::ArgMode::InOut
                                | crate::ir::function::ArgMode::Variadic
                        )
                    })
                    .map(|a| a.ty.render_sql())
                    .collect::<Vec<_>>()
                    .join(",");
                let key = format!("functions.{}({arg_sig})", f.qname);
                if let Some(prior) = locations.get(&key) {
                    return Err(ParseError::DuplicateObject {
                        qname: key,
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(key, location.clone());
                catalog.functions.push(f);
            }
            Statement::CreateProcedure(s) => {
                let routine = builder::create_function_stmt::build_function_or_procedure(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                )?;
                let builder::create_function_stmt::Routine::Procedure(p) = routine else {
                    return Err(ParseError::Structural {
                        location,
                        message: "internal error: expected Procedure from procedure stmt".into(),
                    });
                };
                // Procedure identity is qname-only per arch Decision 2 — PG
                // allows procedure overloading at the catalog level, but
                // pgevolve v0.2 deliberately restricts procedures to a single
                // signature per qname. Two procedures with the same qname
                // (even with different arg types) collide.
                let key = format!("procedures.{}", p.qname);
                if let Some(prior) = locations.get(&key) {
                    return Err(ParseError::DuplicateObject {
                        qname: key,
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(key, location.clone());
                catalog.procedures.push(p);
            }
            Statement::CreateExtension(s) => {
                let ext = builder::create_extension_stmt::build_extension(&s, &location)?;
                let key = format!("extensions.{}", ext.name);
                if let Some(prior) = locations.get(&key) {
                    return Err(ParseError::DuplicateObject {
                        qname: key,
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(key, location.clone());
                catalog.extensions.push(ext);
            }
            Statement::CreateTrigger(s) => {
                let trigger = builder::create_trigger_stmt::build_trigger(&s, &location)?;
                let key = format!("triggers.{}", trigger.qname);
                if let Some(prior) = locations.get(&key) {
                    return Err(ParseError::DuplicateObject {
                        qname: key,
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(key, location.clone());
                catalog.triggers.push(trigger);
            }
            Statement::AlterTableAttachPartition(s) => {
                let attach = builder::alter_table_attach_partition::build_attach_partition(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                )?;
                let child_table = catalog
                    .tables
                    .iter_mut()
                    .find(|t| t.qname == attach.child)
                    .ok_or_else(|| ParseError::Structural {
                        location: location.clone(),
                        message: format!(
                            "ATTACH PARTITION {} must follow its CREATE TABLE statement",
                            attach.child
                        ),
                    })?;
                if child_table.partition_of.is_some() {
                    return Err(ParseError::Structural {
                        location,
                        message: format!("table {} already declared as a partition", attach.child),
                    });
                }
                child_table.partition_of = Some(attach.partition_of);
            }
            Statement::Grant(s) => {
                builder::grants::apply(&s, catalog, &location)?;
            }
            Statement::AlterOwner(s) => {
                use pg_query::protobuf::ObjectType;
                let objtype = ObjectType::try_from(s.object_type).unwrap_or(ObjectType::Undefined);
                if matches!(objtype, ObjectType::ObjectEventTrigger) {
                    // ALTER EVENT TRIGGER … OWNER TO is applied inline against the
                    // event-trigger accumulator (event triggers are not yet in the
                    // catalog at this point).
                    builder::event_trigger_stmt::apply_event_trigger_owner(
                        &s,
                        &location,
                        event_triggers,
                    )?;
                } else if matches!(objtype, ObjectType::ObjectAggregate) {
                    // ALTER AGGREGATE … OWNER TO is applied inline by identity
                    // against the aggregate accumulator.
                    builder::aggregate_stmt::apply_owner(
                        &s,
                        directives.schema.as_ref(),
                        &location,
                        aggregates,
                    )?;
                } else {
                    builder::owner_stmt::apply(&s, catalog, &location)?;
                }
            }
            Statement::AlterDefaultPrivileges(s) => {
                builder::default_privileges::apply(&s, catalog, &location)?;
            }
            Statement::CreatePolicy(s) => {
                builder::policy_stmt::apply(&s, catalog, &location)?;
            }
            Statement::CreatePublication(s) => {
                builder::publication_stmt::parse_create_publication(&s, location, publications)?;
            }
            Statement::AlterPublication(s) => {
                builder::publication_stmt::parse_alter_publication(&s, location, publications)?;
            }
            Statement::CreateSubscription(s) => {
                builder::subscription_stmt::parse_create_subscription(&s, location, subscriptions)?;
            }
            Statement::AlterSubscription(s) => {
                builder::subscription_stmt::parse_alter_subscription(&s, location, subscriptions)?;
            }
            Statement::CreateStatistics(s) => {
                builder::statistic_stmt::parse_create_statistics(&s, location, statistics)?;
            }
            Statement::AlterStatistics(s) => {
                builder::statistic_stmt::parse_alter_statistics(&s, &location, statistics)?;
            }
            Statement::CreateCollation(s) => {
                let coll = builder::create_collation_stmt::build_collation(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                )?;
                if let Some(prior) = locations.get(&coll.qname.to_string()) {
                    return Err(ParseError::DuplicateObject {
                        qname: coll.qname.to_string(),
                        first: prior.clone(),
                        second: location,
                    });
                }
                locations.insert(coll.qname.to_string(), location.clone());
                catalog.collations.push(coll);
            }
            Statement::CreateAggregate(s) => {
                builder::aggregate_stmt::parse_create(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                    aggregates,
                )?;
            }
            Statement::CreateEventTrigger(s) => {
                builder::event_trigger_stmt::parse_create_event_trigger(
                    &s,
                    directives.schema.as_ref(),
                    location,
                    event_triggers,
                )?;
            }
            Statement::AlterEventTrigger(s) => {
                builder::event_trigger_stmt::parse_alter_event_trigger(
                    &s,
                    location,
                    event_triggers,
                )?;
            }
            Statement::CreateCast(s) => {
                builder::cast_stmt::parse_create(&s, directives.schema.as_ref(), &location, casts)?;
            }
        }
    }
    Ok(())
}

/// Convert a `pg_query` byte offset into a 1-based line/column.
fn stmt_location(path: &Path, contents: &str, byte_offset: i32) -> SourceLocation {
    let offset = usize::try_from(byte_offset).unwrap_or(0);
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, c) in contents.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    SourceLocation::new(path.to_path_buf(), line, col)
}

fn translate_canonicalize_error(
    e: IrError,
    locations: &HashMap<String, SourceLocation>,
) -> ParseError {
    if let IrError::InvalidIdentifier(msg) = &e {
        // Format is "duplicate <kind>: <qname>" (see `Catalog::canonicalize`).
        if let Some(rest) = msg.strip_prefix("duplicate ")
            && let Some((_, qname)) = rest.split_once(": ")
            && let Some(loc) = locations.get(qname)
        {
            return ParseError::DuplicateObject {
                qname: qname.to_string(),
                first: loc.clone(),
                second: loc.clone(),
            };
        }
    }
    let placeholder = SourceLocation::new(PathBuf::new(), 0, 0);
    ParseError::Ir {
        location: placeholder,
        source: e,
    }
}

/// Smoke test: parse a single statement string with `pg_query`.
#[cfg(test)]
pub(crate) fn smoke_parse(sql: &str) -> Result<pg_query::ParseResult, pg_query::Error> {
    pg_query::parse(sql)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_query_round_trips_a_create_table() {
        let sql = "CREATE TABLE app.users (id integer);";
        let result = smoke_parse(sql).expect("pg_query parses");
        // Smoke check: the parse tree contains at least one statement.
        assert!(!result.protobuf.stmts.is_empty());
    }

    #[test]
    fn pg_query_reports_syntax_errors() {
        let sql = "CREATE TABLE !bad!;";
        assert!(smoke_parse(sql).is_err());
    }
}
