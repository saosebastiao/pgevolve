//! Canon for `Catalog::aggregates`: sort by `(schema, name, arg_types)` and
//! reject duplicate `(qname, arg_types)` overload identities.

use crate::ir::IrError;
use crate::ir::catalog::Catalog;
use crate::ir::column_type::ColumnType;

/// Build a stable sort/identity string for a slice of [`ColumnType`].
///
/// `ColumnType` does not implement `Ord`, so we derive a key from
/// [`ColumnType::render_sql`] which is stable and unique for each type.
fn arg_types_key(arg_types: &[ColumnType]) -> String {
    arg_types
        .iter()
        .map(ColumnType::render_sql)
        .collect::<Vec<_>>()
        .join(",")
}

/// Canonicalize all aggregates in `cat`.
///
/// - The collection is sorted by `(schema, name, arg_types_key)`.
/// - A duplicate `(qname, arg_types)` identity raises [`IrError::DuplicateAggregate`].
pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    cat.aggregates.sort_by(|a, b| {
        a.qname
            .schema
            .cmp(&b.qname.schema)
            .then_with(|| a.qname.name.cmp(&b.qname.name))
            .then_with(|| arg_types_key(&a.arg_types).cmp(&arg_types_key(&b.arg_types)))
    });
    for w in cat.aggregates.windows(2) {
        if w[0].qname == w[1].qname && w[0].arg_types == w[1].arg_types {
            return Err(IrError::DuplicateAggregate(w[0].qname.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::aggregate::Aggregate;
    use crate::ir::column_type::ColumnType;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn agg(schema: &str, name: &str, arg_types: Vec<ColumnType>) -> Aggregate {
        Aggregate {
            qname: qname(schema, name),
            arg_types,
            state_type: ColumnType::BigInt,
            sfunc: qname("app", "sfunc"),
            finalfunc: None,
            initcond: None,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn sorts_by_name() {
        let mut cat = Catalog::empty();
        cat.aggregates
            .push(agg("app", "zzz_agg", vec![ColumnType::Integer]));
        cat.aggregates
            .push(agg("app", "aaa_agg", vec![ColumnType::Integer]));
        run(&mut cat).unwrap();
        assert_eq!(cat.aggregates[0].qname.name.as_str(), "aaa_agg");
        assert_eq!(cat.aggregates[1].qname.name.as_str(), "zzz_agg");
    }

    #[test]
    fn rejects_duplicate_overload_identity() {
        let mut cat = Catalog::empty();
        cat.aggregates
            .push(agg("app", "my_sum", vec![ColumnType::Integer]));
        cat.aggregates
            .push(agg("app", "my_sum", vec![ColumnType::Integer]));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::DuplicateAggregate(_)
        ));
    }

    #[test]
    fn allows_distinct_overloads_same_name() {
        let mut cat = Catalog::empty();
        cat.aggregates
            .push(agg("app", "my_sum", vec![ColumnType::Integer]));
        cat.aggregates
            .push(agg("app", "my_sum", vec![ColumnType::BigInt]));
        run(&mut cat).unwrap();
        assert_eq!(cat.aggregates.len(), 2);
    }
}
