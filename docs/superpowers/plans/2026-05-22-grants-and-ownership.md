# Object Grants + Ownership + Default Privileges — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship v0.3.1 — declarative Postgres object permissions (GRANT/REVOKE), object ownership, and ALTER DEFAULT PRIVILEGES — touching all 8 grantable object IR types with shared infrastructure for ACL decoding, drift policy, and lint rules.

**Architecture:** Thirteen sequential stages. The shape is "wide and shallow": one shared `ir::grant` module plus a single `owner: Option<Identifier>` + `grants: Vec<Grant>` pair backfilled into every grantable IR type. The ACL decoder, parser dispatch, differ, and renderer all share infrastructure across object families. Drift is **lenient** — grants to roles not declared in source are surfaced as a lint warning, never silently revoked. An optional `[cluster].project` block in `pgevolve.toml` links to a v0.3.0 cluster project for grantee role-name validation.

**Tech Stack:** Rust 1.95+, `pg_query` 6.x, `tokio_postgres`, `clap` v4, `serde`, `toml`, `blake3`, `proptest`. Builds on v0.3.0 cluster surface.

**Source spec:** `docs/superpowers/specs/2026-05-22-grants-and-ownership-design.md`.

---

## Pre-flight

- [ ] **Step 1: Confirm clean baseline**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --tests
```

All green. v0.3.0 should be committed; main is clean.

- [ ] **Step 2: Re-read spec sections per stage**

Open `docs/superpowers/specs/2026-05-22-grants-and-ownership-design.md` once. Each stage below cites its relevant section.

---

## File structure

```
crates/pgevolve-core/src/
├── ir/
│   ├── grant.rs                NEW — Stage 1 — Grant, Privilege, GrantTarget
│   ├── default_privileges.rs   NEW — Stage 3 — DefaultPrivilegeRule, DefaultPrivObjectType
│   ├── schema.rs               MODIFY — Stage 2 — add owner + grants
│   ├── sequence.rs             MODIFY — Stage 2 — add owner + grants
│   ├── table.rs                MODIFY — Stage 2 — add owner + grants
│   ├── view.rs                 MODIFY — Stage 2 — add owner + grants on View + MaterializedView
│   ├── function.rs             MODIFY — Stage 2 — add owner + grants
│   ├── procedure.rs            MODIFY — Stage 2 — add owner + grants
│   ├── user_type.rs            MODIFY — Stage 2 — add owner + grants
│   ├── catalog.rs              MODIFY — Stage 3 — add default_privileges
│   └── canon/
│       ├── grants.rs           NEW — Stage 4 — sort + dedupe + merge column grants
│       └── default_privileges.rs NEW — Stage 4 — sort default-priv rules
├── catalog/
│   ├── grants.rs               NEW — Stage 5 — aclitem array decoder
│   ├── default_privileges.rs   NEW — Stage 6 — read pg_default_acl
│   ├── queries/
│   │   ├── schemas.rs          MODIFY — Stage 5 — add nspowner + nspacl
│   │   ├── sequences.rs        MODIFY — Stage 5 — add relowner + relacl
│   │   ├── tables.rs           MODIFY — Stage 5 — add relowner + relacl + attacl
│   │   ├── views.rs            MODIFY — Stage 5 — add relowner + relacl
│   │   ├── functions.rs        MODIFY — Stage 5 — add proowner + proacl
│   │   ├── types.rs            MODIFY — Stage 5 — add typowner + typacl
│   │   └── default_privileges.rs NEW — Stage 6 — pg_default_acl query
│   └── assemble/
│       ├── schemas.rs / sequences.rs / tables.rs / views.rs / functions.rs / user_types.rs
│       │                       MODIFY — Stage 5 — wire decoded owner + grants
│       └── default_privileges.rs NEW — Stage 6 — assemble DefaultPrivilegeRule list
├── parse/
│   └── builder/
│       ├── grants.rs           NEW — Stage 7 — shared GRANT/REVOKE statement parsing
│       ├── owner_stmt.rs       NEW — Stage 7 — shared ALTER ... OWNER TO parsing
│       └── default_privileges.rs NEW — Stage 7 — ALTER DEFAULT PRIVILEGES
├── diff/
│   ├── grants.rs               NEW — Stage 8 — Grant set-diff with lenient policy
│   ├── owner_op.rs             NEW — Stage 8 — AlterObjectOwner change kind
│   ├── default_privileges.rs   NEW — Stage 8 — diff default-priv rules
│   └── (per-family diff files)  MODIFY — Stage 8 — emit owner + grant changes
├── plan/
│   ├── raw_step.rs             MODIFY — Stage 9 — six new StepKind variants
│   └── rewrite/
│       └── grants.rs           NEW — Stage 9 — SQL helpers + emit handlers
└── lint/
    ├── rules/
    │   ├── grants_to_unmanaged_role.rs    NEW — Stage 11
    │   ├── revoke_from_owner.rs           NEW — Stage 11
    │   └── grant_references_unknown_role.rs NEW — Stage 10 (cluster-aware)
    └── universal.rs            MODIFY — Stage 11 — extend check_changeset

crates/pgevolve/src/
├── config.rs                   MODIFY — Stage 10 — [cluster] block
└── api/
    └── mod.rs                  MODIFY — Stage 12 — load cluster source if linked

crates/pgevolve-conformance/tests/cases/objects/
└── grants/                     NEW — Stage 12 — 15 fixtures across 4 sub-roots
```

---

## Stage 1 — `ir::grant` module

The shared Grant types. Pure data + Ord/Eq, no behavior yet.

**Files created:** `crates/pgevolve-core/src/ir/grant.rs`.
**Files modified:** `crates/pgevolve-core/src/ir/mod.rs` (add `pub mod grant;`).

### Task 1.1: Create the module

- [ ] **Step 1: Write `crates/pgevolve-core/src/ir/grant.rs`**

```rust
//! Object permissions — `Grant`, `Privilege`, `GrantTarget`.
//!
//! One [`Grant`] = one ACL entry on a grantable object. Shared by every
//! object kind that gains a `grants: Vec<Grant>` field in v0.3.1.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;

/// One ACL entry on a grantable object.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Grant {
    /// Who receives the privilege.
    pub grantee: GrantTarget,
    /// Which privilege.
    pub privilege: Privilege,
    /// `WITH GRANT OPTION` flag. Defaults to false.
    #[serde(default)]
    pub with_grant_option: bool,
    /// Column-level grants. `None` = object-level. `Some(cols)` = only those
    /// columns. Only valid for `Table`/`View`/`MaterializedView`; canon
    /// rejects `Some(_)` on other object kinds.
    #[serde(default)]
    pub columns: Option<Vec<Identifier>>,
}

/// Who a grant targets.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantTarget {
    /// `GRANT ... TO PUBLIC` — sorts before any named role for canon stability.
    Public,
    /// `GRANT ... TO <rolename>`.
    Role(Identifier),
}

/// The full set of privilege keywords pgevolve manages.
///
/// Database-level (`CONNECT`, `TEMPORARY`) and cluster-level (`SET`,
/// `ALTER SYSTEM`) privileges are intentionally absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Privilege {
    // Table-y privileges.
    Select,
    Insert,
    Update,
    Delete,
    Truncate,
    References,
    Trigger,
    // Schema, sequence, type, language.
    Usage,
    // Functions, procedures.
    Execute,
    // Schemas only (CREATE objects within the schema).
    Create,
}

impl Privilege {
    /// PG single-letter ACL code (the form used in `aclitem` text).
    #[must_use]
    pub const fn acl_letter(self) -> char {
        match self {
            Self::Select     => 'r',
            Self::Update     => 'w',
            Self::Insert     => 'a',
            Self::Delete     => 'd',
            Self::Truncate   => 'D',
            Self::References => 'x',
            Self::Trigger    => 't',
            Self::Execute    => 'X',
            Self::Usage      => 'U',
            Self::Create     => 'C',
        }
    }

