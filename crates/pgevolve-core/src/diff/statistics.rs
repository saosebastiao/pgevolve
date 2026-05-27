//! Differ for statistics. Per-statistic granular diff:
//! - Structural change (columns / kinds / target) → `ReplaceStatistic` (skip the rest).
//! - `statistics_target` differs → `AlterStatisticSetTarget`.
//! - owner differs (lenient) → `AlterObjectOwner`.
//! - comment differs → `CommentOnStatistic`.
//!
//! Lenient: target-only statistics do NOT emit `DropStatistic` (surfaces via
//! unmanaged-statistic lint in Stage 9).
//!
//! Spec: `docs/superpowers/specs/2026-05-27-statistics-and-check-option-design.md`.

use std::collections::BTreeMap;

use crate::diff::change::Change;
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::diff::owner_op::{AlterObjectOwner, OwnerObjectKind};
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::statistic::Statistic;

/// Compute granular statistic changes needed to converge `target` toward
/// `source`. Appends all emitted changes to `out`.
pub fn diff_statistics(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<&QualifiedName, &Statistic> =
        target.statistics.iter().map(|s| (&s.qname, s)).collect();
    let source_map: BTreeMap<&QualifiedName, &Statistic> =
        source.statistics.iter().map(|s| (&s.qname, s)).collect();

    // Creates: in source but not in target.
    for (qname, src) in &source_map {
        if !target_map.contains_key(qname) {
            out.push(
                Change::CreateStatistic((*src).clone()),
                Destructiveness::Safe,
            );
        }
    }

    // Target-only: lenient — no auto-drop. Surfaces via unmanaged-statistic lint.
    // Intentionally no-op; Stage 9 adds the unmanaged-statistic lint rule.

    // Modifies: in both.
    for (qname, src) in &source_map {
        let Some(tgt) = target_map.get(qname) else {
            continue;
        };
        diff_one(tgt, src, out);
    }
}

