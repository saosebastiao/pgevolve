//! Differ for subscriptions. Pair by name; per-subscription granular diff.
//!
//! CONNECTION strings compare *modulo password*: a tiny libpq-style
//! tokenizer strips `password=…` from both sides before text-compare.
//! All other connstr keys participate in diff normally.
//!
//! Spec: `docs/superpowers/specs/2026-05-26-subscriptions-design.md`.

use std::collections::BTreeMap;

use crate::diff::change::{Change, SubscriptionChange};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::diff::owner_op::{AlterObjectOwner, OwnerObjectKind};
use crate::identifier::Identifier;
use crate::ir::catalog::Catalog;
use crate::ir::subscription::{Subscription, SubscriptionOptions};

/// Compute granular subscription changes needed to converge `target` toward
/// `source`. Appends all emitted changes to `out`.
pub fn diff_subscriptions(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<&Identifier, &Subscription> =
        target.subscriptions.iter().map(|s| (&s.name, s)).collect();
    let source_map: BTreeMap<&Identifier, &Subscription> =
        source.subscriptions.iter().map(|s| (&s.name, s)).collect();

    // Creates: in source but not in target.
    for (name, src) in &source_map {
        if !target_map.contains_key(name) {
            out.push(
                Change::Subscription(SubscriptionChange::Create((*src).clone())),
                Destructiveness::Safe,
            );
        }
    }

    // Target-only: lenient — no auto-drop. Surfaces via unmanaged-subscription lint.
    // Intentionally no-op; Stage 9 adds the unmanaged-subscription lint rule.

    // Modifies: in both.
    for (name, src) in &source_map {
        let Some(tgt) = target_map.get(name) else {
            continue;
        };
        diff_one(tgt, src, out);
    }
}

