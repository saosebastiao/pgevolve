//! `TEXT SEARCH CONFIGURATION` IR — a schema-scoped object.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::difference::Difference;
use crate::ir::eq::{Equiv, field_difference};

/// A `CREATE TEXT SEARCH CONFIGURATION` object. Identity is `qname`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TsConfiguration {
    /// Schema-qualified configuration name.
    pub qname: QualifiedName,
    /// Unmanaged parser reference (e.g. `pg_catalog.default`).
    pub parser: QualifiedName,
    /// Token-type → ordered dictionary-chain mappings.
    pub mappings: Vec<TsMapping>,
    /// Lenient owner (`None` = unmanaged).
    pub owner: Option<Identifier>,
    /// Optional comment.
    pub comment: Option<String>,
}

impl Equiv for TsConfiguration {
    fn differences(&self, other: &Self) -> Vec<Difference> {
        // Field-completeness guard: the compiler errors if a field is added
        // without being handled below. Bindings are unused (read via `self`).
        let Self {
            qname: _,
            parser: _,
            mappings: _,
            owner: _,
            comment: _,
        } = self;
        let mut out = Vec::new();
        out.extend(field_difference("qname", &self.qname, &other.qname));
        out.extend(field_difference("parser", &self.parser, &other.parser));
        out.extend(field_difference(
            "mappings",
            &format!("{:?}", self.mappings),
            &format!("{:?}", other.mappings),
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

/// A single token-type → dictionary-chain mapping within a
/// `TEXT SEARCH CONFIGURATION`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TsMapping {
    /// Token-type alias name (e.g. `word`, `asciiword`, `numword`).
    pub token_type: String,
    /// Ordered dictionary fallback chain for this token type.
    pub dictionaries: Vec<QualifiedName>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> crate::identifier::Identifier {
        crate::identifier::Identifier::from_unquoted(s).unwrap()
    }

    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn sample_configuration() -> TsConfiguration {
        TsConfiguration {
            qname: qname("public", "english_config"),
            parser: qname("pg_catalog", "default"),
            mappings: vec![
                TsMapping {
                    token_type: "word".to_string(),
                    dictionaries: vec![
                        qname("public", "english_stem"),
                        qname("pg_catalog", "simple"),
                    ],
                },
                TsMapping {
                    token_type: "asciiword".to_string(),
                    dictionaries: vec![qname("public", "english_stem")],
                },
            ],
            owner: Some(id("app_owner")),
            comment: Some("English full-text search configuration.".to_string()),
        }
    }

    #[test]
    fn ts_configuration_serde_round_trip() {
        let cfg = sample_configuration();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: TsConfiguration = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn ts_mapping_serde_round_trip() {
        let mapping = TsMapping {
            token_type: "numword".to_string(),
            dictionaries: vec![qname("pg_catalog", "english_stem")],
        };
        let json = serde_json::to_string(&mapping).unwrap();
        let back: TsMapping = serde_json::from_str(&json).unwrap();
        assert_eq!(mapping, back);
    }

    #[test]
    fn ts_configuration_empty_mappings_round_trip() {
        let cfg = TsConfiguration {
            qname: qname("public", "minimal_config"),
            parser: qname("pg_catalog", "default"),
            mappings: vec![],
            owner: None,
            comment: None,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: TsConfiguration = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
