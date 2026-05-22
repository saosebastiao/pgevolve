//! `feature-grouped` layout profile.
//!
//! Files live under `<schema_dir>/<feature-dir>/`. Multiple objects per file
//! are allowed. The cross-feature overlap rule (spec §12) is deferred for
//! v0.1; this profile currently checks the path shape only.

use std::path::Path;

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// Run the feature-grouped profile rules.
pub fn check(tree: &SourceTree, schema_dir: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    for (key, loc) in &tree.object_locations {
        let rel = loc
            .file
            .strip_prefix(schema_dir)
            .unwrap_or(loc.file.as_path());
        // Must have at least one directory component before the file name.
        if rel.parent().is_none_or(|p| p.as_os_str().is_empty()) {
            out.push(
                Finding::error(
                    "feature_grouped_needs_feature_dir",
                    format!(
                        "{} `{}` at `{}` is not under a feature subdirectory",
                        key.kind_name(),
                        key,
                        rel.display(),
                    ),
                )
                .at(loc.clone()),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::table::Table;
    use crate::lint::source_tree::ObjectKey;
    use crate::parse::SourceLocation;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(id(s), id(n))
    }
    fn loc(p: &str) -> SourceLocation {
        SourceLocation::new(PathBuf::from(p), 1, 1)
    }

    #[test]
    fn passes_when_file_lives_under_feature_dir() {
        let mut c = Catalog::empty();
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
        });
        let mut locs = HashMap::new();
        locs.insert(
            ObjectKey::Table(qn("app", "users")),
            loc("schema/auth/users.sql"),
        );
        let tree = SourceTree::new(c, locs);
        assert!(check(&tree, Path::new("schema")).is_empty());
    }

    #[test]
    fn flags_file_directly_under_schema_dir() {
        let mut c = Catalog::empty();
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
        });
        let mut locs = HashMap::new();
        locs.insert(
            ObjectKey::Table(qn("app", "users")),
            loc("schema/users.sql"),
        );
        let tree = SourceTree::new(c, locs);
        let f = check(&tree, Path::new("schema"));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "feature_grouped_needs_feature_dir");
    }
}