    /// SQL keyword used in GRANT/REVOKE rendering. Always uppercase per the
    /// `sql.rs` casing convention.
    #[must_use]
    pub const fn sql_keyword(self) -> &'static str {
        match self {
            Self::Select     => "SELECT",
            Self::Insert     => "INSERT",
            Self::Update     => "UPDATE",
            Self::Delete     => "DELETE",
            Self::Truncate   => "TRUNCATE",
            Self::References => "REFERENCES",
            Self::Trigger    => "TRIGGER",
            Self::Usage      => "USAGE",
            Self::Execute    => "EXECUTE",
            Self::Create     => "CREATE",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn public_sorts_before_role() {
        let public = GrantTarget::Public;
        let role = GrantTarget::Role(id("foo"));
        assert!(public < role, "Public should sort first for canon stability");
    }

    #[test]
    fn role_targets_sort_lexicographically() {
        let a = GrantTarget::Role(id("alice"));
        let b = GrantTarget::Role(id("bob"));
        assert!(a < b);
    }

    #[test]
    fn acl_letters_match_pg() {
        assert_eq!(Privilege::Select.acl_letter(), 'r');
        assert_eq!(Privilege::Insert.acl_letter(), 'a');
        assert_eq!(Privilege::Update.acl_letter(), 'w');
        assert_eq!(Privilege::Delete.acl_letter(), 'd');
        assert_eq!(Privilege::Truncate.acl_letter(), 'D');
        assert_eq!(Privilege::References.acl_letter(), 'x');
        assert_eq!(Privilege::Trigger.acl_letter(), 't');
        assert_eq!(Privilege::Execute.acl_letter(), 'X');
        assert_eq!(Privilege::Usage.acl_letter(), 'U');
        assert_eq!(Privilege::Create.acl_letter(), 'C');
    }

    #[test]
    fn grants_sort_by_grantee_then_privilege() {
        let g1 = Grant {
            grantee: GrantTarget::Role(id("alice")),
            privilege: Privilege::Update,
            with_grant_option: false,
            columns: None,
        };
        let g2 = Grant {
            grantee: GrantTarget::Role(id("alice")),
            privilege: Privilege::Select,
            with_grant_option: false,
            columns: None,
        };
        let g3 = Grant {
            grantee: GrantTarget::Public,
            privilege: Privilege::Select,
            with_grant_option: false,
            columns: None,
        };
        let mut grants = vec![g1.clone(), g2.clone(), g3.clone()];
        grants.sort();
        assert_eq!(grants, vec![g3, g2, g1]); // Public, then alice/Select, then alice/Update
    }
}
```

- [ ] **Step 2: Wire `pub mod grant;` into `crates/pgevolve-core/src/ir/mod.rs`** (alphabetical position).

- [ ] **Step 3: Run + commit**

```bash
cargo test -p pgevolve-core --lib ir::grant
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add -p crates/pgevolve-core/src/ir/
git commit -m "$(cat <<'EOF'
feat(ir): grant — Grant, Privilege, GrantTarget

Foundation for v0.3.1 object permissions. Grant carries grantee +
privilege + with_grant_option + optional columns for column-level
grants on tables/views/MVs. GrantTarget::Public sorts before
any Role(name) for canon stability. Privilege exposes acl_letter()
(for PG aclitem decoding) and sql_keyword() (for rendering).

DATABASE-level (CONNECT/TEMPORARY) and cluster-level (SET, ALTER
SYSTEM) privileges intentionally absent.

Stage 1 of docs/superpowers/plans/2026-05-22-grants-and-ownership.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 2 — Backfill `owner` + `grants` into 8 IR types

The widest change in the plan. Each grantable IR type gets `pub owner: Option<Identifier>` and `pub grants: Vec<Grant>`. Every literal `<Type> { ... }` in the workspace gets these fields backfilled (compiler-enforced).

**Files modified:**
- `crates/pgevolve-core/src/ir/{schema, sequence, table, view, function, procedure, user_type}.rs` (View module covers both View and MaterializedView).
- Every Rust file in the workspace that constructs one of these struct literals.

### Task 2.1: Per-IR fields

For each of the 8 grantable structs:

- [ ] **Step 1: Add fields**

Example for `Schema` (apply the same pattern to all 8):

```rust
// in crates/pgevolve-core/src/ir/schema.rs::Schema
pub struct Schema {
    pub name: Identifier,
    // ... existing fields ...
    /// Schema owner. `None` = unmanaged (the differ ignores ownership).
    /// `Some(role)` = managed: diff emits `ALTER SCHEMA … OWNER TO role`.
    #[diff(via_debug)]
    pub owner: Option<Identifier>,
    /// Grants on this object. Empty = no grants. Canonicalized.
    #[diff(via_debug)]
    pub grants: Vec<crate::ir::grant::Grant>,
}
```

The two fields go **at the end** of the struct (after `comment`, if there is one). Same shape for `Sequence`, `Table`, `View`, `MaterializedView` (both in `view.rs`), `Function`, `Procedure`, `UserType`.

- [ ] **Step 2: Update `base()` / test helper constructors in each `tests` module**

Each IR file has a `tests` module with helper constructors (`fn base() -> Self`, `fn role(...) -> Role`, etc.). Add the two new fields to each, defaulting to `None` and `Vec::new()`.

- [ ] **Step 3: Workspace-wide backfill via compiler errors**

```bash
cargo check --workspace --all-targets 2>&1 | head -60
```

Compiler will surface every `<Type> { ... }` literal that's missing the new fields. Fix each by adding `owner: None, grants: Vec::new()` (or `vec![]`). Likely sites:

- `diff/columns.rs`, `diff/tables.rs`, `diff/schemas.rs`, etc. — test helper builders
- `catalog/assemble/*.rs` — catalog-read constructors (these will get real values in Stage 5; use `None`/`Vec::new()` placeholders for now)
- `parse/builder/*.rs` — source-parser constructors
- `crates/pgevolve-testkit/src/ir_generator.rs` — property-test generators
- `crates/pgevolve-conformance/tests/*` — fixture constructors (if any inline-build IR)
- `crates/pgevolve-core/src/render/table.rs` — render tests' inline `Catalog` builders

Iterate `cargo check` → fix one site at a time → re-check until clean.

- [ ] **Step 4: Per-type Diff tests**

In each IR struct's `tests` module, add:

```rust
#[test]
fn owner_change_diffs() {
    let mut b = base();
    b.owner = Some(id("new_owner"));
    assert!(base().diff(&b).iter().any(|x| x.path == "owner"));
}

#[test]
fn grants_change_diffs() {
    let mut b = base();
    b.grants.push(crate::ir::grant::Grant {
        grantee: crate::ir::grant::GrantTarget::Public,
        privilege: crate::ir::grant::Privilege::Select,
        with_grant_option: false,
        columns: None,
    });
    assert!(base().diff(&b).iter().any(|x| x.path == "grants"));
}
```

Add this pair to **each** of the 8 grantable types' test modules. (8 × 2 = 16 new tests.)

- [ ] **Step 5: Run + verify**

```bash
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Green required. If clippy flags `clippy::struct_field_names` or `clippy::pub_underscore_fields` warnings on the new fields, justify with `#[allow(...)]` + brief comment, or restructure.

### Task 2.2: Commit

- [ ] **Step 1: Commit**

```bash
git add -p crates/
git commit -m "$(cat <<'EOF'
feat(ir): add owner + grants to 8 grantable IR types

Schema, Sequence, Table, View, MaterializedView, Function, Procedure,
UserType each gain:

  pub owner: Option<Identifier>     // None = unmanaged
  pub grants: Vec<Grant>            // empty = no grants

Both #[diff(via_debug)]. None on owner means "differ skips ownership"
— per-object opt-in. Backfills every struct literal across the
workspace (compiler-enforced; ~25 sites).

Stage 2 of docs/superpowers/plans/2026-05-22-grants-and-ownership.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 3 — Default privileges IR

Top-level `Catalog.default_privileges: Vec<DefaultPrivilegeRule>` field plus the supporting types.

**Files created:** `crates/pgevolve-core/src/ir/default_privileges.rs`.
**Files modified:** `crates/pgevolve-core/src/ir/catalog.rs`, `crates/pgevolve-core/src/ir/mod.rs`.

### Task 3.1: Create the module

- [ ] **Step 1: `crates/pgevolve-core/src/ir/default_privileges.rs`**

```rust
//! `ALTER DEFAULT PRIVILEGES` — future-object grants.
//!
//! pg_default_acl rows. Distinct from per-object `grants`: these say
//! "future objects of type X in schema Y created by role Z get these
//! grants automatically."

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::grant::Grant;

/// One `ALTER DEFAULT PRIVILEGES` rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultPrivilegeRule {
    /// `FOR ROLE x` — whose future objects this applies to.
    pub target_role: Identifier,
    /// `IN SCHEMA y` — scope. `None` = "all schemas owned by `target_role`".
    pub schema: Option<Identifier>,
    /// Object type this rule applies to.
    pub object_type: DefaultPrivObjectType,
    /// Grants applied. Canonicalized (sorted, deduped).
    pub grants: Vec<Grant>,
}

/// Object-type discriminant for default-privilege rules.
///
/// PG's grouping: `TABLES` covers tables + views + MVs;
/// `FUNCTIONS` covers functions + procedures (alias `ROUTINES` in PG 11+).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultPrivObjectType {
    Tables,
    Sequences,
    Functions,
    Types,
    /// PG 14+ only.
    Schemas,
}

impl DefaultPrivObjectType {
    /// PG `pg_default_acl.defaclobjtype` single-char code.
    #[must_use]
    pub const fn pg_char(self) -> char {
        match self {
            Self::Tables    => 'r',
            Self::Sequences => 'S',
            Self::Functions => 'f',
            Self::Types     => 'T',
            Self::Schemas   => 'n',
        }
    }

    /// Decode from `pg_default_acl.defaclobjtype`.
    pub fn from_pg_char(c: char) -> Option<Self> {
        Some(match c {
            'r' => Self::Tables,
            'S' => Self::Sequences,
            'f' => Self::Functions,
            'T' => Self::Types,
            'n' => Self::Schemas,
            _ => return None,
        })
    }

