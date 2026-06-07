//! Differ for `Catalog::aggregates`.
//!
//! Aggregates are **managed** (schema-scoped): a live aggregate that is absent
//! from source IS auto-dropped — unlike lenient objects such as event triggers
//! or statistics.
//!
//! Identity is `(qname, arg_types)`. Because [`ColumnType`] does not implement
//! [`Ord`], the `BTreeMap` key is `(QualifiedName, Vec<String>)` where each
//! `String` is `ColumnType::render_sql()` — a stable, canonical representation.
//!
//! Logic summary:
//! - source-only → `Create` (Safe).
//! - target-only → `Drop` (Safe — aggregates carry no data).
//! - both present, structural diff (`state_type` / `sfunc` / `finalfunc` /
//!   `initcond`) → `Replace` (Safe, subsumes owner/comment).
//! - else: owner lenient (only when source declares one) → `AlterOwner` (Safe).
//! - comment differs → `CommentOn` (Safe).

use std::collections::BTreeMap;

use crate::diff::change::{AggregateChange, Change};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::identifier::QualifiedName;
use crate::ir::aggregate::Aggregate;
use crate::ir::catalog::Catalog;

/// The `BTreeMap` key for an aggregate: canonical `(qname, rendered arg types)`.
type AggKey = (QualifiedName, Vec<String>);

fn agg_key(a: &Aggregate) -> AggKey {
    (
        a.qname.clone(),
        a.arg_types
            .iter()
            .map(crate::ir::column_type::ColumnType::render_sql)
            .collect(),
    )
}

/// Compute aggregate changes needed to converge `target` (live) toward `source`.
///
/// Appends all emitted changes to `out`. Aggregates are **managed**: a
/// target-only aggregate (absent from source) IS auto-dropped.
pub fn diff_aggregates(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<AggKey, &Aggregate> =
        target.aggregates.iter().map(|a| (agg_key(a), a)).collect();
    let source_map: BTreeMap<AggKey, &Aggregate> =
        source.aggregates.iter().map(|a| (agg_key(a), a)).collect();

    // Source-only → Create.
    for (key, src) in &source_map {
        if !target_map.contains_key(key) {
            out.push(
                Change::Aggregate(AggregateChange::Create((*src).clone())),
                Destructiveness::Safe,
            );
        }
    }

    // Target-only → Drop (managed, not lenient).
    for (key, tgt) in &target_map {
        if !source_map.contains_key(key) {
            out.push(
                Change::Aggregate(AggregateChange::Drop {
                    qname: tgt.qname.clone(),
                    arg_types: tgt.arg_types.clone(),
                }),
                Destructiveness::Safe,
            );
        }
    }

    // Both present → granular diff.
    for (key, src) in &source_map {
        let Some(tgt) = target_map.get(key) else {
            continue;
        };
        emit_modify(tgt, src, out);
    }
}

fn structural_differs(t: &Aggregate, s: &Aggregate) -> bool {
    t.state_type != s.state_type
        || t.sfunc != s.sfunc
        || t.finalfunc != s.finalfunc
        || t.initcond != s.initcond
}

