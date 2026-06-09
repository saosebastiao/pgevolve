//! The single definition of the per-object owner + grants diff sequence.
//!
//! Every grantable / ownable catalog object emits the same canonical sequence:
//! an optional `AlterObjectOwner`, then the REVOKEs, then the GRANTs, then the
//! unmanaged-grant observations. This module collapses what used to be ~13
//! copy-pasted copies of that sequence into one [`diff_owner_and_grants`] helper
//! keyed on [`CatalogObjectRef`].
//!
//! The emitted [`ChangeSet`] is byte-identical to the inlined sequences this
//! replaced — same `Change` entries, same order, same observation payloads.

use std::collections::BTreeSet;

use crate::diff::change::Change;
use crate::diff::changeset::{ChangeSet, RevokeWithOwnerObservation, UnmanagedGrantObservation};
use crate::diff::destructiveness::Destructiveness;
use crate::diff::grants::diff_grants;
use crate::diff::owner_op::{AlterObjectOwner, CatalogObjectRef};
use crate::identifier::Identifier;
use crate::ir::grant::{Grant, GrantTarget};

/// Whether the object supports column-scoped grants.
///
/// Tables, views, and materialized views route grants whose `columns` field is
/// `Some(_)` to [`Change::GrantColumnPrivilege`] / [`Change::RevokeColumnPrivilege`]
/// (keyed by bare `QualifiedName`); object-scoped grants on the same object go to
/// the object-level variants. Every other object kind only ever has object-level
/// grants, so it uses [`ColumnGrantMode::ObjectOnly`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColumnGrantMode {
    /// All grants are emitted as object-level GRANT/REVOKE (schema, sequence,
    /// type, function, procedure, and the owner-only kinds).
    ObjectOnly,
    /// Grants carrying a `columns` list are emitted as column-level
    /// GRANT/REVOKE; the rest are object-level. Used by tables / views / MVs.
    ColumnAware,
}

/// Emit the owner change + grant add/revoke/unmanaged entries for one catalog
/// object, in the canonical order (owner, then revokes, then adds, then
/// unmanaged observations). The single definition of the owner+grants diff.
///
/// `object` is the [`CatalogObjectRef`] used both for the emitted `Change`
/// variants and to derive the human-readable observation label
/// (`"{lowercase keyword} {rendered name}"`). `target_grants` is the live
/// catalog's grant list (already adjusted by the caller where needed, e.g.
/// tables strip dropped-column grants before calling) and `source_grants` the
/// desired state.
///
/// `mode` selects whether column-scoped grants are routed to the column-level
/// `Change` variants ([`ColumnGrantMode::ColumnAware`]) or always object-level
/// ([`ColumnGrantMode::ObjectOnly`]).
// The eight parameters are the irreducible per-object owner+grants inputs that
// every call site already had inline; bundling them into a struct would only
// move the same fields one level up without removing any. The flat IR keeps
// owner/grants separate (decision 7b), so they arrive as distinct args.
#[allow(clippy::too_many_arguments)]
pub(crate) fn diff_owner_and_grants(
    object: &CatalogObjectRef,
    target_owner: Option<&Identifier>,
    source_owner: Option<&Identifier>,
    target_grants: &[Grant],
    source_grants: &[Grant],
    managed_roles: &BTreeSet<Identifier>,
    mode: ColumnGrantMode,
    out: &mut ChangeSet,
) {
    // ---- owner diff (lenient: only when source declares an owner that differs) ----
    if let Some(source_owner) = source_owner
        && target_owner != Some(source_owner)
    {
        out.push(
            Change::AlterObjectOwner(AlterObjectOwner {
                object: object.clone(),
                from: target_owner.cloned(),
                to: source_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // ---- grant diff ----
    let object_label = object.observation_label();
    let (to_add, to_revoke, unmanaged) = diff_grants(target_grants, source_grants, managed_roles);
    // Emit REVOKEs before GRANTs (issue #33): revokes must precede grants so
    // that WGO-change pairs (same grantee+privilege, different wgo) don't
    // self-cancel.
    for g in to_revoke {
        if let Some(source_owner) = source_owner {
            out.revokes_with_owner.push(RevokeWithOwnerObservation {
                object_label: object_label.clone(),
                privilege_label: g.privilege.sql_keyword().into(),
                grantee: g.grantee.clone(),
                owner: source_owner.clone(),
            });
        }
        out.push(revoke_change(object, mode, g), Destructiveness::Safe);
    }
    for g in to_add {
        out.push(grant_change(object, mode, g), Destructiveness::Safe);
    }
    for g in unmanaged {
        if let GrantTarget::Role(role_name) = &g.grantee {
            out.unmanaged_grants.push(UnmanagedGrantObservation {
                object_label: object_label.clone(),
                privilege_label: g.privilege.sql_keyword().into(),
                role_name: role_name.clone(),
            });
        }
    }
}

/// Route a single REVOKE to the object- or column-level `Change` variant.
fn revoke_change(object: &CatalogObjectRef, mode: ColumnGrantMode, grant: Grant) -> Change {
    if let Some(qname) = column_target(object, mode, &grant) {
        Change::RevokeColumnPrivilege { qname, grant }
    } else {
        Change::RevokeObjectPrivilege {
            object: object.clone(),
            grant,
        }
    }
}

/// Route a single GRANT to the object- or column-level `Change` variant.
fn grant_change(object: &CatalogObjectRef, mode: ColumnGrantMode, grant: Grant) -> Change {
    if let Some(qname) = column_target(object, mode, &grant) {
        Change::GrantColumnPrivilege { qname, grant }
    } else {
        Change::GrantObjectPrivilege {
            object: object.clone(),
            grant,
        }
    }
}

/// Return the qualified name to use for a column-level grant, or `None` when the
/// grant should be emitted at object level.
///
/// Only [`ColumnGrantMode::ColumnAware`] objects with a `columns` list emit
/// column-level changes; the column-aware kinds (table / view / MV) all carry a
/// `QualifiedName`, so its extraction is total for them.
fn column_target(
    object: &CatalogObjectRef,
    mode: ColumnGrantMode,
    grant: &Grant,
) -> Option<crate::identifier::QualifiedName> {
    if mode != ColumnGrantMode::ColumnAware || grant.columns.is_none() {
        return None;
    }
    match object {
        CatalogObjectRef::Table(q)
        | CatalogObjectRef::View(q)
        | CatalogObjectRef::MaterializedView(q) => Some(q.clone()),
        _ => None,
    }
}
