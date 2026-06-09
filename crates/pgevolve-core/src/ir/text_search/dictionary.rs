//! `TEXT SEARCH DICTIONARY` IR — a schema-scoped object.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::difference::Difference;
use crate::ir::eq::{Equiv, field_difference};

/// A `CREATE TEXT SEARCH DICTIONARY` object. Identity is `qname`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TsDictionary {
    /// Schema-qualified dictionary name.
    pub qname: QualifiedName,
    /// Unmanaged template reference (e.g. `pg_catalog.snowball`).
    pub template: QualifiedName,
    /// Template options as ordered key/value pairs (e.g. `language='english'`).
    /// Canon sorts by key for stable comparison.
    pub options: Vec<(String, String)>,
    /// Lenient owner (`None` = unmanaged).
    pub owner: Option<Identifier>,
    /// Optional comment.
    pub comment: Option<String>,
}

impl Equiv for TsDictionary {
    fn differences(&self, other: &Self) -> Vec<Difference> {
        // Field-completeness guard: the compiler errors if a field is added
        // without being handled below. Bindings are unused (read via `self`).
        let Self {
            qname: _,
            template: _,
            options: _,
            owner: _,
            comment: _,
        } = self;
        let mut out = Vec::new();
        out.extend(field_difference("qname", &self.qname, &other.qname));
        out.extend(field_difference(
            "template",
            &self.template,
            &other.template,
        ));
        out.extend(field_difference(
            "options",
            &format!("{:?}", self.options),
            &format!("{:?}", other.options),
        ));
        out.extend(field_difference(
            "owner",
            &format!("{:?}", self.owner),
            &format!("{:?}", other.owner),
        ));
        out.extend(field_difference(
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

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn sample_dictionary() -> TsDictionary {
        TsDictionary {
            qname: qname("public", "english_stem"),
            template: qname("pg_catalog", "snowball"),
            options: vec![
                ("language".to_string(), "english".to_string()),
                ("stopwords".to_string(), "english".to_string()),
            ],
            owner: Some(id("app_owner")),
            comment: Some("English snowball stemmer dictionary.".to_string()),
        }
    }

    #[test]
    fn ts_dictionary_serde_round_trip() {
        let dict = sample_dictionary();
        let json = serde_json::to_string(&dict).unwrap();
        let back: TsDictionary = serde_json::from_str(&json).unwrap();
        assert_eq!(dict, back);
    }

    #[test]
    fn ts_dictionary_empty_options_round_trip() {
        let dict = TsDictionary {
            qname: qname("public", "simple_dict"),
            template: qname("pg_catalog", "simple"),
            options: vec![],
            owner: None,
            comment: None,
        };
        let json = serde_json::to_string(&dict).unwrap();
        let back: TsDictionary = serde_json::from_str(&json).unwrap();
        assert_eq!(dict, back);
    }
}
