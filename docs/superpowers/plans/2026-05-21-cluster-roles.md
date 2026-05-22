# Cluster Surface + ROLE/USER — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the v0.3.0 cluster-level surface scaffold and its first managed object — Postgres roles with full attribute matrix, membership edges, two lint rules, full CLI, and conformance coverage.

**Architecture:** Twelve sequential stages. Each stage commits at least once and leaves the workspace green. Stages 1–7 add the core mechanics in `pgevolve-core` (IR → canon → parser → catalog reader → differ → render → lint). Stages 8–10 wire the cluster project into `pgevolve` (config → API → CLI). Stages 11–12 close the loop (conformance harness + fixtures, property tests + release). The cluster surface is structurally parallel to the per-DB surface — same patterns, separate code paths, no shared state. Per the v0.2 architecture review's Decision 23: separate command family, separate executor, separate plan directory.

**Tech Stack:** Rust 1.95+, `pg_query` 5.x, `tokio_postgres`, `clap` v4, `toml`, `blake3`, `proptest`, `serde`.

**Source spec:** `docs/superpowers/specs/2026-05-21-cluster-roles-design.md`.

---

## Pre-flight

- [ ] **Step 1: Confirm clean baseline**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --tests
```

Expected: all green. v0.2.1 is committed; main is clean.

- [ ] **Step 2: Re-read the source spec sections relevant to each stage**

Open `docs/superpowers/specs/2026-05-21-cluster-roles-design.md` once. Each stage cites the spec section it implements; you do not need to re-read end-to-end per stage.

---

## File structure (where everything lands)

```
crates/pgevolve-core/src/
├── ir/
│   └── cluster/                NEW — Stage 1
│       ├── catalog.rs            — ClusterCatalog
│       ├── role.rs               — Role, RoleAttributes
│       └── mod.rs                — re-exports
├── parse/
│   └── cluster/                NEW — Stage 3
│       ├── mod.rs                — parse_cluster_directory entry
│       ├── create_role.rs        — CREATE ROLE / CREATE USER builder
│       ├── alter_role.rs         — ALTER ROLE builder
│       ├── grant_membership.rs   — GRANT r TO target builder
│       └── shared.rs             — role-attribute option-list decoder
├── catalog/
│   └── cluster.rs              NEW — Stage 4 — read_cluster_catalog
├── diff/
│   └── cluster.rs              NEW — Stage 5 — ClusterChangeSet, diff_cluster
├── plan/
│   ├── cluster_rewrite/        NEW — Stage 6
│   │   ├── mod.rs                — emit entry
│   │   ├── sql.rs                — SQL helpers
│   │   └── emit.rs               — change → RawStep dispatch
│   └── raw_step.rs               MODIFY — Stage 6 — six new StepKind variants
└── lint/
    ├── rules/                  MODIFY — Stage 7
    │   ├── role_loses_superuser.rs           NEW
    │   └── role_membership_cycle.rs          NEW
    └── universal.rs              MODIFY — Stage 7 — check_cluster_changeset dispatcher

crates/pgevolve/src/
├── cluster_config.rs           NEW — Stage 8 — pgevolve-cluster.toml schema + loader
├── api/
│   └── cluster.rs              NEW — Stage 9 — build_cluster_plan, apply_cluster_plan
├── executor/
│   └── cluster_apply.rs        NEW — Stage 9 — cluster-specific apply wrapper
├── commands/
│   └── cluster/                NEW — Stage 10
│       ├── mod.rs                — re-exports
│       ├── init.rs               — pgevolve cluster init
│       ├── diff.rs               — pgevolve cluster diff
│       ├── plan.rs               — pgevolve cluster plan
│       ├── apply.rs              — pgevolve cluster apply
│       └── status.rs             — pgevolve cluster status
└── cli.rs                        MODIFY — Stage 10 — Cluster subcommand enum

crates/pgevolve-conformance/
├── src/
│   ├── fixture.rs              MODIFY — Stage 11 — Authoring::Cluster
│   └── planning.rs             MODIFY — Stage 11 — render_cluster_plan
├── tests/
│   ├── run.rs                  MODIFY — Stage 11 — run_cluster
│   └── cases/cluster/roles/    NEW — Stage 11 — six fixtures + blessed expected/

crates/pgevolve-testkit/src/
└── ir_generator.rs             MODIFY — Stage 12 — arbitrary_role, arbitrary_cluster_catalog
```

---

## Stage 1 — IR scaffolding

Pure data types for `ClusterCatalog`, `Role`, `RoleAttributes`. No behavior beyond `Diff`. Sets the foundation everything else builds on.

**Files created:** `crates/pgevolve-core/src/ir/cluster/{mod.rs, catalog.rs, role.rs}`.
**Files modified:** `crates/pgevolve-core/src/ir/mod.rs` (add `pub mod cluster;`), `crates/pgevolve-core/src/lib.rs` if needed for re-exports.

### Task 1.1: Create the `cluster` IR module

- [ ] **Step 1: Create `crates/pgevolve-core/src/ir/cluster/mod.rs`**

```rust
//! Cluster-level IR — objects that live above the per-database surface.
//!
//! Currently holds roles only. Tablespaces, cluster settings, foreign servers,
//! user mappings, and the databases list are deferred to follow-up sub-specs.
//! See `docs/superpowers/specs/2026-05-21-cluster-roles-design.md` and
//! `docs/superpowers/specs/2026-05-15-v0.2-architecture-review-design.md` §17.

pub mod catalog;
pub mod role;

pub use catalog::ClusterCatalog;
pub use role::{Role, RoleAttributes};
```

- [ ] **Step 2: Create `crates/pgevolve-core/src/ir/cluster/role.rs`**

```rust
//! `Role` and `RoleAttributes` — Postgres `pg_authid` row, normalized.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::eq::DiffMacro;

/// A managed Postgres role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct Role {
    /// Role name.
    pub name: Identifier,
    /// Boolean + numeric attributes from `pg_authid`.
    #[diff(via_debug)]
    pub attributes: RoleAttributes,
    /// Roles this role is a member of (the `IN ROLE x` direction).
    /// Canonicalized to lexicographic order in [`crate::ir::canon`].
    #[diff(via_debug)]
    pub member_of: Vec<Identifier>,
    /// Optional comment from `pg_shdescription`.
    #[diff(via_debug)]
    pub comment: Option<String>,
}

/// `pg_authid` attribute matrix. Passwords intentionally absent (set out-of-band).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleAttributes {
    /// `SUPERUSER` / `NOSUPERUSER`. Default false.
    pub superuser: bool,
    /// `CREATEDB` / `NOCREATEDB`. Default false.
    pub createdb: bool,
    /// `CREATEROLE` / `NOCREATEROLE`. Default false.
    pub createrole: bool,
    /// `INHERIT` / `NOINHERIT`. Default true (matches PG default).
    pub inherit: bool,
    /// `LOGIN` / `NOLOGIN`. Default false. `CREATE USER` sugar sets this true.
    pub login: bool,
    /// `REPLICATION` / `NOREPLICATION`. Default false.
    pub replication: bool,
    /// `BYPASSRLS` / `NOBYPASSRLS`. Default false.
    pub bypass_rls: bool,
    /// `CONNECTION LIMIT n`. `None` means unlimited (PG `-1`).
    pub connection_limit: Option<i64>,
    /// `VALID UNTIL 'ts'`. RFC 3339 string; opaque to differ.
    pub valid_until: Option<String>,
}

impl Default for RoleAttributes {
    fn default() -> Self {
        Self {
            superuser: false,
            createdb: false,
            createrole: false,
            inherit: true,   // PG default
            login: false,
            replication: false,
            bypass_rls: false,
            connection_limit: None,
            valid_until: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::eq::Diff;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn base() -> Role {
        Role {
            name: id("app_user"),
            attributes: RoleAttributes::default(),
            member_of: vec![],
            comment: None,
        }
    }

    #[test]
    fn equal_roles_have_no_diff() {
        assert!(base().canonical_eq(&base()));
    }

    #[test]
    fn login_change_diffs() {
        let mut b = base();
        b.attributes.login = true;
        assert!(base().diff(&b).iter().any(|x| x.path == "attributes"));
    }

    #[test]
    fn membership_change_diffs() {
        let mut b = base();
        b.member_of.push(id("readers"));
        assert!(base().diff(&b).iter().any(|x| x.path == "member_of"));
    }

    #[test]
    fn comment_change_diffs() {
        let mut b = base();
        b.comment = Some("the app".into());
        assert!(base().diff(&b).iter().any(|x| x.path == "comment"));
    }

    #[test]
    fn default_attributes_match_postgres_defaults() {
        let a = RoleAttributes::default();
        assert!(a.inherit, "PG default for INHERIT is true");
        assert!(!a.superuser);
        assert!(!a.login);
        assert_eq!(a.connection_limit, None);
    }
}
```

- [ ] **Step 3: Create `crates/pgevolve-core/src/ir/cluster/catalog.rs`**

```rust
//! `ClusterCatalog` — the cluster-wide IR root.
//!
//! Currently contains roles only. Other cluster object kinds (tablespaces,
//! cluster_settings, foreign_servers, user_mappings, databases) land in
//! follow-up sub-specs.

use serde::{Deserialize, Serialize};

use crate::ir::cluster::role::Role;

/// The root cluster IR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ClusterCatalog {
    /// Managed roles, sorted by `name` after `canonicalize()`.
    pub roles: Vec<Role>,
}

impl ClusterCatalog {
    /// Empty cluster catalog (no roles).
    #[must_use]
    pub const fn empty() -> Self {
        Self { roles: Vec::new() }
    }

