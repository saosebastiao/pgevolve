//! Source-side parser: SQL bytes → IR.
//!
//! This module accepts a directory of `CREATE`-style DDL files and produces a
//! [`crate::ir::catalog::Catalog`]. Construction is I/O-free at the type level —
//! the only I/O is performed by [`parse_directory`] on behalf of callers.

mod ast_resolution;
pub mod builder;
pub mod directives;
pub mod error;
pub mod normalize_body;
pub mod normalize_expr;
pub mod statement;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use directives::{FileDirectives, extract_file_directives};
pub use error::{ParseError, SourceLocation};
pub use statement::Statement;

use crate::identifier::QualifiedName;
use crate::ir::IrError;
use crate::ir::catalog::Catalog;

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
    let mut deferred_comments: Vec<(
        pg_query::protobuf::CommentStmt,
        SourceLocation,
        Option<crate::identifier::Identifier>,
    )> = Vec::new();

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
            &mut deferred_comments,
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

    // Merge pending FKs from ALTER TABLE statements onto their target tables.
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

    // AST resolution pass: validate that all structural references (FKs,
    // sequence defaults) resolve against the declared IR, before any DB touch.
    ast_resolution::resolve(&catalog, &locations)
        .map_err(ParseError::AstResolution)?;

    let canonical = catalog
        .canonicalize()
        .map_err(|e: IrError| translate_canonicalize_error(e, &locations))?;
    Ok((canonical, locations))
}

#[allow(clippy::too_many_lines)]
fn process_file(
    path: &Path,
    contents: &str,
    catalog: &mut Catalog,
    locations: &mut HashMap<String, SourceLocation>,
    pending_fks: &mut Vec<builder::alter_table_stmt::PendingFk>,
    deferred_comments: &mut Vec<(
        pg_query::protobuf::CommentStmt,
        SourceLocation,
        Option<crate::identifier::Identifier>,
    )>,
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
                let pendings = builder::alter_table_stmt::build_alter_table(
                    &s,
                    directives.schema.as_ref(),
                    &location,
                )?;
                pending_fks.extend(pendings);
            }
            Statement::Comment(s) => {
                deferred_comments.push((s, location, directives.schema.clone()));
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
