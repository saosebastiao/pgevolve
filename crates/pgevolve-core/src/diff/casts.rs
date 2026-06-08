//! Differ for `Catalog::casts`.
//!
//! Casts are **managed** (global, non-schema-scoped): a live cast that is absent
//! from source IS auto-dropped — unlike lenient objects such as event triggers
//! or statistics.
//!
//! Identity is `(source, target)`. Because [`QualifiedName`] does not implement
//! [`Ord`], the `BTreeMap` key is `(String, String)` where each `String` is
//! `QualifiedName::render_sql()` — a stable, canonical representation.
//!
//! Logic summary:
//! - source-only → `Create` (Safe).
//! - target-only → `Drop` (Safe — casts carry no data).
//! - both present, `method` or `context` differ → `Replace` (Safe — Postgres
//!   has no `ALTER CAST`; subsumes comment).
//! - else: comment differs → `CommentOn` (Safe).

use std::collections::BTreeMap;

use crate::diff::change::{CastChange, Change};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::ir::cast::Cast;
use crate::ir::catalog::Catalog;

/// The `BTreeMap` key for a cast: canonical `(rendered source, rendered target)`.
type CastKey = (String, String);

fn cast_key(c: &Cast) -> CastKey {
    (c.source.render_sql(), c.target.render_sql())
}