fn diff_one(target: &Subscription, source: &Subscription, out: &mut ChangeSet) {
    // CONNECTION (modulo password).
    if connection_differs_ignoring_password(&target.connection, &source.connection) {
        out.push(
            Change::Subscription(SubscriptionChange::AlterConnection {
                name: source.name.clone(),
                new_connection: source.connection.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Publications: granular ADD/DROP.
    let t_pubs: std::collections::BTreeSet<_> = target.publications.iter().collect();
    let s_pubs: std::collections::BTreeSet<_> = source.publications.iter().collect();
    for added in s_pubs.difference(&t_pubs) {
        out.push(
            Change::Subscription(SubscriptionChange::AddPublication {
                name: source.name.clone(),
                publication: (*added).clone(),
            }),
            Destructiveness::Safe,
        );
    }
    for dropped in t_pubs.difference(&s_pubs) {
        out.push(
            Change::Subscription(SubscriptionChange::DropPublication {
                name: source.name.clone(),
                publication: (*dropped).clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Options: sparse delta.
    let opts_delta = options_delta(&target.options, &source.options);
    if !options_delta_is_empty(&opts_delta) {
        out.push(
            Change::Subscription(SubscriptionChange::SetOptions {
                name: source.name.clone(),
                options: opts_delta,
            }),
            Destructiveness::Safe,
        );
    }

    // Owner (v0.3.1 lenient pattern — only emit when source declares an owner
    // and it differs from target; source `None` = unmanaged, no change emitted).
    if let Some(s_owner) = &source.owner
        && target.owner.as_ref() != Some(s_owner)
    {
        out.push(
            Change::AlterObjectOwner(AlterObjectOwner {
                kind: OwnerObjectKind::Subscription,
                id: crate::diff::owner_op::OwnedObjectId::Cluster(source.name.clone()),
                signature: String::new(),
                from: target.owner.clone(),
                to: s_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Comment.
    if target.comment != source.comment {
        out.push(
            Change::Subscription(SubscriptionChange::CommentOn {
                name: source.name.clone(),
                comment: source.comment.clone(),
            }),
            Destructiveness::Safe,
        );
    }
}

/// Compare two libpq connection strings ignoring the `password` key.
///
/// Tokenizes by `key=value` pairs separated by whitespace. Values may be
/// single-quoted with backslash-escaping for embedded quotes/backslashes
/// (libpq's documented syntax).
///
/// `${VAR}` placeholders are compared literally — a change in the env-var
/// name DOES trigger a diff (legitimate config change; operator should
/// approve via plan review).
fn connection_differs_ignoring_password(a: &str, b: &str) -> bool {
    tokenize_dropping_password(a) != tokenize_dropping_password(b)
}

fn tokenize_dropping_password(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        // Skip whitespace.
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        if chars.peek().is_none() {
            break;
        }
        // Read key (everything up to '=').
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' {
                chars.next();
                break;
            }
            key.push(c);
            chars.next();
        }
        // Read value: either single-quoted or unquoted.
        let mut value = String::new();
        if chars.peek() == Some(&'\'') {
            chars.next(); // consume opening quote
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(esc) = chars.next() {
                            value.push(esc);
                        }
                    }
                    '\'' => break, // closing quote
                    other => value.push(other),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                value.push(c);
                chars.next();
            }
        }
        if key.eq_ignore_ascii_case("password") {
            continue; // drop the password key/value pair
        }
        out.push((key, value));
    }
    out.sort();
    out
}

/// Compute the sparse options delta: only fields where `source` is `Some` and
/// differs from `target` are included.
///
/// `create_slot` and `copy_data` are CREATE-only PG options — no
/// `ALTER SUBSCRIPTION s SET (create_slot = …)` form exists. They are
/// hard-coded to `None` in the delta to prevent generating SQL PG would
/// reject.
fn options_delta(
    target: &SubscriptionOptions,
    source: &SubscriptionOptions,
) -> SubscriptionOptions {
    macro_rules! delta_field {
        ($field:ident) => {{
            if source.$field.is_some() && target.$field != source.$field {
                source.$field.clone()
            } else {
                None
            }
        }};
    }
    SubscriptionOptions {
        enabled: delta_field!(enabled),
        slot_name: delta_field!(slot_name),
        connect: None,     // CREATE-only; intentionally never diffed.
        create_slot: None, // CREATE-only; intentionally never diffed.
        copy_data: None,   // CREATE-only; intentionally never diffed.
        synchronous_commit: delta_field!(synchronous_commit),
        binary: delta_field!(binary),
        streaming: delta_field!(streaming),
        two_phase: delta_field!(two_phase),
        disable_on_error: delta_field!(disable_on_error),
        password_required: delta_field!(password_required),
        run_as_owner: delta_field!(run_as_owner),
        origin: delta_field!(origin),
        failover: delta_field!(failover),
    }
}

const fn options_delta_is_empty(d: &SubscriptionOptions) -> bool {
    d.enabled.is_none()
        && d.slot_name.is_none()
        && d.connect.is_none()
        && d.create_slot.is_none()
        && d.copy_data.is_none()
        && d.synchronous_commit.is_none()
        && d.binary.is_none()
        && d.streaming.is_none()
        && d.two_phase.is_none()
        && d.disable_on_error.is_none()
        && d.password_required.is_none()
        && d.run_as_owner.is_none()
        && d.origin.is_none()
        && d.failover.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::change::{Change, SubscriptionChange};
    use crate::identifier::Identifier;
    use crate::ir::catalog::Catalog;
    use crate::ir::subscription::{OriginMode, StreamingMode, Subscription, SubscriptionOptions};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn sub_minimal(name: &str) -> Subscription {
        Subscription {
            name: id(name),
            connection: "host=x".into(),
            publications: vec![id("p")],
            options: SubscriptionOptions::default(),
            owner: None,
            comment: None,
        }
    }

    fn catalog_with(subs: Vec<Subscription>) -> Catalog {
        let mut c = Catalog::empty();
        c.subscriptions = subs;
        c
    }

    fn run_diff(target: &Catalog, source: &Catalog) -> ChangeSet {
        let mut out = ChangeSet::new();
        diff_subscriptions(target, source, &mut out);
        out
    }

    // ---- tokenizer tests ----

    #[test]
    fn tokenize_drops_password() {
        let a = tokenize_dropping_password("host=x user=u password=secret dbname=app");
        let b = tokenize_dropping_password("host=x user=u password=different dbname=app");
        assert_eq!(a, b);
    }

    #[test]
    fn tokenize_preserves_other_keys() {
        let a = tokenize_dropping_password("host=x user=u password=p");
        let b = tokenize_dropping_password("host=y user=u password=p");
        assert_ne!(a, b);
    }

    #[test]
    fn tokenize_handles_quoted_values() {
        let a = tokenize_dropping_password("host='db.example.com' password=p");
        assert_eq!(a, vec![("host".to_string(), "db.example.com".to_string())]);
    }

    #[test]
    fn tokenize_handles_escapes_in_quoted_values() {
        let a = tokenize_dropping_password(r"host='db\'.com' password=p");
        assert_eq!(a, vec![("host".to_string(), "db'.com".to_string())]);
    }

    #[test]
    fn tokenize_case_insensitive_password_key() {
        let a = tokenize_dropping_password("host=x PASSWORD=secret");
        let b = tokenize_dropping_password("host=x Password=other");
        assert_eq!(a, b);
    }

    // ---- creates ----

    #[test]
    fn create_subscription_when_source_has_it_and_target_doesnt() {
        let target = Catalog::empty();
        let source = catalog_with(vec![sub_minimal("s")]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Subscription(SubscriptionChange::Create(_))
        ));
    }

    // ---- lenient: no auto-drop ----

    #[test]
    fn no_drop_when_target_has_sub_but_source_doesnt() {
        let target = catalog_with(vec![sub_minimal("s")]);
        let source = Catalog::empty();
        let changes = run_diff(&target, &source);
        assert!(
            changes.is_empty(),
            "expected no changes (lenient), got {changes:?}"
        );
    }

    // ---- identical subscription: no diff ----

    #[test]
    fn identical_subscription_produces_no_changes() {
        let c = catalog_with(vec![sub_minimal("s")]);
        let changes = run_diff(&c, &c);
        assert!(changes.is_empty());
    }

    // ---- connection diff ----

    #[test]
    fn connection_differs_in_non_password_key_emits_alter_connection() {
        let mut tgt = sub_minimal("s");
        tgt.connection = "host=old user=u".into();
        let mut src = sub_minimal("s");
        src.connection = "host=new user=u".into();
        let changes = run_diff(&catalog_with(vec![tgt]), &catalog_with(vec![src]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Subscription(SubscriptionChange::AlterConnection { .. })
        ));
    }

    #[test]
    fn connection_differs_only_in_password_produces_no_change() {
        let mut tgt = sub_minimal("s");
        tgt.connection = "host=x password=old".into();
        let mut src = sub_minimal("s");
        src.connection = "host=x password=new".into();
        let changes = run_diff(&catalog_with(vec![tgt]), &catalog_with(vec![src]));
        assert!(
            changes.is_empty(),
            "password-only change must not trigger diff"
        );
    }

    // ---- publications ----

    #[test]
    fn publication_added_emits_add_publication() {
        let mut tgt = sub_minimal("s");
        tgt.publications = vec![id("p1")];
        let mut src = sub_minimal("s");
        src.publications = vec![id("p1"), id("p2")];
        let changes = run_diff(&catalog_with(vec![tgt]), &catalog_with(vec![src]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Subscription(SubscriptionChange::AddPublication { .. })
        ));
    }

    #[test]
    fn publication_removed_emits_drop_publication() {
        let mut tgt = sub_minimal("s");
        tgt.publications = vec![id("p1"), id("p2")];
        let mut src = sub_minimal("s");
        src.publications = vec![id("p1")];
        let changes = run_diff(&catalog_with(vec![tgt]), &catalog_with(vec![src]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Subscription(SubscriptionChange::DropPublication { .. })
        ));
    }

    // ---- options delta ----

    #[test]
    fn single_option_changed_emits_set_options() {
        let tgt = sub_minimal("s"); // binary = None (unmanaged)
        let mut src = sub_minimal("s");
        src.options.binary = Some(true);
        let changes = run_diff(&catalog_with(vec![tgt]), &catalog_with(vec![src]));
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        if let Change::Subscription(SubscriptionChange::SetOptions { options, .. }) = &entry.change
        {
            assert_eq!(options.binary, Some(true));
            // Other fields must be None (sparse delta).
            assert!(options.enabled.is_none());
            assert!(options.streaming.is_none());
            // create_slot and copy_data always None.
            assert!(options.create_slot.is_none());
            assert!(options.copy_data.is_none());
        } else {
            panic!(
                "expected AlterSubscriptionSetOptions, got {:?}",
                entry.change
            );
        }
    }

    #[test]
    fn source_option_none_does_not_trigger_diff() {
        let mut tgt = sub_minimal("s");
        tgt.options.enabled = Some(true); // catalog has it set
        let src = sub_minimal("s"); // source has None (unmanaged)
        let changes = run_diff(&catalog_with(vec![tgt]), &catalog_with(vec![src]));
        assert!(
            changes.is_empty(),
            "source option=None must not trigger diff (lenient)"
        );
    }

    #[test]
    fn create_slot_never_in_options_delta() {
        let tgt = sub_minimal("s");
        let mut src = sub_minimal("s");
        src.options.create_slot = Some(false); // would be CREATE-only
        // diff must not produce AlterSubscriptionSetOptions for create_slot
        let delta = options_delta(&tgt.options, &src.options);
        assert!(
            delta.create_slot.is_none(),
            "create_slot must always be None in options delta"
        );
    }

    #[test]
    fn copy_data_never_in_options_delta() {
        let tgt = sub_minimal("s");
        let mut src = sub_minimal("s");
        src.options.copy_data = Some(false);
        let delta = options_delta(&tgt.options, &src.options);
        assert!(
            delta.copy_data.is_none(),
            "copy_data must always be None in options delta"
        );
    }

    // ---- owner ----

    #[test]
    fn owner_change_emits_alter_object_owner() {
        let mut tgt = sub_minimal("s");
        tgt.owner = Some(id("alice"));
        let mut src = sub_minimal("s");
        src.owner = Some(id("bob"));
        let changes = run_diff(&catalog_with(vec![tgt]), &catalog_with(vec![src]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::AlterObjectOwner(_)
        ));
    }

    #[test]
    fn no_owner_change_when_source_owner_is_none() {
        let mut tgt = sub_minimal("s");
        tgt.owner = Some(id("alice"));
        let src = sub_minimal("s"); // owner = None (unmanaged)
        let changes = run_diff(&catalog_with(vec![tgt]), &catalog_with(vec![src]));
        assert!(
            changes.is_empty(),
            "source owner None = unmanaged, no change expected"
        );
    }

    // ---- comment ----

    #[test]
    fn comment_change_emits_comment_on_subscription() {
        let tgt = sub_minimal("s");
        let mut src = sub_minimal("s");
        src.comment = Some("my sub".into());
        let changes = run_diff(&catalog_with(vec![tgt]), &catalog_with(vec![src]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Subscription(SubscriptionChange::CommentOn { .. })
        ));
    }

    // ---- multiple options changed → single AlterSubscriptionSetOptions ----

    #[test]
    fn multiple_options_changed_emit_single_set_options() {
        let tgt = sub_minimal("s");
        let mut src = sub_minimal("s");
        src.options.binary = Some(true);
        src.options.streaming = Some(StreamingMode::On);
        src.options.origin = Some(OriginMode::None);
        let changes = run_diff(&catalog_with(vec![tgt]), &catalog_with(vec![src]));
        assert_eq!(changes.len(), 1);
        if let Change::Subscription(SubscriptionChange::SetOptions { options, .. }) =
            &changes.iter().next().unwrap().change
        {
            assert_eq!(options.binary, Some(true));
            assert_eq!(options.streaming, Some(StreamingMode::On));
            assert_eq!(options.origin, Some(OriginMode::None));
            assert!(options.create_slot.is_none());
            assert!(options.copy_data.is_none());
        } else {
            panic!("expected AlterSubscriptionSetOptions");
        }
    }
}
