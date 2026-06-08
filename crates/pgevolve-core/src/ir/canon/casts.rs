//! Canon for `Catalog::casts`: sort by `(source, target)` (rendered) and
//! reject duplicate `(source, target)` identities.

use crate::ir::IrError;
use crate::ir::catalog::Catalog;

/// Canonicalize all casts in `cat`.
///
/// - The collection is sorted by `(source.render_sql(), target.render_sql())`.
/// - A duplicate `(source, target)` identity raises [`IrError::DuplicateCast`].
pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    cat.casts.sort_by(|a, b| {
        a.source
            .render_sql()
            .cmp(&b.source.render_sql())
            .then_with(|| a.target.render_sql().cmp(&b.target.render_sql()))
    });
    for w in cat.casts.windows(2) {
        if w[0].source == w[1].source && w[0].target == w[1].target {
            return Err(IrError::DuplicateCast {
                src: w[0].source.clone(),
                tgt: w[0].target.clone(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::cast::{Cast, CastContext, CastMethod};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn simple_cast(src_schema: &str, src_name: &str, tgt_schema: &str, tgt_name: &str) -> Cast {
        Cast {
            source: qname(src_schema, src_name),
            target: qname(tgt_schema, tgt_name),
            method: CastMethod::Binary,
            context: CastContext::Explicit,
            comment: None,
        }
    }

    #[test]
    fn sorts_by_source_then_target() {
        let mut cat = Catalog::empty();
        // Insert in reverse order: (app.zzz → app.text) before (app.aaa → app.text)
        cat.casts
            .push(simple_cast("app", "zzz_type", "pg_catalog", "text"));
        cat.casts
            .push(simple_cast("app", "aaa_type", "pg_catalog", "text"));
        run(&mut cat).unwrap();
        assert_eq!(cat.casts[0].source.render_sql(), "app.aaa_type");
        assert_eq!(cat.casts[1].source.render_sql(), "app.zzz_type");
    }

    #[test]
    fn sorts_by_target_when_source_equal() {
        let mut cat = Catalog::empty();
        cat.casts
            .push(simple_cast("app", "my_type", "pg_catalog", "varchar"));
        cat.casts
            .push(simple_cast("app", "my_type", "pg_catalog", "int4"));
        run(&mut cat).unwrap();
        assert_eq!(cat.casts[0].target.render_sql(), "pg_catalog.int4");
        assert_eq!(cat.casts[1].target.render_sql(), "pg_catalog.varchar");
    }

    #[test]
    fn rejects_duplicate_identity() {
        let mut cat = Catalog::empty();
        cat.casts
            .push(simple_cast("app", "my_type", "pg_catalog", "text"));
        cat.casts
            .push(simple_cast("app", "my_type", "pg_catalog", "text"));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::DuplicateCast { .. }
        ));
    }

    #[test]
    fn allows_same_source_different_targets() {
        let mut cat = Catalog::empty();
        cat.casts
            .push(simple_cast("app", "my_type", "pg_catalog", "text"));
        cat.casts
            .push(simple_cast("app", "my_type", "pg_catalog", "int4"));
        run(&mut cat).unwrap();
        assert_eq!(cat.casts.len(), 2);
    }

    #[test]
    fn allows_same_target_different_sources() {
        let mut cat = Catalog::empty();
        cat.casts
            .push(simple_cast("app", "type_a", "pg_catalog", "text"));
        cat.casts
            .push(simple_cast("app", "type_b", "pg_catalog", "text"));
        run(&mut cat).unwrap();
        assert_eq!(cat.casts.len(), 2);
    }
}
