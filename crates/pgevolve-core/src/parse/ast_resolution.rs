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

use std::collections::HashMap;

use crate::ir::catalog::Catalog;
use crate::parse::error::SourceLocation;

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

    let known_sequences: std::collections::BTreeSet<_> = catalog
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
