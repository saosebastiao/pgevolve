//! `unmanaged-statistic` (Warning) — catalog has a statistic source doesn't.
//!
//! Standard v0.3.x lenient-drift surface: pgevolve doesn't auto-drop
//! catalog statistics that source doesn't declare. This lint warns so
//! operators can decide to bring them under management (add to source)
//! or drop them out-of-band.

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "unmanaged-statistic";

/// Fires once per target statistic whose `qname` is not in source's statistics list.
pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    target
        .statistics
        .iter()
        .filter(|tgt_stat| {
            !source
                .statistics
                .iter()
                .any(|src_stat| src_stat.qname == tgt_stat.qname)
        })
        .map(|tgt_stat| Finding {
            rule: RULE_ID,
            severity: Severity::Warning,
            message: format!(
                "statistic {}: catalog has a statistic not declared in source",
                tgt_stat.qname,
            ),
            location: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::statistic::{Statistic, StatisticColumn, StatisticKinds};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(id(s), id(n))
    }

    fn make_statistic(qname: QualifiedName) -> Statistic {
        Statistic {
            qname,
            target: qn("app", "t"),
            kinds: StatisticKinds::pg_default(),
            columns: vec![StatisticColumn::Column(id("a"))],
            statistics_target: None,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn empty_catalogs_silent() {
        let source = Catalog::empty();
        let target = Catalog::empty();
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn statistic_in_source_and_target_silent() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        source.statistics.push(make_statistic(qn("app", "s")));
        target.statistics.push(make_statistic(qn("app", "s")));
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn statistic_only_in_target_fires() {
        let source = Catalog::empty();
        let mut target = Catalog::empty();
        target
            .statistics
            .push(make_statistic(qn("app", "unmanaged_stat")));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(
            findings[0].message.contains("app.unmanaged_stat"),
            "message should mention the statistic name: {}",
            findings[0].message
        );
    }

    #[test]
    fn statistic_only_in_source_silent() {
        let mut source = Catalog::empty();
        let target = Catalog::empty();
        source
            .statistics
            .push(make_statistic(qn("app", "managed_stat")));
        // Source-only: not-yet-created; no drift finding needed.
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn source_has_one_target_has_two_fires_for_extra() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        source.statistics.push(make_statistic(qn("app", "s")));
        target.statistics.push(make_statistic(qn("app", "s")));
        target.statistics.push(make_statistic(qn("app", "t")));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("app.t"));
    }

    #[test]
    fn multiple_unmanaged_statistics_each_fire() {
        let source = Catalog::empty();
        let mut target = Catalog::empty();
        target.statistics.push(make_statistic(qn("app", "stat_a")));
        target.statistics.push(make_statistic(qn("app", "stat_b")));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|f| f.rule == RULE_ID));
    }
}
