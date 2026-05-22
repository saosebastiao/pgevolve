//! Build [`NormalizedExpr`] values from `pg_query` AST nodes.
//!
//! Phase-2 scope (per `docs/superpowers/plans/phase-2-parser.md`):
//!
//! - Strip redundant casts to a column's own type. `42::integer` for an integer
//!   column collapses to `42`; `'foo'::text` for a text column collapses to `'foo'`.
//! - Lowercase reserved keywords on the canonical text emitted by `pg_query`'s deparser.
//! - Compute the BLAKE3 hash of the canonical text.
//!
//! Deferred to follow-up phase-2 issues (only affect equivalence sensitivity, not
//! correctness — equivalent inputs that exercise these may diff for now):
//!
//! - Paren folding (collapsing trivial nested `A_Expr` parens).
//! - Sorting commutative operands of `+`, `*`, `AND`, `OR`.

use pg_query::NodeEnum;
use pg_query::protobuf::{self, Node, ResTarget};

use crate::ir::column_type::ColumnType;
use crate::ir::default_expr::NormalizedExpr;
use crate::parse::error::{ParseError, SourceLocation};

/// Build a [`NormalizedExpr`] from a `pg_query` expression node.
///
/// `target_type`, when supplied, enables redundant-cast stripping: a `TypeCast`
/// to that type is replaced by its inner expression.
pub fn from_pg_node(
    node: &NodeEnum,
    target_type: Option<&ColumnType>,
    location: &SourceLocation,
) -> Result<NormalizedExpr, ParseError> {
    let normalized = strip_redundant_cast(node.clone(), target_type);
    let raw = deparse_expr(&normalized).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("could not deparse expression: {e}"),
    })?;
    let canonical_text = lowercase_keywords(&raw);
    Ok(NormalizedExpr::from_text(canonical_text))
}

/// If `node` is a `TypeCast` whose target type matches `target_type`, return
/// the inner argument; otherwise return `node` unchanged.
fn strip_redundant_cast(node: NodeEnum, target_type: Option<&ColumnType>) -> NodeEnum {
    let Some(target) = target_type else {
        return node;
    };
    let NodeEnum::TypeCast(cast) = &node else {
        return node;
    };
    let Some(type_name) = cast.type_name.as_ref() else {
        return node;
    };
    let Some(type_str) = render_type_name(type_name) else {
        return node;
    };
    let Ok(parsed) = ColumnType::parse_from_pg_type_string(&type_str) else {
        return node;
    };
    if &parsed != target {
        return node;
    }
    // Strip: replace the cast with its inner argument.
    cast.arg
        .as_ref()
        .and_then(|inner| inner.node.as_ref())
        .cloned()
        .unwrap_or(node)
}

/// Walk a `TypeName`'s `names` list and join the string fragments with dots.
/// Returns `None` if any element is not a `String` node.
fn render_type_name(type_name: &protobuf::TypeName) -> Option<String> {
    let mut parts = Vec::with_capacity(type_name.names.len());
    for n in &type_name.names {
        let Some(NodeEnum::String(s)) = &n.node else {
            return None;
        };
        parts.push(s.sval.clone());
    }
    if parts.is_empty() {
        return None;
    }
    // pg_query stores types like `pg_catalog.int4`; we only care about the last
    // segment for matching against [`ColumnType::parse_from_pg_type_string`],
    // since that already understands aliases like `int4` → `Integer`.
    Some(parts.last().cloned().unwrap_or_default())
}

/// Wrap an expression in `SELECT <expr>` and deparse, then strip the `SELECT `
/// prefix. This is the workaround for `pg_query`'s deparse expecting a top-level
/// statement node — there is no public `deparse_expr` entry point in v6.
///
/// The `protobuf::ParseResult.version` field must match `libpg_query`'s
/// embedded `PG_VERSION_NUM`, otherwise the C deparser asserts and aborts the
/// process. We borrow that version from a freshly-parsed `SELECT 1` rather than
/// hard-coding it.
fn deparse_expr(node: &NodeEnum) -> Result<String, pg_query::Error> {
    let mut scaffold = pg_query::parse("SELECT 1")?.protobuf;
    let raw = scaffold
        .stmts
        .get_mut(0)
        .ok_or_else(|| pg_query::Error::Parse("scaffold has no stmts".into()))?;
    let select_node = raw
        .stmt
        .as_mut()
        .ok_or_else(|| pg_query::Error::Parse("scaffold stmt is None".into()))?
        .node
        .as_mut()
        .ok_or_else(|| pg_query::Error::Parse("scaffold node is None".into()))?;
    let NodeEnum::SelectStmt(select) = select_node else {
        return Err(pg_query::Error::Parse(
            "scaffold parse did not yield SelectStmt".into(),
        ));
    };
    select.target_list = vec![Node {
        node: Some(NodeEnum::ResTarget(Box::new(ResTarget {
            name: String::new(),
            indirection: vec![],
            val: Some(Box::new(Node {
                node: Some(node.clone()),
            })),
            location: -1,
        }))),
    }];
    let s = pg_query::deparse(&scaffold)?;
    Ok(s.trim_start_matches("SELECT ").to_string())
}