    /// SQL keyword in `ALTER DEFAULT PRIVILEGES ... GRANT ... ON <KIND> ...`.
    #[must_use]
    pub const fn sql_keyword(self) -> &'static str {
        match self {
            Self::Tables    => "TABLES",
            Self::Sequences => "SEQUENCES",
            Self::Functions => "FUNCTIONS",
            Self::Types     => "TYPES",
            Self::Schemas   => "SCHEMAS",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_char_roundtrips() {
        for kind in [
            DefaultPrivObjectType::Tables,
            DefaultPrivObjectType::Sequences,
            DefaultPrivObjectType::Functions,
            DefaultPrivObjectType::Types,
            DefaultPrivObjectType::Schemas,
        ] {
            assert_eq!(DefaultPrivObjectType::from_pg_char(kind.pg_char()), Some(kind));
        }
    }

    #[test]
    fn from_pg_char_rejects_unknown() {
        assert_eq!(DefaultPrivObjectType::from_pg_char('q'), None);
    }
}
```

- [ ] **Step 2: Wire into `ir/mod.rs`**

Add `pub mod default_privileges;` in alphabetical position.

- [ ] **Step 3: Add `Catalog.default_privileges` field**

In `crates/pgevolve-core/src/ir/catalog.rs`, extend `Catalog`:

```rust
pub struct Catalog {
    pub schemas: Vec<Schema>,
    // ... existing fields ...
    /// `ALTER DEFAULT PRIVILEGES` rules. Canonicalized.
    pub default_privileges: Vec<crate::ir::default_privileges::DefaultPrivilegeRule>,
}
```

(Position the new field after the last existing field.)

- [ ] **Step 4: Update `Default for Catalog`** to include `default_privileges: vec![]`.

- [ ] **Step 5: Backfill every `Catalog { ... }` literal in the workspace**

```bash
cargo check --workspace --all-targets 2>&1 | grep -E "missing field|missing fields" | head -20
```

Add `default_privileges: vec![]` to each one.

- [ ] **Step 6: Run + commit**

```bash
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/
git commit -m "$(cat <<'EOF'
feat(ir): default_privileges — ALTER DEFAULT PRIVILEGES IR

New ir::default_privileges module with DefaultPrivilegeRule and
DefaultPrivObjectType (Tables/Sequences/Functions/Types/Schemas).
Catalog gains a default_privileges: Vec<DefaultPrivilegeRule> field.

DefaultPrivObjectType maps to/from pg_default_acl.defaclobjtype
single-char codes. PG-side FUNCTIONS keyword covers both functions
and procedures.

Stage 3 of docs/superpowers/plans/2026-05-22-grants-and-ownership.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 4 — Canon

Sort + dedupe grants on every object; sort default-privilege rules.

**Files created:** `crates/pgevolve-core/src/ir/canon/grants.rs`, `crates/pgevolve-core/src/ir/canon/default_privileges.rs`.
**Files modified:** `crates/pgevolve-core/src/ir/canon/mod.rs`, `crates/pgevolve-core/src/ir/catalog.rs` (canonicalize calls).

### Task 4.1: Grant canon

- [ ] **Step 1: Create `crates/pgevolve-core/src/ir/canon/grants.rs`**

```rust
//! Canon rules for object grants.
//!
//! - Sort `grants` by `(grantee, privilege, columns)`.
//! - Group by `(grantee, privilege, with_grant_option)`: if any group has
//!   only column-level entries, merge them into one `Grant` with the
//!   sorted-deduped union of columns. Object-level (`columns: None`) and
//!   column-level (`columns: Some(_)`) never merge.
//! - Dedupe: identical entries collapse; if any duplicate has
//!   `with_grant_option = true`, the survivor inherits `true`.

use std::collections::BTreeMap;

use crate::identifier::Identifier;
use crate::ir::grant::{Grant, GrantTarget, Privilege};

/// Canonicalize a grant list in place.
pub fn run_on_list(grants: &mut Vec<Grant>) {
    if grants.is_empty() {
        return;
    }

    // First pass: group column-level grants by (grantee, privilege, WGO)
    // and union their columns; promote WGO to true if any group member has it.
    let mut object_level: Vec<Grant> = Vec::new();
    let mut col_groups: BTreeMap<(GrantTarget, Privilege), (bool, Vec<Identifier>)> = BTreeMap::new();

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

    // Dedupe + sort columns within each column-level group.
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

    // Object-level: dedupe identical entries, promoting WGO.
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

    // Sort final list.
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
}
```

### Task 4.2: Default-privileges canon

- [ ] **Step 1: Create `crates/pgevolve-core/src/ir/canon/default_privileges.rs`**

```rust
//! Canon rules for `default_privileges`.

use crate::ir::default_privileges::DefaultPrivilegeRule;

/// Sort default-privilege rules by `(target_role, schema, object_type)` and
/// canonicalize each rule's grants list.
pub fn run(rules: &mut Vec<DefaultPrivilegeRule>) {
    for rule in rules.iter_mut() {
        super::grants::run_on_list(&mut rule.grants);
    }
    rules.sort_by(|a, b| {
        a.target_role.as_str().cmp(b.target_role.as_str())
            .then_with(|| match (&a.schema, &b.schema) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(x), Some(y)) => x.as_str().cmp(y.as_str()),
            })
            .then_with(|| a.object_type.pg_char().cmp(&b.object_type.pg_char()))
    });
}
```

Add two tests: `sorts_by_target_then_schema_then_type` and `delegates_grant_list_to_grants_canon`.

### Task 4.3: Wire canon calls

- [ ] **Step 1: Update `crates/pgevolve-core/src/ir/canon/mod.rs`**

Add:

```rust
pub mod grants;
pub mod default_privileges;
```

- [ ] **Step 2: Call from `Catalog::canonicalize`**

In `crates/pgevolve-core/src/ir/catalog.rs::Catalog::canonicalize`, after existing canon passes (read the file to find the exact location), add:

```rust
// Canonicalize per-object grant lists.
for s in &mut self.schemas { canon::grants::run_on_list(&mut s.grants); }
for s in &mut self.sequences { canon::grants::run_on_list(&mut s.grants); }
for t in &mut self.tables { canon::grants::run_on_list(&mut t.grants); }
for v in &mut self.views { canon::grants::run_on_list(&mut v.grants); }
for m in &mut self.materialized_views { canon::grants::run_on_list(&mut m.grants); }
for f in &mut self.functions { canon::grants::run_on_list(&mut f.grants); }
for p in &mut self.procedures { canon::grants::run_on_list(&mut p.grants); }
for t in &mut self.user_types { canon::grants::run_on_list(&mut t.grants); }
canon::default_privileges::run(&mut self.default_privileges);
```

Adjust field names to match `Catalog`'s actual ones (read the file). Some may be `materialized_views`, others may be different.

### Task 4.4: Run + commit

```bash
cargo test -p pgevolve-core --lib ir::canon
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve-core/src/ir/canon/ crates/pgevolve-core/src/ir/catalog.rs
git commit -m "$(cat <<'EOF'
feat(canon): grants + default_privileges canonicalization

Two new canon modules:

  grants::run_on_list — sort + dedupe per-object grant lists. Column-
  level grants for the same (grantee, privilege, WGO) merge into a
  single Grant with the sorted-deduped column union. Object-level
  and column-level entries never merge.

  default_privileges::run — sort rules by (target_role, schema, type)
  and delegate per-rule grants to grants::run_on_list.

Catalog::canonicalize now runs both passes across all 8 grantable
families.

Stage 4 of docs/superpowers/plans/2026-05-22-grants-and-ownership.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 5 — Catalog reader: owner + grants on 8 families

Add the ACL decoder, then extend each per-family catalog query + assemble to populate `owner` and `grants`. This is wide work — 6 query files modify, 6 assemble files modify.

**Files created:** `crates/pgevolve-core/src/catalog/grants.rs` (the ACL decoder).
**Files modified:**
- `crates/pgevolve-core/src/catalog/queries/{schemas, sequences, tables, views, functions, types}.rs`
- `crates/pgevolve-core/src/catalog/assemble/{schemas, sequences, tables, views, functions, user_types}.rs`
- `crates/pgevolve-core/src/catalog/mod.rs`

### Task 5.1: ACL decoder

- [ ] **Step 1: Create `crates/pgevolve-core/src/catalog/grants.rs`**

```rust
//! Decode PG `aclitem` text into `Grant` structs.
//!
//! `aclitem` text form: `grantee=privileges/grantor`. Empty grantee means
//! PUBLIC. Privilege letters: `r`=Select, `w`=Update, `a`=Insert, `d`=Delete,
//! `D`=Truncate, `x`=References, `t`=Trigger, `X`=Execute, `U`=Usage,
//! `C`=Create. An asterisk after a letter marks `WITH GRANT OPTION`
//! (e.g., `r*` = SELECT WITH GRANT OPTION).

use crate::catalog::error::CatalogError;
use crate::identifier::Identifier;
use crate::ir::grant::{Grant, GrantTarget, Privilege};

/// Decode an array of aclitem strings into `Grant` entries.
/// `columns: None` for object-level; caller is responsible for marking
/// column-level grants with `Some(vec![colname])` when decoding
/// `pg_attribute.attacl`.
pub(crate) fn decode_aclitem_array(items: &[String]) -> Result<Vec<Grant>, CatalogError> {
    let mut out = Vec::with_capacity(items.len());
    for raw in items {
        out.extend(decode_one(raw)?);
    }
    Ok(out)
}

fn decode_one(raw: &str) -> Result<Vec<Grant>, CatalogError> {
    // `<grantee>=<privs>/<grantor>` — grantor side is ignored.
    let body = raw.split('/').next().ok_or_else(|| {
        CatalogError::BadColumnType(format!("malformed aclitem {raw:?}"))
    })?;
    let (grantee_str, privs) = body.split_once('=').ok_or_else(|| {
        CatalogError::BadColumnType(format!("malformed aclitem {raw:?}"))
    })?;

    let grantee = if grantee_str.is_empty() {
        GrantTarget::Public
    } else {
        // PG quotes role names with embedded special chars; strip a single
        // leading/trailing double-quote pair if present, then trust the rest.
        let trimmed = grantee_str.trim_start_matches('"').trim_end_matches('"');
        GrantTarget::Role(Identifier::from_unquoted(trimmed).map_err(|e| {
            CatalogError::BadColumnType(format!("aclitem grantee {grantee_str:?}: {e}"))
        })?)
    };

    let mut out = Vec::new();
    let mut chars = privs.chars().peekable();
    while let Some(c) = chars.next() {
        let priv_ = match c {
            'r' => Privilege::Select,
            'w' => Privilege::Update,
            'a' => Privilege::Insert,
            'd' => Privilege::Delete,
            'D' => Privilege::Truncate,
            'x' => Privilege::References,
            't' => Privilege::Trigger,
            'X' => Privilege::Execute,
            'U' => Privilege::Usage,
            'C' => Privilege::Create,
            // Privilege letters pgevolve doesn't manage at this layer:
            //   'T' (TEMPORARY on database)
            //   'c' (CONNECT on database)
            //   's' (SET on parameter)
            //   'A' (ALTER SYSTEM on parameter)
            // Silently skip and consume any trailing '*'.
            _ => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                }
                continue;
            }
        };
        let with_grant_option = chars.peek() == Some(&'*');
        if with_grant_option {
            chars.next();
        }
        out.push(Grant {
            grantee: grantee.clone(),
            privilege: priv_,
            with_grant_option,
            columns: None,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_select() {
        let g = decode_one("=r/owner").unwrap();
        assert_eq!(g.len(), 1);
        assert!(matches!(g[0].grantee, GrantTarget::Public));
        assert_eq!(g[0].privilege, Privilege::Select);
        assert!(!g[0].with_grant_option);
    }

    #[test]
    fn role_multiple_privileges() {
        let g = decode_one("alice=arwd/owner").unwrap();
        assert_eq!(g.len(), 4);
        let privs: Vec<Privilege> = g.iter().map(|x| x.privilege).collect();
        assert!(privs.contains(&Privilege::Insert));
        assert!(privs.contains(&Privilege::Select));
        assert!(privs.contains(&Privilege::Update));
        assert!(privs.contains(&Privilege::Delete));
    }

    #[test]
    fn with_grant_option_flag() {
        let g = decode_one("alice=r*/owner").unwrap();
        assert_eq!(g.len(), 1);
        assert!(g[0].with_grant_option);
    }

    #[test]
    fn unmanaged_privileges_skipped() {
        // 'T' (TEMPORARY) is db-level — skip silently.
        let g = decode_one("alice=Tr/owner").unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].privilege, Privilege::Select);
    }

    #[test]
    fn malformed_aclitem_errors() {
        assert!(decode_one("no_equals_sign").is_err());
    }

    #[test]
    fn array_decode_combines() {
        let arr = vec!["alice=r/o".to_string(), "=a/o".to_string()];
        let g = decode_aclitem_array(&arr).unwrap();
        assert_eq!(g.len(), 2);
    }
}
```

- [ ] **Step 2: Wire `pub(crate) mod grants;` into `crates/pgevolve-core/src/catalog/mod.rs`**

### Task 5.2: Per-family query updates

Each family's existing query gains:
- `<obj>owner` (join to `pg_authid` for role name)
- `<obj>acl::text[]` (the ACL itself, cast to text array)

The exact column names vary per family:

| Family       | Owner column        | ACL column        |
|--------------|---------------------|-------------------|
| Schemas      | `nspowner`          | `nspacl::text[]`  |
| Sequences    | `relowner`          | `relacl::text[]`  |
| Tables       | `relowner`          | `relacl::text[]`  |
| Views/MVs    | `relowner`          | `relacl::text[]`  |
| Functions    | `proowner`          | `proacl::text[]`  |
| User types   | `typowner`          | `typacl::text[]`  |

Pattern for each query (e.g., `schemas.rs`):

```sql
SELECT n.nspname,
       owner_role.rolname AS owner,
       coalesce(n.nspacl::text[], '{}'::text[]) AS acl,
       d.description AS comment
FROM pg_namespace n
JOIN pg_authid owner_role ON owner_role.oid = n.nspowner
LEFT JOIN pg_shdescription d ON d.objoid = n.oid AND d.classoid = 'pg_namespace'::regclass
WHERE n.nspname = ANY($1::text[])
```

Repeat the pattern for each of the 6 query files.

For tables/views/MVs, **also** query column-level ACLs from `pg_attribute.attacl::text[]` joined per column. The existing COLUMNS_QUERY needs a new `attacl::text[]` column.

### Task 5.3: Per-family assemble updates

For each family's `catalog/assemble/<fam>.rs::build_<fam>`, after the existing decode:

```rust
let owner_str = row.get_text(q, "owner")?;
let owner = Some(Identifier::from_unquoted(&owner_str).map_err(|e| {
    CatalogError::BadColumnType(format!("invalid owner {owner_str:?}: {e}"))
})?);

let acl_strings: Vec<String> = row.get_text_array(q, "acl")?;
let grants = crate::catalog::grants::decode_aclitem_array(&acl_strings)?;

