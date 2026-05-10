//! Tier-3 snapshot helpers.
//!
//! Serializes a [`Catalog`] to canonical JSON for golden comparison and
//! provides helpers used by both the round-trip test and the
//! `cargo xtask bless` command.

use anyhow::Context;
use pgevolve_core::ir::catalog::Catalog;

/// Serialize a [`Catalog`] to pretty-printed JSON. Keys are sorted by serde's
/// default ordering for deterministic golden files.
pub fn to_canonical_json(catalog: &Catalog) -> anyhow::Result<String> {
    let mut buf = serde_json::to_string_pretty(catalog)
        .with_context(|| "failed to serialize catalog to JSON")?;
    if !buf.ends_with('\n') {
        buf.push('\n');
    }
    Ok(buf)
}

/// Parse a canonical JSON snapshot back into a [`Catalog`].
pub fn from_canonical_json(s: &str) -> anyhow::Result<Catalog> {
    serde_json::from_str(s).with_context(|| "failed to parse catalog snapshot")
}