/// Compute cast changes needed to converge `target` (live) toward `source`.
///
/// Appends all emitted changes to `out`. Casts are **managed**: a target-only
/// cast (absent from source) IS auto-dropped.
pub fn diff_casts(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<CastKey, &Cast> =
        target.casts.iter().map(|c| (cast_key(c), c)).collect();
    let source_map: BTreeMap<CastKey, &Cast> =
        source.casts.iter().map(|c| (cast_key(c), c)).collect();

    // Source-only → Create.
    for (key, src) in &source_map {
        if !target_map.contains_key(key) {
            out.push(
                Change::Cast(CastChange::Create((*src).clone())),
                Destructiveness::Safe,
            );
        }
    }

    // Target-only → Drop (managed, not lenient).
    for (key, tgt) in &target_map {
        if !source_map.contains_key(key) {
            out.push(
                Change::Cast(CastChange::Drop {
                    source: tgt.source.clone(),
                    target: tgt.target.clone(),
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

fn emit_modify(t: &Cast, s: &Cast, out: &mut ChangeSet) {
    // Any structural change (method or context) requires DROP + CREATE.
    if t.method != s.method || t.context != s.context {
        out.push(
            Change::Cast(CastChange::Replace {
                from: t.clone(),
                to: s.clone(),
            }),
            Destructiveness::Safe,
        );
        return;
    }

    // Comment differs → CommentOn (both directions: set or clear).
    if t.comment != s.comment {
        out.push(
            Change::Cast(CastChange::CommentOn {
                source: s.source.clone(),
                target: s.target.clone(),
                comment: s.comment.clone(),
            }),
            Destructiveness::Safe,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::change::{CastChange, Change};
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::cast::{CastContext, CastMethod};
    use crate::ir::catalog::Catalog;
    use crate::ir::column_type::ColumnType;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    /// Build a minimal cast: `app.my_type` → `pg_catalog.text`, binary, explicit.
    fn basic_cast() -> Cast {
        Cast {
            source: qn("app", "my_type"),
            target: qn("pg_catalog", "text"),
            method: CastMethod::Binary,
            context: CastContext::Explicit,
            comment: None,
        }
    }

    fn cat(casts: Vec<Cast>) -> Catalog {
        let mut c = Catalog::empty();
        c.casts = casts;
        c
    }

    fn run(target: &Catalog, source: &Catalog) -> ChangeSet {
        let mut out = ChangeSet::new();
        diff_casts(target, source, &mut out);
        out
    }

    // ---- source-only → Create ----

    #[test]
    fn source_only_creates() {
        let changes = run(&cat(vec![]), &cat(vec![basic_cast()]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Cast(CastChange::Create(_))
        ));
    }

    // ---- target-only → Drop (managed, NOT lenient) ----

    #[test]
    fn target_only_drops() {
        let changes = run(&cat(vec![basic_cast()]), &cat(vec![]));
        assert_eq!(
            changes.len(),
            1,
            "managed cast must emit Drop when absent from source"
        );
        assert!(
            matches!(
                changes.iter().next().unwrap().change,
                Change::Cast(CastChange::Drop { .. })
            ),
            "expected Drop, got {:?}",
            changes.iter().next().unwrap().change
        );
    }

    // ---- method differs → Replace ----

    #[test]
    fn different_method_binary_vs_function_replaces() {
        let t = basic_cast(); // Binary
        let mut s = basic_cast();
        s.method = CastMethod::Function {
            name: qn("app", "my_cast_fn"),
            arg_types: vec![ColumnType::Integer],
        };
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Cast(CastChange::Replace { .. })
        ));
    }

    #[test]
    fn different_method_binary_vs_inout_replaces() {
        let t = basic_cast(); // Binary
        let mut s = basic_cast();
        s.method = CastMethod::Inout;
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Cast(CastChange::Replace { .. })
        ));
    }

    // ---- context differs → Replace ----

    #[test]
    fn different_context_explicit_vs_implicit_replaces() {
        let t = basic_cast(); // Explicit
        let mut s = basic_cast();
        s.context = CastContext::Implicit;
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Cast(CastChange::Replace { .. })
        ));
    }

    #[test]
    fn different_context_explicit_vs_assignment_replaces() {
        let t = basic_cast(); // Explicit
        let mut s = basic_cast();
        s.context = CastContext::Assignment;
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Cast(CastChange::Replace { .. })
        ));
    }

    // ---- Replace from/to direction: from=target(live), to=source(desired) ----

    #[test]
    fn replace_from_is_target_to_is_source() {
        let mut t = basic_cast();
        t.context = CastContext::Implicit; // live state
        let mut s = basic_cast();
        s.context = CastContext::Explicit; // desired state
        let changes = run(&cat(vec![t.clone()]), &cat(vec![s.clone()]));
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        if let Change::Cast(CastChange::Replace { from, to }) = &entry.change {
            assert_eq!(from, &t, "from should be the target (live) cast");
            assert_eq!(to, &s, "to should be the source (desired) cast");
        } else {
            panic!("expected Replace, got {:?}", entry.change);
        }
    }

    // ---- only comment differs → CommentOn ----

    #[test]
    fn comment_change_emits_comment_on() {
        let t = basic_cast(); // comment = None
        let mut s = basic_cast();
        s.comment = Some("converts my_type to text".into());
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        assert!(
            matches!(
                &entry.change,
                Change::Cast(CastChange::CommentOn { comment, .. }) if comment.as_deref() == Some("converts my_type to text")
            ),
            "expected CommentOn with source comment, got {:?}",
            entry.change
        );
    }

    #[test]
    fn comment_clear_emits_comment_on_none() {
        let mut t = basic_cast();
        t.comment = Some("old comment".into()); // live has comment
        let s = basic_cast(); // desired has no comment
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Cast(CastChange::CommentOn { comment: None, .. })
        ));
    }

    // ---- identical → no changes ----

    #[test]
    fn identical_casts_produce_no_changes() {
        let cast = basic_cast();
        let c = cat(vec![cast]);
        let changes = run(&c, &c);
        assert!(changes.is_empty());
    }

    // ---- different type pairs are distinct identities → Create + Drop ----

    #[test]
    fn different_source_type_are_distinct_identities() {
        // target has (app.my_type → pg_catalog.text)
        // source has (app.other_type → pg_catalog.text)
        // → Drop + Create
        let t = basic_cast();
        let mut s = basic_cast();
        s.source = qn("app", "other_type");
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(
            changes.len(),
            2,
            "different source type = distinct identity: Drop + Create"
        );
        assert!(
            changes
                .iter()
                .any(|e| matches!(&e.change, Change::Cast(CastChange::Create(_)))),
            "expected Create"
        );
        assert!(
            changes
                .iter()
                .any(|e| matches!(&e.change, Change::Cast(CastChange::Drop { .. }))),
            "expected Drop"
        );
    }
}