// ... assemble the struct including these new fields ...
```

`row.get_text_array(q, "acl")?` — confirm the existing row API supports text arrays. If not, add a helper to `pg_querier.rs` mirroring `get_text` / `get_bool` shape.

For **tables/views/MVs**, also produce column-level grants:

```rust
for col in &raw_columns {
    if let Some(col_acl) = &col.attacl {
        let col_grants = crate::catalog::grants::decode_aclitem_array(col_acl)?;
        for mut g in col_grants {
            g.columns = Some(vec![col.name.clone()]);
            grants.push(g);
        }
    }
}
```

(Canon merges these per-column entries into combined column-list grants downstream.)

### Task 5.4: Docker-gated integration tests

Add a single Docker-gated test in `crates/pgevolve-core/tests/catalog_grants.rs` exercising:

```rust
#[tokio::test]
#[cfg_attr(not(feature = "docker"), ignore)]
async fn reads_table_grants_and_owner() {
    let pg = ephemeral_pg().await;
    pg.exec("CREATE SCHEMA app").await;
    pg.exec("CREATE ROLE app_owner").await;
    pg.exec("CREATE ROLE readers").await;
    pg.exec("ALTER SCHEMA app OWNER TO app_owner").await;
    pg.exec("CREATE TABLE app.t (id bigint, name text)").await;
    pg.exec("ALTER TABLE app.t OWNER TO app_owner").await;
    pg.exec("GRANT SELECT ON app.t TO readers").await;
    pg.exec("GRANT INSERT (name) ON app.t TO readers").await;

    let cat = read_catalog(pg.querier(), &["app".to_string()]).await.unwrap();
    let t = cat.tables.iter().find(|t| t.qname.name.as_str() == "t").unwrap();

    assert_eq!(t.owner.as_ref().map(|i| i.as_str()), Some("app_owner"));
    // After canon, expect two grants: one SELECT object-level + one INSERT(name) column-level.
    assert_eq!(t.grants.len(), 2);
    assert!(t.grants.iter().any(|g| g.privilege == Privilege::Select && g.columns.is_none()));
    assert!(t.grants.iter().any(|g| g.privilege == Privilege::Insert && g.columns.is_some()));
}
```

### Task 5.5: Run + commit

```bash
cargo test -p pgevolve-core --lib catalog
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve-core/src/catalog/
git commit -m "$(cat <<'EOF'
feat(catalog): read owner + grants on 8 grantable families

New catalog::grants module decodes pg aclitem text format into
Grant entries (handles WITH GRANT OPTION '*' flags, PUBLIC empty
grantee, skips unmanaged privilege letters like 'T' and 'c').

Six per-family queries gain <obj>owner + <obj>acl columns; assemble
populates Role.owner (Some(...) from catalog) + grants Vec.
Tables/views/MVs also decode pg_attribute.attacl per column into
column-level Grant entries that canon merges by (grantee, privilege).

Stage 5 of docs/superpowers/plans/2026-05-22-grants-and-ownership.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 6 — Default-privileges catalog read

**Files created:**
- `crates/pgevolve-core/src/catalog/queries/default_privileges.rs`
- `crates/pgevolve-core/src/catalog/assemble/default_privileges.rs`

**Files modified:** `crates/pgevolve-core/src/catalog/{mod, queries/mod, assemble/mod}.rs`.

### Task 6.1: Query + assemble

- [ ] **Step 1: Create the query**

```rust
// crates/pgevolve-core/src/catalog/queries/default_privileges.rs
pub const DEFAULT_PRIVILEGES_QUERY: &str = r"
SELECT r.rolname AS target_role,
       n.nspname AS schema_name,
       d.defaclobjtype::text AS object_type,
       coalesce(d.defaclacl::text[], '{}'::text[]) AS acl
FROM pg_default_acl d
JOIN pg_authid r ON r.oid = d.defaclrole
LEFT JOIN pg_namespace n ON n.oid = d.defaclnamespace
WHERE r.rolname NOT LIKE 'pg\_%' ESCAPE '\'
ORDER BY r.rolname, n.nspname, d.defaclobjtype
";
```

- [ ] **Step 2: Create the assembler**

```rust
// crates/pgevolve-core/src/catalog/assemble/default_privileges.rs
use crate::catalog::error::CatalogError;
use crate::identifier::Identifier;
use crate::ir::default_privileges::{DefaultPrivObjectType, DefaultPrivilegeRule};

pub(crate) fn build_default_privileges(
    rows: &[/* whatever row type the project uses */],
    q: /* query handle */,
) -> Result<Vec<DefaultPrivilegeRule>, CatalogError> {
    let mut out = Vec::new();
    for row in rows {
        let target_role_str = row.get_text(q, "target_role")?;
        let target_role = Identifier::from_unquoted(&target_role_str)
            .map_err(|e| CatalogError::BadColumnType(format!("invalid target role: {e}")))?;

        let schema = row.get_opt_text(q, "schema_name")?
            .map(|s| Identifier::from_unquoted(&s))
            .transpose()
            .map_err(|e| CatalogError::BadColumnType(format!("invalid schema: {e}")))?;

        let object_type_str = row.get_text(q, "object_type")?;
        let object_type_char = object_type_str.chars().next().ok_or_else(|| {
            CatalogError::BadColumnType("empty object_type".into())
        })?;
        let object_type = DefaultPrivObjectType::from_pg_char(object_type_char)
            .ok_or_else(|| CatalogError::BadColumnType(format!("unknown object_type {object_type_char:?}")))?;

        let acl_strings: Vec<String> = row.get_text_array(q, "acl")?;
        let grants = crate::catalog::grants::decode_aclitem_array(&acl_strings)?;

        out.push(DefaultPrivilegeRule { target_role, schema, object_type, grants });
    }
    Ok(out)
}
```

Adjust the row-API calls to match the project's pattern.

- [ ] **Step 3: Wire into `read_catalog`**

Find the existing `read_catalog` entry point. After all family-builds, before the final `canonicalize()`:

```rust
let default_priv_rows = querier.fetch(CatalogQuery::DefaultPrivileges, &[]).await?;
catalog.default_privileges = build_default_privileges(&default_priv_rows, CatalogQuery::DefaultPrivileges)?;
```

Add a `CatalogQuery::DefaultPrivileges` variant to the `CatalogQuery` enum and dispatch it from `queries/mod.rs::query_for`.

- [ ] **Step 4: Docker-gated test**

Add to `catalog_grants.rs`:

```rust
#[tokio::test]
#[cfg_attr(not(feature = "docker"), ignore)]
async fn reads_default_privileges() {
    let pg = ephemeral_pg().await;
    pg.exec("CREATE SCHEMA app").await;
    pg.exec("CREATE ROLE app_owner").await;
    pg.exec("CREATE ROLE readers").await;
    pg.exec("ALTER DEFAULT PRIVILEGES FOR ROLE app_owner IN SCHEMA app GRANT SELECT ON TABLES TO readers").await;

    let cat = read_catalog(pg.querier(), &["app".to_string()]).await.unwrap();
    let dp = &cat.default_privileges;
    assert_eq!(dp.len(), 1);
    assert_eq!(dp[0].target_role.as_str(), "app_owner");
    assert_eq!(dp[0].schema.as_ref().map(|s| s.as_str()), Some("app"));
    assert_eq!(dp[0].object_type, DefaultPrivObjectType::Tables);
}
```

### Task 6.2: Run + commit

```bash
cargo test -p pgevolve-core --lib catalog
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve-core/src/catalog/
git commit -m "$(cat <<'EOF'
feat(catalog): read pg_default_acl into default_privileges

New query against pg_default_acl + assembler that decodes
defaclobjtype char and reuses catalog::grants::decode_aclitem_array
for the acl text array. pg_* prefixed roles filtered.

Stage 6 of docs/superpowers/plans/2026-05-22-grants-and-ownership.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 7 — Source parser

Three new parse paths integrated into the existing per-family dispatch:

1. `GRANT priv ON object TO grantee [WITH GRANT OPTION]` — shared `parse/builder/grants.rs`.
2. `ALTER <objkind> name OWNER TO role` — shared `parse/builder/owner_stmt.rs`.
3. `ALTER DEFAULT PRIVILEGES ... GRANT ...` — `parse/builder/default_privileges.rs`.

REVOKE in source → `ParseError`. `GRANT ALL` expands to the explicit privilege list. Column-level grants (`GRANT priv (col) ON TABLE`) supported.

**Files created:**
- `crates/pgevolve-core/src/parse/builder/grants.rs`
- `crates/pgevolve-core/src/parse/builder/owner_stmt.rs`
- `crates/pgevolve-core/src/parse/builder/default_privileges.rs`

**Files modified:** `crates/pgevolve-core/src/parse/mod.rs` (dispatch new statement kinds), per-family builders that handle OWNER as part of an existing ALTER statement.

### Task 7.1: Implement `parse/builder/grants.rs`

The parser hooks into pg_query's `GrantStmt` node (object-level GRANT/REVOKE — distinct from the v0.3.0 `GrantRoleStmt` for role membership).

`pg_query::protobuf::GrantStmt` shape (verify in actual bindings):
- `is_grant: bool` (false → REVOKE)
- `targtype: i32` (target enum: ACL_TARGET_OBJECT, ACL_TARGET_ALL_IN_SCHEMA, ACL_TARGET_DEFAULTS)
- `objtype: i32` (OBJECT_TABLE, OBJECT_SCHEMA, OBJECT_FUNCTION, etc.)
- `objects: Vec<Node>` (the named objects)
- `privileges: Vec<Node>` (AccessPriv nodes; empty = ALL)
- `grantees: Vec<Node>` (RoleSpec nodes)
- `grant_option: bool`

Skeleton:

```rust
//! `GRANT priv ON object TO grantee` — object-level grants.
//!
//! Maps to pg_query::NodeEnum::GrantStmt. Updates the named object's
//! `grants: Vec<Grant>` field. REVOKE is rejected with a clear error
//! (revokes come from diff, not source).

use pg_query::protobuf::{GrantStmt, ObjectType};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::grant::{Grant, GrantTarget, Privilege};
use crate::parse::error::{ParseError, SourceLocation};

pub(crate) fn apply(s: &GrantStmt, cat: &mut Catalog, loc: &SourceLocation) -> Result<(), ParseError> {
    if !s.is_grant {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "REVOKE in source is not supported — revokes happen via diff".into(),
        });
    }
    if s.targtype != 0 {
        // ACL_TARGET_OBJECT = 0. ACL_TARGET_ALL_IN_SCHEMA / DEFAULTS not supported here.
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "GRANT ... ON ALL IN SCHEMA and FOR DEFAULTS forms are not supported".into(),
        });
    }

    let object_type = ObjectType::try_from(s.objtype).unwrap_or(ObjectType::Undefined);
    let privileges = decode_privileges(&s.privileges, object_type, loc)?;
    let grantees = decode_grantees(&s.grantees, loc)?;
    let with_grant_option = s.grant_option;

    for object_node in &s.objects {
        let target_qname = extract_target_qname(object_node, object_type, loc)?;
        for grantee in &grantees {
            for (priv_, columns) in &privileges {
                let g = Grant {
                    grantee: grantee.clone(),
                    privilege: *priv_,
                    with_grant_option,
                    columns: columns.clone(),
                };
                attach_grant(cat, object_type, &target_qname, g, loc)?;
            }
        }
    }
    Ok(())
}

