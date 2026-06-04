//! Canon rules for object grants.
//!
//! - Strip owner self-grants: grants where `grantee == owner` are removed.
//!   Postgres automatically grants the owner all privileges; any explicit
//!   grant to the owner is redundant. The live catalog reader strips these
//!   too (via `catalog::grants::strip_owner_self_grants`). Applying the same
//!   rule here keeps source IR and live IR symmetric.
//! - Sort `grants` by `(grantee, privilege, columns)`.
//! - Group column-level grants by `(grantee, privilege, with_grant_option)`:
//!   merge each group into one `Grant` with the sorted-deduped column union.
//!   Object-level (`columns: None`) and column-level (`columns: Some(_)`)
//!   never merge.
//! - Dedupe object-level: identical entries collapse; if any duplicate has
//!   `with_grant_option = true`, the survivor inherits `true`.

use std::collections::BTreeMap;

use crate::identifier::Identifier;
use crate::ir::grant::{Grant, GrantTarget, Privilege};

/// Remove grants where the grantee is the named owner.
///
/// Postgres automatically grants the owner all privileges on their own
/// objects. Any explicit `GRANT priv ON obj TO owner` is therefore redundant:
/// after `ALTER … OWNER TO owner`, the live DB shows the grant as an
/// owner-implicit ACL entry and the catalog reader discards it via
/// `catalog::grants::strip_owner_self_grants`. To keep source IR and live IR
/// symmetric, the canon pass applies the same rule: if `owner` is `Some(name)`,
/// every grant whose grantee equals `name` is dropped before the rest of the
/// grant-list canonicalization runs.
///
/// `PUBLIC` grants are never considered owner self-grants and are always kept.
pub fn strip_owner_self_grants(grants: &mut Vec<Grant>, owner: Option<&Identifier>) {
    let Some(owner_name) = owner else {
        return;
    };
    grants.retain(|g| match &g.grantee {
        GrantTarget::Role(name) => name != owner_name,
        GrantTarget::Public => true,
    });
}