    /// Normalize the catalog: sorts roles by name, sorts each role's
    /// `member_of` lexicographically. Idempotent. See
    /// [`crate::ir::canon`] for the per-rule details.
    pub fn canonicalize(&mut self) {
        crate::ir::canon::cluster::run(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::cluster::role::{Role, RoleAttributes};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn role(name: &str) -> Role {
        Role {
            name: id(name),
            attributes: RoleAttributes::default(),
            member_of: vec![],
            comment: None,
        }
    }

    #[test]
    fn empty_catalog_canonicalizes_idempotently() {
        let mut c = ClusterCatalog::empty();
        c.canonicalize();
        c.canonicalize();
        assert!(c.roles.is_empty());
    }

    #[test]
    fn canonicalize_sorts_roles_by_name() {
        let mut c = ClusterCatalog {
            roles: vec![role("zebra"), role("alpha"), role("middle")],
        };
        c.canonicalize();
        let names: Vec<_> = c.roles.iter().map(|r| r.name.as_str().to_owned()).collect();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }
}
```

- [ ] **Step 4: Wire the new module into `crates/pgevolve-core/src/ir/mod.rs`**

Add `pub mod cluster;` near the other `pub mod ...;` declarations in alphabetical order.

- [ ] **Step 5: Run cluster IR tests**

```bash
cargo test -p pgevolve-core --lib ir::cluster
```

Expected: all green except the `canonicalize` test, which will fail because `ir::canon::cluster` doesn't exist yet. That's a forward dependency on Stage 2.

- [ ] **Step 6: Temporarily stub `ir::canon::cluster::run`**

In `crates/pgevolve-core/src/ir/canon/mod.rs`, add:

```rust
/// Cluster-IR canon rules. Implemented in Stage 2.
pub mod cluster {
    use crate::ir::cluster::catalog::ClusterCatalog;

    /// Stage 2 fills this in. For now, sort roles by name so Stage 1's
    /// IR test passes.
    pub fn run(cat: &mut ClusterCatalog) {
        cat.roles.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    }
}
```

- [ ] **Step 7: Re-run + commit**

```bash
cargo test -p pgevolve-core --lib ir::cluster
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve-core/src/ir/
git commit -m "$(cat <<'EOF'
feat(ir): cluster-level IR — Role, RoleAttributes, ClusterCatalog

First sub-spec of v0.3. ir::cluster carries managed roles only;
tablespaces, GUCs, foreign servers, and the databases list are
deferred to follow-up sub-specs per the v0.2 architecture review.

RoleAttributes uses concrete bools (defaults match PG: INHERIT=true,
all others false) plus Option<i64> for CONNECTION LIMIT (None means
unlimited / PG's -1) and Option<String> for VALID UNTIL.
Passwords intentionally absent (set out-of-band).

Stage 1 of docs/superpowers/plans/2026-05-21-cluster-roles.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 2 — Canon

Sort `member_of` lexicographically on every role. The catalog reader emits these in PG's OID order; canon normalizes.

**Files modified:** `crates/pgevolve-core/src/ir/canon/cluster.rs` (promote from inline stub to its own file).

### Task 2.1: Replace the stub with a real canon module

- [ ] **Step 1: Remove the inline `pub mod cluster` stub from `ir/canon/mod.rs`**

Delete the temporary module added in Stage 1 Step 6. Replace with `pub mod cluster;`.

- [ ] **Step 2: Create `crates/pgevolve-core/src/ir/canon/cluster.rs`**

```rust
//! Canon rules for the cluster IR. Currently:
//!
//! - Sort `ClusterCatalog::roles` by name.
//! - Sort each role's `member_of` lexicographically.

use crate::ir::cluster::catalog::ClusterCatalog;
use crate::ir::cluster::role::Role;

/// Run every cluster-canon rule. Idempotent.
pub fn run(cat: &mut ClusterCatalog) {
    for role in &mut cat.roles {
        normalize_membership_order(role);
    }
    cat.roles.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
}

/// Sort `member_of` lexicographically by role name.
fn normalize_membership_order(role: &mut Role) {
    role.member_of.sort_by(|a, b| a.as_str().cmp(b.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::cluster::role::{Role, RoleAttributes};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn role_with(name: &str, members: Vec<&str>) -> Role {
        Role {
            name: id(name),
            attributes: RoleAttributes::default(),
            member_of: members.into_iter().map(id).collect(),
            comment: None,
        }
    }

    #[test]
    fn sorts_member_of() {
        let mut r = role_with("r", vec!["zebra", "alpha", "middle"]);
        normalize_membership_order(&mut r);
        let names: Vec<_> = r.member_of.iter().map(|i| i.as_str().to_owned()).collect();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }

    #[test]
    fn run_is_idempotent() {
        let mut c = ClusterCatalog {
            roles: vec![
                role_with("z", vec!["b", "a"]),
                role_with("a", vec!["c", "b"]),
            ],
        };
        run(&mut c);
        let snap1 = format!("{:?}", c);
        run(&mut c);
        let snap2 = format!("{:?}", c);
        assert_eq!(snap1, snap2);
    }
}
```

- [ ] **Step 3: Run + commit**

```bash
cargo test -p pgevolve-core --lib ir::canon::cluster
cargo test -p pgevolve-core --lib ir::cluster
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve-core/src/ir/canon/
git commit -m "$(cat <<'EOF'
feat(canon): cluster — sort roles and member_of lexicographically

Promotes the Stage 1 inline stub to its own canon module. Two rules:
sort ClusterCatalog.roles by name (so diff is order-stable) and sort
each role's member_of by name (so source and catalog round-trip
equally regardless of OID order).

Stage 2 of docs/superpowers/plans/2026-05-21-cluster-roles.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 3 — Parser

Parse `roles/*.sql` into a `ClusterCatalog`. Five statement kinds supported: `CREATE ROLE`, `CREATE USER`, `ALTER ROLE`, `GRANT role TO target`, `COMMENT ON ROLE`. Reject `DROP ROLE` and non-cluster GRANTs. Warn on `PASSWORD` clauses.

**Files created:** `crates/pgevolve-core/src/parse/cluster/{mod.rs, create_role.rs, alter_role.rs, grant_membership.rs, shared.rs}`.
**Files modified:** `crates/pgevolve-core/src/parse/mod.rs` (add `pub mod cluster;`).

### Task 3.1: Module skeleton + entry point

- [ ] **Step 1: Create `crates/pgevolve-core/src/parse/cluster/mod.rs`**

```rust
//! Cluster-level source parser. Reads `roles/*.sql` (alphabetical) into a
//! [`ClusterCatalog`]. Mirrors the shape of the per-DB `parse::parse_directory`.

mod alter_role;
mod create_role;
mod grant_membership;
mod shared;

use std::path::Path;

use crate::ir::cluster::catalog::ClusterCatalog;
use crate::parse::error::ParseError;

/// Parse every `*.sql` file under `roles/`, alphabetical order. Returns a
/// canonicalized [`ClusterCatalog`].
pub fn parse_cluster_directory(roles_dir: &Path) -> Result<ClusterCatalog, ParseError> {
    let mut cat = ClusterCatalog::empty();
    let entries = collect_sql_files(roles_dir)?;
    for path in entries {
        let sql = std::fs::read_to_string(&path).map_err(|e| ParseError::Io {
            path: path.clone(),
            source: e,
        })?;
        apply_file(&sql, &path, &mut cat)?;
    }
    cat.canonicalize();
    Ok(cat)
}

fn collect_sql_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, ParseError> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| ParseError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?
        .filter_map(Result::ok)
        .map(|d| d.path())
        .filter(|p| p.extension().is_some_and(|e| e == "sql"))
        .collect();
    out.sort();
    Ok(out)
}

fn apply_file(sql: &str, path: &Path, cat: &mut ClusterCatalog) -> Result<(), ParseError> {
    let parsed = pg_query::parse(sql).map_err(|e| ParseError::PgQuery {
        path: path.to_path_buf(),
        source: e.to_string(),
    })?;
    for stmt in &parsed.protobuf.stmts {
        let Some(node) = stmt.stmt.as_ref().and_then(|s| s.node.as_ref()) else {
            continue;
        };
        let loc = crate::parse::error::SourceLocation::file(path.to_path_buf());
        match node {
            pg_query::NodeEnum::CreateRoleStmt(s) => create_role::apply(s, cat, &loc)?,
            pg_query::NodeEnum::AlterRoleStmt(s) => alter_role::apply(s, cat, &loc)?,
            pg_query::NodeEnum::GrantRoleStmt(s) => grant_membership::apply(s, cat, &loc)?,
            pg_query::NodeEnum::CommentStmt(s) => apply_comment(s, cat, &loc)?,
            pg_query::NodeEnum::DropRoleStmt(_) => {
                return Err(ParseError::Structural {
                    location: loc,
                    message: "DROP ROLE in source is not supported — drops happen via diff".into(),
                });
            }
            other => {
                return Err(ParseError::Structural {
                    location: loc,
                    message: format!(
                        "{} is not supported in cluster source (roles/); allowed: CREATE ROLE, CREATE USER, ALTER ROLE, GRANT role TO target, COMMENT ON ROLE",
                        crate::parse::node_kind_name(other)
                    ),
                });
            }
        }
    }
    Ok(())
}

fn apply_comment(
    s: &pg_query::protobuf::CommentStmt,
    cat: &mut ClusterCatalog,
    loc: &crate::parse::error::SourceLocation,
) -> Result<(), ParseError> {
    use pg_query::protobuf::ObjectType;
    let kind = ObjectType::try_from(s.objtype).unwrap_or(ObjectType::Undefined);
    if kind != ObjectType::ObjectRole {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "only COMMENT ON ROLE is supported in cluster source".into(),
        });
    }
    let role_name = shared::extract_role_name_from_object_node(&s.object, loc)?;
    let comment = if s.comment.is_empty() {
        None
    } else {
        Some(s.comment.clone())
    };
    let role = cat
        .roles
        .iter_mut()
        .find(|r| r.name == role_name)
        .ok_or_else(|| ParseError::Structural {
            location: loc.clone(),
            message: format!("COMMENT ON ROLE references unknown role {role_name}"),
        })?;
    role.comment = comment;
    Ok(())
}
```

(Reference `crate::parse::node_kind_name` — search the existing parser for an equivalent helper that names an unexpected NodeEnum variant; if it doesn't exist by that name, use `format!("{:?}", other)` and call the variant out by Debug-name. Update the error message to match.)

- [ ] **Step 2: Wire the module into `crates/pgevolve-core/src/parse/mod.rs`**

Add `pub mod cluster;` alongside the existing `pub mod` declarations.

### Task 3.2: Attribute option-list decoder (`shared.rs`)

The PG grammar represents `CREATE ROLE r WITH LOGIN CREATEDB CONNECTION LIMIT 50 VALID UNTIL '2030-01-01'` as a list of `DefElem` option nodes. A single shared decoder handles them for both CREATE and ALTER.

- [ ] **Step 1: Create `crates/pgevolve-core/src/parse/cluster/shared.rs`**

```rust
//! Shared helpers for cluster parsers.

use crate::identifier::Identifier;
use crate::ir::cluster::role::RoleAttributes;
use crate::parse::error::{ParseError, SourceLocation};

/// Apply the parsed `WITH (option…)` list to `attrs`. Each option mutates one
/// field. Unknown options surface as ParseError::Structural; PASSWORD-related
/// options are silently dropped with a warning logged (the spec says
/// passwords are out-of-band).
pub(super) fn apply_options(
    options: &[pg_query::protobuf::Node],
    attrs: &mut RoleAttributes,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    for opt_node in options {
        let Some(pg_query::NodeEnum::DefElem(def)) = opt_node.node.as_ref() else {
            continue;
        };
        apply_one(def, attrs, loc)?;
    }
    Ok(())
}

fn apply_one(
    def: &pg_query::protobuf::DefElem,
    attrs: &mut RoleAttributes,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    // pg_query lowercases the option name in `defname`.
    match def.defname.as_str() {
        "superuser"    => attrs.superuser     = extract_bool(def, loc)?,
        "createdb"     => attrs.createdb      = extract_bool(def, loc)?,
        "createrole"   => attrs.createrole    = extract_bool(def, loc)?,
        "inherit"      => attrs.inherit       = extract_bool(def, loc)?,
        "canlogin"     => attrs.login         = extract_bool(def, loc)?,
        "isreplication"=> attrs.replication   = extract_bool(def, loc)?,
        "bypassrls"    => attrs.bypass_rls    = extract_bool(def, loc)?,
        "connectionlimit" => attrs.connection_limit = match extract_int(def, loc)? {
            -1 => None,
            n  => Some(n),
        },
        "validuntil"   => attrs.valid_until   = Some(extract_string(def, loc)?),
        "password" | "encryptedpassword" | "unencryptedpassword" => {
            // Spec: passwords are not stored in source. Silently drop.
            // (A warn-log path could go here once warning plumbing exists; for
            // now silent drop matches the spec's "set out-of-band" stance.)
        }
        "rolemembers" | "addroleto" => {
            // Handled separately as membership; CREATE ROLE r IN ROLE x lands
            // these in the option list. The caller's create_role.rs lifts
            // them into the membership list before calling apply_options.
            // Reaching this arm means the caller forgot to filter.
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "internal: membership option '{}' should be handled by create_role::apply, not shared::apply_options",
                    def.defname
                ),
            });
        }
        other => return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("unknown role option '{other}'"),
        }),
    }
    Ok(())
}

fn extract_bool(def: &pg_query::protobuf::DefElem, loc: &SourceLocation) -> Result<bool, ParseError> {
    // pg_query encodes booleans as Integer 0/1 inside the arg.
    let int = extract_int(def, loc)?;
    Ok(int != 0)
}

fn extract_int(def: &pg_query::protobuf::DefElem, loc: &SourceLocation) -> Result<i64, ParseError> {
    let Some(arg) = def.arg.as_ref().and_then(|a| a.node.as_ref()) else {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("option '{}' missing argument", def.defname),
        });
    };
    match arg {
        pg_query::NodeEnum::Integer(i) => Ok(i64::from(i.ival)),
        pg_query::NodeEnum::Boolean(b) => Ok(i64::from(b.boolval)),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "option '{}' expected integer/boolean, got {other:?}",
                def.defname
            ),
        }),
    }
}

fn extract_string(def: &pg_query::protobuf::DefElem, loc: &SourceLocation) -> Result<String, ParseError> {
    let Some(arg) = def.arg.as_ref().and_then(|a| a.node.as_ref()) else {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("option '{}' missing argument", def.defname),
        });
    };
    match arg {
        pg_query::NodeEnum::String(s) => Ok(s.sval.clone()),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "option '{}' expected string, got {other:?}",
                def.defname
            ),
        }),
    }
}

/// Decode a list of role-name option-nodes (`IN ROLE x, y` / `ROLE x, y`) into Identifiers.
pub(super) fn extract_role_name_list(
    def: &pg_query::protobuf::DefElem,
    loc: &SourceLocation,
) -> Result<Vec<Identifier>, ParseError> {
    let Some(arg_node) = def.arg.as_ref().and_then(|a| a.node.as_ref()) else {
        return Ok(vec![]);
    };
    let pg_query::NodeEnum::List(list) = arg_node else {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("expected list for option '{}', got {arg_node:?}", def.defname),
        });
    };
    let mut out = Vec::with_capacity(list.items.len());
    for item in &list.items {
        let Some(node) = item.node.as_ref() else { continue };
        let name_str = match node {
            pg_query::NodeEnum::RoleSpec(rs) => rs.rolename.clone(),
            pg_query::NodeEnum::String(s) => s.sval.clone(),
            other => return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!("expected role name, got {other:?}"),
            }),
        };
        out.push(Identifier::from_unquoted(&name_str).map_err(|e| ParseError::Structural {
            location: loc.clone(),
            message: format!("invalid role name {name_str:?}: {e}"),
        })?);
    }
    Ok(out)
}

/// Extract role name from the `object` field of a `COMMENT ON ROLE` statement.
pub(super) fn extract_role_name_from_object_node(
    node: &Option<Box<pg_query::protobuf::Node>>,
    loc: &SourceLocation,
) -> Result<Identifier, ParseError> {
    let Some(boxed) = node.as_ref() else {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "COMMENT ON ROLE missing target".into(),
        });
    };
    match boxed.node.as_ref() {
        Some(pg_query::NodeEnum::RoleSpec(rs)) => {
            Identifier::from_unquoted(&rs.rolename).map_err(|e| ParseError::Structural {
                location: loc.clone(),
                message: format!("invalid role name {:?}: {e}", rs.rolename),
            })
        }
        Some(pg_query::NodeEnum::String(s)) => {
            Identifier::from_unquoted(&s.sval).map_err(|e| ParseError::Structural {
                location: loc.clone(),
                message: format!("invalid role name {:?}: {e}", s.sval),
            })
        }
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("unexpected COMMENT target {other:?}"),
        }),
    }
}
```

(The exact pg_query field names — `defname`, `rolename`, `sval`, `boolval`, `ival` — are correct per the 5.x bindings. If a name differs, the compiler will say so.)

### Task 3.3: CREATE ROLE / CREATE USER

- [ ] **Step 1: Create `crates/pgevolve-core/src/parse/cluster/create_role.rs`**

```rust
//! `CREATE ROLE` / `CREATE USER` builder.
//!
//! `CREATE USER r WITH ...` is sugar for `CREATE ROLE r WITH LOGIN ...`.
//! pg_query stamps `stmt_type == 1` (RoleStmtKind::User) for `CREATE USER`;
//! we read that flag and OR in `login = true` after option processing.

use pg_query::protobuf::CreateRoleStmt;

use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::ir::cluster::role::{Role, RoleAttributes};
use crate::parse::error::{ParseError, SourceLocation};

use super::shared;

pub(super) fn apply(
    s: &CreateRoleStmt,
    cat: &mut ClusterCatalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    let name = Identifier::from_unquoted(&s.role).map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("invalid role name {:?}: {e}", s.role),
    })?;
    if cat.roles.iter().any(|r| r.name == name) {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("role {name} declared more than once"),
        });
    }
    let mut attrs = RoleAttributes::default();
    let mut member_of = Vec::new();

    // CREATE USER sugar: stmt_type 1 means RoleStmtKind::User in pg_query;
    // 0 means RoleStmtKind::Role. Use whichever name the binding actually
    // exposes — Verify by reading pg_query::protobuf::CreateRoleStmt source.
    let is_user_sugar = s.stmt_type == 1;
    if is_user_sugar {
        attrs.login = true;
    }

    for opt_node in &s.options {
        let Some(pg_query::NodeEnum::DefElem(def)) = opt_node.node.as_ref() else {
            continue;
        };
        match def.defname.as_str() {
            // Membership options: lift into member_of, don't pass to apply_options.
            "addroleto" => member_of.extend(shared::extract_role_name_list(def, loc)?),
            "rolemembers" => {
                return Err(ParseError::Structural {
                    location: loc.clone(),
                    message: "CREATE ROLE r ROLE x (reverse-membership) is not supported; use GRANT x TO r".into(),
                });
            }
            "adminmembers" => {
                return Err(ParseError::Structural {
                    location: loc.clone(),
                    message: "CREATE ROLE r ADMIN x is not supported; use GRANT x TO r WITH ADMIN OPTION".into(),
                });
            }
            _ => {}
        }
    }

    // Filter out membership options + apply the rest.
    let attribute_opts: Vec<pg_query::protobuf::Node> = s.options.iter().filter(|opt_node| {
        match opt_node.node.as_ref() {
            Some(pg_query::NodeEnum::DefElem(def)) => !matches!(
                def.defname.as_str(),
                "addroleto" | "rolemembers" | "adminmembers"
            ),
            _ => true,
        }
    }).cloned().collect();
    shared::apply_options(&attribute_opts, &mut attrs, loc)?;

    cat.roles.push(Role {
        name,
        attributes: attrs,
        member_of,
        comment: None,
    });
    Ok(())
}
```

### Task 3.4: ALTER ROLE

- [ ] **Step 1: Create `crates/pgevolve-core/src/parse/cluster/alter_role.rs`**

```rust
//! `ALTER ROLE r [option...]` builder. Mutates the named role's attributes
//! in-place. ALTER is *additive* relative to the already-declared role;
//! options override prior CREATE/ALTER settings.

use pg_query::protobuf::AlterRoleStmt;

use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::parse::error::{ParseError, SourceLocation};

use super::shared;

pub(super) fn apply(
    s: &AlterRoleStmt,
    cat: &mut ClusterCatalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    let Some(role_spec_node) = s.role.as_ref().and_then(|n| n.node.as_ref()) else {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "ALTER ROLE missing role name".into(),
        });
    };
    let pg_query::NodeEnum::RoleSpec(rs) = role_spec_node else {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("ALTER ROLE expected role name, got {role_spec_node:?}"),
        });
    };
    let name = Identifier::from_unquoted(&rs.rolename).map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("invalid role name {:?}: {e}", rs.rolename),
    })?;

    let role = cat.roles.iter_mut().find(|r| r.name == name).ok_or_else(|| {
        ParseError::Structural {
            location: loc.clone(),
            message: format!("ALTER ROLE references unknown role {name} — declare with CREATE ROLE first"),
        }
    })?;

    shared::apply_options(&s.options, &mut role.attributes, loc)?;
    Ok(())
}
```

### Task 3.5: GRANT role TO target

- [ ] **Step 1: Create `crates/pgevolve-core/src/parse/cluster/grant_membership.rs`**

```rust
//! `GRANT role TO target` — cluster-level role membership.
//! Adds `role` to `target.member_of`.

use pg_query::protobuf::GrantRoleStmt;

use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::parse::error::{ParseError, SourceLocation};

pub(super) fn apply(
    s: &GrantRoleStmt,
    cat: &mut ClusterCatalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    if !s.is_grant {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "REVOKE role FROM target in source is not supported — revocations happen via diff".into(),
        });
    }
    let parents = extract_role_specs(&s.granted_roles, loc, "granted role")?;
    let members = extract_role_specs(&s.grantee_roles, loc, "grantee role")?;
    for member_name in &members {
        let member_role = cat.roles.iter_mut().find(|r| &r.name == member_name).ok_or_else(|| {
            ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "GRANT ... TO {member_name} — unknown role; declare with CREATE ROLE first"
                ),
            }
        })?;
        for parent_name in &parents {
            if !member_role.member_of.contains(parent_name) {
                member_role.member_of.push(parent_name.clone());
            }
        }
    }
    Ok(())
}

fn extract_role_specs(
    nodes: &[pg_query::protobuf::Node],
    loc: &SourceLocation,
    label: &str,
) -> Result<Vec<Identifier>, ParseError> {
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        let role_name_str = match n.node.as_ref() {
            Some(pg_query::NodeEnum::RoleSpec(rs)) => rs.rolename.clone(),
            Some(pg_query::NodeEnum::AccessPriv(ap)) => ap.priv_name.clone(),
            other => return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!("expected {label}, got {other:?}"),
            }),
        };
        out.push(Identifier::from_unquoted(&role_name_str).map_err(|e| ParseError::Structural {
            location: loc.clone(),
            message: format!("invalid {label} {role_name_str:?}: {e}"),
        })?);
    }
    Ok(out)
}
```

### Task 3.6: Parser integration tests

- [ ] **Step 1: Create `crates/pgevolve-core/tests/cluster_parse.rs`**

```rust
//! Cluster-source parser end-to-end tests using a temp directory.

use std::fs;

use pgevolve_core::ir::cluster::role::{Compression as _, RoleAttributes, StorageKind as _};
// (the imports above are placeholders — strip the ones you don't need; the
// actual imports you want are `Role`, `RoleAttributes` from
// `pgevolve_core::ir::cluster::role`. Adjust to what compiles.)
use pgevolve_core::parse::cluster::parse_cluster_directory;
use tempfile::TempDir;

fn write_roles(td: &TempDir, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = td.path().join("roles");
    fs::create_dir(&dir).unwrap();
    for (name, sql) in files {
        fs::write(dir.join(name), sql).unwrap();
    }
    dir
}

#[test]
fn create_role_defaults() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "CREATE ROLE app_user;")]);
    let cat = parse_cluster_directory(&dir).unwrap();
    assert_eq!(cat.roles.len(), 1);
    let r = &cat.roles[0];
    assert_eq!(r.name.as_str(), "app_user");
    assert!(!r.attributes.login);
    assert!(r.attributes.inherit);
    assert!(r.member_of.is_empty());
}

#[test]
fn create_user_implies_login() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "CREATE USER app_user;")]);
    let cat = parse_cluster_directory(&dir).unwrap();
    assert!(cat.roles[0].attributes.login, "CREATE USER must imply LOGIN=true");
}

#[test]
fn full_attribute_matrix() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "
        CREATE ROLE admin WITH SUPERUSER CREATEDB CREATEROLE LOGIN
            CONNECTION LIMIT 50 VALID UNTIL '2030-01-01T00:00:00Z';
    ")]);
    let cat = parse_cluster_directory(&dir).unwrap();
    let r = &cat.roles[0];
    assert!(r.attributes.superuser);
    assert!(r.attributes.createdb);
    assert!(r.attributes.createrole);
    assert!(r.attributes.login);
    assert_eq!(r.attributes.connection_limit, Some(50));
    assert_eq!(r.attributes.valid_until.as_deref(), Some("2030-01-01T00:00:00Z"));
}

#[test]
fn grant_role_to_role() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "
        CREATE ROLE readers;
        CREATE ROLE app_user;
        GRANT readers TO app_user;
    ")]);
    let cat = parse_cluster_directory(&dir).unwrap();
    let app = cat.roles.iter().find(|r| r.name.as_str() == "app_user").unwrap();
    assert_eq!(app.member_of.iter().map(|i| i.as_str()).collect::<Vec<_>>(), vec!["readers"]);
}

#[test]
fn in_role_inline_form() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "
        CREATE ROLE readers;
        CREATE ROLE app_user IN ROLE readers;
    ")]);
    let cat = parse_cluster_directory(&dir).unwrap();
    let app = cat.roles.iter().find(|r| r.name.as_str() == "app_user").unwrap();
    assert_eq!(app.member_of.iter().map(|i| i.as_str()).collect::<Vec<_>>(), vec!["readers"]);
}

#[test]
fn alter_role_modifies_existing() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "
        CREATE ROLE app_user;
        ALTER ROLE app_user WITH LOGIN CREATEDB;
    ")]);
    let cat = parse_cluster_directory(&dir).unwrap();
    let r = &cat.roles[0];
    assert!(r.attributes.login);
    assert!(r.attributes.createdb);
}

#[test]
fn drop_role_in_source_errors() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "DROP ROLE app_user;")]);
    let err = parse_cluster_directory(&dir).unwrap_err();
    assert!(err.to_string().contains("DROP ROLE"), "got: {err}");
}

#[test]
fn password_clause_is_dropped_silently() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "
        CREATE ROLE app_user WITH LOGIN PASSWORD 'hunter2';
    ")]);
    let cat = parse_cluster_directory(&dir).unwrap();
    assert!(cat.roles[0].attributes.login);
    // No assertion on password — the IR doesn't carry it.
}

#[test]
fn comment_on_role() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "
        CREATE ROLE app_user;
        COMMENT ON ROLE app_user IS 'application service account';
    ")]);
    let cat = parse_cluster_directory(&dir).unwrap();
    assert_eq!(cat.roles[0].comment.as_deref(), Some("application service account"));
}

#[test]
fn unknown_statement_kind_errors() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "GRANT SELECT ON TABLE foo TO bar;")]);
    let err = parse_cluster_directory(&dir).unwrap_err();
    assert!(err.to_string().contains("not supported"), "got: {err}");
}
```

(Strip the bogus first line `use ... Compression as _, StorageKind as _;` — that's a copy-paste residue from my plan-writing notes; you only need `use pgevolve_core::ir::cluster::role::{Role, RoleAttributes};` if any test constructs one directly.)

### Task 3.7: Run + commit

- [ ] **Step 1: Test**

```bash
cargo test -p pgevolve-core --tests cluster_parse
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
```

If any pg_query field name differs from what the plan assumed (e.g., `s.role` vs `s.rolename`), fix at the call site — the compiler will say so.

- [ ] **Step 2: Commit**

```bash
git add -p crates/pgevolve-core/src/parse/cluster/ crates/pgevolve-core/src/parse/mod.rs crates/pgevolve-core/tests/cluster_parse.rs
git commit -m "$(cat <<'EOF'
feat(parse): cluster — CREATE/ALTER ROLE, CREATE USER, GRANT membership

Source parser for the cluster project's roles/*.sql files.
Five statement kinds:
  CREATE ROLE r [WITH options...]
  CREATE USER r [WITH options...]     — sugar for CREATE ROLE … LOGIN
  ALTER ROLE r [options...]           — mutates a prior CREATE
  GRANT r TO target                   — role membership
  COMMENT ON ROLE r IS '...'

Rejects DROP ROLE and non-cluster GRANTs (objects-level grants land
in the next sub-spec). PASSWORD clauses parse but are silently
dropped from the IR — the spec is explicit that passwords are
set out-of-band.

Stage 3 of docs/superpowers/plans/2026-05-21-cluster-roles.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 4 — Catalog reader

Query `pg_authid` + `pg_auth_members` against a live PG (requires superuser). Filter predefined `pg_*` roles and the configured bootstrap roles.

**Files created:** `crates/pgevolve-core/src/catalog/cluster.rs`.
**Files modified:** `crates/pgevolve-core/src/catalog/mod.rs` (export the new entry point).

### Task 4.1: Create `catalog/cluster.rs`

- [ ] **Step 1: Write the module**

```rust
//! Cluster catalog reader. Queries pg_authid + pg_auth_members.

use crate::catalog::error::CatalogError;
use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::ir::cluster::role::{Role, RoleAttributes};
use crate::pg_querier::PgQuerier;

const ROLES_QUERY: &str = r"
SELECT r.rolname,
       r.rolsuper, r.rolcreatedb, r.rolcreaterole, r.rolinherit,
       r.rolcanlogin, r.rolreplication, r.rolbypassrls,
       r.rolconnlimit::bigint,
       to_char(r.rolvaliduntil AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS valid_until,
       d.description AS comment
FROM pg_authid r
LEFT JOIN pg_shdescription d
  ON d.objoid = r.oid AND d.classoid = 'pg_authid'::regclass
WHERE r.rolname NOT LIKE 'pg\_%' ESCAPE '\'
  AND r.rolname <> ALL($1::text[])
ORDER BY r.rolname
";

const MEMBERS_QUERY: &str = r"
SELECT memb.rolname AS member, parent.rolname AS member_of
FROM pg_auth_members am
JOIN pg_authid memb   ON memb.oid = am.member
JOIN pg_authid parent ON parent.oid = am.roleid
WHERE memb.rolname NOT LIKE 'pg\_%' ESCAPE '\'
  AND parent.rolname NOT LIKE 'pg\_%' ESCAPE '\'
  AND memb.rolname <> ALL($1::text[])
  AND parent.rolname <> ALL($1::text[])
";

/// Read the full cluster catalog from a live Postgres. `bootstrap_roles` are
/// the role names (e.g. `["postgres"]`) that pgevolve treats as PG-owned and
/// never diffs.
pub async fn read_cluster_catalog(
    pg: &PgQuerier,
    bootstrap_roles: &[String],
) -> Result<ClusterCatalog, CatalogError> {
    let roles_rows = pg.query(ROLES_QUERY, &[&bootstrap_roles]).await?;
    let mut roles: Vec<Role> = roles_rows.into_iter().map(decode_role).collect::<Result<_, _>>()?;

    let member_rows = pg.query(MEMBERS_QUERY, &[&bootstrap_roles]).await?;
    for row in member_rows {
        let member_name: String = row.get_text("member")?;
        let parent_name: String = row.get_text("member_of")?;
        let member_id = Identifier::from_unquoted(&member_name)
            .map_err(|e| CatalogError::Structural(format!("invalid role name {member_name:?}: {e}")))?;
        let parent_id = Identifier::from_unquoted(&parent_name)
            .map_err(|e| CatalogError::Structural(format!("invalid role name {parent_name:?}: {e}")))?;
        if let Some(r) = roles.iter_mut().find(|r| r.name == member_id) {
            r.member_of.push(parent_id);
        }
        // If the member role was filtered (predefined/bootstrap), skip the edge silently.
    }

    let mut cat = ClusterCatalog { roles };
    cat.canonicalize();
    Ok(cat)
}

fn decode_role(row: pgevolve_core_row_type) -> Result<Role, CatalogError> {
    // Adapt the row type to whatever the existing PgQuerier returns —
    // look at the per-DB catalog reader (catalog/assemble/tables.rs) for
    // the exact shape. The per-DB code uses `r.get_text(q, "...")?`,
    // `r.get_bool(q, "...")?`, `r.get_opt_text(q, "...")?` — mirror those.
    let name_str = row.get_text("rolname")?;
    let name = Identifier::from_unquoted(&name_str)
        .map_err(|e| CatalogError::Structural(format!("invalid role name {name_str:?}: {e}")))?;
    let connection_limit = match row.get_int8("rolconnlimit")? {
        -1 => None,
        n  => Some(n),
    };
    let valid_until = row.get_opt_text("valid_until")?;
    Ok(Role {
        name,
        attributes: RoleAttributes {
            superuser:        row.get_bool("rolsuper")?,
            createdb:         row.get_bool("rolcreatedb")?,
            createrole:       row.get_bool("rolcreaterole")?,
            inherit:          row.get_bool("rolinherit")?,
            login:            row.get_bool("rolcanlogin")?,
            replication:      row.get_bool("rolreplication")?,
            bypass_rls:       row.get_bool("rolbypassrls")?,
            connection_limit,
            valid_until,
        },
        member_of: Vec::new(),
        comment: row.get_opt_text("comment")?,
    })
}
```

(The `pgevolve_core_row_type` placeholder is a stand-in for the project's actual row type — likely `Row` from a custom binding. Open `crates/pgevolve-core/src/catalog/assemble/tables.rs` and `pg_querier.rs` to see the actual API and adapt. The helper names `get_text`, `get_bool`, `get_int8`, `get_opt_text` come from the existing per-DB reader; use the same names.)

- [ ] **Step 2: Wire the module in `crates/pgevolve-core/src/catalog/mod.rs`**

Add `pub mod cluster;` next to existing `pub mod` lines.

### Task 4.2: Docker-gated integration test

- [ ] **Step 1: Create `crates/pgevolve-core/tests/cluster_catalog.rs`**

Mirror the pattern in `crates/pgevolve-core/tests/catalog_round_trip.rs`:

```rust
//! Docker-gated read tests for cluster catalog.

use pgevolve_core::catalog::cluster::read_cluster_catalog;
use pgevolve_testkit::ephemeral_pg;

#[tokio::test]
#[cfg_attr(not(feature = "docker"), ignore)]
async fn reads_simple_role() {
    let pg = ephemeral_pg().await;
    pg.exec_as_superuser("CREATE ROLE app_user WITH LOGIN CREATEDB CONNECTION LIMIT 50").await;
    let cat = read_cluster_catalog(pg.querier(), &["postgres".into()]).await.unwrap();
    let r = cat.roles.iter().find(|r| r.name.as_str() == "app_user").unwrap();
    assert!(r.attributes.login);
    assert!(r.attributes.createdb);
    assert_eq!(r.attributes.connection_limit, Some(50));
}

#[tokio::test]
#[cfg_attr(not(feature = "docker"), ignore)]
async fn reads_membership_edges() {
    let pg = ephemeral_pg().await;
    pg.exec_as_superuser("CREATE ROLE readers").await;
    pg.exec_as_superuser("CREATE ROLE app_user").await;
    pg.exec_as_superuser("GRANT readers TO app_user").await;
    let cat = read_cluster_catalog(pg.querier(), &["postgres".into()]).await.unwrap();
    let app = cat.roles.iter().find(|r| r.name.as_str() == "app_user").unwrap();
    assert_eq!(app.member_of.iter().map(|i| i.as_str()).collect::<Vec<_>>(), vec!["readers"]);
}

#[tokio::test]
#[cfg_attr(not(feature = "docker"), ignore)]
async fn filters_predefined_and_bootstrap_roles() {
    let pg = ephemeral_pg().await;
    let cat = read_cluster_catalog(pg.querier(), &["postgres".into()]).await.unwrap();
    assert!(!cat.roles.iter().any(|r| r.name.as_str().starts_with("pg_")));
    assert!(!cat.roles.iter().any(|r| r.name.as_str() == "postgres"));
}
```

If `ephemeral_pg` from testkit doesn't expose an `exec_as_superuser` helper, add one or mirror an existing helper that already runs DDL with the testcontainer's superuser DSN.

### Task 4.3: Run + commit

- [ ] **Step 1: Test**

```bash
cargo test -p pgevolve-core --lib catalog::cluster
# If docker available:
cargo test -p pgevolve-core --tests cluster_catalog --features docker
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 2: Commit**

```bash
git add -p crates/pgevolve-core/src/catalog/ crates/pgevolve-core/tests/cluster_catalog.rs
git commit -m "$(cat <<'EOF'
feat(catalog): cluster — read pg_authid + pg_auth_members

Two queries: pg_authid (with pg_shdescription LEFT JOIN for comments)
for role attributes; pg_auth_members joined back to pg_authid for
membership edges. Both filter pg_*-prefixed predefined roles and the
caller-supplied bootstrap-roles list (defaults to ["postgres"] in
the config layer).

attconnlimit = -1 normalizes to None (unlimited); rolvaliduntil is
emitted as an RFC 3339 string for opaque round-trip.

Stage 4 of docs/superpowers/plans/2026-05-21-cluster-roles.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 5 — Differ

`ClusterChangeSet` + `diff_cluster(target, source)` produces the change list. Pair-by-name on roles. Six change kinds.

**Files created:** `crates/pgevolve-core/src/diff/cluster.rs`.
**Files modified:** `crates/pgevolve-core/src/diff/mod.rs`.

### Task 5.1: Create `diff/cluster.rs`

- [ ] **Step 1: Write the module**

```rust
//! Cluster diffing. Pair-by-name on roles; emit one ClusterChange per
//! difference. All ops are catalog-only metadata (DDL through pg_authid),
//! so they're safe by default — except DropRole, which is intent-gated
//! because it can orphan grants in other DBs.

use std::collections::BTreeMap;

use crate::diff::destructiveness::Destructiveness;
use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::ir::cluster::role::{Role, RoleAttributes};

/// One change to apply to a cluster's role layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClusterChange {
    CreateRole(Role),
    DropRole {
        name: Identifier,
    },
    AlterRoleAttributes {
        name: Identifier,
        from: RoleAttributes,
        to:   RoleAttributes,
    },
    GrantRoleMembership {
        member: Identifier,  // who gains the membership
        role:   Identifier,  // which role they become a member of
    },
    RevokeRoleMembership {
        member: Identifier,
        role:   Identifier,
    },
    CommentOnRole {
        name:    Identifier,
        comment: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterChangeEntry {
    pub change: ClusterChange,
    pub destructiveness: Destructiveness,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClusterChangeSet {
    pub entries: Vec<ClusterChangeEntry>,
}

impl ClusterChangeSet {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Diff `source` against `target`. `target` = current live cluster state;
/// `source` = desired state from `roles/*.sql`. Resulting ops applied to
/// `target` produce `source`.
#[must_use]
pub fn diff_cluster(target: &ClusterCatalog, source: &ClusterCatalog) -> ClusterChangeSet {
    let mut entries = Vec::new();

    let target_map: BTreeMap<&Identifier, &Role> =
        target.roles.iter().map(|r| (&r.name, r)).collect();
    let source_map: BTreeMap<&Identifier, &Role> =
        source.roles.iter().map(|r| (&r.name, r)).collect();

    // Adds.
    for (name, source_role) in &source_map {
        if !target_map.contains_key(name) {
            entries.push(ClusterChangeEntry {
                change: ClusterChange::CreateRole((*source_role).clone()),
                destructiveness: Destructiveness::Safe,
            });
        }
    }

    // Drops + alters.
    for (name, target_role) in &target_map {
        match source_map.get(name) {
            None => entries.push(ClusterChangeEntry {
                change: ClusterChange::DropRole { name: (*name).clone() },
                destructiveness: Destructiveness::RequiresApprovalAndDataLossWarning {
                    reason: format!(
                        "drops role {name} — may orphan grants in other DBs"
                    ),
                },
            }),
            Some(source_role) => diff_role(target_role, source_role, &mut entries),
        }
    }

    ClusterChangeSet { entries }
}

fn diff_role(target: &Role, source: &Role, out: &mut Vec<ClusterChangeEntry>) {
    if target.attributes != source.attributes {
        out.push(ClusterChangeEntry {
            change: ClusterChange::AlterRoleAttributes {
                name: target.name.clone(),
                from: target.attributes.clone(),
                to:   source.attributes.clone(),
            },
            destructiveness: Destructiveness::Safe,
        });
    }

    // Membership: emit one Grant per added edge, one Revoke per removed.
    let target_membership: std::collections::BTreeSet<&Identifier> =
        target.member_of.iter().collect();
    let source_membership: std::collections::BTreeSet<&Identifier> =
        source.member_of.iter().collect();
    for added in source_membership.difference(&target_membership) {
        out.push(ClusterChangeEntry {
            change: ClusterChange::GrantRoleMembership {
                member: target.name.clone(),
                role:   (*added).clone(),
            },
            destructiveness: Destructiveness::Safe,
        });
    }
    for removed in target_membership.difference(&source_membership) {
        out.push(ClusterChangeEntry {
            change: ClusterChange::RevokeRoleMembership {
                member: target.name.clone(),
                role:   (*removed).clone(),
            },
            destructiveness: Destructiveness::Safe,
        });
    }

    if target.comment != source.comment {
        out.push(ClusterChangeEntry {
            change: ClusterChange::CommentOnRole {
                name:    target.name.clone(),
                comment: source.comment.clone(),
            },
            destructiveness: Destructiveness::Safe,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn role(name: &str) -> Role {
        Role {
            name: id(name),
            attributes: RoleAttributes::default(),
            member_of: vec![],
            comment: None,
        }
    }

    #[test]
    fn equal_catalogs_yield_no_changes() {
        let c = ClusterCatalog { roles: vec![role("a")] };
        let cs = diff_cluster(&c, &c);
        assert!(cs.is_empty());
    }

    #[test]
    fn added_role_creates() {
        let target = ClusterCatalog::empty();
        let source = ClusterCatalog { roles: vec![role("a")] };
        let cs = diff_cluster(&target, &source);
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(cs.entries[0].change, ClusterChange::CreateRole(_)));
        assert_eq!(cs.entries[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn removed_role_drops_with_intent_gate() {
        let target = ClusterCatalog { roles: vec![role("a")] };
        let source = ClusterCatalog::empty();
        let cs = diff_cluster(&target, &source);
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(cs.entries[0].change, ClusterChange::DropRole { .. }));
        assert!(cs.entries[0].destructiveness.requires_approval());
        assert!(cs.entries[0].destructiveness.data_loss_risk());
    }

    #[test]
    fn attribute_change_emits_alter() {
        let mut t = role("a");
        let mut s = role("a");
        s.attributes.login = true;
        let cs = diff_cluster(
            &ClusterCatalog { roles: vec![t] },
            &ClusterCatalog { roles: vec![s] },
        );
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(cs.entries[0].change, ClusterChange::AlterRoleAttributes { .. }));
    }

    #[test]
    fn added_membership_emits_grant() {
        let mut t = role("a");
        let mut s = role("a");
        s.member_of.push(id("readers"));
        let cs = diff_cluster(
            &ClusterCatalog { roles: vec![t] },
            &ClusterCatalog { roles: vec![s] },
        );
        assert_eq!(cs.entries.len(), 1);
        match &cs.entries[0].change {
            ClusterChange::GrantRoleMembership { member, role } => {
                assert_eq!(member.as_str(), "a");
                assert_eq!(role.as_str(), "readers");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn removed_membership_emits_revoke() {
        let mut t = role("a");
        let mut s = role("a");
        t.member_of.push(id("readers"));
        let cs = diff_cluster(
            &ClusterCatalog { roles: vec![t] },
            &ClusterCatalog { roles: vec![s] },
        );
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(cs.entries[0].change, ClusterChange::RevokeRoleMembership { .. }));
    }

    #[test]
    fn comment_change_emits_comment_op() {
        let mut t = role("a");
        let mut s = role("a");
        s.comment = Some("hello".into());
        let cs = diff_cluster(
            &ClusterCatalog { roles: vec![t] },
            &ClusterCatalog { roles: vec![s] },
        );
        assert_eq!(cs.entries.len(), 1);
        assert!(matches!(cs.entries[0].change, ClusterChange::CommentOnRole { .. }));
    }
}
```

- [ ] **Step 2: Wire the module in `crates/pgevolve-core/src/diff/mod.rs`**

```rust
pub mod cluster;
```

(Re-export `cluster::{ClusterChange, ClusterChangeEntry, ClusterChangeSet, diff_cluster}` if convenient.)

### Task 5.2: Run + commit

```bash
cargo test -p pgevolve-core --lib diff::cluster
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve-core/src/diff/
git commit -m "$(cat <<'EOF'
feat(diff): cluster — ClusterChangeSet + diff_cluster

Pair-by-name on roles. Six change kinds: CreateRole, DropRole,
AlterRoleAttributes (carries from + to for lint downstream),
GrantRoleMembership, RevokeRoleMembership, CommentOnRole.

All ops are Destructiveness::Safe except DropRole, which is gated
behind RequiresApprovalAndDataLossWarning — dropping a role can
orphan grants in other DBs (PG will reject if grants exist, but
the intent gate makes it explicit).

Stage 5 of docs/superpowers/plans/2026-05-21-cluster-roles.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 6 — Render / emit + StepKind

SQL helpers + emit handlers that turn each `ClusterChange` into a `RawStep`.

**Files created:** `crates/pgevolve-core/src/plan/cluster_rewrite/{mod.rs, sql.rs, emit.rs}`.
**Files modified:** `crates/pgevolve-core/src/plan/raw_step.rs`, `crates/pgevolve-core/src/plan/mod.rs`.

### Task 6.1: Add `StepKind` variants

- [ ] **Step 1: Extend `StepKind` in `crates/pgevolve-core/src/plan/raw_step.rs`**

Add after the existing variants (place by family — after the per-DB families is fine since these are a new family):

```rust
    CreateRole,
    DropRole,
    AlterRole,
    GrantRoleMembership,
    RevokeRoleMembership,
    CommentOnRole,
```

The serialization round-trip test must list every variant — extend it.

### Task 6.2: SQL helpers (`cluster_rewrite/sql.rs`)

- [ ] **Step 1: Create `crates/pgevolve-core/src/plan/cluster_rewrite/sql.rs`**

```rust
//! SQL rendering for cluster ops. Mirrors plan/rewrite/sql.rs style.
//! Postgres SQL keywords + attribute keywords are uppercase; identifiers
//! and codec/algorithm names stay lowercase (project convention).

use crate::identifier::Identifier;
use crate::ir::cluster::role::{Role, RoleAttributes};

/// `CREATE ROLE r WITH <options>;`
#[must_use]
pub fn create_role(role: &Role) -> String {
    let mut out = format!("CREATE ROLE {}", role.name.render_sql());
    write_with_options(&mut out, &role.attributes);
    if !role.member_of.is_empty() {
        out.push_str(" IN ROLE ");
        let names: Vec<String> = role.member_of.iter().map(|i| i.render_sql()).collect();
        out.push_str(&names.join(", "));
    }
    out.push(';');
    out
}

/// `DROP ROLE r;`
#[must_use]
pub fn drop_role(name: &Identifier) -> String {
    format!("DROP ROLE {};", name.render_sql())
}

/// `ALTER ROLE r WITH <only changed options>;`
#[must_use]
pub fn alter_role_attributes(
    name: &Identifier,
    from: &RoleAttributes,
    to: &RoleAttributes,
) -> String {
    let mut out = format!("ALTER ROLE {}", name.render_sql());
    let mut wrote_any = false;
    macro_rules! emit_bool {
        ($field:ident, $on:literal, $off:literal) => {
            if from.$field != to.$field {
                out.push(' ');
                out.push_str(if to.$field { $on } else { $off });
                wrote_any = true;
            }
        };
    }
    emit_bool!(superuser,   "SUPERUSER",   "NOSUPERUSER");
    emit_bool!(createdb,    "CREATEDB",    "NOCREATEDB");
    emit_bool!(createrole,  "CREATEROLE",  "NOCREATEROLE");
    emit_bool!(inherit,     "INHERIT",     "NOINHERIT");
    emit_bool!(login,       "LOGIN",       "NOLOGIN");
    emit_bool!(replication, "REPLICATION", "NOREPLICATION");
    emit_bool!(bypass_rls,  "BYPASSRLS",   "NOBYPASSRLS");
    if from.connection_limit != to.connection_limit {
        let n = to.connection_limit.unwrap_or(-1);
        out.push_str(&format!(" CONNECTION LIMIT {n}"));
        wrote_any = true;
    }
    if from.valid_until != to.valid_until {
        match &to.valid_until {
            Some(ts) => out.push_str(&format!(" VALID UNTIL '{ts}'")),
            None     => out.push_str(" VALID UNTIL 'infinity'"),
        }
        wrote_any = true;
    }
    let _ = wrote_any;  // assertion-only; if no diffs we wouldn't be here
    out.push(';');
    out
}

/// `GRANT role TO member;`
#[must_use]
pub fn grant_role_membership(role: &Identifier, member: &Identifier) -> String {
    format!("GRANT {} TO {};", role.render_sql(), member.render_sql())
}

/// `REVOKE role FROM member;`
#[must_use]
pub fn revoke_role_membership(role: &Identifier, member: &Identifier) -> String {
    format!("REVOKE {} FROM {};", role.render_sql(), member.render_sql())
}

/// `COMMENT ON ROLE r IS '...';` or `IS NULL` to clear.
#[must_use]
pub fn comment_on_role(name: &Identifier, comment: Option<&str>) -> String {
    match comment {
        Some(text) => format!(
            "COMMENT ON ROLE {} IS '{}';",
            name.render_sql(),
            text.replace('\'', "''")
        ),
        None => format!("COMMENT ON ROLE {} IS NULL;", name.render_sql()),
    }
}

fn write_with_options(out: &mut String, attrs: &RoleAttributes) {
    out.push_str(" WITH");
    out.push_str(if attrs.superuser   { " SUPERUSER"   } else { " NOSUPERUSER"   });
    out.push_str(if attrs.createdb    { " CREATEDB"    } else { " NOCREATEDB"    });
    out.push_str(if attrs.createrole  { " CREATEROLE"  } else { " NOCREATEROLE"  });
    out.push_str(if attrs.inherit     { " INHERIT"     } else { " NOINHERIT"     });
    out.push_str(if attrs.login       { " LOGIN"       } else { " NOLOGIN"       });
    out.push_str(if attrs.replication { " REPLICATION" } else { " NOREPLICATION" });
    out.push_str(if attrs.bypass_rls  { " BYPASSRLS"   } else { " NOBYPASSRLS"   });
    if let Some(n) = attrs.connection_limit {
        out.push_str(&format!(" CONNECTION LIMIT {n}"));
    }
    if let Some(ts) = &attrs.valid_until {
        out.push_str(&format!(" VALID UNTIL '{ts}'"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn create_role_default_attributes() {
        let r = Role {
            name: id("app_user"),
            attributes: RoleAttributes::default(),
            member_of: vec![],
            comment: None,
        };
        let sql = create_role(&r);
        assert!(sql.starts_with("CREATE ROLE app_user WITH"), "got: {sql}");
        assert!(sql.contains("NOLOGIN"));
        assert!(sql.ends_with(';'));
    }

    #[test]
    fn create_role_with_inherit_inherit_default() {
        let r = Role {
            name: id("app_user"),
            attributes: RoleAttributes::default(),
            member_of: vec![],
            comment: None,
        };
        let sql = create_role(&r);
        assert!(sql.contains("INHERIT"));
        assert!(!sql.contains("NOINHERIT"));
    }

    #[test]
    fn create_role_with_membership() {
        let r = Role {
            name: id("app_user"),
            attributes: RoleAttributes::default(),
            member_of: vec![id("readers"), id("writers")],
            comment: None,
        };
        let sql = create_role(&r);
        assert!(sql.contains("IN ROLE readers, writers"), "got: {sql}");
    }

    #[test]
    fn alter_role_only_emits_changed_attrs() {
        let from = RoleAttributes::default();
        let mut to = RoleAttributes::default();
        to.login = true;
        to.createdb = true;
        let sql = alter_role_attributes(&id("app_user"), &from, &to);
        assert!(sql.contains("LOGIN"));
        assert!(sql.contains("CREATEDB"));
        assert!(!sql.contains("SUPERUSER"));
    }

    #[test]
    fn grant_revoke_membership() {
        assert_eq!(
            grant_role_membership(&id("readers"), &id("app_user")),
            "GRANT readers TO app_user;"
        );
        assert_eq!(
            revoke_role_membership(&id("readers"), &id("app_user")),
            "REVOKE readers FROM app_user;"
        );
    }

    #[test]
    fn comment_quotes_apostrophes() {
        let sql = comment_on_role(&id("app_user"), Some("it's fine"));
        assert!(sql.contains("'it''s fine'"), "got: {sql}");
    }

    #[test]
    fn drop_role_renders() {
        assert_eq!(drop_role(&id("app_user")), "DROP ROLE app_user;");
    }
}
```

### Task 6.3: Emit handlers (`cluster_rewrite/emit.rs`)

- [ ] **Step 1: Create `crates/pgevolve-core/src/plan/cluster_rewrite/mod.rs`**

```rust
pub mod sql;
pub mod emit;

pub use emit::emit_cluster_changes;
```

- [ ] **Step 2: Create `crates/pgevolve-core/src/plan/cluster_rewrite/emit.rs`**

```rust
//! Translate ClusterChange → RawStep. Sibling of `plan/rewrite/emit/table.rs`.

use crate::diff::cluster::{ClusterChange, ClusterChangeEntry, ClusterChangeSet};
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

use super::sql;

/// Emit one RawStep per ClusterChange. All cluster ops run InTransaction.
#[must_use]
pub fn emit_cluster_changes(cs: &ClusterChangeSet) -> Vec<RawStep> {
    cs.entries.iter().map(emit_one).collect()
}

fn emit_one(entry: &ClusterChangeEntry) -> RawStep {
    match &entry.change {
        ClusterChange::CreateRole(role) => RawStep {
            step_no: 0,
            kind: StepKind::CreateRole,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![/* cluster-scope: no qname; use a sentinel or leave empty per the project convention */],
            sql: sql::create_role(role),
            transactional: TransactionConstraint::InTransaction,
        },
        ClusterChange::DropRole { name } => RawStep {
            step_no: 0,
            kind: StepKind::DropRole,
            destructive: true,
            destructive_reason: entry.destructiveness.reason().map(str::to_owned),
            intent_id: None,
            targets: vec![],
            sql: sql::drop_role(name),
            transactional: TransactionConstraint::InTransaction,
        },
        ClusterChange::AlterRoleAttributes { name, from, to } => RawStep {
            step_no: 0,
            kind: StepKind::AlterRole,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: sql::alter_role_attributes(name, from, to),
            transactional: TransactionConstraint::InTransaction,
        },
        ClusterChange::GrantRoleMembership { member, role } => RawStep {
            step_no: 0,
            kind: StepKind::GrantRoleMembership,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: sql::grant_role_membership(role, member),
            transactional: TransactionConstraint::InTransaction,
        },
        ClusterChange::RevokeRoleMembership { member, role } => RawStep {
            step_no: 0,
            kind: StepKind::RevokeRoleMembership,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: sql::revoke_role_membership(role, member),
            transactional: TransactionConstraint::InTransaction,
        },
        ClusterChange::CommentOnRole { name, comment } => RawStep {
            step_no: 0,
            kind: StepKind::CommentOnRole,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: sql::comment_on_role(name, comment.as_deref()),
            transactional: TransactionConstraint::InTransaction,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::cluster::ClusterChangeEntry;
    use crate::diff::destructiveness::Destructiveness;
    use crate::identifier::Identifier;
    use crate::ir::cluster::role::{Role, RoleAttributes};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn create_role_emits_create_kind() {
        let entry = ClusterChangeEntry {
            change: ClusterChange::CreateRole(Role {
                name: id("app_user"),
                attributes: RoleAttributes::default(),
                member_of: vec![],
                comment: None,
            }),
            destructiveness: Destructiveness::Safe,
        };
        let step = emit_one(&entry);
        assert!(matches!(step.kind, StepKind::CreateRole));
        assert!(step.sql.starts_with("CREATE ROLE"));
        assert!(!step.destructive);
    }

    #[test]
    fn drop_role_emits_destructive() {
        let entry = ClusterChangeEntry {
            change: ClusterChange::DropRole { name: id("old") },
            destructiveness: Destructiveness::RequiresApprovalAndDataLossWarning {
                reason: "drops".into(),
            },
        };
        let step = emit_one(&entry);
        assert!(step.destructive);
        assert!(step.destructive_reason.is_some());
    }
}
```

Adapt the `targets: vec![]` line to whatever the existing `RawStep.targets` field requires. Cluster ops aren't scoped to a `QualifiedName` (they live at cluster scope); if the type insists on at least one entry, introduce a synthetic `cluster.role.<name>` target. Read the existing `RawStep` definition and its callers for the convention.

- [ ] **Step 3: Wire `cluster_rewrite` into `crates/pgevolve-core/src/plan/mod.rs`**

Add `pub mod cluster_rewrite;` and re-export the entry point.

### Task 6.4: Run + commit

```bash
cargo test -p pgevolve-core --lib plan::cluster_rewrite
cargo test -p pgevolve-core --lib plan::raw_step  # round-trip test now includes new variants
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve-core/src/plan/
git commit -m "$(cat <<'EOF'
feat(plan): cluster — SQL renderers + emit handlers + 6 StepKinds

plan::cluster_rewrite is the cluster-side mirror of plan::rewrite.
sql.rs holds the per-change DDL renderers; emit.rs turns each
ClusterChange into a RawStep. raw_step::StepKind gains six new
variants: CreateRole, DropRole, AlterRole, GrantRoleMembership,
RevokeRoleMembership, CommentOnRole. All cluster ops run InTransaction.

Stage 6 of docs/superpowers/plans/2026-05-21-cluster-roles.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 7 — Lint rules

Two universal cluster lints: `role-loses-superuser` (warning) and `role-membership-cycle` (error). New dispatcher `check_cluster_changeset`.

**Files created:** `crates/pgevolve-core/src/lint/rules/role_loses_superuser.rs`, `crates/pgevolve-core/src/lint/rules/role_membership_cycle.rs`.
**Files modified:** `crates/pgevolve-core/src/lint/rules/mod.rs`, `crates/pgevolve-core/src/lint/universal.rs`, `crates/pgevolve-core/src/lint/mod.rs`.

### Task 7.1: `role-loses-superuser`

- [ ] **Step 1: Create `crates/pgevolve-core/src/lint/rules/role_loses_superuser.rs`**

```rust
//! Warns when an ALTER ROLE flips SUPERUSER true → false. Losing superuser
//! is rarely a routine config change; usually intentional but worth surfacing.

use crate::diff::cluster::{ClusterChange, ClusterChangeSet};
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "role-loses-superuser";

pub(crate) fn check(cs: &ClusterChangeSet) -> Vec<Finding> {
    let mut findings = Vec::new();
    for entry in &cs.entries {
        if let ClusterChange::AlterRoleAttributes { name, from, to } = &entry.change
            && from.superuser
            && !to.superuser
        {
            findings.push(Finding {
                rule_id: RULE_ID.into(),
                severity: Severity::Warning,
                message: format!(
                    "role {name} loses SUPERUSER — confirm this is intentional; \
                     downgrading superuser is rarely a routine change"
                ),
                target: target_for_role(name),
            });
        }
    }
    findings
}

fn target_for_role(name: &crate::identifier::Identifier) -> /* the project's Target type */ todo!() {
    // Inspect crate::lint::finding::Finding::target's type. If it's a
    // QualifiedName, construct one in the synthetic "cluster" schema; if
    // it's an enum with a Role variant, use that; etc.
}
```

(The `target` field shape depends on what `Finding` carries — verify in `finding.rs`. If `Finding.target: QualifiedName` and there's no cluster-scope variant, propose a small extension to `Finding` to carry an enum `LintTarget::PerDb(QualifiedName) | LintTarget::Cluster { kind: &'static str, name: String }`. If that's too disruptive, use a synthetic qname like `cluster.role.<name>` for v0.3.0.)

Add tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::cluster::{ClusterChangeEntry, ClusterChange};
    use crate::diff::destructiveness::Destructiveness;
    use crate::identifier::Identifier;
    use crate::ir::cluster::role::RoleAttributes;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn alter_with(name: &str, from_super: bool, to_super: bool) -> ClusterChangeSet {
        let mut from = RoleAttributes::default();
        from.superuser = from_super;
        let mut to = RoleAttributes::default();
        to.superuser = to_super;
        ClusterChangeSet {
            entries: vec![ClusterChangeEntry {
                change: ClusterChange::AlterRoleAttributes {
                    name: id(name),
                    from,
                    to,
                },
                destructiveness: Destructiveness::Safe,
            }],
        }
    }

    #[test]
    fn loses_superuser_fires() {
        let cs = alter_with("admin", true, false);
        let f = check(&cs);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule_id, RULE_ID);
        assert_eq!(f[0].severity, Severity::Warning);
    }

    #[test]
    fn gains_superuser_silent() {
        let cs = alter_with("admin", false, true);
        assert!(check(&cs).is_empty());
    }

    #[test]
    fn no_superuser_change_silent() {
        let cs = alter_with("admin", true, true);
        assert!(check(&cs).is_empty());
    }
}
```

### Task 7.2: `role-membership-cycle`

- [ ] **Step 1: Create `crates/pgevolve-core/src/lint/rules/role_membership_cycle.rs`**

```rust
//! Errors when the projected post-apply membership graph contains a cycle.
//!
//! Approach: build the membership graph from the current source IR plus the
//! changeset's pending grants; check for a cycle reachable from each grant's
//! `member`. PG rejects cycles at apply time; we catch them pre-plan for a
//! better error.

use std::collections::{BTreeMap, BTreeSet};

use crate::diff::cluster::{ClusterChange, ClusterChangeSet};
use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "role-membership-cycle";

/// Cycle detection needs the IR (for existing membership) plus the changeset
/// (for pending grants). Signature differs from check_changeset's single-arg
/// shape — see `universal::check_cluster_changeset` for how this gets called.
pub(crate) fn check(source: &ClusterCatalog, cs: &ClusterChangeSet) -> Vec<Finding> {
    // Build the post-apply membership graph: source IR's edges minus pending
    // revokes plus pending grants.
    let mut graph: BTreeMap<Identifier, BTreeSet<Identifier>> = BTreeMap::new();
    for r in &source.roles {
        graph.insert(r.name.clone(), r.member_of.iter().cloned().collect());
    }
    for entry in &cs.entries {
        match &entry.change {
            ClusterChange::GrantRoleMembership { member, role } => {
                graph.entry(member.clone()).or_default().insert(role.clone());
            }
            ClusterChange::RevokeRoleMembership { member, role } => {
                if let Some(set) = graph.get_mut(member) {
                    set.remove(role);
                }
            }
            _ => {}
        }
    }

    // For each pending grant, check that the post-apply graph from `member`
    // doesn't reach back to `member`.
    let mut findings = Vec::new();
    for entry in &cs.entries {
        if let ClusterChange::GrantRoleMembership { member, role } = &entry.change
            && reaches(&graph, role, member)
        {
            findings.push(Finding {
                rule_id: RULE_ID.into(),
                severity: Severity::Error,
                message: format!(
                    "GRANT {role} TO {member} creates a role-membership cycle; \
                     Postgres will reject this at apply time"
                ),
                target: /* same target shape as role_loses_superuser */ todo!(),
            });
        }
    }
    findings
}

fn reaches(graph: &BTreeMap<Identifier, BTreeSet<Identifier>>, from: &Identifier, target: &Identifier) -> bool {
    if from == target {
        return true;
    }
    let mut stack = vec![from.clone()];
    let mut seen = BTreeSet::new();
    while let Some(node) = stack.pop() {
        if !seen.insert(node.clone()) {
            continue;
        }
        if let Some(parents) = graph.get(&node) {
            for p in parents {
                if p == target {
                    return true;
                }
                stack.push(p.clone());
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::cluster::{ClusterChangeEntry, ClusterChange};
    use crate::diff::destructiveness::Destructiveness;
    use crate::ir::cluster::role::{Role, RoleAttributes};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn role(name: &str, parents: Vec<&str>) -> Role {
        Role {
            name: id(name),
            attributes: RoleAttributes::default(),
            member_of: parents.into_iter().map(id).collect(),
            comment: None,
        }
    }

    fn grant(member: &str, role: &str) -> ClusterChangeEntry {
        ClusterChangeEntry {
            change: ClusterChange::GrantRoleMembership {
                member: id(member),
                role: id(role),
            },
            destructiveness: Destructiveness::Safe,
        }
    }

    #[test]
    fn direct_cycle_fires() {
        // a -> b already exists in source. Pending: b -> a. Cycle.
        let src = ClusterCatalog { roles: vec![role("a", vec!["b"]), role("b", vec![])] };
        let cs = ClusterChangeSet { entries: vec![grant("b", "a")] };
        let f = check(&src, &cs);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Error);
    }

    #[test]
    fn self_cycle_fires() {
        let src = ClusterCatalog { roles: vec![role("a", vec![])] };
        let cs = ClusterChangeSet { entries: vec![grant("a", "a")] };
        let f = check(&src, &cs);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn dag_silent() {
        let src = ClusterCatalog { roles: vec![role("a", vec![]), role("b", vec![])] };
        let cs = ClusterChangeSet { entries: vec![grant("a", "b")] };
        assert!(check(&src, &cs).is_empty());
    }
}
```

### Task 7.3: Wire dispatcher

- [ ] **Step 1: Register both rules in `crates/pgevolve-core/src/lint/rules/mod.rs`**

```rust
pub mod role_loses_superuser;
pub mod role_membership_cycle;
```

- [ ] **Step 2: Add `check_cluster_changeset` in `crates/pgevolve-core/src/lint/universal.rs`**

```rust
/// Run all cluster-changeset-level lint rules. Mirrors check_changeset
/// for per-DB lints. Takes both `source` (for graph context) and `cs`.
pub fn check_cluster_changeset(
    source: &crate::ir::cluster::catalog::ClusterCatalog,
    cs: &crate::diff::cluster::ClusterChangeSet,
) -> Vec<crate::lint::finding::Finding> {
    let mut out = Vec::new();
    out.extend(rules::role_loses_superuser::check(cs));
    out.extend(rules::role_membership_cycle::check(source, cs));
    out
}
```

Update the module-level doc-comment index in `universal.rs` to list both new rules under a "Cluster changeset-level rules" heading.

- [ ] **Step 3: Re-export from `crates/pgevolve-core/src/lint/mod.rs`**

```rust
pub use universal::check_cluster_changeset;
```

### Task 7.4: Run + commit

```bash
cargo test -p pgevolve-core --lib lint
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve-core/src/lint/
git commit -m "$(cat <<'EOF'
feat(lint): cluster — role-loses-superuser + role-membership-cycle

Two new lint rules + check_cluster_changeset dispatcher.

  role-loses-superuser (warning): fires when AlterRoleAttributes
  flips superuser true → false. Losing superuser is rarely routine;
  surfacing it lets operators catch unintended downgrades.

  role-membership-cycle (error): builds the post-apply membership
  graph from source IR + pending grants/revokes and checks that no
  pending grant creates a cycle. PG rejects cycles at apply time;
  pre-plan detection gives a better error.

Stage 7 of docs/superpowers/plans/2026-05-21-cluster-roles.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 8 — Cluster config (`pgevolve-cluster.toml`)

Schema + loader for the cluster project's config file.

**Files created:** `crates/pgevolve/src/cluster_config.rs`.
**Files modified:** `crates/pgevolve/src/lib.rs` (re-export the module).

### Task 8.1: Schema + loader

- [ ] **Step 1: Create `crates/pgevolve/src/cluster_config.rs`**

Mirror the existing `crates/pgevolve/src/config.rs` shape. Schema:

```rust
//! `pgevolve-cluster.toml` schema + loader. Sibling of pgevolve.toml.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Deserialize)]
pub struct ClusterConfig {
    pub project: ClusterProject,
    pub connection: ClusterConnection,
    #[serde(default)]
    pub bootstrap: Bootstrap,
}

#[derive(Debug, Deserialize)]
pub struct ClusterProject {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct ClusterConnection {
    pub dsn: String,
}

#[derive(Debug, Deserialize)]
pub struct Bootstrap {
    /// Roles that pgevolve treats as PG-owned and never diffs in/out.
    /// Defaults to ["postgres"].
    pub roles: Vec<String>,
}

impl Default for Bootstrap {
    fn default() -> Self {
        Self { roles: vec!["postgres".into()] }
    }
}

#[derive(Debug, Error)]
pub enum ClusterConfigError {
    #[error("i/o reading {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
    #[error("parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("missing required section: {0}")]
    Missing(&'static str),
}

/// Load `pgevolve-cluster.toml` from disk.
pub fn load(path: &Path) -> Result<ClusterConfig, ClusterConfigError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| ClusterConfigError::Io(path.to_path_buf(), e))?;
    let cfg: ClusterConfig = toml::from_str(&raw)?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal() {
        let toml_text = r#"
            [project]
            name = "my-cluster"
            [connection]
            dsn = "postgresql://superuser@localhost:5432/postgres"
        "#;
        let cfg: ClusterConfig = toml::from_str(toml_text).unwrap();
        assert_eq!(cfg.project.name, "my-cluster");
        assert_eq!(cfg.bootstrap.roles, vec!["postgres"]);
    }

