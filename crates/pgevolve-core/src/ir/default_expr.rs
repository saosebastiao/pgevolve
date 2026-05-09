//! `DefaultExpr` — column default expression.
//!
//! Three kinds:
//! - [`DefaultExpr::Literal`]: a typed literal (numbers, strings, bytea, NULL, etc.).
//! - [`DefaultExpr::Sequence`]: `nextval('seq')` references — recognized as desugared
//!   identity / `SERIAL` sources.
//! - [`DefaultExpr::Expr`]: any other expression, stored as a [`NormalizedExpr`].
//!
//! Real AST normalization (cast stripping, paren folding, commutative-operand sorting)
//! lands in phase 2 once `pg_query` is wired in. Phase 1 ships the structural types.

use serde::{Deserialize, Serialize};

use crate::identifier::QualifiedName;
use crate::ir::difference::Difference;
use crate::ir::eq::Diff;

/// A column-default expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DefaultExpr {
    /// A literal value.
    Literal(LiteralValue),
    /// A reference to a sequence (e.g., `nextval('app.seq1'::regclass)`).
    Sequence(QualifiedName),
    /// Any other expression.
    Expr(NormalizedExpr),
}

/// A typed literal default value.
///
/// Note: `Float(f64)` precludes deriving `Eq` and `Hash` on this type;
/// equality is `PartialEq` only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiteralValue {
    /// Boolean literal.
    Bool(bool),
    /// Integer literal (covers smallint/int/bigint).
    Integer(i64),
    /// Floating-point literal.
    Float(f64),
    /// Text-like literal (text, varchar, char, etc.).
    Text(String),
    /// Bytea literal.
    Bytea(Vec<u8>),
    /// SQL `NULL`.
    Null,
}

/// A normalized SQL expression — its canonical text plus a hash of the canonical AST.
///
/// The `ast_hash` is a BLAKE3 hash of `canonical_text` (the canonical bytes after
/// keyword lowercasing, paren folding, cast stripping, and commutative-operand
/// sorting). Two expressions hash-equal iff their canonical texts match exactly,
/// so equivalence is decided byte-wise on the canonical form.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NormalizedExpr {
    /// Canonical textual form (lowercased keywords, sorted commutative operands,
    /// stripped redundant casts).
    pub canonical_text: String,
    /// BLAKE3 hash of `canonical_text` as bytes.
    pub ast_hash: [u8; 32],
}

impl NormalizedExpr {
    /// Construct from already-canonical text. The hash is computed from the text.
    ///
    /// This constructor does NOT run any normalization passes — it assumes the
    /// caller has already produced canonical form. Source-side construction goes
    /// through `pgevolve_core::parse::normalize_expr::from_pg_node` which deparses
    /// and normalizes a `pg_query` node.
    pub fn from_text(canonical_text: impl Into<String>) -> Self {
        let canonical_text = canonical_text.into();
        let ast_hash = blake3::hash(canonical_text.as_bytes()).into();
        Self {
            canonical_text,
            ast_hash,
        }
    }

    /// Alias for [`Self::from_text`], matching the phase-2 plan vocabulary.
    pub fn from_canonical_text(text: impl Into<String>) -> Self {
        Self::from_text(text)
    }
}

impl Diff for DefaultExpr {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        if self == other {
            Vec::new()
        } else {
            vec![Difference::new("", display(self), display(other))]
        }
    }
}

fn display(d: &DefaultExpr) -> String {
    match d {
        DefaultExpr::Literal(LiteralValue::Bool(b)) => b.to_string(),
        DefaultExpr::Literal(LiteralValue::Integer(i)) => i.to_string(),
        DefaultExpr::Literal(LiteralValue::Float(f)) => f.to_string(),
        DefaultExpr::Literal(LiteralValue::Text(t)) => format!("'{}'", t.replace('\'', "''")),
        DefaultExpr::Literal(LiteralValue::Bytea(b)) => format!("\\x{}", hex(b)),
        DefaultExpr::Literal(LiteralValue::Null) => "NULL".into(),
        DefaultExpr::Sequence(q) => format!("nextval('{}')", q.render_sql()),
        DefaultExpr::Expr(e) => e.canonical_text.clone(),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;

    #[test]
    fn equal_text_literals_canonical_eq() {
        let a = DefaultExpr::Literal(LiteralValue::Text("foo".into()));
        let b = DefaultExpr::Literal(LiteralValue::Text("foo".into()));
        assert!(a.canonical_eq(&b));
    }

    #[test]
    fn different_text_literals_diff() {
        let a = DefaultExpr::Literal(LiteralValue::Text("foo".into()));
        let b = DefaultExpr::Literal(LiteralValue::Text("bar".into()));
        let d = a.diff(&b);
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn sequence_differs_from_literal() {
        let q = QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted("seq1").unwrap(),
        );
        let a = DefaultExpr::Sequence(q);
        let b = DefaultExpr::Literal(LiteralValue::Integer(1));
        assert!(!a.canonical_eq(&b));
    }

    #[test]
    fn integer_and_text_literals_distinct() {
        let a = DefaultExpr::Literal(LiteralValue::Integer(1));
        let b = DefaultExpr::Literal(LiteralValue::Text("1".into()));
        assert!(!a.canonical_eq(&b));
    }

    #[test]
    fn null_literals_equal() {
        let a = DefaultExpr::Literal(LiteralValue::Null);
        let b = DefaultExpr::Literal(LiteralValue::Null);
        assert!(a.canonical_eq(&b));
    }

    #[test]
    fn normalized_expr_round_trips() {
        let e = NormalizedExpr::from_text("now()");
        let json = serde_json::to_string(&e).unwrap();
        let back: NormalizedExpr = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}
