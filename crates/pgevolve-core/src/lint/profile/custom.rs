//! Custom layout profile loaded from a TOML file. Spec §12 / Phase 10 Task 10.8.
//!
//! A custom profile is a list of [`PathPattern`]s. Each pattern is a regex
//! (with optional named captures `schema`, `kind`, `name`) plus a list of
//! [`Assertion`]s that constrain how the captured fragments must relate to
//! the matched object's qname.
//!
//! For each object, the engine finds the first pattern whose regex matches
//! its file path (relative to `schema_dir`). If no pattern matches, the
//! object gets an `unmatched_path` finding. If a pattern matches but an
//! assertion fails, each failed assertion produces a finding.

use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;
use serde::Deserialize;

use crate::lint::finding::Finding;
use crate::lint::source_tree::{ObjectKey, SourceTree};

/// One custom profile loaded from disk.
#[derive(Debug, Clone, Deserialize)]
pub struct CustomProfile {
    /// Ordered list of path patterns. First match wins.
    pub patterns: Vec<PathPattern>,
}

/// One path pattern. Compiled lazily by [`check`].
#[derive(Debug, Clone, Deserialize)]
pub struct PathPattern {
    /// Regex applied to the object's path relative to `schema_dir`. Use
    /// forward-slash separators in the regex.
    pub regex: String,
    /// Assertions checked when the regex matches.
    #[serde(default)]
    pub assertions: Vec<Assertion>,
}

/// One assertion on a matched path. See module docs for semantics.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Assertion {
    /// `qname.schema == captures["schema"]`.
    SchemaMatchesCapture,
    /// `qname.name == captures["name"]`.
    NameMatchesCapture,
    /// `kind_name(object) == map[captures["kind"]]`. Map the captured kind
    /// fragment to one of the object kinds (`schema`, `table`, `index`,
    /// `sequence`).
    KindMatchesCapture {
        /// Captured kind fragment → expected object kind.
        allowed_values: BTreeMap<String, String>,
    },
    /// The matched file must contain exactly one object.
    OneObjectPerFile,
}

/// Run the custom-profile rules.
pub fn check(profile: &CustomProfile, tree: &SourceTree, schema_dir: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    let compiled: Vec<_> = profile
        .patterns
        .iter()
        .map(|p| {
            (
                Regex::new(&p.regex).map_err(|e| (p.regex.clone(), e.to_string())),
                p,
            )
        })
        .collect();

    // Surface bad regexes as findings (one per pattern).
    for (compiled_re, _) in &compiled {
        if let Err((re_str, err)) = compiled_re {
            out.push(Finding::error(
                "custom_profile_invalid_regex",
                format!("regex `{re_str}` failed to compile: {err}"),
            ));
        }
    }

    let by_file = tree.objects_by_file();

    for (key, loc) in &tree.object_locations {
        let rel = loc
            .file
            .strip_prefix(schema_dir)
            .unwrap_or(loc.file.as_path());
        let rel_string = rel.to_string_lossy().replace('\\', "/");

        let mut matched_any = false;
        for (compiled_re, pattern) in &compiled {
            let Ok(re) = compiled_re else { continue };
            if let Some(caps) = re.captures(&rel_string) {
                matched_any = true;
                for a in &pattern.assertions {
                    if let Some(finding) = check_assertion(a, key, &caps, loc, &by_file) {
                        out.push(finding);
                    }
                }
                break;
            }
        }

        if !matched_any {
            out.push(
                Finding::error(
                    "custom_profile_unmatched_path",
                    format!("path `{rel_string}` did not match any custom-profile pattern"),
                )
                .at(loc.clone()),
            );
        }
    }

    out
}

