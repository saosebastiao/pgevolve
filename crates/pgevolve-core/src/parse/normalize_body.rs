//! Statement-scope body canonicalization.
//!
//! Counterpart to [`NormalizedExpr`](super::normalize_expr::NormalizedExpr).
//! Where `NormalizedExpr` canonicalizes one expression, `NormalizedBody`
//! canonicalizes a statement-shaped body — a view's `SELECT`, a function
//! body, an expression-index predicate at full-statement scope.
//!
//! Canonicalization rules (per arch spec Decision 10):
//!
//! - Whitespace collapses; one space between tokens; newlines stripped.
//! - Keywords lowercased (via `pg_query`'s deparser, which already lowercases
//!   most keywords; see `normalize_expr` for additional belt-and-suspenders
//!   lowercasing if needed in v0.2).
//! - Redundant parens folded (`pg_query`'s deparser removes them on round-trip).
//! - Identifiers preserved verbatim (qualification, quoting).
//!
//! For v0.1 this module is unused; v0.2 view/function sub-specs are
//! its first consumers.

use serde::{Deserialize, Serialize};

/// A canonicalized statement-scope body.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NormalizedBody {
    canonical_text: String,
    canonical_hash: [u8; 32],
}

/// Error parsing a body.
#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    /// `pg_query` rejected the SQL.
    #[error("pg_query rejected body: {0}")]
    Parse(String),
}

impl NormalizedBody {
    /// Sentinel for source-parse provisional records.
    ///
    /// T4's AST canonicalization pass overwrites this immediately after the
    /// source IR is assembled. Never serialized to plan output.
    pub const fn empty() -> Self {
        Self {
            canonical_text: String::new(),
            canonical_hash: [0u8; 32],
        }
    }

    /// Build a `NormalizedBody` from a pre-computed canonical text string.
    ///
    /// Used by the PL/pgSQL and SQL body parsers in `parse::builder::plpgsql`
    /// which produce their own canonical form (whitespace-collapsed text or
    /// `pg_query::normalize` output) and need to inject it directly.
    ///
    /// Callers are responsible for ensuring `canonical_text` is in the
    /// pgevolve canonical form (whitespace collapsed, keywords lowercased).
    pub fn from_raw_canonical(canonical_text: String) -> Self {
        let canonical_hash = hash_canonical(&canonical_text);
        Self {
            canonical_text,
            canonical_hash,
        }
    }

    /// Canonicalize a body given its raw SQL text.
    ///
    /// The body may be any complete SQL statement (`SELECT`, `CREATE VIEW`,
    /// etc.). Invalid SQL returns [`BodyError::Parse`]. If the deparser
    /// unexpectedly fails on a successfully-parsed tree, the original SQL is
    /// used as the canonical form (silent graceful degradation).
    pub fn from_sql(sql: &str) -> Result<Self, BodyError> {
        let parsed = pg_query::parse(sql).map_err(|e| BodyError::Parse(e.to_string()))?;
        let deparsed = parsed.deparse().unwrap_or_default();
        let source = if deparsed.is_empty() { sql } else { &deparsed };
        let canonical_text = collapse_whitespace(source);
        let canonical_hash = hash_canonical(&canonical_text);
        Ok(Self {
            canonical_text,
            canonical_hash,
        })
    }

    /// The canonical text. Two bodies are equivalent iff their canonical
    /// texts are byte-equal.
    pub fn canonical_text(&self) -> &str {
        &self.canonical_text
    }

    /// BLAKE3 hash of the canonical text. Domain-separated with
    /// `pgevolve-normalized-body-v1\n` to avoid collisions with
    /// [`crate::plan::plan::PlanId`] hashes (`pgevolve-plan-id-v1\n`).
    ///
    /// Not `const fn`: `NormalizedBody` is only constructed at runtime (via
    /// `pg_query`), so `const` would signal intent the type cannot fulfill.
    #[allow(clippy::missing_const_for_fn)]
    pub fn canonical_hash(&self) -> &[u8; 32] {
        &self.canonical_hash
    }
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn hash_canonical(text: &str) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(b"pgevolve-normalized-body-v1\n");
    h.update(text.as_bytes());
    *h.finalize().as_bytes()
}