    #[test]
    fn parses_with_custom_bootstrap_roles() {
        let toml_text = r#"
            [project]
            name = "x"
            [connection]
            dsn = "postgres://x"
            [bootstrap]
            roles = ["postgres", "cloudsqlsuperuser"]
        "#;
        let cfg: ClusterConfig = toml::from_str(toml_text).unwrap();
        assert_eq!(cfg.bootstrap.roles.len(), 2);
    }
}
```

- [ ] **Step 2: Re-export from `crates/pgevolve/src/lib.rs`**

Add `pub mod cluster_config;` near the existing module declarations.

- [ ] **Step 3: Run + commit**

```bash
cargo test -p pgevolve --lib cluster_config
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve/src/
git commit -m "$(cat <<'EOF'
feat(config): pgevolve-cluster.toml schema + loader

Schema mirrors the per-DB pgevolve.toml shape: [project], [connection],
[bootstrap]. The [bootstrap].roles list (defaulting to ["postgres"])
tells the catalog reader which role names to treat as PG-owned and
never diff in/out.

Stage 8 of docs/superpowers/plans/2026-05-21-cluster-roles.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 9 — API + executor

The library-level entry points: `build_cluster_plan` and `apply_cluster_plan`. Mirror the per-DB shape in `crates/pgevolve/src/api/mod.rs`.

**Files created:** `crates/pgevolve/src/api/cluster.rs`, `crates/pgevolve/src/executor/cluster_apply.rs`.
**Files modified:** `crates/pgevolve/src/api/mod.rs` (re-exports).

### Task 9.1: `build_cluster_plan`

- [ ] **Step 1: Create `crates/pgevolve/src/api/cluster.rs`**

```rust
//! Cluster-level library entry points. Mirrors api::build_plan / api::apply_plan
//! for the per-DB surface but operates over ClusterCatalog + ClusterChangeSet.

use std::path::Path;

use pgevolve_core::diff::cluster::{diff_cluster, ClusterChangeSet};
use pgevolve_core::ir::cluster::catalog::ClusterCatalog;
use pgevolve_core::lint::check_cluster_changeset;
use pgevolve_core::parse::cluster::parse_cluster_directory;
use pgevolve_core::plan::cluster_rewrite::emit_cluster_changes;
use pgevolve_core::plan::raw_step::RawStep;
use pgevolve_core::catalog::cluster::read_cluster_catalog;

use crate::cluster_config::ClusterConfig;
use crate::connection::open;

/// Output of building a cluster plan.
pub struct ClusterPlan {
    pub steps:    Vec<RawStep>,
    pub source:   ClusterCatalog,
    pub target:   ClusterCatalog,
    pub findings: Vec<pgevolve_core::lint::finding::Finding>,
}

/// Build a cluster plan: parse roles/, read live cluster, diff, lint, emit.
pub async fn build_cluster_plan(
    project_root: &Path,
    cfg: &ClusterConfig,
) -> Result<ClusterPlan, /* an error type — see existing build_plan for the variant set */ Box<dyn std::error::Error + Send + Sync>> {
    let roles_dir = project_root.join("roles");
    let source = parse_cluster_directory(&roles_dir)?;

    let pg = open(&cfg.connection.dsn).await?;
    let target = read_cluster_catalog(&pg, &cfg.bootstrap.roles).await?;

    let changes = diff_cluster(&target, &source);
    let findings = check_cluster_changeset(&source, &changes);
    let steps = emit_cluster_changes(&changes);

    Ok(ClusterPlan { steps, source, target, findings })
}
```

Adjust the error type to whatever the existing `build_plan` returns. If `build_plan` uses a custom error enum, extend it with cluster variants instead of `Box<dyn ...>`.

### Task 9.2: `apply_cluster_plan`

- [ ] **Step 1: Create `crates/pgevolve/src/executor/cluster_apply.rs`**

The per-step apply loop already exists in `crates/pgevolve/src/executor/execute.rs`. Cluster apply reuses the per-step runner but takes a `Vec<RawStep>` directly (no plan directory yet — that's where `pgevolve cluster plan` writes them; `pgevolve cluster apply` reads them back).

Skeleton:

```rust
//! Cluster apply: read cluster-plans/<id>/, run each step under superuser DSN.

use std::path::Path;

use pgevolve_core::plan::raw_step::RawStep;

use crate::cluster_config::ClusterConfig;
use crate::connection::open;

pub async fn apply_cluster_plan_dir(
    plan_dir: &Path,
    cfg: &ClusterConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let steps = load_cluster_plan_steps(plan_dir)?;
    let pg = open(&cfg.connection.dsn).await?;
    for step in &steps {
        crate::executor::execute::run_step(&pg, step).await?;
    }
    Ok(())
}

fn load_cluster_plan_steps(_plan_dir: &Path) -> Result<Vec<RawStep>, /* error */ Box<dyn std::error::Error + Send + Sync>> {
    // Mirror the per-DB read_plan_dir shape. Cluster plan files have the
    // same layout (plan.sql + intent.toml + manifest.toml) — just a
    // different directory root (cluster-plans/<id>/).
    todo!("port plan deserialization from crate::executor::plan_dir::read")
}
```

The `todo!` needs implementation: copy the deserialization path used by per-DB apply and adapt it to read `cluster-plans/<id>/{plan.sql, intent.toml, manifest.toml}`. If the existing `read_plan_dir` is generic enough, parameterize the root path.

### Task 9.3: Re-export + commit

- [ ] **Step 1: Update `crates/pgevolve/src/api/mod.rs`** to add `pub mod cluster;` and re-export `build_cluster_plan` / `ClusterPlan`.

- [ ] **Step 2: Update `crates/pgevolve/src/executor/mod.rs`** to add `pub mod cluster_apply;`.

- [ ] **Step 3: Test + commit**

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
git add -p crates/pgevolve/src/
git commit -m "$(cat <<'EOF'
feat(api): cluster — build_cluster_plan + apply_cluster_plan

api::cluster::build_cluster_plan runs parse → catalog read → diff
→ lint → emit against the cluster project. executor::cluster_apply
runs the resulting steps against the superuser DSN.

Stage 9 of docs/superpowers/plans/2026-05-21-cluster-roles.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 10 — CLI commands

`pgevolve cluster {init,diff,plan,apply,status}`.

**Files created:** `crates/pgevolve/src/commands/cluster/{mod.rs, init.rs, diff.rs, plan.rs, apply.rs, status.rs}`.
**Files modified:** `crates/pgevolve/src/cli.rs`, `crates/pgevolve/src/commands/mod.rs`, `crates/pgevolve/src/main.rs`.

### Task 10.1: CLI enum

- [ ] **Step 1: Extend the `Command` enum in `crates/pgevolve/src/cli.rs`**

Add a new variant:

```rust
    /// Cluster-level commands (roles, future: tablespaces, GUCs, etc.).
    Cluster(ClusterArgs),
```

And define `ClusterArgs`:

```rust
#[derive(Args, Debug)]
pub struct ClusterArgs {
    /// Path to pgevolve-cluster.toml. Defaults to ./pgevolve-cluster.toml.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub cmd: ClusterCommand,
}

#[derive(Subcommand, Debug)]
pub enum ClusterCommand {
    /// Scaffold a new cluster project.
    Init { path: Option<PathBuf> },
    /// Show the diff between source and live cluster.
    Diff,
    /// Produce a cluster plan directory.
    Plan,
    /// Apply a cluster plan directory.
    Apply { plan_id: Option<String> },
    /// Show recent cluster applies.
    Status,
}
```

### Task 10.2: Command implementations

Mirror the per-DB equivalents (`commands/init.rs`, `commands/diff.rs`, etc.) — they're the cleanest templates.

- [ ] **Step 1: `commands/cluster/init.rs`**

Scaffolds:
- `pgevolve-cluster.toml` with placeholder DSN
- `roles/` empty directory
- `.gitignore` entry for `cluster-plans/`

- [ ] **Step 2: `commands/cluster/diff.rs`**

Calls `api::cluster::build_cluster_plan`, prints the resulting `Vec<RawStep>` as a human-readable diff (or JSON via the global `--format`).

- [ ] **Step 3: `commands/cluster/plan.rs`**

Calls `build_cluster_plan`, writes `cluster-plans/<plan_id>/plan.sql + intent.toml + manifest.toml`. Mirror `commands/plan.rs` for the file-writing logic; the plan-id BLAKE3 hash is computed over the canonical `ClusterCatalog`.

- [ ] **Step 4: `commands/cluster/apply.rs`**

Loads `cluster-plans/<id>/`, calls `executor::cluster_apply::apply_cluster_plan_dir`.

- [ ] **Step 5: `commands/cluster/status.rs`**

Lists `cluster-plans/` directory contents with applied state. Mirror per-DB `status`.

- [ ] **Step 6: `commands/cluster/mod.rs`**

```rust
pub mod apply;
pub mod diff;
pub mod init;
pub mod plan;
pub mod status;
```

### Task 10.3: Wire main dispatcher

- [ ] **Step 1: Update `crates/pgevolve/src/main.rs`**

In the `match cli.cmd { ... }` dispatch, add:

```rust
Command::Cluster(args) => {
    let cfg_path = args.config.unwrap_or_else(|| "pgevolve-cluster.toml".into());
    let cfg = crate::cluster_config::load(&cfg_path)?;
    match args.cmd {
        ClusterCommand::Init { path } => commands::cluster::init::run(path).await?,
        ClusterCommand::Diff           => commands::cluster::diff::run(&cfg).await?,
        ClusterCommand::Plan           => commands::cluster::plan::run(&cfg).await?,
        ClusterCommand::Apply { plan_id } => commands::cluster::apply::run(&cfg, plan_id.as_deref()).await?,
        ClusterCommand::Status         => commands::cluster::status::run(&cfg).await?,
    }
}
```

(Adapt to the existing dispatch shape — it may already use a different error type or async runtime setup.)

### Task 10.4: Run + commit

```bash
cargo build --workspace
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p pgevolve -- cluster --help    # smoke test
git add -p crates/pgevolve/src/
git commit -m "$(cat <<'EOF'
feat(cli): cluster — init, diff, plan, apply, status

New command family: pgevolve cluster {init,diff,plan,apply,status}.
Each loads pgevolve-cluster.toml (path overridable via --config) and
dispatches to the matching api/executor entry point. Cluster plans
live in cluster-plans/<plan_id>/ — sibling of per-DB plans/<plan_id>/.

Stage 10 of docs/superpowers/plans/2026-05-21-cluster-roles.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 11 — Conformance harness + fixtures

Add `authoring = "cluster"` mode to the conformance harness, add six fixtures, bless.

**Files modified:** `crates/pgevolve-conformance/src/fixture.rs`, `crates/pgevolve-conformance/src/planning.rs`, `crates/pgevolve-conformance/tests/run.rs`.
**Files created:** `crates/pgevolve-conformance/tests/cases/cluster/roles/*` (six fixtures).

### Task 11.1: Harness extension

- [ ] **Step 1: Add `Authoring::Cluster` variant**

In `crates/pgevolve-conformance/src/fixture.rs`, find the `Authoring` enum (string-deserialized from `[meta].authoring`). Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Authoring {
    Objects,
    Scenarios,
    Intent,
    Cluster,   // NEW
}
```

- [ ] **Step 2: Add `render_cluster_plan` in `planning.rs`**

Sibling of `render_plan`. Reads `before.sql`/`after.sql` as cluster source (writes them into a temp `roles/` dir), runs the cluster pipeline (`parse_cluster_directory + diff_cluster + check_cluster_changeset + emit_cluster_changes`), returns `(steps_sql_string, advisory_findings)`.

- [ ] **Step 3: Add `run_cluster` in `tests/run.rs`**

Sibling of `run_objects`. Reads the fixture, dispatches `render_cluster_plan`, asserts against `expected/plan.sql`, `expected/advisory.json`, etc. — match the per-objects assertion shape.

In the top-level test driver, when `meta.authoring == Cluster`, call `run_cluster` instead of `run_objects`.

### Task 11.2: Six fixtures

For each fixture, create `crates/pgevolve-conformance/tests/cases/cluster/roles/<fixture>/`:
- `before.sql` — cluster source representing the existing live state
- `after.sql` — cluster source representing the desired state
- `fixture.toml`
- `expected/` — bless populates

**Fixture 1: `create-simple-role/`**

`before.sql`: empty
`after.sql`:
```sql
CREATE ROLE app_user;
```
`fixture.toml`:
```toml
[meta]
title     = "CREATE ROLE app_user with defaults"
authoring = "cluster"
spec_refs = ["cluster.role.create"]

[pg]
min = 14
max = 17

[expect.plan]
steps = 1
```

**Fixture 2: `create-login-user/`**

`after.sql`:
```sql
CREATE USER app_user;
```
`fixture.toml`: assert plan contains `LOGIN`.

**Fixture 3: `alter-role-attributes/`**

`before.sql`:
```sql
CREATE ROLE r;
```
`after.sql`:
```sql
CREATE ROLE r WITH CREATEDB CONNECTION LIMIT 50;
```
Assert single `ALTER ROLE` step with `CREATEDB` and `CONNECTION LIMIT 50` only.

**Fixture 4: `add-membership/`**

`before.sql`:
```sql
CREATE ROLE readers;
CREATE ROLE app_user;
```
`after.sql`:
```sql
CREATE ROLE readers;
CREATE ROLE app_user;
GRANT readers TO app_user;
```
Assert one `GRANT readers TO app_user` step.

**Fixture 5: `drop-role-intent-gated/`**

`before.sql`:
```sql
CREATE ROLE old_user;
```
`after.sql`: empty.

`fixture.toml`: assert `[expect.intent] destructive_steps = 1`.

**Fixture 6: `comment-on-role/`**

`before.sql`:
```sql
CREATE ROLE app_user;
```
`after.sql`:
```sql
CREATE ROLE app_user;
COMMENT ON ROLE app_user IS 'application service account';
```
Assert one `COMMENT ON ROLE` step.

### Task 11.3: Bless + run

```bash
cargo xtask bless --conformance
cargo test -p pgevolve-conformance
```

Verify the blessed `expected/plan.sql` files contain the expected SQL.

### Task 11.4: Commit

```bash
git add -p crates/pgevolve-conformance/
git commit -m "$(cat <<'EOF'
test(conformance): cluster harness + 6 role fixtures

Conformance harness now supports authoring = "cluster" — render_cluster_plan
mirrors render_plan but uses the cluster pipeline. Six new fixtures
under cases/cluster/roles/ cover the v0.3.0 surface end-to-end:
create simple/user, alter attributes, add membership, drop with
intent gate, and comment.

Stage 11 of docs/superpowers/plans/2026-05-21-cluster-roles.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 12 — Property tests + docs + v0.3.0 release

### Task 12.1: Property test additions

- [ ] **Step 1: Add `arbitrary_role` and `arbitrary_cluster_catalog` to testkit**

In `crates/pgevolve-testkit/src/ir_generator.rs`, add `arbitrary_role()` (generates a `Role` with random attributes + 0-3 member_of entries) and `arbitrary_cluster_catalog()` (generates a `ClusterCatalog` with 0-5 roles, ensures no membership cycles by topological generation).

- [ ] **Step 2: Add diff-round-trip property test**

```rust
proptest! {
    #[test]
    fn cluster_diff_then_apply_in_memory_yields_target(
        a in arbitrary_cluster_catalog(),
        b in arbitrary_cluster_catalog(),
    ) {
        let changes = diff_cluster(&a, &b);
        let mut applied = a.clone();
        apply_changes_in_memory(&mut applied, &changes);
        applied.canonicalize();
        let mut expected = b.clone();
        expected.canonicalize();
        prop_assert_eq!(applied, expected);
    }
}
```

`apply_changes_in_memory` is a test helper that mutates a `ClusterCatalog` per the change list (separate from the SQL-level apply).

- [ ] **Step 3: Run 10× per constitution §9**

```bash
for i in 1 2 3 4 5 6 7 8 9 10; do
    PROPTEST_CASES=512 cargo test -p pgevolve-testkit --release 2>&1 | tail -3
done
```

All 10 green.

- [ ] **Step 4: Commit**

```bash
git add -p crates/pgevolve-testkit/src/
git commit -m "$(cat <<'EOF'
test(proptest): cluster — arbitrary_role + diff round-trip invariant

Generators for Role + ClusterCatalog with cycle-free membership.
Round-trip property: diff(A, B) applied to A yields B (modulo canon).
10× per constitution §9; all green.

Stage 12.1 of docs/superpowers/plans/2026-05-21-cluster-roles.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 12.2: Docs

- [ ] **Step 1: Update `docs/spec/objects.md`**

Find line 248 (role row). Replace:
```markdown
| `ROLE` (`CREATE ROLE / USER`) | 📋 Planned, v0.3 | Membership and inheritance modeled. `LOGIN` attribute kept. Passwords are *not* stored in source — set out-of-band. |
```
with:
```markdown
| `ROLE` (`CREATE ROLE / USER`) | ✅ Supported | Cluster-level surface (`pgevolve cluster …`). Full attribute matrix + role membership. Passwords intentionally not modeled — set out-of-band. change_kinds: [create, alter, drop, grant, revoke, comment] |
```

- [ ] **Step 2: New file `docs/spec/cluster.md`**

```markdown
# Cluster-level surface

pgevolve manages cluster-level state — roles (v0.3), with tablespaces,
cluster settings, foreign servers, and user mappings planned — through
a parallel project type and command family separate from per-database
projects.

## Project shape

```
my-cluster/
  pgevolve-cluster.toml
  roles/
    app.sql
    ops.sql
```

## Commands

- `pgevolve cluster init [path]` — scaffold a new cluster project
- `pgevolve cluster diff` — show diff between source and live cluster
- `pgevolve cluster plan` — write a cluster plan directory
- `pgevolve cluster apply [<plan_id>]` — apply a cluster plan
- `pgevolve cluster status` — list applied/pending plans

## Currently managed

| Object | Status |
|---|---|
| Roles (CREATE/ALTER/DROP ROLE, CREATE USER) | ✅ v0.3.0 |
| Role membership (GRANT role TO target) | ✅ v0.3.0 |
| Tablespaces | 🔮 Future |
| Cluster GUCs (postgresql.conf) | 🔮 Future |
| Foreign servers / user mappings | 🔮 Future |
| Databases list | 🔮 Future |

## Passwords

Passwords are **not stored in source**. The catalog reader skips
`rolpassword`; the source parser drops `PASSWORD '...'` clauses
silently. Set passwords out-of-band (`psql`, secret manager, etc.).

## Bootstrap roles

The `[bootstrap].roles` list in `pgevolve-cluster.toml` names roles
that pgevolve treats as PG-owned and never diffs in or out. Defaults
to `["postgres"]`. Cloud Postgres (RDS, Cloud SQL, etc.) typically
needs additional entries (e.g., `["postgres", "cloudsqlsuperuser"]`).
```

- [ ] **Step 3: Update `CHANGELOG.md`**

Add a new `[0.3.0]` section at the top (above `[0.2.1]`):

```markdown
## [Unreleased]

## [0.3.0] — 2026-05-21

### Added

- **Cluster-level surface** — new project type (`pgevolve-cluster.toml + roles/`), new command family (`pgevolve cluster init/diff/plan/apply/status`), new executor running against a superuser DSN. Per Decision 23 of the v0.2 architecture review.
- **`ROLE` / `CREATE USER` fully managed** — `ClusterCatalog.roles` with full PG attribute matrix (superuser, createdb, createrole, inherit, login, replication, bypass_rls, connection_limit, valid_until), plus role membership via inline `IN ROLE` or `GRANT role TO target`. Passwords intentionally not modeled.
- **Two new universal lint rules**:
  - `role-loses-superuser` (warning) — fires on `ALTER ROLE … NOSUPERUSER` when the role had superuser.
  - `role-membership-cycle` (error) — detects cycles in the projected post-apply membership graph; pre-empts PG's apply-time rejection.
- **Conformance harness** — new `authoring = "cluster"` mode + six fixtures under `cases/cluster/roles/`.

### Catalog reader

- New `read_cluster_catalog(pg, bootstrap_roles)` querying `pg_authid` + `pg_auth_members`. Filters `pg_*` predefined roles and caller-supplied bootstrap roles.

## [0.2.1] — 2026-05-21
```

- [ ] **Step 4: Bump version**

```bash
# crates/pgevolve-core/Cargo.toml — version → 0.3.0
# crates/pgevolve-core-macros/Cargo.toml — version → 0.3.0 (and version of pgevolve-core dep)
# crates/pgevolve/Cargo.toml — version → 0.3.0
# crates/pgevolve-conformance/Cargo.toml — version → 0.3.0
# crates/pgevolve-testkit/Cargo.toml — version → 0.3.0
# Root Cargo.toml [workspace.package] version → 0.3.0
cargo build --workspace  # refreshes Cargo.lock
```

### Task 12.3: Full verify + release commit

- [ ] **Step 1: Verify per constitution §9**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo doc --workspace --no-deps 2>&1 | grep -cE "^warning"  # expect 0
cargo deny check
```

All green.

- [ ] **Step 2: Re-bless conformance** (plan_id hash depends on version)

```bash
cargo xtask bless --conformance
cargo test -p pgevolve-conformance
```

- [ ] **Step 3: Commit**

```bash
git add docs/spec/objects.md docs/spec/cluster.md CHANGELOG.md Cargo.toml Cargo.lock crates/*/Cargo.toml crates/pgevolve-conformance/tests/cases/
git commit -m "$(cat <<'EOF'
release: v0.3.0 — cluster surface + ROLE/USER

First v0.3 sub-spec. Introduces the cluster-level project type
(pgevolve cluster ...) and the first managed cluster object: roles
with full attribute matrix + membership. Per Decision 23 in the
v0.2 architecture review.

GRANT (object-level) and RLS POLICY ship in v0.3.1 and v0.3.2.

Closes issue #2.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 4: Stop here** — tag + push require user's signing key. Report DONE.

---

## Done.

After Stage 12, v0.3.0 is committed and ready for tag + push:
- Cluster IR (ClusterCatalog, Role, RoleAttributes) + canon
- Parser (CREATE/ALTER ROLE, CREATE USER, GRANT membership, COMMENT)
- Catalog reader (pg_authid + pg_auth_members with bootstrap filter)
- Differ (ClusterChangeSet, 6 change kinds)
- Render + emit (SQL helpers + 6 new StepKind)
- Lint rules (role-loses-superuser, role-membership-cycle)
- Cluster config (pgevolve-cluster.toml)
- API + executor (build_cluster_plan, apply_cluster_plan_dir)
- CLI (`pgevolve cluster {init,diff,plan,apply,status}`)
- Conformance harness extension + 6 fixtures
- Property test coverage
- Updated docs + CHANGELOG + v0.3.0 release commit

Next plan target: **v0.3.1 — GRANT/REVOKE on objects + ALTER DEFAULT PRIVILEGES** (issue #3), extending the per-DB Catalog with grant lists.
