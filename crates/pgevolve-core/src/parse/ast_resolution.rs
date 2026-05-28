//! AST resolution pass.
//!
//! Runs after the structural parse builds the provisional IR and before
//! `Catalog::canonicalize`. For v0.1 the pass validates that structural
//! references (FKs, default-using sequences) resolve. v0.2 sub-specs
//! extend it to walk body ASTs (view body, function body, etc.) and
//! produce `DepEdge` records with `DepSource::AstExtracted`.
//!
//! # Location keys
//!
//! The `locations` map passed in from `parse_directory_with_locations` is keyed
//! by the declaring object's qname string (e.g. `"app.users"`, `"app"`), not by
//! a hierarchical path. When reporting an unresolved FK or sequence default we
//! attach the *table*'s location as the best available source position.
//!
//! # Cross-schema references to unmanaged schemas
//!
//! Only tables and sequences present in the source IR (`catalog.tables`,
//! `catalog.sequences`) are recognised as valid referents. A FK that points to
//! an unmanaged schema or a PG built-in will be flagged as unresolved. This
//! behaviour is intentional for v0.1, where every object referenced in source
//! DDL must itself be declared in source DDL.

use std::collections::{BTreeSet, HashMap};

use crate::ir::catalog::Catalog;
use crate::ir::column_type::ColumnType;
use crate::parse::error::SourceLocation;
use crate::plan::edges::{DepEdge, DepSource, NodeId};

/// One unresolved reference in the source IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstResolutionError {
    /// Free-text description of the missing reference, e.g.
    /// `"FK app.orders.fk_user references app.users which is not declared"`.
    pub message: String,
    /// Source location of the *referencer* (the table containing the FK or
    /// the column with the sequence default). `None` when the location map
    /// does not contain an entry for the containing object.
    pub location: Option<SourceLocation>,
}

