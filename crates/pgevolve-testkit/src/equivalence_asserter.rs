//! `assert_canonical_eq` — pretty-prints IR diffs on failure.
//!
//! Wraps the core `Equiv` trait so test failures show the offending paths and
//! before/after values rather than a wall of `Debug` text.

use anyhow::{Result, anyhow};

use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::difference::Difference;
use pgevolve_core::ir::eq::Equiv;

/// Assert two catalogs are structurally equal (per `Catalog::diff`).
///
/// On failure, returns an `anyhow::Error` whose `Display` contains an
/// indented list of every difference.
pub fn assert_canonical_eq(a: &Catalog, b: &Catalog) -> Result<()> {
    let diffs = a.differences(b);
    if diffs.is_empty() {
        return Ok(());
    }
    Err(anyhow!("catalogs differ:\n{}", render_diffs(&diffs)))
}

fn render_diffs(diffs: &[Difference]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for d in diffs {
        let _ = writeln!(
            out,
            "  - {path}: `{from}` vs `{to}`",
            path = d.path,
            from = d.from,
            to = d.to,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgevolve_core::identifier::Identifier;
    use pgevolve_core::ir::schema::Schema;

    #[test]
    fn ok_when_catalogs_match() {
        let a = Catalog::empty();
        let b = Catalog::empty();
        assert!(assert_canonical_eq(&a, &b).is_ok());
    }

    #[test]
    fn err_includes_offending_paths() {
        let mut a = Catalog::empty();
        a.schemas
            .push(Schema::new(Identifier::from_unquoted("app").unwrap()));
        let b = Catalog::empty();
        let err = assert_canonical_eq(&a, &b).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("catalogs differ"));
        assert!(msg.contains("schemas"));
    }
}
