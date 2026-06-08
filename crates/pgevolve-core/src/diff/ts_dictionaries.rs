//! Differ for `Catalog::ts_dictionaries`.
//!
//! Text-search dictionaries are **managed** (schema-scoped): a live dictionary
//! that is absent from source IS auto-dropped — unlike lenient objects such as
//! event triggers or statistics.
//!
//! Identity is `qname`. The `BTreeMap` key is `qname.render_sql()` — a stable,
//! canonical representation.
//!
//! Logic summary:
//! - source-only → `Create` (Safe).
//! - target-only → `Drop` (Safe — dictionaries carry no data).
//! - both present, `template` differs → `Replace` (Safe, subsumes owner/comment;
//!   PG has no `ALTER … TEMPLATE`).
//! - else: options differ → `AlterOptions` (Safe).
//! - owner lenient (only when source declares one) → `AlterOwner` (Safe).
//! - comment differs → `CommentOn` (Safe).

use std::collections::BTreeMap;

use crate::diff::change::{Change, TsDictionaryChange};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::ir::catalog::Catalog;
use crate::ir::text_search::dictionary::TsDictionary;

/// Compute text-search dictionary changes needed to converge `target` (live)
/// toward `source`.
///
/// Appends all emitted changes to `out`. Dictionaries are **managed**: a
/// target-only dictionary (absent from source) IS auto-dropped.
pub fn diff_ts_dictionaries(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<String, &TsDictionary> = target
        .ts_dictionaries
        .iter()
        .map(|d| (d.qname.render_sql(), d))
        .collect();
    let source_map: BTreeMap<String, &TsDictionary> = source
        .ts_dictionaries
        .iter()
        .map(|d| (d.qname.render_sql(), d))
        .collect();

    // Source-only → Create.
    for (key, src) in &source_map {
        if !target_map.contains_key(key) {
            out.push(
                Change::TsDictionary(TsDictionaryChange::Create((*src).clone())),
                Destructiveness::Safe,
            );
        }
    }

    // Target-only → Drop (managed, not lenient).
    for (key, tgt) in &target_map {
        if !source_map.contains_key(key) {
            out.push(
                Change::TsDictionary(TsDictionaryChange::Drop {
                    qname: tgt.qname.clone(),
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

fn emit_modify(t: &TsDictionary, s: &TsDictionary, out: &mut ChangeSet) {
    // Template change requires DROP + CREATE; subsumes owner/comment.
    if t.template != s.template {
        out.push(
            Change::TsDictionary(TsDictionaryChange::Replace {
                from: t.clone(),
                to: s.clone(),
            }),
            Destructiveness::Safe,
        );
        return;
    }

    // Options differ → AlterOptions.
    if t.options != s.options {
        out.push(
            Change::TsDictionary(TsDictionaryChange::AlterOptions {
                qname: s.qname.clone(),
                options: s.options.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Owner is lenient: only when source declares one and it differs.
    if let Some(src_owner) = &s.owner
        && t.owner.as_ref() != Some(src_owner)
    {
        out.push(
            Change::TsDictionary(TsDictionaryChange::AlterOwner {
                qname: s.qname.clone(),
                owner: src_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Comment differs → CommentOn (both directions: set or clear).
    if t.comment != s.comment {
        out.push(
            Change::TsDictionary(TsDictionaryChange::CommentOn {
                qname: s.qname.clone(),
                comment: s.comment.clone(),
            }),
            Destructiveness::Safe,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::change::{Change, TsDictionaryChange};
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    /// Build a minimal dictionary with the `snowball` template.
    fn basic_dict(name: &str) -> TsDictionary {
        TsDictionary {
            qname: qn("app", name),
            template: qn("pg_catalog", "snowball"),
            options: vec![("language".to_string(), "english".to_string())],
            owner: None,
            comment: None,
        }
    }

    fn cat(dicts: Vec<TsDictionary>) -> Catalog {
        let mut c = Catalog::empty();
        c.ts_dictionaries = dicts;
        c
    }

    fn run(target: &Catalog, source: &Catalog) -> ChangeSet {
        let mut out = ChangeSet::new();
        diff_ts_dictionaries(target, source, &mut out);
        out
    }

    // ---- source-only → Create ----

    #[test]
    fn source_only_creates() {
        let changes = run(&cat(vec![]), &cat(vec![basic_dict("english")]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::TsDictionary(TsDictionaryChange::Create(_))
        ));
    }

    // ---- target-only → Drop (managed, NOT lenient) ----

    #[test]
    fn target_only_drops() {
        let changes = run(&cat(vec![basic_dict("english")]), &cat(vec![]));
        assert_eq!(
            changes.len(),
            1,
            "managed dictionary must emit Drop when absent from source"
        );
        assert!(
            matches!(
                changes.iter().next().unwrap().change,
                Change::TsDictionary(TsDictionaryChange::Drop { .. })
            ),
            "expected Drop, got {:?}",
            changes.iter().next().unwrap().change
        );
    }

    // ---- template change → Replace (from=target/live, to=source/desired) ----

    #[test]
    fn different_template_replaces() {
        let t = basic_dict("english");
        let mut s = basic_dict("english");
        s.template = qn("pg_catalog", "ispell");
        let changes = run(&cat(vec![t.clone()]), &cat(vec![s.clone()]));
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        match &entry.change {
            Change::TsDictionary(TsDictionaryChange::Replace { from, to }) => {
                assert_eq!(from, &t, "from must be the target (live) dictionary");
                assert_eq!(to, &s, "to must be the source (desired) dictionary");
            }
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    // ---- template Replace subsumes owner/comment ----

    #[test]
    fn replace_subsumes_owner_and_comment() {
        let t = basic_dict("english");
        let mut s = basic_dict("english");
        s.template = qn("pg_catalog", "ispell"); // structural
        s.owner = Some(id("alice"));
        s.comment = Some("ispell dict".into());
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1, "Replace must subsume owner + comment");
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::TsDictionary(TsDictionaryChange::Replace { .. })
        ));
    }

    // ---- options change → AlterOptions ----

    #[test]
    fn different_options_emits_alter_options() {
        let t = basic_dict("english");
        let mut s = basic_dict("english");
        s.options = vec![
            ("language".to_string(), "english".to_string()),
            ("stopwords".to_string(), "english".to_string()),
        ];
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::TsDictionary(TsDictionaryChange::AlterOptions { .. })
        ));
    }

    // ---- owner change → AlterOwner (lenient) ----

    #[test]
    fn owner_change_emits_alter_owner() {
        let mut t = basic_dict("english");
        t.owner = Some(id("alice"));
        let mut s = basic_dict("english");
        s.owner = Some(id("bob"));
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::TsDictionary(TsDictionaryChange::AlterOwner { .. })
        ));
    }

    #[test]
    fn source_owner_none_no_alter_owner() {
        let mut t = basic_dict("english");
        t.owner = Some(id("alice"));
        let s = basic_dict("english"); // owner = None
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert!(
            changes.is_empty(),
            "source owner None = unmanaged, no change expected"
        );
    }

    // ---- comment change → CommentOn ----

    #[test]
    fn comment_change_emits_comment_on() {
        let t = basic_dict("english");
        let mut s = basic_dict("english");
        s.comment = Some("English snowball stemmer".into());
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::TsDictionary(TsDictionaryChange::CommentOn { .. })
        ));
    }

    // ---- identical → no changes ----

    #[test]
    fn identical_dictionaries_produce_no_changes() {
        let dict = basic_dict("english");
        let c = cat(vec![dict]);
        let changes = run(&c, &c);
        assert!(changes.is_empty());
    }
}
