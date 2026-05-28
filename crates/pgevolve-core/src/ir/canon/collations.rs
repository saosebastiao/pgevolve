//! Canon pass for the Catalog's `collations` field.
//!
//! - Sorts by qname for byte-stable comparison.
//! - Rejects `nondeterministic + Libc` combinations with a clear error
//!   (PG would reject at runtime; we surface it at canon time with a
//!   targeted error).

use crate::ir::IrError;
use crate::ir::catalog::Catalog;
use crate::ir::collation::CollationProvider;

/// Sort `cat.collations` by qname and reject invalid provider / deterministic
/// combinations.
pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    cat.collations.sort_by(|a, b| a.qname.cmp(&b.qname));
    for c in &cat.collations {
        if !c.deterministic && c.provider == CollationProvider::Libc {
            return Err(IrError::InvalidCollation {
                qname: c.qname.clone(),
                reason: "nondeterministic = false is only valid with provider = icu".into(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::collation::Collation;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(id(s), id(n))
    }

    fn make_libc(qname: QualifiedName, deterministic: bool) -> Collation {
        Collation {
            qname,
            provider: CollationProvider::Libc,
            lc_collate: "en_US.utf8".into(),
            lc_ctype: "en_US.utf8".into(),
            deterministic,
            version: None,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn sorts_by_qname() {
        let mut cat = Catalog::empty();
        cat.collations.push(make_libc(qn("app", "z"), true));
        cat.collations.push(make_libc(qn("app", "a"), true));
        run(&mut cat).unwrap();
        assert_eq!(cat.collations[0].qname.name.as_str(), "a");
    }

    #[test]
    fn rejects_libc_nondeterministic() {
        let mut cat = Catalog::empty();
        cat.collations.push(make_libc(qn("app", "bad"), false));
        let err = run(&mut cat).unwrap_err();
        assert!(matches!(err, IrError::InvalidCollation { .. }));
    }
}
