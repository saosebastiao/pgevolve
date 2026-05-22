//! Identifier and qualified-name types.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A single Postgres identifier (e.g., a table name).
///
/// Postgres identifier rules:
/// - Length: 1..=63 bytes (NAMEDATALEN).
/// - Unquoted: starts with `[A-Za-z_]` followed by `[A-Za-z0-9_$]*`.
/// - Quoted: any UTF-8 except `"` (we accept any non-empty UTF-8 here; `pg_query`
///   will reject anything postgres can't actually accept at parse time).
///
/// We store identifiers in their *case-folded canonical form* for unquoted
/// inputs (Postgres lowercases unquoted identifiers) and in their original
/// form for quoted inputs. The constructor distinguishes the two cases via
/// [`Identifier::from_unquoted`] vs [`Identifier::from_quoted`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Identifier(String);

/// Errors raised when constructing an [`Identifier`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdentifierError {
    /// The identifier was empty.
    #[error("identifier is empty")]
    Empty,
    /// The identifier exceeded Postgres's 63-byte limit.
    #[error("identifier exceeds 63 bytes: got {0}")]
    TooLong(usize),
    /// The unquoted identifier contained invalid characters.
    #[error("unquoted identifier contains invalid characters: {0:?}")]
    InvalidUnquotedChars(String),
}

impl Identifier {
    /// Construct from an unquoted identifier source — lowercases per Postgres rules.
    pub fn from_unquoted(s: &str) -> Result<Self, IdentifierError> {
        if s.is_empty() {
            return Err(IdentifierError::Empty);
        }
        if s.len() > 63 {
            return Err(IdentifierError::TooLong(s.len()));
        }
        let mut chars = s.chars();
        // SAFETY: s is non-empty — the empty check above ensures chars.next() yields Some.
        let Some(first) = chars.next() else {
            unreachable!("non-empty string has at least one char")
        };
        if !(first.is_ascii_alphabetic() || first == '_') {
            return Err(IdentifierError::InvalidUnquotedChars(s.to_string()));
        }
        for c in chars {
            if !(c.is_ascii_alphanumeric() || c == '_' || c == '$') {
                return Err(IdentifierError::InvalidUnquotedChars(s.to_string()));
            }
        }
        Ok(Self(s.to_ascii_lowercase()))
    }

    /// Construct from a quoted identifier — preserves case.
    pub fn from_quoted(s: &str) -> Result<Self, IdentifierError> {
        if s.is_empty() {
            return Err(IdentifierError::Empty);
        }
        if s.len() > 63 {
            return Err(IdentifierError::TooLong(s.len()));
        }
        Ok(Self(s.to_string()))
    }

    /// Returns the inner canonical string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Renders this identifier as it would appear in SQL — quoted iff necessary.
    pub fn render_sql(&self) -> String {
        if needs_quoting(&self.0) {
            format!("\"{}\"", self.0.replace('"', "\"\""))
        } else {
            self.0.clone()
        }
    }
}

fn needs_quoting(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    if RESERVED_KEYWORDS.binary_search(&s).is_ok() {
        return true;
    }
    let mut chars = s.chars();
    // SAFETY: s is non-empty — the `is_empty()` guard above returns early.
    let Some(first) = chars.next() else {
        unreachable!("non-empty string has at least one char")
    };
    if !(first.is_ascii_lowercase() || first == '_') {
        return true;
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return true;
        }
    }
    false
}

// Sorted; binary searched. Source: Postgres docs (reserved + reserved-non-function-or-type).
// This list is intentionally conservative — it errs on the side of quoting.
static RESERVED_KEYWORDS: &[&str] = &[
    "all",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "asymmetric",
    "both",
    "case",
    "cast",
    "check",
    "collate",
    "column",
    "constraint",
    "create",
    "current_catalog",
    "current_date",
    "current_role",
    "current_time",
    "current_timestamp",
    "current_user",
    "default",
    "deferrable",
    "desc",
    "distinct",
    "do",
    "else",
    "end",
    "except",
    "false",
    "fetch",
    "for",
    "foreign",
    "from",
    "grant",
    "group",
    "having",
    "in",
    "initially",
    "intersect",
    "into",
    "lateral",
    "leading",
    "limit",
    "localtime",
    "localtimestamp",
    "not",
    "null",
    "offset",
    "on",
    "only",
    "or",
    "order",
    "placing",
    "primary",
    "references",
    "returning",
    "select",
    "session_user",
    "some",
    "symmetric",
    "table",
    "then",
    "to",
    "trailing",
    "true",
    "union",
    "unique",
    "user",
    "using",
    "variadic",
    "when",
    "where",
    "window",
    "with",
];

impl fmt::Display for Identifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Identifier {
    type Err = IdentifierError;
    /// Parses as an unquoted identifier.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_unquoted(s)
    }
}

/// A schema-qualified identifier — e.g., `app.users` or `"AppSchema".users`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct QualifiedName {
    /// The schema component.
    pub schema: Identifier,
    /// The object name.
    pub name: Identifier,
}

impl QualifiedName {
    /// Construct from two identifiers.
    pub const fn new(schema: Identifier, name: Identifier) -> Self {
        Self { schema, name }
    }

    /// Renders as it would appear in SQL.
    pub fn render_sql(&self) -> String {
        format!("{}.{}", self.schema.render_sql(), self.name.render_sql())
    }
}

impl fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.schema, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn unquoted_lowercases() {
        let id = Identifier::from_unquoted("Users").unwrap();
        assert_eq!(id.as_str(), "users");
    }

    #[test]
    fn quoted_preserves_case() {
        let id = Identifier::from_quoted("Users").unwrap();
        assert_eq!(id.as_str(), "Users");
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(Identifier::from_unquoted(""), Err(IdentifierError::Empty));
        assert_eq!(Identifier::from_quoted(""), Err(IdentifierError::Empty));
    }

    #[test]
    fn rejects_overlong() {
        let long = "a".repeat(64);
        assert!(matches!(
            Identifier::from_unquoted(&long),
            Err(IdentifierError::TooLong(64))
        ));
    }

    #[test]
    fn rejects_unquoted_starting_with_digit() {
        assert!(matches!(
            Identifier::from_unquoted("1foo"),
            Err(IdentifierError::InvalidUnquotedChars(_))
        ));
    }

    #[test]
    fn quoted_allows_special_chars() {
        let id = Identifier::from_quoted("foo bar").unwrap();
        assert_eq!(id.as_str(), "foo bar");
    }

    #[test]
    fn render_sql_quotes_when_necessary() {
        assert_eq!(
            Identifier::from_unquoted("users").unwrap().render_sql(),
            "users"
        );
        assert_eq!(
            Identifier::from_quoted("Users").unwrap().render_sql(),
            "\"Users\""
        );
        assert_eq!(
            Identifier::from_quoted("select").unwrap().render_sql(),
            "\"select\""
        );
    }

    #[test]
    fn render_sql_escapes_embedded_quotes() {
        let id = Identifier::from_quoted("a\"b").unwrap();
        assert_eq!(id.render_sql(), "\"a\"\"b\"");
    }

    #[test]
    fn qualified_name_renders() {
        let qn = QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted("users").unwrap(),
        );
        assert_eq!(qn.render_sql(), "app.users");
        assert_eq!(qn.to_string(), "app.users");
    }

    #[test]
    fn from_str_uses_unquoted_rules() {
        let id: Identifier = "Foo".parse().unwrap();
        assert_eq!(id.as_str(), "foo");
    }
}
