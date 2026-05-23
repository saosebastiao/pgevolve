//! Canon rules for reloptions.
//!
//! `extra` is `BTreeMap<String, String>` (already key-ordered). This module
//! is currently a no-op pass-through, intentionally; it exists so future
//! normalization (lowercase keys, value trimming, etc.) has an obvious home.

use crate::ir::catalog::Catalog;

/// Canonicalize all reloption fields in the catalog. Currently a no-op.
pub const fn run(cat: &mut Catalog) {
    // Tables, indexes, MVs — each storage struct's `extra` is BTreeMap,
    // already ordered. Nothing to do today. If future PG quirks require
    // value-normalization (e.g., '1'/'true'/'on' canonicalization on bool
    // reloptions, or lowercasing extra-bag keys), add it here.
    let _ = cat;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_on_empty_catalog_is_no_op() {
        let mut c = Catalog::empty();
        run(&mut c);
        assert!(c.tables.is_empty());
    }

    #[test]
    fn run_is_idempotent() {
        let mut c = Catalog::empty();
        run(&mut c);
        let snap1 = format!("{c:?}");
        run(&mut c);
        let snap2 = format!("{c:?}");
        assert_eq!(snap1, snap2);
    }
}