// Helpers below: decode_privileges, decode_grantees, extract_target_qname,
// attach_grant. attach_grant dispatches by ObjectType to find the right
// IR collection on `cat` and push the new Grant onto its grants list.
```

The full implementation is mechanical but verbose — about 200–300 lines. Key helpers:

- `decode_privileges(&[Node], ObjectType, &Loc) -> Vec<(Privilege, Option<Vec<Identifier>>)>` — `AccessPriv` nodes carry the privilege name and optional column list. An empty `privileges` list in pg_query means `GRANT ALL`; expand to the privileges applicable to the object type.
- `decode_grantees(&[Node], &Loc) -> Vec<GrantTarget>` — `RoleSpec` nodes; rolename = "public" decodes to `GrantTarget::Public`.
- `extract_target_qname(&Node, ObjectType, &Loc) -> QualifiedName` — varies by object type; tables/views/MVs/sequences use `RangeVar`, functions use `ObjectWithArgs`, schemas use a bare string.
- `attach_grant(&mut Catalog, ObjectType, &QualifiedName, Grant, &Loc)` — find the named object, push to its grants. Error if not found.

`GRANT ALL` per object type:
- Table/View/MV: Select, Insert, Update, Delete, Truncate, References, Trigger
- Schema: Usage, Create
- Sequence: Usage, Select, Update
- Function/Procedure: Execute
- Type: Usage

Add 10 unit tests covering: basic table GRANT, GRANT ALL expansion, column-level GRANT, GRANT TO PUBLIC, multi-privilege expansion, multi-grantee expansion, schema GRANT, function GRANT with argument types, REVOKE rejected, unsupported target form rejected.

### Task 7.2: Implement `parse/builder/owner_stmt.rs`

Handle `ALTER <objkind> name OWNER TO role`. Each object family already has an ALTER statement path; the OWNER-TO subcommand should already get routed through pg_query's `AlterOwnerStmt` (separate node) or as a subform of `AlterTableStmt::AlterOwner`.

pg_query has both:
- `AlterOwnerStmt { object_type, object, newowner }` for schemas, types, functions, etc.
- `AlterTableStmt::AlterOwner` subcommand for tables/views/MVs/sequences.

Handle both. Skeleton for the standalone path:

```rust
//! `ALTER <objkind> name OWNER TO role` — sets the named object's owner.

use pg_query::protobuf::{AlterOwnerStmt, ObjectType};

use crate::identifier::Identifier;
use crate::ir::catalog::Catalog;
use crate::parse::error::{ParseError, SourceLocation};

pub(crate) fn apply(s: &AlterOwnerStmt, cat: &mut Catalog, loc: &SourceLocation) -> Result<(), ParseError> {
    let new_owner = extract_role_name(&s.newowner, loc)?;
    let object_type = ObjectType::try_from(s.object_type).unwrap_or(ObjectType::Undefined);
    let qname = extract_target_qname(&s.object, object_type, loc)?;
    set_owner(cat, object_type, &qname, new_owner, loc)
}

fn set_owner(
    cat: &mut Catalog,
    obj_type: ObjectType,
    qname: &crate::identifier::QualifiedName,
    new_owner: Identifier,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    use ObjectType::*;
    match obj_type {
        ObjectSchema => {
            let s = cat.schemas.iter_mut().find(|s| s.name == qname.name).ok_or_else(|| {
                ParseError::Structural { location: loc.clone(), message: format!("unknown schema {qname}") }
            })?;
            s.owner = Some(new_owner);
        }
        ObjectTable => { /* same shape against cat.tables */ }
        ObjectView => { /* ... */ }
        ObjectMatview => { /* ... */ }
        ObjectSequence => { /* ... */ }
        ObjectFunction => { /* match by qname + arg signature */ }
        ObjectProcedure => { /* same */ }
        ObjectType_ => { /* against cat.user_types */ }
        _ => return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("OWNER TO not supported for {obj_type:?}"),
        }),
    }
    Ok(())
}
```

Add 5 unit tests: schema owner, table owner, function owner with signature, unknown object → error, unsupported object type → error.

### Task 7.3: Implement `parse/builder/default_privileges.rs`

Handle `ALTER DEFAULT PRIVILEGES [FOR ROLE x] [IN SCHEMA y] GRANT ... TO z`. pg_query node: `AlterDefaultPrivilegesStmt { options, action }` where `action` is a `GrantStmt` and `options` carries `for_role` + `schemas`.

```rust
pub(crate) fn apply(
    s: &pg_query::protobuf::AlterDefaultPrivilegesStmt,
    cat: &mut Catalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    // Decode options: [DefElem { defname: "schemas" | "roles", arg: List }]
    let (target_roles, schemas) = decode_alter_default_options(&s.options, loc)?;
    let action = s.action.as_ref().ok_or_else(|| ParseError::Structural {
        location: loc.clone(),
        message: "ALTER DEFAULT PRIVILEGES missing action".into(),
    })?;
    if !action.is_grant {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "REVOKE in ALTER DEFAULT PRIVILEGES is not supported in source".into(),
        });
    }

    let object_type = decode_default_priv_object_type(action.objtype, loc)?;
    let privileges = decode_privileges_for_default(&action.privileges, object_type, loc)?;
    let grantees = decode_grantees(&action.grantees, loc)?;
    let with_grant_option = action.grant_option;

    // For each (target_role, schema, object_type) tuple, append a rule.
    let scope_schemas: Vec<Option<Identifier>> = if schemas.is_empty() {
        vec![None]
    } else {
        schemas.into_iter().map(Some).collect()
    };

    for target_role in &target_roles {
        for schema in &scope_schemas {
            let mut grants = Vec::new();
            for grantee in &grantees {
                for priv_ in &privileges {
                    grants.push(Grant {
                        grantee: grantee.clone(),
                        privilege: *priv_,
                        with_grant_option,
                        columns: None,
                    });
                }
            }
            cat.default_privileges.push(DefaultPrivilegeRule {
                target_role: target_role.clone(),
                schema: schema.clone(),
                object_type,
                grants,
            });
        }
    }
    Ok(())
}
```

Default-priv rules use a different `DefaultPrivObjectType` (TABLES vs OBJECT_TABLE in GRANT) — `decode_default_priv_object_type` maps `OBJECT_TABLE → Tables`, `OBJECT_SEQUENCE → Sequences`, etc.

5 unit tests: in-schema tables, global functions, for-role explicit, missing for-role uses current_user (pgevolve doesn't have a current_user — error if missing), REVOKE rejected.

### Task 7.4: Wire dispatch

In `crates/pgevolve-core/src/parse/mod.rs` (or wherever the top-level statement dispatch lives), find the `NodeEnum::match` and add:

```rust
pg_query::NodeEnum::GrantStmt(s) => grants::apply(s, cat, &loc)?,
pg_query::NodeEnum::AlterOwnerStmt(s) => owner_stmt::apply(s, cat, &loc)?,
pg_query::NodeEnum::AlterDefaultPrivilegesStmt(s) => default_privileges::apply(s, cat, &loc)?,
```

For tables/views/MVs/sequences: extend the existing `AlterTableStmt` handler to recognize `AT_ChangeOwner` subcommand and call into `owner_stmt::set_owner`.

### Task 7.5: Run + commit

```bash
cargo test -p pgevolve-core --lib parse::builder::grants
cargo test -p pgevolve-core --lib parse::builder::owner_stmt
cargo test -p pgevolve-core --lib parse::builder::default_privileges
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve-core/src/parse/
git commit -m "$(cat <<'EOF'
feat(parse): GRANT/REVOKE + ALTER OWNER + ALTER DEFAULT PRIVILEGES

Three new parse paths:
  parse::builder::grants — object-level GRANT, including column-level
    grants on tables/views/MVs and GRANT ALL expansion to the explicit
    privilege list per object kind.
  parse::builder::owner_stmt — ALTER <objkind> ... OWNER TO role, both
    via standalone AlterOwnerStmt and via AlterTableStmt::AlterOwner
    subcommands for relation-family objects.
  parse::builder::default_privileges — ALTER DEFAULT PRIVILEGES ...
    GRANT (with multi-schema / multi-role expansion to one
    DefaultPrivilegeRule per cross-product entry).

REVOKE is rejected in source (revokes come from diff). Object-level
GRANTs on unmanaged object kinds (DATABASE/TABLESPACE/LANGUAGE) error.

Stage 7 of docs/superpowers/plans/2026-05-22-grants-and-ownership.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 8 — Differ

Per-object diff extensions for owner + grants; new top-level differ for `default_privileges`.

**Files created:**
- `crates/pgevolve-core/src/diff/grants.rs` — set-diff with lenient policy
- `crates/pgevolve-core/src/diff/owner_op.rs` — `AlterObjectOwner` change
- `crates/pgevolve-core/src/diff/default_privileges.rs`

**Files modified:** per-family diff files (`diff/tables.rs`, `diff/schemas.rs`, etc.) emit owner + grant changes; `diff/mod.rs` exposes the new modules.

### Task 8.1: Shared grant-diff helper

- [ ] **Step 1: `crates/pgevolve-core/src/diff/grants.rs`**

```rust
//! Shared grant-list diffing with lenient drift policy.

use std::collections::BTreeSet;

use crate::identifier::Identifier;
use crate::ir::grant::{Grant, GrantTarget};

/// Compute additions and removals between target (catalog) and source (desired).
///
/// `managed_roles`: the set of role names mentioned anywhere in the source
/// catalog. Grants whose grantee is not in this set are considered unmanaged
/// and excluded from the revoke side (lenient policy). Public is always
/// considered "managed."
///
/// Returns `(to_add, to_revoke, unmanaged_observed)`.
#[must_use]
pub fn diff_grants(
    target: &[Grant],
    source: &[Grant],
    managed_roles: &BTreeSet<Identifier>,
) -> (Vec<Grant>, Vec<Grant>, Vec<Grant>) {
    let target_set: BTreeSet<&Grant> = target.iter().collect();
    let source_set: BTreeSet<&Grant> = source.iter().collect();

    let to_add: Vec<Grant> = source_set.difference(&target_set).map(|g| (*g).clone()).collect();

    let mut to_revoke = Vec::new();
    let mut unmanaged_observed = Vec::new();
    for g in target_set.difference(&source_set) {
        if grantee_is_managed(&g.grantee, managed_roles) {
            to_revoke.push((*g).clone());
        } else {
            unmanaged_observed.push((*g).clone());
        }
    }
    (to_add, to_revoke, unmanaged_observed)
}

fn grantee_is_managed(target: &GrantTarget, managed_roles: &BTreeSet<Identifier>) -> bool {
    match target {
        GrantTarget::Public => true,
        GrantTarget::Role(name) => managed_roles.contains(name),
    }
}

/// Collect every role name referenced anywhere in the source catalog —
/// in grants, owners, and default-privilege rules. Used as input to
/// `diff_grants`.
#[must_use]
pub fn collect_managed_roles(cat: &crate::ir::catalog::Catalog) -> BTreeSet<Identifier> {
    let mut out = BTreeSet::new();
    let mut collect_from_grants = |grants: &[Grant], out: &mut BTreeSet<Identifier>| {
        for g in grants {
            if let GrantTarget::Role(name) = &g.grantee {
                out.insert(name.clone());
            }
        }
    };
    for s in &cat.schemas { collect_from_grants(&s.grants, &mut out); if let Some(o) = &s.owner { out.insert(o.clone()); } }
    for s in &cat.sequences { collect_from_grants(&s.grants, &mut out); if let Some(o) = &s.owner { out.insert(o.clone()); } }
    for t in &cat.tables { collect_from_grants(&t.grants, &mut out); if let Some(o) = &t.owner { out.insert(o.clone()); } }
    for v in &cat.views { collect_from_grants(&v.grants, &mut out); if let Some(o) = &v.owner { out.insert(o.clone()); } }
    for m in &cat.materialized_views { collect_from_grants(&m.grants, &mut out); if let Some(o) = &m.owner { out.insert(o.clone()); } }
    for f in &cat.functions { collect_from_grants(&f.grants, &mut out); if let Some(o) = &f.owner { out.insert(o.clone()); } }
    for p in &cat.procedures { collect_from_grants(&p.grants, &mut out); if let Some(o) = &p.owner { out.insert(o.clone()); } }
    for t in &cat.user_types { collect_from_grants(&t.grants, &mut out); if let Some(o) = &t.owner { out.insert(o.clone()); } }
    for r in &cat.default_privileges {
        out.insert(r.target_role.clone());
        collect_from_grants(&r.grants, &mut out);
    }
    out
}
```

