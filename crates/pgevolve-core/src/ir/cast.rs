//! `CAST` IR — a global (non-schema-scoped) object. Identity is `(source, target)`.

use serde::{Deserialize, Serialize};

use crate::identifier::QualifiedName;
use crate::ir::column_type::ColumnType;

/// A `CREATE CAST` object. Identity is `(source, target)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cast {
    /// Source type (part of identity).
    pub source: QualifiedName,
    /// Target type (part of identity).
    pub target: QualifiedName,
    /// How the cast is performed.
    pub method: CastMethod,
    /// When the cast is applied implicitly.
    pub context: CastContext,
    /// Optional comment.
    pub comment: Option<String>,
}

/// The conversion mechanism for a `CREATE CAST`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CastMethod {
    /// `WITH FUNCTION fn(argtypes)` — a managed function.
    ///
    /// `arg_types` is the recorded conversion-function signature
    /// (1–3 args: source\[, int4, bool\]).
    Function {
        /// Schema-qualified function name.
        name: QualifiedName,
        /// Argument types of the conversion function.
        arg_types: Vec<ColumnType>,
    },
    /// `WITH INOUT` — uses the types' I/O functions.
    Inout,
    /// `WITHOUT FUNCTION` — binary-coercible; no function required.
    Binary,
}

/// Controls when Postgres automatically applies the cast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CastContext {
    /// Default — explicit casts only (`CAST(x AS t)` or `x::t`).
    Explicit,
    /// `AS ASSIGNMENT` — implicitly applied on assignment.
    Assignment,
    /// `AS IMPLICIT` — applied implicitly in expressions.
    Implicit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn cast_function() -> Cast {
        Cast {
            source: qname("app", "my_type"),
            target: qname("pg_catalog", "text"),
            method: CastMethod::Function {
                name: qname("app", "my_type_to_text"),
                arg_types: vec![ColumnType::Integer],
            },
            context: CastContext::Explicit,
            comment: Some("Converts my_type to text.".to_string()),
        }
    }

    fn cast_inout() -> Cast {
        Cast {
            source: qname("app", "domain_a"),
            target: qname("app", "domain_b"),
            method: CastMethod::Inout,
            context: CastContext::Assignment,
            comment: None,
        }
    }

    fn cast_binary() -> Cast {
        Cast {
            source: qname("app", "type_x"),
            target: qname("app", "type_y"),
            method: CastMethod::Binary,
            context: CastContext::Implicit,
            comment: None,
        }
    }

    #[test]
    fn cast_function_serde_round_trip() {
        let c = cast_function();
        let json = serde_json::to_string(&c).unwrap();
        let back: Cast = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn cast_inout_serde_round_trip() {
        let c = cast_inout();
        let json = serde_json::to_string(&c).unwrap();
        let back: Cast = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn cast_binary_serde_round_trip() {
        let c = cast_binary();
        let json = serde_json::to_string(&c).unwrap();
        let back: Cast = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn cast_context_serde_variants() {
        let explicit = serde_json::to_string(&CastContext::Explicit).unwrap();
        let assignment = serde_json::to_string(&CastContext::Assignment).unwrap();
        let implicit = serde_json::to_string(&CastContext::Implicit).unwrap();
        assert_eq!(explicit, "\"explicit\"");
        assert_eq!(assignment, "\"assignment\"");
        assert_eq!(implicit, "\"implicit\"");

        let back_e: CastContext = serde_json::from_str(&explicit).unwrap();
        let back_a: CastContext = serde_json::from_str(&assignment).unwrap();
        let back_i: CastContext = serde_json::from_str(&implicit).unwrap();
        assert_eq!(back_e, CastContext::Explicit);
        assert_eq!(back_a, CastContext::Assignment);
        assert_eq!(back_i, CastContext::Implicit);
    }
}