impl std::fmt::Display for AstResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(loc) = &self.location {
            write!(f, "{}: {}", loc, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

/// Run the AST resolution pass over a provisional catalog and its
/// source-location map.
///
/// Returns `Ok(())` when every reference resolves. Returns `Err(Vec<...>)`
/// listing every unresolved reference — errors are accumulated rather than
/// short-circuited so the user sees the full picture in one pass.
pub fn resolve(
    catalog: &Catalog,
    locations: &HashMap<String, SourceLocation>,
) -> Result<(), Vec<AstResolutionError>> {
    let mut errors = Vec::new();
    resolve_fk_references(catalog, locations, &mut errors);
    resolve_default_sequence_references(catalog, locations, &mut errors);
    resolve_user_defined_references(catalog, locations, &mut errors);
    resolve_routine_references(catalog, locations, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn resolve_fk_references(
    catalog: &Catalog,
    locations: &HashMap<String, SourceLocation>,
    errors: &mut Vec<AstResolutionError>,
) {
    use crate::ir::constraint::ConstraintKind;

    let known_tables: std::collections::BTreeSet<_> =
        catalog.tables.iter().map(|t| t.qname.to_string()).collect();

    for table in &catalog.tables {
        for constraint in &table.constraints {
            if let ConstraintKind::ForeignKey(fk) = &constraint.kind {
                let ref_key = fk.referenced_table.to_string();
                if !known_tables.contains(&ref_key) {
                    // Use the containing table's location as the best available
                    // source position for this FK.
                    let location = locations.get(&table.qname.to_string()).cloned();
                    errors.push(AstResolutionError {
                        message: format!(
                            "FK {}.{} references {} which is not declared in source",
                            table.qname, constraint.qname.name, fk.referenced_table,
                        ),
                        location,
                    });
                }
            }
        }
    }
}

fn resolve_default_sequence_references(
    catalog: &Catalog,
    locations: &HashMap<String, SourceLocation>,
    errors: &mut Vec<AstResolutionError>,
) {
    use crate::ir::default_expr::DefaultExpr;

    let known_sequences: BTreeSet<_> = catalog
        .sequences
        .iter()
        .map(|s| s.qname.to_string())
        .collect();

    for table in &catalog.tables {
        for column in &table.columns {
            if let Some(DefaultExpr::Sequence(qname)) = &column.default {
                let seq_key = qname.to_string();
                if !known_sequences.contains(&seq_key) {
                    // Use the containing table's location as the best available
                    // source position for this column default.
                    let location = locations.get(&table.qname.to_string()).cloned();
                    errors.push(AstResolutionError {
                        message: format!(
                            "column {}.{} defaults to nextval({}) but sequence is not declared",
                            table.qname, column.name, qname,
                        ),
                        location,
                    });
                }
            }
        }
    }
}

fn resolve_user_defined_references(
    catalog: &Catalog,
    locations: &HashMap<String, SourceLocation>,
    errors: &mut Vec<AstResolutionError>,
) {
    use crate::ir::user_type::UserTypeKind;

    let known_types: BTreeSet<String> = catalog.types.iter().map(|t| t.qname.to_string()).collect();

    // 1. Walk table columns.
    for table in &catalog.tables {
        for column in &table.columns {
            if let ColumnType::UserDefined(qname) = &column.ty {
                let key = qname.to_string();
                if !known_types.contains(&key) {
                    let location = locations.get(&table.qname.to_string()).cloned();
                    errors.push(AstResolutionError {
                        message: format!(
                            "column {}.{} has type {} which is not declared in source",
                            table.qname, column.name, qname,
                        ),
                        location,
                    });
                }
            }
        }
    }

    // 2. Walk composite attributes and domain bases.
    for ut in &catalog.types {
        match &ut.kind {
            UserTypeKind::Composite { attributes } => {
                for attr in attributes {
                    if let ColumnType::UserDefined(qname) = &attr.ty {
                        let key = qname.to_string();
                        if !known_types.contains(&key) {
                            let location = locations.get(&ut.qname.to_string()).cloned();
                            errors.push(AstResolutionError {
                                message: format!(
                                    "composite type {}.{} has type {} which is not declared in source",
                                    ut.qname, attr.name, qname,
                                ),
                                location,
                            });
                        }
                    }
                }
            }
            UserTypeKind::Domain { base, .. } => {
                if let ColumnType::UserDefined(qname) = base {
                    let key = qname.to_string();
                    if !known_types.contains(&key) {
                        let location = locations.get(&ut.qname.to_string()).cloned();
                        errors.push(AstResolutionError {
                            message: format!(
                                "domain {} base type {} is not declared in source",
                                ut.qname, qname,
                            ),
                            location,
                        });
                    }
                }
            }
            UserTypeKind::Enum { .. } => {
                // Enums have no UserDefined references — their values are bare labels.
            }
            UserTypeKind::Range { subtype, .. } => {
                // Built-in subtypes (pg_catalog.*) need no closed-world check.
                // Managed user-type subtypes do — they must be declared in source.
                if subtype.schema.as_str() != "pg_catalog" {
                    let key = subtype.to_string();
                    if !known_types.contains(&key) {
                        let location = locations.get(&ut.qname.to_string()).cloned();
                        errors.push(AstResolutionError {
                            message: format!(
                                "range type {} subtype {} is not declared in source",
                                ut.qname, subtype,
                            ),
                            location,
                        });
                    }
                }
            }
        }
    }
}

/// Resolve `body_dependencies` edges on every function and procedure.
///
/// For `AstExtracted` edges the target `NodeId::Table(q)` is a relation
/// reference extracted from the SQL/PL/pgSQL body. It is valid if `q` is
/// declared as a table, view, or materialized view.
///
/// For `AstDeclared` edges the target `NodeId::Table(q)` is a placeholder
/// (the actual object kind is unknown at directive-scan time). The resolver
/// probes all catalog collections by qname and accepts if any matches.
fn resolve_routine_references(
    catalog: &Catalog,
    locations: &HashMap<String, SourceLocation>,
    errors: &mut Vec<AstResolutionError>,
) {
    // Build qname sets for fast membership checks.
    let known_tables: BTreeSet<String> =
        catalog.tables.iter().map(|t| t.qname.to_string()).collect();
    let known_views: BTreeSet<String> = catalog.views.iter().map(|v| v.qname.to_string()).collect();
    let known_mvs: BTreeSet<String> = catalog
        .materialized_views
        .iter()
        .map(|mv| mv.qname.to_string())
        .collect();
    let known_types: BTreeSet<String> = catalog.types.iter().map(|t| t.qname.to_string()).collect();
    let known_functions_by_qname: BTreeSet<String> = catalog
        .functions
        .iter()
        .map(|f| f.qname.to_string())
        .collect();
    let known_procedures: BTreeSet<String> = catalog
        .procedures
        .iter()
        .map(|p| p.qname.to_string())
        .collect();

    // Walk function body_dependencies.
    for func in &catalog.functions {
        let location = locations.get(&format!(
            "functions.{}({})",
            func.qname,
            func.args
                .iter()
                .filter(|a| matches!(
                    a.mode,
                    crate::ir::function::ArgMode::In
                        | crate::ir::function::ArgMode::InOut
                        | crate::ir::function::ArgMode::Variadic
                ))
                .map(|a| a.ty.render_sql())
                .collect::<Vec<_>>()
                .join(",")
        ));
        check_body_deps(
            &func.body_dependencies,
            &func.qname.to_string(),
            location,
            &known_tables,
            &known_views,
            &known_mvs,
            &known_types,
            &known_functions_by_qname,
            &known_procedures,
            errors,
        );
    }

    // Walk procedure body_dependencies.
    for proc in &catalog.procedures {
        let location = locations.get(&format!("procedures.{}", proc.qname));
        check_body_deps(
            &proc.body_dependencies,
            &proc.qname.to_string(),
            location,
            &known_tables,
            &known_views,
            &known_mvs,
            &known_types,
            &known_functions_by_qname,
            &known_procedures,
            errors,
        );
    }
}

/// Check a list of body dependency edges, emitting errors for unresolved ones.
#[allow(clippy::too_many_arguments)]
fn check_body_deps(
    deps: &[DepEdge],
    routine_qname: &str,
    location: Option<&SourceLocation>,
    known_tables: &BTreeSet<String>,
    known_views: &BTreeSet<String>,
    known_mvs: &BTreeSet<String>,
    known_types: &BTreeSet<String>,
    known_functions_by_qname: &BTreeSet<String>,
    known_procedures: &BTreeSet<String>,
    errors: &mut Vec<AstResolutionError>,
) {
    for dep in deps {
        match dep.source {
            DepSource::AstExtracted => {
                // AstExtracted edges use NodeId::Table as a relation-reference
                // placeholder. Valid targets are tables, views, and MVs.
                if let NodeId::Table(ref target_qname) = dep.to {
                    let key = target_qname.to_string();
                    if !known_tables.contains(&key)
                        && !known_views.contains(&key)
                        && !known_mvs.contains(&key)
                    {
                        errors.push(AstResolutionError {
                            message: format!(
                                "routine {routine_qname} body references {key} which is not \
                                 declared in source (not a table, view, or materialized view)"
                            ),
                            location: location.cloned(),
                        });
                    }
                }
                // Other NodeId variants (Type, Function, Procedure, etc.) are
                // not currently emitted by the body parser, so no action needed.
            }
            DepSource::AstDeclared => {
                // AstDeclared edges use NodeId::Table as a placeholder for the
                // unknown object kind. Probe all collections by qname.
                if let NodeId::Table(ref target_qname) = dep.to {
                    let key = target_qname.to_string();
                    let found = known_tables.contains(&key)
                        || known_views.contains(&key)
                        || known_mvs.contains(&key)
                        || known_types.contains(&key)
                        || known_functions_by_qname.contains(&key)
                        || known_procedures.contains(&key);
                    if !found {
                        errors.push(AstResolutionError {
                            message: format!(
                                "routine {routine_qname} has `-- @pgevolve dep: {key}` directive \
                                 but {key} is not declared in source (probed tables, views, \
                                 materialized views, types, functions, and procedures)"
                            ),
                            location: location.cloned(),
                        });
                    }
                }
            }
            DepSource::Structural => {
                // Structural edges are not produced by body parsing; skip.
            }
        }
    }
}