Adjust field names (`materialized_views` vs `mvs`, etc.) to actual Catalog shape.

5 unit tests: empty, add only, revoke only managed, ignore unmanaged grantee (returned in `unmanaged_observed`), PUBLIC always treated as managed.

### Task 8.2: Owner-change operation

- [ ] **Step 1: `crates/pgevolve-core/src/diff/owner_op.rs`**

```rust
//! `AlterObjectOwner` — uniform owner-change op across grantable families.

use crate::identifier::{Identifier, QualifiedName};

/// Object kind discriminant for the renderer (`ALTER TABLE x OWNER TO`,
/// `ALTER SCHEMA x OWNER TO`, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerObjectKind {
    Schema,
    Sequence,
    Table,
    View,
    MaterializedView,
    Function,
    Procedure,
    UserType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlterObjectOwner {
    pub kind: OwnerObjectKind,
    /// For functions/procedures, include the argument signature in the
    /// rendered SQL. For all others, the qname is sufficient.
    pub qname: QualifiedName,
    /// Optional signature suffix for routines (e.g. `(int, text)`). Empty
    /// for non-routine kinds.
    #[serde(default)]
    pub signature: String,
    pub from: Identifier,
    pub to: Identifier,
}
```

Serialize derives as needed by the project's RawStep emit path.

### Task 8.3: Per-family diff integration

For each family's diff function (e.g., `diff/schemas.rs::diff_schema`, `diff/tables.rs::diff_table`), after existing field diffs:

```rust
// Owner.
if let Some(source_owner) = &source.owner {
    if target.owner.as_ref() != Some(source_owner) {
        out.push(AlterObjectOwner {
            kind: OwnerObjectKind::Schema, // or Table, etc.
            qname: target.qname().clone(),
            signature: "".into(),
            from: target.owner.clone().unwrap_or_else(|| Identifier::from_unquoted("UNKNOWN").unwrap()),
            to: source_owner.clone(),
        });
    }
}
// Grants.
let (to_add, to_revoke, unmanaged) = crate::diff::grants::diff_grants(&target.grants, &source.grants, managed_roles);
for g in to_add { out.push(GrantObjectPrivilege { kind: ..., qname: ..., grant: g }); }
for g in to_revoke { out.push(RevokeObjectPrivilege { kind: ..., qname: ..., grant: g }); }
// unmanaged accumulator surfaces in Stage 11 lint.
```

`managed_roles` is computed once in the top-level diff entry point (likely `diff/mod.rs::diff`) by calling `collect_managed_roles(source_catalog)` and passing the resulting `BTreeSet` down through each per-family diff.

Add new change variants (`GrantObjectPrivilege`, `RevokeObjectPrivilege`, `GrantColumnPrivilege`, `RevokeColumnPrivilege`) to whatever enum `diff/changeset.rs` uses for per-family ops, plus a top-level `AlterObjectOwner` variant.

For column-level grants: when `Grant.columns.is_some()`, emit `GrantColumnPrivilege`/`RevokeColumnPrivilege` instead of object-level variants.

### Task 8.4: Default-privileges differ

- [ ] **Step 1: `crates/pgevolve-core/src/diff/default_privileges.rs`**

Pair-by-`(target_role, schema, object_type)`, then `diff_grants` per pair.

```rust
pub fn diff_default_privileges(
    target: &[DefaultPrivilegeRule],
    source: &[DefaultPrivilegeRule],
    managed_roles: &BTreeSet<Identifier>,
    out: &mut Vec<Change>, // whichever enum the project uses
) {
    let key = |r: &DefaultPrivilegeRule| (r.target_role.clone(), r.schema.clone(), r.object_type);
    let target_map: BTreeMap<_, _> = target.iter().map(|r| (key(r), r)).collect();
    let source_map: BTreeMap<_, _> = source.iter().map(|r| (key(r), r)).collect();

    let mut all_keys: BTreeSet<_> = target_map.keys().cloned().collect();
    all_keys.extend(source_map.keys().cloned());

    for k in all_keys {
        let target_grants = target_map.get(&k).map(|r| r.grants.as_slice()).unwrap_or(&[]);
        let source_grants = source_map.get(&k).map(|r| r.grants.as_slice()).unwrap_or(&[]);
        let (add, rev, _unmanaged) = crate::diff::grants::diff_grants(target_grants, source_grants, managed_roles);
        for g in add {
            out.push(/* AlterDefaultPrivileges Grant op */);
        }
        for g in rev {
            out.push(/* AlterDefaultPrivileges Revoke op */);
        }
    }
}
```

(Default-privileges diff doesn't track unmanaged grantees the same way — those would also surface through per-rule lints if needed.)

### Task 8.5: Extend `Changeset` with observation fields for Stage 11 lints

The two changeset-level lints in Stage 11 (`grants-to-unmanaged-role`, `revoke-from-owner`) read from structured observations on the per-DB `Changeset`. Add them now so Stage 11 has something to read.

- [ ] **Step 1: Extend `crates/pgevolve-core/src/diff/changeset.rs` (or wherever the Changeset struct lives)**

```rust
pub struct Changeset {
    // ... existing fields ...

    /// Catalog grants whose grantee was not declared in source. The diff
    /// did NOT emit a REVOKE for these (lenient policy); the lint surfaces
    /// them so operators can choose to declare the role or accept drift.
    #[serde(default)]
    pub unmanaged_grants: Vec<UnmanagedGrantObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnmanagedGrantObservation {
    /// Human-readable object descriptor for the finding message, e.g. "table app.t".
    pub object_label: String,
    /// PG privilege keyword, e.g. "SELECT".
    pub privilege_label: String,
    /// Unmanaged grantee role name.
    pub role_name: Identifier,
}
```

The per-family differs populate `unmanaged_grants` whenever `diff_grants` returns a non-empty third vec.

- [ ] **Step 2: Add a revoke-with-owner-context iterator**

`revoke-from-owner` lint needs each revoke step paired with the target object's owner. Either:

(a) Stash `(revoke_change, owner)` pairs on the Changeset during per-family diff, or
(b) Have the lint walk the `Changeset` + `target` catalog at lint time.

Pick (a) — keeps the lint pure-changeset (no extra catalog parameter):

```rust
pub struct Changeset {
    // ... existing fields ...
    pub unmanaged_grants: Vec<UnmanagedGrantObservation>,
    /// Pairs of (revoke step, object owner) — used by revoke-from-owner lint.
    /// Populated whenever a per-family differ emits a Revoke change and the
    /// object's `owner` was set.
    #[serde(default)]
    pub revokes_with_owner: Vec<RevokeWithOwnerObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokeWithOwnerObservation {
    pub object_label: String,
    pub privilege_label: String,
    pub grantee: crate::ir::grant::GrantTarget,
    pub owner: Identifier,
}
```

The per-family differs populate this whenever they emit a Revoke and the source/target carries an owner.

### Task 8.6: Run + commit

```bash
cargo test -p pgevolve-core --lib diff
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve-core/src/diff/
git commit -m "$(cat <<'EOF'
feat(diff): grants + ownership + default privileges

Three new diff modules:
  diff::grants::diff_grants — set-diff with lenient drift policy.
    Unmanaged-grantee catalog entries return through a third output
    Vec for downstream lint surface (Stage 11) rather than producing
    REVOKE steps.
  diff::owner_op::AlterObjectOwner — uniform owner-change op across
    8 grantable families. Source.owner = None means "skip ownership".
  diff::default_privileges::diff_default_privileges — pair-by
    (target_role, schema, object_type) then per-pair grant diff.

Per-family differs now compute owner + grant changes after existing
field diffs. managed_roles set is built once in the top-level diff
entry point via collect_managed_roles().

Stage 8 of docs/superpowers/plans/2026-05-22-grants-and-ownership.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 9 — Render / emit + StepKind variants

Add SQL helpers and emit handlers for the six new change kinds.

**Files created:** `crates/pgevolve-core/src/plan/rewrite/grants.rs`.
**Files modified:** `crates/pgevolve-core/src/plan/raw_step.rs` (six new variants), `crates/pgevolve-core/src/plan/rewrite/emit/mod.rs` (dispatch).

### Task 9.1: New StepKind variants

- [ ] **Step 1: Extend `StepKind` in `crates/pgevolve-core/src/plan/raw_step.rs`**

After the existing per-DB variants (and the v0.3.0 cluster variants):

```rust
    AlterObjectOwner,
    GrantObjectPrivilege,
    RevokeObjectPrivilege,
    GrantColumnPrivilege,
    RevokeColumnPrivilege,
    AlterDefaultPrivileges,
```

Extend the existing round-trip serialization test to include all 6.

Extend `plan::plan::kind_name` + `parse_kind_name` exhaustive matches with the 6 new snake_case strings.

### Task 9.2: SQL helpers

- [ ] **Step 1: Create `crates/pgevolve-core/src/plan/rewrite/grants.rs`**

```rust
//! SQL rendering for object grants + ownership + default privileges.

use crate::diff::owner_op::OwnerObjectKind;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_privileges::DefaultPrivObjectType;
use crate::ir::grant::{Grant, GrantTarget, Privilege};

/// `ALTER <objkind> qname OWNER TO new_owner;`
#[must_use]
pub fn alter_object_owner(
    kind: OwnerObjectKind,
    qname: &QualifiedName,
    signature: &str,
    new_owner: &Identifier,
) -> String {
    let objkind_token = match kind {
        OwnerObjectKind::Schema => "SCHEMA",
        OwnerObjectKind::Sequence => "SEQUENCE",
        OwnerObjectKind::Table => "TABLE",
        OwnerObjectKind::View => "VIEW",
        OwnerObjectKind::MaterializedView => "MATERIALIZED VIEW",
        OwnerObjectKind::Function => "FUNCTION",
        OwnerObjectKind::Procedure => "PROCEDURE",
        OwnerObjectKind::UserType => "TYPE",
    };
    let suffix = if signature.is_empty() { "" } else { signature };
    if matches!(kind, OwnerObjectKind::Schema) {
        format!("ALTER SCHEMA {} OWNER TO {};", qname.name.render_sql(), new_owner.render_sql())
    } else {
        format!(
            "ALTER {objkind_token} {}{suffix} OWNER TO {};",
            qname.render_sql(),
            new_owner.render_sql()
        )
    }
}

/// `GRANT priv ON <objkind> qname TO grantee [WITH GRANT OPTION];`
#[must_use]
pub fn grant_object_privilege(
    kind: OwnerObjectKind,
    qname: &QualifiedName,
    signature: &str,
    grant: &Grant,
) -> String {
    // Same dispatch table as alter_object_owner for kind → SQL keyword.
    let objkind_token = owner_kind_to_sql(kind);
    let grantee_sql = render_grantee(&grant.grantee);
    let suffix = if signature.is_empty() { "" } else { signature };
    let wgo = if grant.with_grant_option { " WITH GRANT OPTION" } else { "" };
    if matches!(kind, OwnerObjectKind::Schema) {
        format!(
            "GRANT {} ON SCHEMA {} TO {grantee_sql}{wgo};",
            grant.privilege.sql_keyword(),
            qname.name.render_sql(),
        )
    } else {
        format!(
            "GRANT {} ON {objkind_token} {}{suffix} TO {grantee_sql}{wgo};",
            grant.privilege.sql_keyword(),
            qname.render_sql(),
        )
    }
}

/// `REVOKE priv ON <objkind> qname FROM grantee;`
#[must_use]
pub fn revoke_object_privilege(
    kind: OwnerObjectKind,
    qname: &QualifiedName,
    signature: &str,
    grant: &Grant,
) -> String {
    // Same shape as grant_object_privilege; "GRANT" → "REVOKE", "TO" → "FROM".
    let objkind_token = owner_kind_to_sql(kind);
    let grantee_sql = render_grantee(&grant.grantee);
    let suffix = if signature.is_empty() { "" } else { signature };
    if matches!(kind, OwnerObjectKind::Schema) {
        format!(
            "REVOKE {} ON SCHEMA {} FROM {grantee_sql};",
            grant.privilege.sql_keyword(),
            qname.name.render_sql(),
        )
    } else {
        format!(
            "REVOKE {} ON {objkind_token} {}{suffix} FROM {grantee_sql};",
            grant.privilege.sql_keyword(),
            qname.render_sql(),
        )
    }
}

/// `GRANT priv (col, col) ON TABLE qname TO grantee;`
#[must_use]
pub fn grant_column_privilege(
    qname: &QualifiedName,
    grant: &Grant,
) -> String {
    let cols = grant.columns.as_ref().expect("column grant requires columns");
    let col_list: Vec<String> = cols.iter().map(Identifier::render_sql).collect();
    let grantee_sql = render_grantee(&grant.grantee);
    let wgo = if grant.with_grant_option { " WITH GRANT OPTION" } else { "" };
    format!(
        "GRANT {} ({}) ON TABLE {} TO {grantee_sql}{wgo};",
        grant.privilege.sql_keyword(),
        col_list.join(", "),
        qname.render_sql(),
    )
}

/// `REVOKE priv (col, col) ON TABLE qname FROM grantee;`
#[must_use]
pub fn revoke_column_privilege(qname: &QualifiedName, grant: &Grant) -> String {
    let cols = grant.columns.as_ref().expect("column grant requires columns");
    let col_list: Vec<String> = cols.iter().map(Identifier::render_sql).collect();
    let grantee_sql = render_grantee(&grant.grantee);
    format!(
        "REVOKE {} ({}) ON TABLE {} FROM {grantee_sql};",
        grant.privilege.sql_keyword(),
        col_list.join(", "),
        qname.render_sql(),
    )
}

/// `ALTER DEFAULT PRIVILEGES FOR ROLE x IN SCHEMA y GRANT priv ON TABLES TO z;`
#[must_use]
pub fn alter_default_privileges(
    target_role: &Identifier,
    schema: Option<&Identifier>,
    object_type: DefaultPrivObjectType,
    is_grant: bool,
    grant: &Grant,
) -> String {
    let mut sql = format!("ALTER DEFAULT PRIVILEGES FOR ROLE {}", target_role.render_sql());
    if let Some(sch) = schema {
        sql.push_str(&format!(" IN SCHEMA {}", sch.render_sql()));
    }
    let verb = if is_grant { "GRANT" } else { "REVOKE" };
    let direction = if is_grant { "TO" } else { "FROM" };
    let wgo = if is_grant && grant.with_grant_option { " WITH GRANT OPTION" } else { "" };
    sql.push_str(&format!(
        " {verb} {} ON {} {direction} {}{wgo};",
        grant.privilege.sql_keyword(),
        object_type.sql_keyword(),
        render_grantee(&grant.grantee),
    ));
    sql
}

fn owner_kind_to_sql(k: OwnerObjectKind) -> &'static str {
    match k {
        OwnerObjectKind::Schema => "SCHEMA",
        OwnerObjectKind::Sequence => "SEQUENCE",
        OwnerObjectKind::Table => "TABLE",
        OwnerObjectKind::View => "VIEW",
        OwnerObjectKind::MaterializedView => "MATERIALIZED VIEW",
        OwnerObjectKind::Function => "FUNCTION",
        OwnerObjectKind::Procedure => "PROCEDURE",
        OwnerObjectKind::UserType => "TYPE",
    }
}