fn check_assertion(
    assertion: &Assertion,
    key: &ObjectKey,
    caps: &regex::Captures<'_>,
    loc: &crate::parse::SourceLocation,
    by_file: &std::collections::HashMap<std::path::PathBuf, Vec<ObjectKey>>,
) -> Option<Finding> {
    match assertion {
        Assertion::SchemaMatchesCapture => {
            let captured = caps.name("schema")?.as_str();
            if key.schema().as_str() == captured {
                None
            } else {
                Some(
                    Finding::error(
                        "custom_profile_schema_mismatch",
                        format!(
                            "object `{key}` schema `{}` does not match captured `{captured}`",
                            key.schema()
                        ),
                    )
                    .at(loc.clone()),
                )
            }
        }
        Assertion::NameMatchesCapture => {
            let captured = caps.name("name")?.as_str();
            if key.bare_name().as_str() == captured {
                None
            } else {
                Some(
                    Finding::error(
                        "custom_profile_name_mismatch",
                        format!(
                            "object `{key}` name `{}` does not match captured `{captured}`",
                            key.bare_name()
                        ),
                    )
                    .at(loc.clone()),
                )
            }
        }
        Assertion::KindMatchesCapture { allowed_values } => {
            let captured = caps.name("kind")?.as_str();
            let expected = allowed_values.get(captured);
            match expected {
                Some(want) if want == key.kind_name() => None,
                Some(want) => Some(
                    Finding::error(
                        "custom_profile_kind_mismatch",
                        format!(
                            "object `{key}` is a {actual}, but its path implies kind `{captured}` → `{want}`",
                            actual = key.kind_name()
                        ),
                    )
                    .at(loc.clone()),
                ),
                None => Some(
                    Finding::error(
                        "custom_profile_kind_unknown",
                        format!(
                            "captured kind `{captured}` is not in the allowed_values map",
                        ),
                    )
                    .at(loc.clone()),
                ),
            }
        }
        Assertion::OneObjectPerFile => {
            let count = by_file.get(&loc.file).map_or(0, Vec::len);
            if count <= 1 {
                None
            } else {
                Some(Finding::error(
                    "custom_profile_one_object_per_file",
                    format!(
                        "file `{}` contains {count} objects (custom profile pattern requires one)",
                        loc.file.display(),
                    ),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
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

    fn schema_mirror_equivalent() -> CustomProfile {
        let mut map = BTreeMap::new();
        map.insert("tables".to_string(), "table".to_string());
        map.insert("indexes".to_string(), "index".to_string());
        map.insert("sequences".to_string(), "sequence".to_string());
        CustomProfile {
            patterns: vec![PathPattern {
                regex: r"^schema/(?P<schema>[^/]+)/(?P<kind>tables|indexes|sequences)/(?P<name>[^/]+)\.sql$"
                    .into(),
                assertions: vec![
                    Assertion::SchemaMatchesCapture,
                    Assertion::NameMatchesCapture,
                    Assertion::KindMatchesCapture {
                        allowed_values: map,
                    },
                    Assertion::OneObjectPerFile,
                ],
            }],
        }
    }

    #[test]
    fn schema_mirror_equivalent_passes_compliant_tree() {
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
            loc("schema/app/tables/users.sql"),
        );
        let tree = SourceTree::new(c, locs);
        let f = check(&schema_mirror_equivalent(), &tree, Path::new(""));
        assert!(f.is_empty(), "got: {f:?}");
    }

    #[test]
    fn schema_mismatch_flagged() {
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
            loc("schema/other/tables/users.sql"),
        );
        let tree = SourceTree::new(c, locs);
        let f = check(&schema_mirror_equivalent(), &tree, Path::new(""));
        assert!(f.iter().any(|x| x.rule == "custom_profile_schema_mismatch"));
    }

    #[test]
    fn unmatched_path_flagged() {
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
            loc("schema/whatever.sql"),
        );
        let tree = SourceTree::new(c, locs);
        let f = check(&schema_mirror_equivalent(), &tree, Path::new(""));
        assert!(f.iter().any(|x| x.rule == "custom_profile_unmatched_path"));
    }
}
