//! Canon pass for statistics. Validates and sorts.
//!
//! Invariants enforced:
//! - `Statistic.kinds` has at least one enabled.
//! - `Statistic.columns` is non-empty.
//!
//! Sorts:
//! - `Statistic.columns` with Columns first (sorted by Identifier), then
//!   Expressions (sorted by `canonical_text`). Duplicates silently deduped.
//! - The statistics collection itself is sorted by `sort_and_dedupe`,
//!   not here.

use crate::ir::IrError;
use crate::ir::catalog::Catalog;
use crate::ir::statistic::{Statistic, StatisticColumn};

/// Validate and sort every [`Statistic`] in `cat`.
///
/// Returns [`IrError::EmptyStatisticKinds`] if any statistic has no kinds
/// enabled, or [`IrError::EmptyStatisticColumns`] if any has an empty column
/// list.
pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    for s in &mut cat.statistics {
        validate_and_sort(s)?;
    }
    Ok(())
}

fn validate_and_sort(s: &mut Statistic) -> Result<(), IrError> {
    if s.kinds.is_empty() {
        return Err(IrError::EmptyStatisticKinds(s.qname.clone()));
    }
    if s.columns.is_empty() {
        return Err(IrError::EmptyStatisticColumns(s.qname.clone()));
    }
    s.columns.sort_by(|a, b| match (a, b) {
        (StatisticColumn::Column(a), StatisticColumn::Column(b)) => a.cmp(b),
        (StatisticColumn::Column(_), StatisticColumn::Expression(_)) => std::cmp::Ordering::Less,
        (StatisticColumn::Expression(_), StatisticColumn::Column(_)) => std::cmp::Ordering::Greater,
        (StatisticColumn::Expression(a), StatisticColumn::Expression(b)) => {
            a.canonical_text.cmp(&b.canonical_text)
        }
    });
    s.columns.dedup();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::default_expr::NormalizedExpr;
    use crate::ir::statistic::StatisticKinds;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(id(s), id(n))
    }

    fn stat(cols: Vec<StatisticColumn>, kinds: StatisticKinds) -> Statistic {
        Statistic {
            qname: qn("app", "s"),
            target: qn("app", "t"),
            kinds,
            columns: cols,
            statistics_target: None,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn rejects_empty_kinds() {
        let mut cat = Catalog::empty();
        cat.statistics.push(stat(
            vec![StatisticColumn::Column(id("a"))],
            StatisticKinds {
                ndistinct: false,
                dependencies: false,
                mcv: false,
            },
        ));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::EmptyStatisticKinds(_)
        ));
    }

    #[test]
    fn rejects_empty_columns() {
        let mut cat = Catalog::empty();
        cat.statistics
            .push(stat(vec![], StatisticKinds::pg_default()));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::EmptyStatisticColumns(_)
        ));
    }

    #[test]
    fn sorts_columns_then_expressions() {
        let mut cat = Catalog::empty();
        // NormalizedExpr::from_text returns Self (not Result).
        let e1 = NormalizedExpr::from_text("lower(name)");
        let e2 = NormalizedExpr::from_text("abs(id)");
        cat.statistics.push(stat(
            vec![
                StatisticColumn::Expression(e1),
                StatisticColumn::Column(id("b")),
                StatisticColumn::Column(id("a")),
                StatisticColumn::Expression(e2),
            ],
            StatisticKinds::pg_default(),
        ));
        run(&mut cat).unwrap();
        let cols = &cat.statistics[0].columns;
        assert_eq!(cols.len(), 4);
        // Columns first, then expressions.
        assert!(matches!(cols[0], StatisticColumn::Column(ref i) if i.as_str() == "a"));
        assert!(matches!(cols[1], StatisticColumn::Column(ref i) if i.as_str() == "b"));
        // Expressions sorted by canonical_text: "abs(id)" < "lower(name)".
        assert!(
            matches!(cols[2], StatisticColumn::Expression(ref e) if e.canonical_text == "abs(id)")
        );
        assert!(
            matches!(cols[3], StatisticColumn::Expression(ref e) if e.canonical_text == "lower(name)")
        );
    }

    #[test]
    fn dedupes_duplicate_columns() {
        let mut cat = Catalog::empty();
        cat.statistics.push(stat(
            vec![
                StatisticColumn::Column(id("a")),
                StatisticColumn::Column(id("a")),
                StatisticColumn::Column(id("b")),
            ],
            StatisticKinds::pg_default(),
        ));
        run(&mut cat).unwrap();
        assert_eq!(cat.statistics[0].columns.len(), 2);
    }

    #[test]
    fn passes_through_valid_statistic() {
        let mut cat = Catalog::empty();
        cat.statistics.push(stat(
            vec![StatisticColumn::Column(id("a"))],
            StatisticKinds::pg_default(),
        ));
        assert!(run(&mut cat).is_ok());
    }
}