fn render_grantee(g: &GrantTarget) -> String {
    match g {
        GrantTarget::Public => "PUBLIC".to_string(),
        GrantTarget::Role(id) => id.render_sql(),
    }
}
```

`expect` on `Grant.columns` in the two column helpers is acceptable — they're called only by emit handlers that have already pattern-matched on `Some(_)`. Add a `// SAFETY: ...` comment.

12 unit tests: 1 per helper × happy path + 3 edge cases (PUBLIC grantee, WITH GRANT OPTION, function signature suffix).

### Task 9.3: Emit handlers + dispatch

Extend the per-family emit modules (or add to `plan/rewrite/emit/mod.rs` if shared) to dispatch the new change kinds to the new SQL helpers, producing `RawStep` with `transactional: InTransaction`, `destructive: false`.

### Task 9.4: Run + commit

```bash
cargo test -p pgevolve-core --lib plan
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve-core/src/plan/
git commit -m "$(cat <<'EOF'
feat(plan): grants + ownership + default privileges — render + emit

Six new StepKind variants (AlterObjectOwner, GrantObjectPrivilege,
RevokeObjectPrivilege, GrantColumnPrivilege, RevokeColumnPrivilege,
AlterDefaultPrivileges). New plan::rewrite::grants module renders each.

SCHEMA owner/grants render without the schema qualifier prefix
(schema qnames are single-part). Routines (functions/procedures)
include the argument signature in the rendered SQL. PG keywords
uppercase per the established sql.rs casing convention.

All ops run InTransaction. None destructive.

Stage 9 of docs/superpowers/plans/2026-05-22-grants-and-ownership.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 10 — Cluster-link config + cross-cluster lint

**Files modified:** `crates/pgevolve/src/config.rs` (new `[cluster]` block).
**Files created:** `crates/pgevolve-core/src/lint/rules/grant_references_unknown_role.rs`.

### Task 10.1: Config schema

- [ ] **Step 1: Extend `crates/pgevolve/src/config.rs`**

```rust
#[derive(Debug, Deserialize)]
pub struct PgevolveConfig {
    // ... existing sections ...
    #[serde(default)]
    pub cluster: Option<ClusterLink>,
}

#[derive(Debug, Deserialize)]
pub struct ClusterLink {
    /// Path to the cluster project this DB belongs to. Relative paths
    /// resolve against pgevolve.toml's directory.
    pub project: String,
}
```

Add a test parsing `[cluster] project = "../my-cluster"`. Add a test parsing config without `[cluster]` and asserting `cluster: None`.

### Task 10.2: `grant-references-unknown-role` lint

The lint runs only when the cluster source has been loaded (Stage 12 wires this through `build_plan`).

- [ ] **Step 1: Create `crates/pgevolve-core/src/lint/rules/grant_references_unknown_role.rs`**

```rust
//! Lint: a grant's grantee role isn't declared in the linked cluster source.

use std::collections::BTreeSet;

use crate::identifier::Identifier;
use crate::ir::catalog::Catalog;
use crate::ir::grant::GrantTarget;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "grant-references-unknown-role";

/// Check requires `cluster_role_names` — the set of role names declared
/// in the linked cluster project's roles/*.sql. When the user hasn't set
/// `[cluster].project`, the caller passes `None`, and this rule emits
/// nothing.
pub(crate) fn check(
    cat: &Catalog,
    cluster_role_names: Option<&BTreeSet<Identifier>>,
) -> Vec<Finding> {
    let Some(cluster_roles) = cluster_role_names else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    let mut visit_grants = |obj_label: &str, grants: &[crate::ir::grant::Grant], qname: &str| {
        for g in grants {
            if let GrantTarget::Role(name) = &g.grantee {
                if !cluster_roles.contains(name) {
                    findings.push(Finding {
                        rule: RULE_ID,
                        severity: Severity::Error,
                        message: format!(
                            "{obj_label} {qname}: GRANT to role {name} \
                             but that role is not declared in the linked cluster project"
                        ),
                        location: None,
                    });
                }
            }
        }
    };
    for s in &cat.schemas { visit_grants("schema", &s.grants, s.name.as_str()); }
    // ... repeat for all 8 grantable families ...
    // Also check owners and default-priv grantees.
    findings
}
```

Add 4 tests: cluster_roles is None → silent; known role → silent; unknown role → error; PUBLIC always silent.

### Task 10.3: Wire into universal dispatcher

In `crates/pgevolve-core/src/lint/universal.rs`, the existing `check_universal` function signature already takes `&SourceTree`. Add a new signature that also accepts the optional cluster roles set:

```rust
pub fn check_universal_with_cluster(
    source: &SourceTree,
    cluster_role_names: Option<&BTreeSet<Identifier>>,
) -> Vec<Finding> {
    let mut out = check_universal(source);
    out.extend(rules::grant_references_unknown_role::check(&source.catalog, cluster_role_names));
    out
}
```

Keep `check_universal` as a thin wrapper that calls the new function with `None`.

### Task 10.4: Run + commit

```bash
cargo test -p pgevolve --lib config
cargo test -p pgevolve-core --lib lint::rules::grant_references_unknown_role
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/
git commit -m "$(cat <<'EOF'
feat(lint+config): cross-cluster role validation

New optional [cluster] section in pgevolve.toml:
  [cluster]
  project = "../my-cluster"

When set, the new grant-references-unknown-role lint cross-checks
every grantee role name (in object grants, owners, default-priv
grantees) against the linked cluster project's roles/*.sql.
Missing role → Error severity, catching typos pre-apply.

When absent, the lint silently no-ops — per-DB independence preserved.

Stage 10 of docs/superpowers/plans/2026-05-22-grants-and-ownership.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 11 — Two more lint rules

**Files created:**
- `crates/pgevolve-core/src/lint/rules/grants_to_unmanaged_role.rs`
- `crates/pgevolve-core/src/lint/rules/revoke_from_owner.rs`

**Files modified:** `crates/pgevolve-core/src/lint/{rules/mod.rs, universal.rs}`.

### Task 11.1: `grants-to-unmanaged-role`

Fires on the changeset-side `unmanaged_observed` accumulator from Stage 8.

```rust
//! Warns when the catalog has grants to roles not declared in the source.

use crate::diff::changeset::Changeset;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "grants-to-unmanaged-role";

pub(crate) fn check(cs: &Changeset) -> Vec<Finding> {
    let mut findings = Vec::new();
    // Stage 8 stashed observations on cs.unmanaged_grants. Format each
    // as a Warning finding.
    for entry in &cs.unmanaged_grants {
        findings.push(Finding {
            rule: RULE_ID,
            severity: Severity::Warning,
            message: format!(
                "{}: catalog has grant {} to role {} which is not declared in source",
                entry.object_label, entry.privilege_label, entry.role_name,
            ),
            location: None,
        });
    }
    findings
}
```

