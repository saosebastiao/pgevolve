//! `Schema` — a Postgres namespace.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::eq::DiffMacro;

/// A Postgres schema (namespace).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, DiffMacro)]
pub struct Schema {
    /// Schema name.
    pub name: Identifier,
    /// Optional comment.
    #[diff(via_debug)]
    pub comment: Option<String>,
}

impl Schema {
    /// Construct a `Schema`.
    pub const fn new(name: Identifier) -> Self {
        Self {
            name,
            comment: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::eq::Diff;

    #[test]
    fn equal_schemas_have_no_diff() {
        let a = Schema::new(Identifier::from_unquoted("app").unwrap());
        let b = Schema::new(Identifier::from_unquoted("app").unwrap());
        assert!(a.canonical_eq(&b));
    }

    #[test]
    fn different_names_diff() {
        let a = Schema::new(Identifier::from_unquoted("app").unwrap());
        let b = Schema::new(Identifier::from_unquoted("billing").unwrap());
        let d = a.diff(&b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "name");
    }

    #[test]
    fn comment_diffs() {
        let a = Schema {
            name: Identifier::from_unquoted("app").unwrap(),
            comment: Some("v1".into()),
        };
        let b = Schema {
            name: Identifier::from_unquoted("app").unwrap(),
            comment: Some("v2".into()),
        };
        let d = a.diff(&b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "comment");
    }
}
