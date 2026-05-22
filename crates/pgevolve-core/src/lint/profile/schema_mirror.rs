//! `schema-mirror` layout profile.
//!
//! Required paths (relative to `schema_dir`):
//! - tables/indexes/sequences: `<schema>/<kind_plural>/<name>.sql`
//! - schema declarations: `<schema>/_schema.sql`
//!
//! Plus a one-object-per-file rule.

use std::path::Path;

use crate::lint::finding::Finding;
use crate::lint::source_tree::{ObjectKey, SourceTree};

/// Run the schema-mirror profile rules.
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
                    "schema_mirror_path",
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
        // _schema.sql is the only file that's allowed to host a schema; multiple
        // schemas in one file would be unusual but rejected by parse_directory
        // already. The schema-mirror rule below just enforces single-object
        // files for tables/indexes/sequences.
        let non_schema_count = keys
            .iter()
            .filter(|k| !matches!(k, ObjectKey::Schema(_)))
            .count();
        if non_schema_count > 1 {
            out.push(Finding::error(
                "schema_mirror_one_object_per_file",
                format!(
                    "file `{}` contains {} non-schema objects (schema-mirror allows only one)",
                    file.display(),
                    non_schema_count,
                ),
            ));
        }
    }

    out
}

fn expected_path(key: &ObjectKey) -> String {
    match key {
        ObjectKey::Schema(name) => format!("{name}/_schema.sql"),
        ObjectKey::Table(q) | ObjectKey::Index(q) | ObjectKey::Sequence(q) => {
            format!("{}/{}/{}.sql", q.schema, key.kind_plural(), q.name)
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
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
        });
        let mut locs = HashMap::new();
        locs.insert(ObjectKey::Schema(id("app")), loc("schema/app/_schema.sql"));
        locs.insert(
            ObjectKey::Table(qn("app", "users")),
            loc("schema/app/tables/users.sql"),
        );
        let tree = SourceTree::new(c, locs);
        let f = check(&tree, Path::new("schema"));
        assert!(f.is_empty(), "got findings: {f:?}");
    }

    #[test]
    fn flags_table_at_wrong_path() {
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
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
        });
        let mut locs = HashMap::new();
        locs.insert(
            ObjectKey::Table(qn("app", "users")),
            loc("schema/oops/users.sql"),
        );
        let tree = SourceTree::new(c, locs);
        let f = check(&tree, Path::new("schema"));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "schema_mirror_path");
        assert!(f[0].message.contains("app/tables/users.sql"));
    }

    #[test]
    fn flags_two_objects_in_one_file() {
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
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
        });
        c.tables.push(Table {
            qname: qn("app", "orgs"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
        });
        let mut locs = HashMap::new();
        locs.insert(
            ObjectKey::Table(qn("app", "users")),
            loc("schema/app/tables/users.sql"),
        );
        locs.insert(
            ObjectKey::Table(qn("app", "orgs")),
            loc("schema/app/tables/users.sql"),
        );
        let tree = SourceTree::new(c, locs);
        let f = check(&tree, Path::new("schema"));
        // 1 path mismatch (orgs file not at orgs.sql) + 1 one-object-per-file.
        let multi = f
            .iter()
            .filter(|x| x.rule == "schema_mirror_one_object_per_file")
            .count();
        assert_eq!(multi, 1);
    }
}