This requires extending `Changeset` (or whatever the per-DB diff-result type is) with an `unmanaged_grants: Vec<UnmanagedGrantObservation>` field that Stage 8 populates. Update the Stage 8 spec retroactively if needed.

3 tests.

### Task 11.2: `revoke-from-owner`

Fires when a `RevokeObjectPrivilege` or `RevokeColumnPrivilege` change would target the object's owner.

```rust
//! Errors when a REVOKE step targets the object's owner (no-op DDL, PG silently rejects).

use crate::diff::changeset::Changeset;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "revoke-from-owner";

pub(crate) fn check(cs: &Changeset) -> Vec<Finding> {
    let mut findings = Vec::new();
    // Stage 8 stashed observations on cs.revokes_with_owner. Emit an
    // Error finding for any entry whose grantee equals the owner.
    for entry in &cs.revokes_with_owner {
        let grantee_matches_owner = match &entry.grantee {
            crate::ir::grant::GrantTarget::Role(name) => name == &entry.owner,
            crate::ir::grant::GrantTarget::Public => false,
        };
        if grantee_matches_owner {
            findings.push(Finding {
                rule: RULE_ID,
                severity: Severity::Error,
                message: format!(
                    "REVOKE {} ON {} would target the object's owner {}; \
                     PG silently rejects (owner has implicit privileges)",
                    entry.privilege_label, entry.object_label, entry.owner,
                ),
                location: None,
            });
        }
    }
    findings
}
```

4 tests.

### Task 11.3: Wire into dispatcher

`check_universal_with_cluster` already calls Stage 10's rule; add the two new ones. The two new rules operate on `Changeset`, not `SourceTree`, so they belong in `check_changeset` (the v0.2.1 changeset-level dispatcher).

```rust
// in crates/pgevolve-core/src/lint/universal.rs::check_changeset
out.extend(rules::grants_to_unmanaged_role::check(cs));
out.extend(rules::revoke_from_owner::check(cs));
```

### Task 11.4: Run + commit

```bash
cargo test -p pgevolve-core --lib lint
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve-core/src/lint/
git commit -m "$(cat <<'EOF'
feat(lint): grants-to-unmanaged-role + revoke-from-owner

Two new changeset-level rules wired through check_changeset:

  grants-to-unmanaged-role (warning, waivable): fires when the catalog
  has grants to roles not declared in source. Diff already filtered
  these out of REVOKE — the lint surfaces them so operators can
  decide whether to manage them or accept the drift.

  revoke-from-owner (error, non-waivable): fires when a REVOKE step
  targets the object's owner. PG silently rejects (owner has
  implicit privileges); we pre-empt with a clear plan-time error.

Stage 11 of docs/superpowers/plans/2026-05-22-grants-and-ownership.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 12 — API wire-up + conformance fixtures

**Files modified:** `crates/pgevolve/src/api/mod.rs` (load cluster source if linked, pass into lint).
**Files created:** 15 fixtures under `crates/pgevolve-conformance/tests/cases/objects/grants/`.

### Task 12.1: API wire-up

- [ ] **Step 1: Extend `build_plan`** to load cluster roles when `[cluster].project` is set:

```rust
let cluster_role_names: Option<BTreeSet<Identifier>> = if let Some(link) = &cfg.cluster {
    let cluster_root = pgevolve_toml_dir.join(&link.project);
    let cluster_cat = pgevolve_core::parse::cluster::parse_cluster_directory(
        &cluster_root.join("roles")
    )?;
    Some(cluster_cat.roles.iter().map(|r| r.name.clone()).collect())
} else {
    None
};
let universal_findings = pgevolve_core::lint::check_universal_with_cluster(
    &source_tree, cluster_role_names.as_ref(),
);
```

Where `pgevolve_toml_dir` is derived from the loaded config path. Surface failures cleanly (e.g., `[cluster].project` points at a non-existent path → clear error).

- [ ] **Step 2: Verify the lint path actually fires** by adding an integration test:

```rust
// crates/pgevolve/tests/api_build_plan.rs (extend existing)
#[tokio::test]
#[cfg_attr(not(feature = "docker"), ignore)]
async fn build_plan_surfaces_grant_references_unknown_role() {
    let td = tempfile::TempDir::new().unwrap();
    // ... set up a per-DB project with [cluster].project pointing at a
    //     temp cluster project containing only `readers` role.
    //     Add a table grant to `unknown_role`.
    //     Build plan, assert advisory_findings contains "grant-references-unknown-role".
}
```

### Task 12.2: Conformance fixtures — 15 total

Mirror the v0.3.0 / v0.2.1 fixture pattern. Each has `before.sql`, `after.sql`, `fixture.toml`, and an empty `expected/` directory (bless populates).

Sub-roots and fixtures:

**`grants/table/`:**
- `grant-select/` — basic table SELECT grant.
- `revoke-on-drop-from-source/` — drop a previously-granted privilege in source → REVOKE step.
- `column-level-grant/` — column-level INSERT(name) → renders correctly.
- `grant-all-expands/` — `GRANT ALL ON t TO role` → canon expands; round-trip stable.

**`grants/schema/`:**
- `grant-usage-and-create/` — schema USAGE + CREATE → 2 grant steps.

**`grants/function/`:**
- `grant-execute-with-signature/` — function GRANT preserves the argument signature in rendered SQL.

**`grants/sequence/`:**
- `grant-usage/` — sequence USAGE.

**`grants/owner/`:**
- `alter-owner-emits-one-step/` — set owner in source → single ALTER OWNER step.
- `unmanaged-owner-skipped/` — source.owner = None (no explicit OWNER TO in source) → no diff for ownership even though catalog has one.

**`grants/default-privs/`:**
- `in-schema-tables/` — `ALTER DEFAULT PRIVILEGES IN SCHEMA app FOR ROLE owner GRANT SELECT ON TABLES TO readers` round-trips.
- `global-functions/` — same with no IN SCHEMA, object type FUNCTIONS.

**`grants/lint/`:**
- `grants-to-unmanaged-role/` — catalog has grant to unmanaged role; warning fires.
- `revoke-from-owner-error/` — source omits the owner's implicit grant; error fires.

**`grants/cluster-link/`:**
- `role-mention-validated/` — `[cluster].project` set; valid role passes.
- `role-mention-rejected/` — `[cluster].project` set; missing role → error.

For each lint fixture, the `fixture.toml` includes the appropriate `[expect.advisory]` or `[expect.lint]` key (whichever the harness uses for which severity).

### Task 12.3: Bless + verify

```bash
cargo xtask bless --conformance
cargo test -p pgevolve-conformance
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

All green. Inspect the new `expected/plan.sql` files to verify the rendered SQL matches the spec promise.

### Task 12.4: Commit

```bash
git add -p crates/pgevolve/src/api/ crates/pgevolve-conformance/
git commit -m "$(cat <<'EOF'
test(conformance): 15 grants/ownership fixtures + cluster-link wire-up

build_plan now loads the linked cluster project's roles (when
[cluster].project is set) and passes the role-name set into
check_universal_with_cluster so grant-references-unknown-role
fires through the production path. Docker-gated integration test
verifies the wiring end-to-end.

Fifteen new conformance fixtures under objects/grants/ cover
the surface across 4 sub-roots (table, schema, function, sequence,
owner, default-privs, lint, cluster-link).

Stage 12 of docs/superpowers/plans/2026-05-22-grants-and-ownership.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 13 — Proptest + docs + v0.3.1 release

### Task 13.1: Property tests

Extend `arbitrary_table` (and the 7 other grantable arbitrary generators) to include `owner` and `grants`. Add a top-level `arbitrary_default_privileges` strategy.

The diff-round-trip property test should already exist; extend it to also include owner/grants in the search space.

Run 10× per constitution §9:

```bash
for i in 1 2 3 4 5 6 7 8 9 10; do
    PROPTEST_CASES=512 cargo test --workspace --release 2>&1 | tail -3
done
```

All 10 green.

Commit:

```
test(proptest): owner + grants + default_privileges in IR generators

Cluster-cycle-free generation for grants too (no membership in this
sub-spec, but ensure each generated grantee role name appears in
the proptest's universe). 10× per §9; all green.

Stage 13.1 of docs/superpowers/plans/2026-05-22-grants-and-ownership.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

### Task 13.2: Docs

- [ ] `docs/spec/objects.md:249` — move row → ✅ Supported.
- [ ] Create `docs/spec/grants.md` — overview of the surface, drift policy, cluster-link option.
- [ ] `docs/spec/cluster.md` — add a cross-ref note about `[cluster].project`.
- [ ] `CHANGELOG.md` — new `[0.3.1]` section.

### Task 13.3: Version bump + release

```bash
# Cargo.toml [workspace.package].version → "0.3.1"
# crates/pgevolve-core-macros/Cargo.toml → "0.3.1"
cargo build --workspace  # refresh Cargo.lock

# Verify CHANGELOG-version sync
v=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
grep -q "^## \[$v\] — " CHANGELOG.md && echo OK || echo MISMATCH
```

### Task 13.4: §9 verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo doc --workspace --no-deps 2>&1 | grep -cE "^warning"  # expect 0
```

All green.

### Task 13.5: Re-bless conformance (plan-id hash depends on version)

```bash
cargo xtask bless --conformance
cargo test -p pgevolve-conformance
```

### Task 13.6: Release commit

```bash
git add docs/spec/objects.md docs/spec/grants.md docs/spec/cluster.md CHANGELOG.md Cargo.toml Cargo.lock crates/*/Cargo.toml crates/pgevolve-conformance/tests/cases/
git commit -m "$(cat <<'EOF'
release: v0.3.1 — object grants + ownership + default privileges

Second v0.3 sub-spec. All 8 grantable IR types gain owner + grants;
Catalog gains default_privileges. Drift policy is lenient — catalog
grants to unmanaged roles surface as a warning, never silently
revoked.

Optional [cluster].project block in pgevolve.toml links to a v0.3.0
cluster project for grantee role-name validation (Error severity
when a grantee isn't declared in the cluster source).

RLS policies ship in v0.3.2.

Closes issue #3.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 13.7: STOP

Do NOT tag or push. Report DONE.

---

## Done.

After Stage 13, v0.3.1 is committed locally and ready for tag + push:
- 8 grantable IR types extended with `owner` + `grants`
- New `ir::grant` + `ir::default_privileges` modules + canon
- ACL decoder + 6 family catalog readers extended + pg_default_acl reader
- 3 new parser paths (GRANT, OWNER, ALTER DEFAULT PRIVILEGES)
- Differ with lenient drift policy + new top-level managed-roles set
- 6 new StepKind variants + SQL helpers + emit handlers
- Optional `[cluster].project` cross-link
- Three new lint rules (grant-references-unknown-role, grants-to-unmanaged-role, revoke-from-owner)
- 15 conformance fixtures + property-test coverage
- v0.3.1 release commit

Next plan target: **RLS policies** (issue #4), the final leg of the trilogy.
