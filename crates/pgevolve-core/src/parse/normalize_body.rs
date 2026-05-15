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

/// A canonicalized statement-scope body.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// `pg_query`'s deparser failed on a successfully-parsed input.
    #[error("pg_query deparser failed: {0}")]
    Deparse(String),
}

impl NormalizedBody {
    /// Canonicalize a body given its raw SQL text.
    ///
    /// The body may be any complete SQL statement (`SELECT`, `CREATE VIEW`,
    /// etc.). Invalid SQL returns [`BodyError::Parse`].
    pub fn from_sql(sql: &str) -> Result<Self, BodyError> {
        let parsed =
            pg_query::parse(sql).map_err(|e| BodyError::Parse(e.to_string()))?;
        let deparsed = parsed
            .deparse()
            .map_err(|e| BodyError::Deparse(e.to_string()))?;
        let canonical_text = collapse_whitespace(&deparsed);
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
    pub const fn canonical_hash(&self) -> &[u8; 32] {
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
