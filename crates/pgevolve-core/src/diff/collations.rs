//! Differ for collations. Per-collation granular diff:
//! - Structural change (provider / `lc_collate` / `lc_ctype` / deterministic) →
//!   `Replace` (skip the rest); `version` is read-only and ignored.
//! - owner differs (lenient) → `AlterObjectOwner`.
//! - comment differs → `CommentOn`.
//!
//! Lenient: target-only collations do NOT emit `Drop` (surfaces via the
//! `unmanaged-collation` lint in a later stage). Rename is not emitted
//! either — name mismatch is treated as drop-old (lenient skip) plus
//! create-new, since rename intent is not structurally derivable.

use std::collections::BTreeMap;

use crate::diff::change::{Change, CollationChange};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::diff::owner_op::{AlterObjectOwner, GrantableObject};
use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::ir::collation::Collation;

/// Compute granular collation changes needed to converge `target` toward
/// `source`. Appends all emitted changes to `out`.
pub fn diff_collations(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<&QualifiedName, &Collation> =
        target.collations.iter().map(|c| (&c.qname, c)).collect();
    let source_map: BTreeMap<&QualifiedName, &Collation> =
        source.collations.iter().map(|c| (&c.qname, c)).collect();

    // Creates: in source but not in target.
    for (qname, src) in &source_map {
        if !target_map.contains_key(qname) {
            out.push(
                Change::Collation(CollationChange::Create((*src).clone())),
                Destructiveness::Safe,
            );
        }
    }

    // Target-only: lenient — no auto-drop. Surfaces via unmanaged-collation lint.

    // Modifies: in both.
    for (qname, src) in &source_map {
        let Some(tgt) = target_map.get(qname) else {
            continue;
        };
        diff_one(tgt, src, out);
    }
}

