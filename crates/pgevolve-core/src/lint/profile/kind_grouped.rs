//! `kind-grouped` layout profile.
//!
//! Required paths (relative to `schema_dir`):
//! - tables/indexes/sequences: `<kind_plural>/<schema>.<name>.sql`
//! - schemas: `schemas/<name>.sql`
//!
//! Plus one-object-per-file (same rule as `schema-mirror`).

use std::path::Path;

use crate::lint::finding::Finding;
use crate::lint::source_tree::{ObjectKey, SourceTree};

/// Run the kind-grouped profile rules.
pub fn check(tree: &SourceTree, schema_dir: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    let by_file = tree.objects_by_file();

    for (key, loc) in &tree.object_locations {
        let rel = loc
            .file
            .strip_prefix(schema_dir)
            .unwrap_or(loc.file.as_path());
        let expected = expected_path(key);
        if !path_equals(rel, &expected) {
            out.push(
                Finding::error(
                    "kind_grouped_path",
                    format!(
                        "{} should be at `{}`; found at `{}`",
                        key.kind_name(),
                        expected,
                        rel.display(),
                    ),
                )
                .at(loc.clone()),
            );
        }
    }

    for (file, keys) in &by_file {
        if keys.len() > 1 {
            out.push(Finding::error(
                "kind_grouped_one_object_per_file",
                format!(
                    "file `{}` contains {} objects (kind-grouped allows only one)",
                    file.display(),
                    keys.len(),
                ),
            ));
        }
    }

    out
}

fn expected_path(key: &ObjectKey) -> String {
    match key {
        ObjectKey::Schema(name) => format!("schemas/{name}.sql"),
        ObjectKey::Table(q) | ObjectKey::Index(q) | ObjectKey::Sequence(q) => {
            format!("{}/{}.{}.sql", key.kind_plural(), q.schema, q.name)
        }
    }
}

fn path_equals(rel: &Path, expected: &str) -> bool {
    rel.to_str()
        .is_some_and(|s| s.replace('\\', "/") == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::schema::Schema;
    use crate::ir::table::Table;
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
    fn passes_when_paths_match_convention() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
        });
        let mut locs = HashMap::new();
        locs.insert(ObjectKey::Schema(id("app")), loc("schema/schemas/app.sql"));
        locs.insert(
            ObjectKey::Table(qn("app", "users")),
            loc("schema/tables/app.users.sql"),
        );
        let tree = SourceTree::new(c, locs);
        assert!(check(&tree, Path::new("schema")).is_empty());
    }

    #[test]
    fn flags_misplaced_table() {
        let mut c = Catalog::empty();
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
        });
        let mut locs = HashMap::new();
        locs.insert(
            ObjectKey::Table(qn("app", "users")),
            loc("schema/wrong/app.users.sql"),
        );
        let tree = SourceTree::new(c, locs);
        let f = check(&tree, Path::new("schema"));
        assert!(f.iter().any(|x| x.rule == "kind_grouped_path"));
    }
}