/// Reserved Postgres keywords that should appear lowercased in canonical text.
/// We deliberately keep this small — `pg_query`'s deparser already emits most
/// keywords lowercased; this list is a belt-and-suspenders pass for any node
/// kinds where the deparser preserves the source's casing.
const RESERVED_FUNC_KEYWORDS: &[&str] = &[
    "AND", "OR", "NOT", "NULL", "TRUE", "FALSE", "IS", "IN", "LIKE", "BETWEEN", "CASE", "WHEN",
    "THEN", "ELSE", "END", "CAST", "AS", "DISTINCT", "FROM", "WHERE", "ORDER", "BY", "GROUP",
    "HAVING", "LIMIT", "OFFSET", "ASC", "DESC", "NULLS", "FIRST", "LAST", "WITH", "USING",
    "COLLATE",
];

/// Lowercase whole-word reserved keywords in `s`. Quoted-string contents are not
/// modified.
fn lowercase_keywords(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            // Pass through quoted string bodies verbatim, including doubled `''`.
            out.push(c);
            while let Some(&n) = chars.peek() {
                chars.next();
                out.push(n);
                if n == '\'' {
                    if chars.peek() == Some(&'\'') {
                        // SAFETY: peek() returned Some, so next() yields the same char.
                        if let Some(escaped) = chars.next() {
                            out.push(escaped);
                        }
                        continue;
                    }
                    break;
                }
            }
            continue;
        }
        if c == '"' {
            out.push(c);
            for n in chars.by_ref() {
                out.push(n);
                if n == '"' {
                    break;
                }
            }
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let mut word = String::from(c);
            while let Some(&n) = chars.peek() {
                if n.is_ascii_alphanumeric() || n == '_' {
                    word.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            let upper = word.to_ascii_uppercase();
            if RESERVED_FUNC_KEYWORDS.contains(&upper.as_str()) {
                out.push_str(&word.to_ascii_lowercase());
            } else {
                out.push_str(&word);
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    /// Parse the *value* expression of `SELECT <sql>` and return its `NodeEnum`.
    fn parse_expr(sql: &str) -> NodeEnum {
        let full = format!("SELECT {sql}");
        let parsed = pg_query::parse(&full).expect("expression parses");
        let stmt = parsed.protobuf.stmts.into_iter().next().expect("one stmt");
        let select = stmt.stmt.expect("stmt").node.expect("node");
        let NodeEnum::SelectStmt(s) = select else {
            panic!("expected SelectStmt, got {select:?}");
        };
        let target = s
            .target_list
            .into_iter()
            .next()
            .expect("target")
            .node
            .expect("target node");
        let NodeEnum::ResTarget(rt) = target else {
            panic!("expected ResTarget");
        };
        rt.val
            .expect("val")
            .node
            .expect("inner node of ResTarget val")
    }

    #[test]
    fn cast_to_target_integer_strips() {
        let node = parse_expr("42 :: integer");
        let n = from_pg_node(&node, Some(&ColumnType::Integer), &loc()).expect("normalizes");
        assert_eq!(n.canonical_text, "42");
    }

    #[test]
    fn cast_to_target_text_strips() {
        let node = parse_expr("'foo' :: text");
        let n = from_pg_node(&node, Some(&ColumnType::Text), &loc()).expect("normalizes");
        assert_eq!(n.canonical_text, "'foo'");
    }

    #[test]
    fn cast_to_other_type_kept() {
        let node = parse_expr("42 :: integer");
        let n = from_pg_node(&node, Some(&ColumnType::Text), &loc()).expect("normalizes");
        // The cast survives because the target type does not match.
        assert!(
            n.canonical_text.contains("integer") || n.canonical_text.contains("::"),
            "got: {}",
            n.canonical_text
        );
    }

    #[test]
    fn cast_with_no_target_kept() {
        let node = parse_expr("42 :: integer");
        let n = from_pg_node(&node, None, &loc()).expect("normalizes");
        // Without a target type, no cast stripping happens.
        assert!(
            n.canonical_text.contains("integer") || n.canonical_text.contains("::"),
            "got: {}",
            n.canonical_text
        );
    }

    #[test]
    fn keywords_lowercased() {
        // pg_query already lowercases most keywords; this asserts the canonical
        // text has no uppercase reserved-word artifacts.
        let node = parse_expr("LOWER('FOO')");
        let n = from_pg_node(&node, None, &loc()).expect("normalizes");
        assert!(
            !n.canonical_text.contains("AS") && !n.canonical_text.contains("DISTINCT"),
            "got: {}",
            n.canonical_text
        );
    }

    #[test]
    fn lowercase_helper_preserves_quoted_strings() {
        assert_eq!(lowercase_keywords("'AND OR'"), "'AND OR'");
        assert_eq!(lowercase_keywords("foo 'AND' OR"), "foo 'AND' or");
        assert_eq!(lowercase_keywords("AND OR"), "and or");
    }

    #[test]
    fn lowercase_helper_handles_doubled_quote() {
        assert_eq!(lowercase_keywords("'a''b'"), "'a''b'");
    }

    #[test]
    fn lowercase_helper_preserves_quoted_identifiers() {
        assert_eq!(lowercase_keywords("\"AND\""), "\"AND\"");
    }

    #[test]
    fn hash_matches_text() {
        let n = NormalizedExpr::from_text("now()");
        let expected: [u8; 32] = blake3::hash(b"now()").into();
        assert_eq!(n.ast_hash, expected);
    }

    #[test]
    fn equivalent_casts_hash_equal() {
        let a = parse_expr("42 :: integer");
        let na = from_pg_node(&a, Some(&ColumnType::Integer), &loc()).unwrap();
        let nb = NormalizedExpr::from_text("42");
        assert_eq!(na.ast_hash, nb.ast_hash);
    }
}