fn diff_one(target: &Statistic, source: &Statistic, out: &mut ChangeSet) {
    // Structural change → ReplaceStatistic; skip the rest for this statistic.
    if target.columns != source.columns
        || target.kinds != source.kinds
        || target.target != source.target
    {
        out.push(
            Change::ReplaceStatistic {
                from: target.clone(),
                to: source.clone(),
            },
            Destructiveness::RequiresApproval {
                reason: format!(
                    "structural change to statistic {} requires DROP + CREATE (PG has no in-place ALTER for columns/kinds/target)",
                    source.qname
                ),
            },
        );
        return;
    }

    // statistics_target diff — lenient: only emit when source declares a value.
    if let Some(s_target) = source.statistics_target
        && target.statistics_target != Some(s_target)
    {
        out.push(
            Change::AlterStatisticSetTarget {
                qname: source.qname.clone(),
                value: s_target,
            },
            Destructiveness::Safe,
        );
    }

    // Owner: v0.3.1 lenient — only emit when source declares an owner and it
    // differs from target. Source `None` = unmanaged, no change emitted.
    if let Some(s_owner) = &source.owner
        && target.owner.as_ref() != Some(s_owner)
    {
        let from = target.owner.clone().unwrap_or_else(|| {
            Identifier::from_unquoted("__unknown_owner__")
                .expect("literal is always a valid unquoted identifier")
        });
        out.push(
            Change::AlterObjectOwner(AlterObjectOwner {
                kind: OwnerObjectKind::Statistic,
                qname: source.qname.clone(),
                signature: String::new(),
                from,
                to: s_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Comment.
    if target.comment != source.comment {
        out.push(
            Change::CommentOnStatistic {
                qname: source.qname.clone(),
                comment: source.comment.clone(),
            },
            Destructiveness::Safe,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::change::Change;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::statistic::{Statistic, StatisticColumn, StatisticKinds};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn basic_statistic(stat_name: &str, table_name: &str) -> Statistic {
        Statistic {
            qname: qn("app", stat_name),
            target: qn("app", table_name),
            kinds: StatisticKinds::pg_default(),
            columns: vec![
                StatisticColumn::Column(id("a")),
                StatisticColumn::Column(id("b")),
            ],
            statistics_target: None,
            owner: None,
            comment: None,
        }
    }

    fn catalog_with(stats: Vec<Statistic>) -> Catalog {
        let mut c = Catalog::empty();
        c.statistics = stats;
        c
    }

    fn run_diff(target: &Catalog, source: &Catalog) -> ChangeSet {
        let mut out = ChangeSet::new();
        diff_statistics(target, source, &mut out);
        out
    }

    // ---- creates ----

    #[test]
    fn create_statistic_when_source_has_it_and_target_doesnt() {
        let target = Catalog::empty();
        let source = catalog_with(vec![basic_statistic("s", "t")]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::CreateStatistic(_)
        ));
    }

    #[test]
    fn create_statistic_is_safe() {
        let target = Catalog::empty();
        let source = catalog_with(vec![basic_statistic("s", "t")]);
        let changes = run_diff(&target, &source);
        let entry = changes.iter().next().unwrap();
        assert!(!entry.destructiveness.requires_approval());
    }

    // ---- lenient: no auto-drop ----

    #[test]
    fn no_drop_when_target_has_statistic_but_source_doesnt() {
        let target = catalog_with(vec![basic_statistic("s", "t")]);
        let source = Catalog::empty();
        let changes = run_diff(&target, &source);
        assert!(
            changes.is_empty(),
            "expected no changes (lenient), got {changes:?}"
        );
    }

    // ---- identical: no diff ----

    #[test]
    fn identical_statistics_produce_no_changes() {
        let c = catalog_with(vec![basic_statistic("s", "t")]);
        let changes = run_diff(&c, &c);
        assert!(changes.is_empty());
    }

    // ---- structural changes → ReplaceStatistic ----

    #[test]
    fn columns_differ_emits_replace_statistic() {
        let mut src_stat = basic_statistic("s", "t");
        src_stat.columns = vec![
            StatisticColumn::Column(id("a")),
            StatisticColumn::Column(id("c")),
        ];
        let target = catalog_with(vec![basic_statistic("s", "t")]);
        let source = catalog_with(vec![src_stat]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        assert!(
            matches!(entry.change, Change::ReplaceStatistic { .. }),
            "expected ReplaceStatistic, got {:?}",
            entry.change
        );
        assert!(
            entry.destructiveness.requires_approval(),
            "structural change must be RequiresApproval"
        );
    }

    #[test]
    fn kinds_differ_emits_replace_statistic() {
        let mut src_stat = basic_statistic("s", "t");
        src_stat.kinds = StatisticKinds {
            ndistinct: true,
            dependencies: false,
            mcv: false,
        };
        let target = catalog_with(vec![basic_statistic("s", "t")]);
        let source = catalog_with(vec![src_stat]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::ReplaceStatistic { .. }
        ));
    }

    #[test]
    fn target_table_differs_emits_replace_statistic() {
        let mut src_stat = basic_statistic("s", "t");
        src_stat.target = qn("app", "t2");
        let target = catalog_with(vec![basic_statistic("s", "t")]);
        let source = catalog_with(vec![src_stat]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::ReplaceStatistic { .. }
        ));
    }

    #[test]
    fn structural_change_skips_downstream_per_field_checks() {
        // Even if statistics_target and owner also differ, only ReplaceStatistic is emitted.
        let mut tgt_stat = basic_statistic("s", "t");
        tgt_stat.statistics_target = Some(100);
        tgt_stat.owner = Some(id("alice"));
        tgt_stat.comment = Some("old".into());

        let mut src_stat = basic_statistic("s", "t");
        src_stat.columns = vec![StatisticColumn::Column(id("x"))]; // structural diff
        src_stat.statistics_target = Some(200);
        src_stat.owner = Some(id("bob"));
        src_stat.comment = Some("new".into());

        let target = catalog_with(vec![tgt_stat]);
        let source = catalog_with(vec![src_stat]);
        let changes = run_diff(&target, &source);
        assert_eq!(
            changes.len(),
            1,
            "only ReplaceStatistic, no downstream diffs"
        );
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::ReplaceStatistic { .. }
        ));
    }

    // ---- statistics_target diff ----

    #[test]
    fn only_statistics_target_differs_emits_alter_statistic_set_target() {
        let mut src_stat = basic_statistic("s", "t");
        src_stat.statistics_target = Some(500);
        let target = catalog_with(vec![basic_statistic("s", "t")]); // None
        let source = catalog_with(vec![src_stat]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::AlterStatisticSetTarget { value: 500, .. }
        ));
    }

    #[test]
    fn source_statistics_target_none_does_not_trigger_diff() {
        // Source `None` = unmanaged; no change emitted even if target has a value.
        let mut tgt_stat = basic_statistic("s", "t");
        tgt_stat.statistics_target = Some(500);
        let src_stat = basic_statistic("s", "t"); // statistics_target = None
        let target = catalog_with(vec![tgt_stat]);
        let source = catalog_with(vec![src_stat]);
        let changes = run_diff(&target, &source);
        assert!(
            changes.is_empty(),
            "source statistics_target=None must not trigger diff (lenient)"
        );
    }

    // ---- owner diff ----

    #[test]
    fn owner_change_emits_alter_object_owner() {
        let mut tgt_stat = basic_statistic("s", "t");
        tgt_stat.owner = Some(id("alice"));
        let mut src_stat = basic_statistic("s", "t");
        src_stat.owner = Some(id("bob"));
        let target = catalog_with(vec![tgt_stat]);
        let source = catalog_with(vec![src_stat]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::AlterObjectOwner(_)
        ));
    }

    #[test]
    fn no_owner_change_when_source_owner_is_none() {
        // Source `None` = unmanaged; no change emitted.
        let mut tgt_stat = basic_statistic("s", "t");
        tgt_stat.owner = Some(id("alice"));
        let src_stat = basic_statistic("s", "t"); // owner = None
        let target = catalog_with(vec![tgt_stat]);
        let source = catalog_with(vec![src_stat]);
        let changes = run_diff(&target, &source);
        assert!(
            changes.is_empty(),
            "source owner None = unmanaged, no change expected"
        );
    }

    // ---- comment diff ----

    #[test]
    fn comment_change_emits_comment_on_statistic() {
        let src_stat = {
            let mut s = basic_statistic("s", "t");
            s.comment = Some("my stat".into());
            s
        };
        let target = catalog_with(vec![basic_statistic("s", "t")]);
        let source = catalog_with(vec![src_stat]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::CommentOnStatistic { .. }
        ));
    }

    #[test]
    fn clear_comment_emits_comment_on_statistic_with_none() {
        let mut tgt_stat = basic_statistic("s", "t");
        tgt_stat.comment = Some("old comment".into());
        let src_stat = basic_statistic("s", "t"); // comment = None
        let target = catalog_with(vec![tgt_stat]);
        let source = catalog_with(vec![src_stat]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        if let Change::CommentOnStatistic { comment, .. } = &entry.change {
            assert!(comment.is_none());
        } else {
            panic!("expected CommentOnStatistic, got {:?}", entry.change);
        }
    }

    // ---- multiple independent fields changed ----

    #[test]
    fn statistics_target_and_comment_both_changed_emit_two_changes() {
        let tgt_stat = basic_statistic("s", "t"); // no target, no comment
        let mut src_stat = basic_statistic("s", "t");
        src_stat.statistics_target = Some(200);
        src_stat.comment = Some("new comment".into());
        let target = catalog_with(vec![tgt_stat]);
        let source = catalog_with(vec![src_stat]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 2);
        let kinds: Vec<_> = changes
            .iter()
            .map(|e| std::mem::discriminant(&e.change))
            .collect();
        assert!(
            kinds.iter().any(|d| *d
                == std::mem::discriminant(&Change::AlterStatisticSetTarget {
                    qname: qn("x", "y"),
                    value: 0
                })),
            "expected AlterStatisticSetTarget in changes"
        );
        assert!(
            kinds.iter().any(|d| *d
                == std::mem::discriminant(&Change::CommentOnStatistic {
                    qname: qn("x", "y"),
                    comment: None
                })),
            "expected CommentOnStatistic in changes"
        );
    }
}