fn emit_modify(t: &Aggregate, s: &Aggregate, out: &mut ChangeSet) {
    if structural_differs(t, s) {
        // Replace subsumes owner/comment — the recreate carries them.
        out.push(
            Change::Aggregate(AggregateChange::Replace {
                from: t.clone(),
                to: s.clone(),
            }),
            Destructiveness::Safe,
        );
        return;
    }

    // Owner is lenient: only when source declares one and it differs.
    if let Some(src_owner) = &s.owner
        && t.owner.as_ref() != Some(src_owner)
    {
        out.push(
            Change::Aggregate(AggregateChange::AlterOwner {
                qname: s.qname.clone(),
                arg_types: s.arg_types.clone(),
                owner: src_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Comment differs → CommentOn (both directions: set or clear).
    if t.comment != s.comment {
        out.push(
            Change::Aggregate(AggregateChange::CommentOn {
                qname: s.qname.clone(),
                arg_types: s.arg_types.clone(),
                comment: s.comment.clone(),
            }),
            Destructiveness::Safe,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::change::{AggregateChange, Change};
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::column_type::ColumnType;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    /// Build a minimal aggregate with one `INTEGER` argument.
    fn basic_agg(name: &str) -> Aggregate {
        Aggregate {
            qname: qn("app", name),
            arg_types: vec![ColumnType::Integer],
            state_type: ColumnType::BigInt,
            sfunc: qn("app", "my_sfunc"),
            finalfunc: None,
            initcond: None,
            owner: None,
            comment: None,
        }
    }

    fn cat(aggs: Vec<Aggregate>) -> Catalog {
        let mut c = Catalog::empty();
        c.aggregates = aggs;
        c
    }

    fn run(target: &Catalog, source: &Catalog) -> ChangeSet {
        let mut out = ChangeSet::new();
        diff_aggregates(target, source, &mut out);
        out
    }

    // ---- source-only → Create ----

    #[test]
    fn source_only_creates() {
        let changes = run(&cat(vec![]), &cat(vec![basic_agg("my_sum")]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Aggregate(AggregateChange::Create(_))
        ));
    }

    // ---- target-only → Drop (managed, NOT lenient) ----

    #[test]
    fn target_only_drops() {
        let changes = run(&cat(vec![basic_agg("my_sum")]), &cat(vec![]));
        assert_eq!(
            changes.len(),
            1,
            "managed aggregate must emit Drop when absent from source"
        );
        assert!(
            matches!(
                changes.iter().next().unwrap().change,
                Change::Aggregate(AggregateChange::Drop { .. })
            ),
            "expected Drop, got {:?}",
            changes.iter().next().unwrap().change
        );
    }

    // ---- structural change → Replace ----

    #[test]
    fn different_sfunc_replaces() {
        let t = basic_agg("my_sum");
        let mut s = basic_agg("my_sum");
        s.sfunc = qn("app", "other_sfunc");
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Aggregate(AggregateChange::Replace { .. })
        ));
    }

    #[test]
    fn different_state_type_replaces() {
        let t = basic_agg("my_sum");
        let mut s = basic_agg("my_sum");
        s.state_type = ColumnType::Numeric {
            precision: None,
            scale: None,
        };
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Aggregate(AggregateChange::Replace { .. })
        ));
    }

    #[test]
    fn different_finalfunc_replaces() {
        let t = basic_agg("my_sum");
        let mut s = basic_agg("my_sum");
        s.finalfunc = Some(qn("app", "my_final"));
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Aggregate(AggregateChange::Replace { .. })
        ));
    }

    #[test]
    fn different_initcond_replaces() {
        let t = basic_agg("my_sum");
        let mut s = basic_agg("my_sum");
        s.initcond = Some("0".into());
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Aggregate(AggregateChange::Replace { .. })
        ));
    }

    // ---- Replace subsumes owner/comment ----

    #[test]
    fn replace_subsumes_owner_and_comment() {
        let t = basic_agg("my_sum");
        let mut s = basic_agg("my_sum");
        s.sfunc = qn("app", "other_sfunc"); // structural
        s.owner = Some(id("alice"));
        s.comment = Some("new".into());
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1, "Replace must subsume owner + comment");
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Aggregate(AggregateChange::Replace { .. })
        ));
    }

    // ---- owner change → AlterOwner (lenient) ----

    #[test]
    fn owner_change_emits_alter_owner() {
        let mut t = basic_agg("my_sum");
        t.owner = Some(id("alice"));
        let mut s = basic_agg("my_sum");
        s.owner = Some(id("bob"));
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Aggregate(AggregateChange::AlterOwner { .. })
        ));
    }

    #[test]
    fn source_owner_none_no_alter_owner() {
        let mut t = basic_agg("my_sum");
        t.owner = Some(id("alice"));
        let s = basic_agg("my_sum"); // owner = None
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert!(
            changes.is_empty(),
            "source owner None = unmanaged, no change expected"
        );
    }

    // ---- comment change → CommentOn ----

    #[test]
    fn comment_change_emits_comment_on() {
        let t = basic_agg("my_sum");
        let mut s = basic_agg("my_sum");
        s.comment = Some("aggregates integers".into());
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Aggregate(AggregateChange::CommentOn { .. })
        ));
    }

    // ---- different arg_types = distinct identities → Create + Drop ----

    #[test]
    fn different_arg_types_are_distinct_identities() {
        // target has my_sum(integer), source has my_sum(bigint) → Drop + Create.
        let mut t = basic_agg("my_sum"); // arg_types = [Integer]
        t.arg_types = vec![ColumnType::Integer];
        let mut s = basic_agg("my_sum");
        s.arg_types = vec![ColumnType::BigInt];
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(
            changes.len(),
            2,
            "different arg_types are distinct: Drop + Create"
        );
        assert!(
            changes
                .iter()
                .any(|e| matches!(&e.change, Change::Aggregate(AggregateChange::Create(_)))),
            "expected Create"
        );
        assert!(
            changes
                .iter()
                .any(|e| matches!(&e.change, Change::Aggregate(AggregateChange::Drop { .. }))),
            "expected Drop"
        );
    }

    // ---- identical → no changes ----

    #[test]
    fn identical_aggregates_produce_no_changes() {
        let agg = basic_agg("my_sum");
        let c = cat(vec![agg]);
        let changes = run(&c, &c);
        assert!(changes.is_empty());
    }
}