fn diff_one(target: &Collation, source: &Collation, out: &mut ChangeSet) {
    // Structural change (provider / lc_collate / lc_ctype / deterministic)
    // → Replace; skip the rest for this collation. `version` is read-only and
    // intentionally not compared.
    if (
        target.provider,
        &target.lc_collate,
        &target.lc_ctype,
        target.deterministic,
    ) != (
        source.provider,
        &source.lc_collate,
        &source.lc_ctype,
        source.deterministic,
    ) {
        out.push(
            Change::Collation(CollationChange::Replace {
                from: target.clone(),
                to: source.clone(),
            }),
            Destructiveness::RequiresApproval {
                reason: format!(
                    "structural change to collation {} requires DROP + CREATE (PG has no in-place ALTER for provider/locale/deterministic)",
                    source.qname
                ),
            },
        );
        return;
    }

    // Owner: lenient — only emit when source declares an owner and it differs.
    if let Some(s_owner) = &source.owner
        && target.owner.as_ref() != Some(s_owner)
    {
        out.push(
            Change::AlterObjectOwner(AlterObjectOwner {
                object: GrantableObject::Collation(source.qname.clone()),
                from: target.owner.clone(),
                to: s_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Comment.
    if target.comment != source.comment {
        out.push(
            Change::Collation(CollationChange::CommentOn {
                qname: source.qname.clone(),
                comment: source.comment.clone(),
            }),
            Destructiveness::Safe,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::collation::CollationProvider;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn basic_collation(name: &str) -> Collation {
        Collation {
            qname: qn("app", name),
            provider: CollationProvider::Libc,
            lc_collate: "en_US.utf8".into(),
            lc_ctype: "en_US.utf8".into(),
            deterministic: true,
            version: None,
            owner: None,
            comment: None,
        }
    }

    fn catalog_with(collations: Vec<Collation>) -> Catalog {
        let mut c = Catalog::empty();
        c.collations = collations;
        c
    }

    fn run_diff(target: &Catalog, source: &Catalog) -> ChangeSet {
        let mut out = ChangeSet::new();
        diff_collations(target, source, &mut out);
        out
    }

    // ---- creates ----

    #[test]
    fn create_collation_when_source_has_it_and_target_doesnt() {
        let target = Catalog::empty();
        let source = catalog_with(vec![basic_collation("c")]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Collation(CollationChange::Create(_))
        ));
    }

    #[test]
    fn create_collation_is_safe() {
        let target = Catalog::empty();
        let source = catalog_with(vec![basic_collation("c")]);
        let changes = run_diff(&target, &source);
        let entry = changes.iter().next().unwrap();
        assert!(!entry.destructiveness.requires_approval());
    }

    // ---- lenient: no auto-drop ----

    #[test]
    fn no_drop_when_target_has_collation_but_source_doesnt() {
        let target = catalog_with(vec![basic_collation("c")]);
        let source = Catalog::empty();
        let changes = run_diff(&target, &source);
        assert!(
            changes.is_empty(),
            "expected no changes (lenient), got {changes:?}"
        );
    }

    // ---- identical: no diff ----

    #[test]
    fn identical_collations_produce_no_changes() {
        let c = catalog_with(vec![basic_collation("c")]);
        let changes = run_diff(&c, &c);
        assert!(changes.is_empty());
    }

    // ---- structural changes → Replace ----

    #[test]
    fn provider_differs_emits_replace() {
        let mut src = basic_collation("c");
        src.provider = CollationProvider::Icu;
        let target = catalog_with(vec![basic_collation("c")]);
        let source = catalog_with(vec![src]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        assert!(matches!(
            entry.change,
            Change::Collation(CollationChange::Replace { .. })
        ));
        assert!(entry.destructiveness.requires_approval());
    }

    #[test]
    fn lc_collate_differs_emits_replace() {
        let mut src = basic_collation("c");
        src.lc_collate = "de_DE.utf8".into();
        let target = catalog_with(vec![basic_collation("c")]);
        let source = catalog_with(vec![src]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Collation(CollationChange::Replace { .. })
        ));
    }

    #[test]
    fn lc_ctype_differs_emits_replace() {
        let mut src = basic_collation("c");
        src.lc_ctype = "de_DE.utf8".into();
        let target = catalog_with(vec![basic_collation("c")]);
        let source = catalog_with(vec![src]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Collation(CollationChange::Replace { .. })
        ));
    }

    #[test]
    fn deterministic_differs_emits_replace() {
        let mut tgt = basic_collation("c");
        tgt.provider = CollationProvider::Icu;
        tgt.lc_collate = "und".into();
        tgt.lc_ctype = "und".into();
        tgt.deterministic = true;
        let mut src = tgt.clone();
        src.deterministic = false;
        let target = catalog_with(vec![tgt]);
        let source = catalog_with(vec![src]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Collation(CollationChange::Replace { .. })
        ));
    }

    #[test]
    fn version_differs_does_not_emit_change() {
        // version is read-only — must be ignored.
        let mut tgt = basic_collation("c");
        tgt.version = Some("153.97".into());
        let mut src = basic_collation("c");
        src.version = Some("153.128".into());
        let target = catalog_with(vec![tgt]);
        let source = catalog_with(vec![src]);
        let changes = run_diff(&target, &source);
        assert!(
            changes.is_empty(),
            "version field must be ignored by differ, got {changes:?}"
        );
    }

    #[test]
    fn structural_change_skips_downstream_per_field_checks() {
        // Even if owner + comment also differ, only Replace is emitted.
        let mut tgt = basic_collation("c");
        tgt.owner = Some(id("alice"));
        tgt.comment = Some("old".into());

        let mut src = basic_collation("c");
        src.provider = CollationProvider::Icu; // structural diff
        src.owner = Some(id("bob"));
        src.comment = Some("new".into());

        let target = catalog_with(vec![tgt]);
        let source = catalog_with(vec![src]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1, "only Replace, no downstream diffs");
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Collation(CollationChange::Replace { .. })
        ));
    }

    // ---- owner diff ----

    #[test]
    fn owner_change_emits_alter_object_owner() {
        let mut tgt = basic_collation("c");
        tgt.owner = Some(id("alice"));
        let mut src = basic_collation("c");
        src.owner = Some(id("bob"));
        let target = catalog_with(vec![tgt]);
        let source = catalog_with(vec![src]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        if let Change::AlterObjectOwner(op) = &entry.change {
            assert!(matches!(op.object, GrantableObject::Collation(_)));
            assert_eq!(op.to, id("bob"));
        } else {
            panic!("expected AlterObjectOwner, got {:?}", entry.change);
        }
    }

    #[test]
    fn no_owner_change_when_source_owner_is_none() {
        // Source `None` = unmanaged; no change emitted.
        let mut tgt = basic_collation("c");
        tgt.owner = Some(id("alice"));
        let src = basic_collation("c"); // owner = None
        let target = catalog_with(vec![tgt]);
        let source = catalog_with(vec![src]);
        let changes = run_diff(&target, &source);
        assert!(
            changes.is_empty(),
            "source owner None = unmanaged, no change expected"
        );
    }

    // ---- comment diff ----

    #[test]
    fn comment_change_emits_comment_on() {
        let src = {
            let mut s = basic_collation("c");
            s.comment = Some("pinned for sorting".into());
            s
        };
        let target = catalog_with(vec![basic_collation("c")]);
        let source = catalog_with(vec![src]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Collation(CollationChange::CommentOn { .. })
        ));
    }

    #[test]
    fn clear_comment_emits_comment_on_with_none() {
        let mut tgt = basic_collation("c");
        tgt.comment = Some("old comment".into());
        let src = basic_collation("c"); // comment = None
        let target = catalog_with(vec![tgt]);
        let source = catalog_with(vec![src]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        if let Change::Collation(CollationChange::CommentOn { comment, .. }) = &entry.change {
            assert!(comment.is_none());
        } else {
            panic!(
                "expected CollationChange::CommentOn, got {:?}",
                entry.change
            );
        }
    }

    // ---- partial overlap ----

    #[test]
    fn owner_and_comment_both_changed_emit_two_changes() {
        let mut tgt = basic_collation("c");
        tgt.owner = Some(id("alice"));
        let mut src = basic_collation("c");
        src.owner = Some(id("bob"));
        src.comment = Some("new comment".into());
        let target = catalog_with(vec![tgt]);
        let source = catalog_with(vec![src]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 2);
        assert!(
            changes
                .iter()
                .any(|e| matches!(&e.change, Change::AlterObjectOwner(_)))
        );
        assert!(changes.iter().any(|e| matches!(
            &e.change,
            Change::Collation(CollationChange::CommentOn { .. })
        )));
    }
}
