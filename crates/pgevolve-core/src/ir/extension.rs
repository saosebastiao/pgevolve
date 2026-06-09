//! `Extension` — a Postgres extension declared via `CREATE EXTENSION`.
//!
//! Source IR can carry `schema = None` and `version = None` to mean
//! "any" — the differ treats source-None as "don't care". Catalog IR
//! always populates both fields.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, diff_field};

/// A Postgres extension.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Extension {
    /// Extension name (e.g. `pgcrypto`, `pg_trgm`).
    pub name: Identifier,
    /// Target schema. `None` = "use extension's default schema"
    /// (matches omitting `WITH SCHEMA` in source SQL).
    pub schema: Option<Identifier>,
    /// Pinned version. `None` = "any installed version is fine"
    /// (matches omitting `VERSION` in source SQL).
    pub version: Option<String>,
    /// Optional `COMMENT ON EXTENSION` text.
    pub comment: Option<String>,
}

impl Diff for Extension {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let Self {
            name: _,
            schema: _,
            version: _,
            comment: _,
        } = self;
        let mut out = Vec::new();
        out.extend(diff_field("name", &self.name, &other.name));
        out.extend(diff_field(
            "schema",
            &format!("{:?}", self.schema),
            &format!("{:?}", other.schema),
        ));
        out.extend(diff_field(
            "version",
            &format!("{:?}", self.version),
            &format!("{:?}", other.version),
        ));
        out.extend(diff_field(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::eq::Diff;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn ext(name: &str) -> Extension {
        Extension {
            name: id(name),
            schema: None,
            version: None,
            comment: None,
        }
    }

    #[test]
    fn identical_extensions_diff_empty() {
        let a = ext("pgcrypto");
        let b = ext("pgcrypto");
        assert!(a.canonical_eq(&b));
    }

    #[test]
    fn different_versions_diff_reports_version() {
        let a = ext("pgcrypto");
        let mut b = ext("pgcrypto");
        b.version = Some("1.4".into());
        let d = a.diff(&b);
        assert!(d.iter().any(|x| x.path == "version"));
    }

    #[test]
    fn different_schemas_diff_reports_schema() {
        let a = ext("pgcrypto");
        let mut b = ext("pgcrypto");
        b.schema = Some(id("app"));
        let d = a.diff(&b);
        assert!(d.iter().any(|x| x.path == "schema"));
    }
}
