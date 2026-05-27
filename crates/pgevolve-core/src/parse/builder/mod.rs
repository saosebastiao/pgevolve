//! AST → IR builders.
//!
//! Each submodule consumes one classified [`crate::parse::Statement`] variant
//! and produces zero-or-more IR objects, optionally appended to a partial
//! [`crate::ir::catalog::Catalog`] via [`Builder`].

pub mod alter_table_attach_partition;
pub mod alter_table_stmt;
pub mod comment_stmt;
pub mod create_composite_type_stmt;
pub mod create_domain_stmt;
pub mod create_enum_stmt;
pub mod create_extension_stmt;
pub mod create_function_stmt;
pub mod create_materialized_view_stmt;
pub mod create_schema_stmt;
pub mod create_seq_stmt;
pub mod create_stmt;
pub mod create_trigger_stmt;
pub mod create_view_stmt;
pub mod default_privileges;
pub mod desugar_serial;
pub mod grants;
pub mod index_stmt;
pub mod owner_stmt;
pub mod plpgsql;
pub mod policy_stmt;
pub mod publication_stmt;
pub mod reloptions;
pub mod shared;
pub mod statistic_stmt;
pub mod subscription_stmt;

use std::collections::HashMap;

use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::parse::error::SourceLocation;

/// Mutable accumulator passed through builders during a single
/// `parse_directory` pass.
#[derive(Debug, Default)]
pub struct Builder {
    /// The catalog being assembled.
    pub catalog: Catalog,
    /// First-seen source location for every object qname, for duplicate diagnostics.
    pub locations: HashMap<String, SourceLocation>,
}

impl Builder {
    /// Construct an empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the source location at which `qname` was first defined.
    /// Returns the prior location if the qname is already known.
    pub fn record_location(
        &mut self,
        qname: &QualifiedName,
        location: SourceLocation,
    ) -> Option<SourceLocation> {
        let key = qname.to_string();
        if let Some(prior) = self.locations.get(&key) {
            return Some(prior.clone());
        }
        self.locations.insert(key, location);
        None
    }
}