/// Canonicalize a grant list in place.
pub fn run_on_list(grants: &mut Vec<Grant>) {
    if grants.is_empty() {
        return;
    }

    let mut object_level: Vec<Grant> = Vec::new();
    let mut col_groups: BTreeMap<(GrantTarget, Privilege), (bool, Vec<Identifier>)> =
        BTreeMap::new();

    for g in grants.drain(..) {
        match g.columns {
            None => object_level.push(g),
            Some(cols) => {
                let entry = col_groups
                    .entry((g.grantee.clone(), g.privilege))
                    .or_insert_with(|| (false, Vec::new()));
                if g.with_grant_option {
                    entry.0 = true;
                }
                entry.1.extend(cols);
            }
        }
    }

    for (key, (wgo, mut cols)) in col_groups {
        cols.sort();
        cols.dedup();
        grants.push(Grant {
            grantee: key.0,
            privilege: key.1,
            with_grant_option: wgo,
            columns: Some(cols),
        });
    }

    let mut object_seen: BTreeMap<(GrantTarget, Privilege), bool> = BTreeMap::new();
    for g in object_level {
        let entry = object_seen.entry((g.grantee, g.privilege)).or_insert(false);
        if g.with_grant_option {
            *entry = true;
        }
    }
    for ((grantee, privilege), wgo) in object_seen {
        grants.push(Grant {
            grantee,
            privilege,
            with_grant_option: wgo,
            columns: None,
        });
    }

    grants.sort();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn role_grant(name: &str, priv_: Privilege, wgo: bool, cols: Option<Vec<&str>>) -> Grant {
        Grant {
            grantee: GrantTarget::Role(id(name)),
            privilege: priv_,
            with_grant_option: wgo,
            columns: cols.map(|c| c.into_iter().map(id).collect()),
        }
    }

    #[test]
    fn empty_list_is_a_no_op() {
        let mut g = vec![];
        run_on_list(&mut g);
        assert!(g.is_empty());
    }

    #[test]
    fn duplicates_collapse() {
        let mut g = vec![
            role_grant("alice", Privilege::Select, false, None),
            role_grant("alice", Privilege::Select, false, None),
        ];
        run_on_list(&mut g);
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn duplicate_with_wgo_wins() {
        let mut g = vec![
            role_grant("alice", Privilege::Select, false, None),
            role_grant("alice", Privilege::Select, true, None),
        ];
        run_on_list(&mut g);
        assert_eq!(g.len(), 1);
        assert!(g[0].with_grant_option);
    }

    #[test]
    fn column_grants_merge_by_grantee_privilege() {
        let mut g = vec![
            role_grant("alice", Privilege::Select, false, Some(vec!["c"])),
            role_grant("alice", Privilege::Select, false, Some(vec!["a"])),
            role_grant("alice", Privilege::Select, false, Some(vec!["b"])),
        ];
        run_on_list(&mut g);
        assert_eq!(g.len(), 1);
        let cols = g[0].columns.as_ref().unwrap();
        let names: Vec<&str> = cols.iter().map(Identifier::as_str).collect();
        assert_eq!(names, vec!["a", "b", "c"]); // sorted-deduped union
    }

    #[test]
    fn object_and_column_grants_do_not_merge() {
        let mut g = vec![
            role_grant("alice", Privilege::Select, false, None),
            role_grant("alice", Privilege::Select, false, Some(vec!["c"])),
        ];
        run_on_list(&mut g);
        assert_eq!(g.len(), 2, "object-level + column-level must stay distinct");
    }

    #[test]
    fn public_sorts_before_role() {
        let mut g = vec![
            role_grant("alice", Privilege::Select, false, None),
            Grant {
                grantee: GrantTarget::Public,
                privilege: Privilege::Select,
                with_grant_option: false,
                columns: None,
            },
        ];
        run_on_list(&mut g);
        assert!(matches!(g[0].grantee, GrantTarget::Public));
    }

    // ── strip_owner_self_grants ───────────────────────────────────────────

    #[test]
    fn strip_owner_self_grants_removes_owner_grant() {
        let owner = id("app_owner");
        let mut g = vec![
            role_grant("app_owner", Privilege::Usage, false, None),
            role_grant("readers", Privilege::Select, false, None),
        ];
        strip_owner_self_grants(&mut g, Some(&owner));
        assert_eq!(g.len(), 1, "owner grant should be removed");
        assert!(
            matches!(&g[0].grantee, GrantTarget::Role(n) if n.as_str() == "readers"),
            "readers grant should remain"
        );
    }

    #[test]
    fn strip_owner_self_grants_no_owner_is_noop() {
        let mut g = vec![role_grant("app_owner", Privilege::Usage, false, None)];
        strip_owner_self_grants(&mut g, None);
        assert_eq!(g.len(), 1, "no-owner call must not modify the list");
    }

    #[test]
    fn strip_owner_self_grants_keeps_public() {
        let owner = id("app_owner");
        let mut g = vec![Grant {
            grantee: GrantTarget::Public,
            privilege: Privilege::Usage,
            with_grant_option: false,
            columns: None,
        }];
        strip_owner_self_grants(&mut g, Some(&owner));
        assert_eq!(g.len(), 1, "PUBLIC grant must survive owner-strip");
    }

    /// Regression guard for issue #36: round_trip failures where
    /// source IR carried an owner self-grant (e.g. `app_owner/Usage`
    /// on a sequence owned by `app_owner`), but the live catalog reader
    /// stripped it. The asymmetry caused `diff(live, source)` to be
    /// non-empty and `assert_convergent` to fail.
    #[test]
    fn strip_then_run_on_list_symmetric_with_live_reader() {
        let owner = id("app_owner");
        // Source IR as generated by the IR generator: owner=app_owner, grants=[app_owner/Usage,
        // readers/Select]
        let mut g = vec![
            role_grant("app_owner", Privilege::Usage, false, None),
            role_grant("readers", Privilege::Select, false, None),
        ];
        strip_owner_self_grants(&mut g, Some(&owner));
        run_on_list(&mut g);
        // After the combined pass the owner self-grant is gone
        assert_eq!(g.len(), 1, "only readers/Select survives");
        assert!(
            matches!(&g[0].grantee, GrantTarget::Role(n) if n.as_str() == "readers"),
        );
    }
}
