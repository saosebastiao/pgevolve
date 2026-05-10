//! Schema/object filter applied to introspected rows.

use glob::Pattern;

use crate::catalog::error::CatalogError;
use crate::identifier::{Identifier, QualifiedName};

/// Schemas that pgevolve will refuse to manage even if they're listed in
/// configuration. Mirrors the source-parser's hard exclusions.
pub const RESERVED_SCHEMAS: &[&str] = &["pg_catalog", "pg_toast", "information_schema", "pgevolve"];

/// Filter combining a managed-schema list with a set of ignore globs.
///
/// At read time, every catalog query is parameterized by the managed-schema
/// list; rows are then post-filtered by the ignore globs. Ignore globs match
/// against the rendered `schema.name` qualified name.
#[derive(Debug, Clone)]
pub struct CatalogFilter {
    managed_schemas: Vec<Identifier>,
    ignore_globs: Vec<Pattern>,
}

impl CatalogFilter {
    /// Construct from a managed-schema list and a list of ignore globs.
    ///
    /// Returns [`CatalogError::CannotManageReservedSchema`] if any reserved
    /// schema appears in `managed`, and [`CatalogError::InvalidIgnoreGlob`]
    /// for any ill-formed glob.
    pub fn new(managed: Vec<Identifier>, ignores: Vec<String>) -> Result<Self, CatalogError> {
        for s in &managed {
            if RESERVED_SCHEMAS.contains(&s.as_str()) {
                return Err(CatalogError::CannotManageReservedSchema(
                    s.as_str().to_string(),
                ));
            }
        }
        let mut ignore_globs = Vec::with_capacity(ignores.len());
        for raw in ignores {
            let pat =
                Pattern::new(&raw).map_err(|e| CatalogError::InvalidIgnoreGlob(raw.clone(), e))?;
            ignore_globs.push(pat);
        }
        Ok(Self {
            managed_schemas: managed,
            ignore_globs,
        })
    }

    /// Borrow the managed-schema list as a `&[&str]` slice for parameter binding.
    #[must_use]
    pub fn managed_schemas_param(&self) -> Vec<&str> {
        self.managed_schemas
            .iter()
            .map(Identifier::as_str)
            .collect()
    }

    /// Whether a managed schema is in scope.
    #[must_use]
    pub fn includes_schema(&self, name: &Identifier) -> bool {
        self.managed_schemas.iter().any(|m| m == name)
    }

    /// Whether `qname` is allowed by ignore globs.
    #[must_use]
    pub fn allows(&self, qname: &QualifiedName) -> bool {
        let rendered = qname.to_string();
        !self.ignore_globs.iter().any(|p| p.matches(&rendered))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn rejects_reserved_schema() {
        let err = CatalogFilter::new(vec![id("pgevolve")], vec![]).unwrap_err();
        assert!(matches!(err, CatalogError::CannotManageReservedSchema(_)));
    }

    #[test]
    fn invalid_glob_rejected() {
        let err = CatalogFilter::new(vec![id("app")], vec!["[".into()]).unwrap_err();
        assert!(matches!(err, CatalogError::InvalidIgnoreGlob(_, _)));
    }

    #[test]
    fn allows_non_matching_qname() {
        let f = CatalogFilter::new(vec![id("app")], vec!["app.legacy_*".into()]).unwrap();
        assert!(f.allows(&QualifiedName::new(id("app"), id("users"))));
        assert!(!f.allows(&QualifiedName::new(id("app"), id("legacy_orders"))));
    }

    #[test]
    fn managed_param_mirrors_input() {
        let f = CatalogFilter::new(vec![id("app"), id("billing")], vec![]).unwrap();
        assert_eq!(f.managed_schemas_param(), vec!["app", "billing"]);
    }
}
