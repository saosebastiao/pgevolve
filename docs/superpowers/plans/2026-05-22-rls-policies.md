# Row-Level Security Policies Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship v0.3.2 — declarative Postgres Row-Level Security (RLS): per-table `rls_enabled` + `rls_forced` flags, embedded `policies: Vec<Policy>`, and full parser/catalog/diff/render/lint coverage. Closes the v0.3 security/permissions trilogy.

**Architecture:** Nine sequential stages. All policy IR embeds on `Table` (no orphan policies possible). Expression canonicalization reuses `NormalizedExpr` from check constraints. The cross-cluster lint from v0.3.1 extends to cover policy `TO` clauses rather than gaining a sibling rule. The differ uses straight pair-by-name on `(table_qname, policy_name)` — command-kind changes recreate (DROP + CREATE) because PG doesn't allow `ALTER POLICY` to change the command.

**Tech Stack:** Rust 1.95+, `pg_query` 6.x, `tokio_postgres`, `serde`, `blake3`, `proptest`. Builds on v0.3.0 (cluster roles) + v0.3.1 (`GrantTarget`, cross-cluster lint).

**Source spec:** `docs/superpowers/specs/2026-05-22-rls-policies-design.md`.

---

## Pre-flight

- [ ] **Step 1: Confirm clean baseline**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --tests
```

All green. v0.3.1 is committed; main is clean.

- [ ] **Step 2: Skim spec sections relevant to each stage**

Open `docs/superpowers/specs/2026-05-22-rls-policies-design.md` once. Each stage below cites the spec section it implements.

---

## File structure

```
crates/pgevolve-core/src/
├── ir/
│   ├── policy.rs                NEW — Stage 1 — Policy, PolicyCommand
│   ├── table.rs                 MODIFY — Stage 1 — add rls_enabled, rls_forced, policies
│   └── canon/
│       └── policies.rs          NEW — Stage 2 — sort policies + role lists
├── catalog/
│   ├── queries/
│   │   ├── policies.rs          NEW — Stage 3 — pg_policies query
│   │   └── tables.rs            MODIFY — Stage 3 — relrowsecurity + relforcerowsecurity
│   └── assemble/
│       ├── policies.rs          NEW — Stage 3 — decode policy rows, attach to tables
│       └── tables.rs            MODIFY — Stage 3 — populate rls_enabled + rls_forced
├── parse/
│   └── builder/
│       ├── policy_stmt.rs       NEW — Stage 4 — CREATE POLICY
│       └── alter_table_stmt.rs  MODIFY — Stage 4 — 4 RLS subcommands
├── diff/
│   ├── policies.rs              NEW — Stage 5 — diff_policies + 5 Change variants
│   ├── change.rs                MODIFY — Stage 5 — new variants
│   └── tables.rs                MODIFY — Stage 5 — call diff_policies
├── plan/
│   ├── raw_step.rs              MODIFY — Stage 6 — 5 new StepKind variants
│   └── rewrite/
│       └── policies.rs          NEW — Stage 6 — SQL helpers + emit
└── lint/
    └── rules/
        ├── grant_references_unknown_role.rs  MODIFY — Stage 7 — extend to policy TO clauses
        └── force_rls_without_policies.rs     NEW — Stage 7

crates/pgevolve-conformance/tests/cases/objects/
└── policies/                    NEW — Stage 8 — 11 fixtures
```

---

## Stage 1 — Policy IR + Table extensions

Pure data types + Table struct extensions. Canon + behavior land in subsequent stages.

**Files created:** `crates/pgevolve-core/src/ir/policy.rs`.
**Files modified:** `crates/pgevolve-core/src/ir/mod.rs`, `crates/pgevolve-core/src/ir/table.rs`.

### Task 1.1: Create the `ir::policy` module

- [ ] **Step 1: Write `crates/pgevolve-core/src/ir/policy.rs`**

```rust
//! Row-level security policies — `Policy`, `PolicyCommand`.
//!
//! Policies embed on [`crate::ir::table::Table`] — there's no orphan
//! shape possible in PG. USING / WITH CHECK expressions reuse the
//! [`NormalizedExpr`] canonicalization shared with check constraints.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::grant::GrantTarget;

/// A row-level security policy attached to a table.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Policy {
    /// Policy name. Unique per table.
    pub name: Identifier,
    /// `AS PERMISSIVE` (true; PG default) vs `AS RESTRICTIVE` (false).
    pub permissive: bool,
    /// Which command(s) this policy applies to. `All` covers all DML.
    pub command: PolicyCommand,
    /// `TO roles` list. Source omission canonicalizes to
    /// `vec![GrantTarget::Public]` at parse time so source and catalog
    /// round-trip equally.
    pub roles: Vec<GrantTarget>,
    /// `USING (expr)` — row-visibility filter. PG default: absent.
    pub using: Option<NormalizedExpr>,
    /// `WITH CHECK (expr)` — write-time filter. Valid only on commands
    /// that write rows (Insert/Update/All); parser rejects on Select/Delete.
    pub with_check: Option<NormalizedExpr>,
}

/// The command kind a policy applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyCommand {
    /// `FOR ALL` — covers SELECT, INSERT, UPDATE, DELETE.
    All,
    /// `FOR SELECT`.
    Select,
    /// `FOR INSERT`.
    Insert,
    /// `FOR UPDATE`.
    Update,
    /// `FOR DELETE`.
    Delete,
}

impl PolicyCommand {
    /// SQL keyword used in CREATE POLICY rendering.
    #[must_use]
    pub const fn sql_keyword(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Select => "SELECT",
            Self::Insert => "INSERT",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
        }
    }

    /// `pg_policies.cmd` text value. PG emits one of these strings.
    #[must_use]
    pub fn from_pg_text(s: &str) -> Option<Self> {
        match s {
            "ALL" => Some(Self::All),
            "SELECT" => Some(Self::Select),
            "INSERT" => Some(Self::Insert),
            "UPDATE" => Some(Self::Update),
            "DELETE" => Some(Self::Delete),
            _ => None,
        }
    }

    /// Whether `WITH CHECK` is valid for this command. PG rejects WITH CHECK
    /// on FOR SELECT and FOR DELETE policies.
    #[must_use]
    pub const fn allows_with_check(self) -> bool {
        matches!(self, Self::All | Self::Insert | Self::Update)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn pg_text_roundtrips() {
        for cmd in [
            PolicyCommand::All,
            PolicyCommand::Select,
            PolicyCommand::Insert,
            PolicyCommand::Update,
            PolicyCommand::Delete,
        ] {
            assert_eq!(PolicyCommand::from_pg_text(cmd.sql_keyword()), Some(cmd));
        }
    }

    #[test]
    fn from_pg_text_rejects_unknown() {
        assert_eq!(PolicyCommand::from_pg_text("BOGUS"), None);
    }

    #[test]
    fn select_and_delete_reject_with_check() {
        assert!(!PolicyCommand::Select.allows_with_check());
        assert!(!PolicyCommand::Delete.allows_with_check());
        assert!(PolicyCommand::All.allows_with_check());
        assert!(PolicyCommand::Insert.allows_with_check());
        assert!(PolicyCommand::Update.allows_with_check());
    }

    #[test]
    fn policy_sort_by_name() {
        let a = Policy {
            name: id("alpha"),
            permissive: true,
            command: PolicyCommand::All,
            roles: vec![GrantTarget::Public],
            using: None,
            with_check: None,
        };
        let b = Policy {
            name: id("beta"),
            permissive: true,
            command: PolicyCommand::All,
            roles: vec![GrantTarget::Public],
            using: None,
            with_check: None,
        };
        let mut policies = vec![b.clone(), a.clone()];
        policies.sort();
        assert_eq!(policies, vec![a, b]);
    }
}
```

- [ ] **Step 2: Wire into `crates/pgevolve-core/src/ir/mod.rs`**

Add `pub mod policy;` in alphabetical position (between `partition` and `procedure`).

### Task 1.2: Extend `Table` IR

- [ ] **Step 1: Add fields to `Table` struct in `crates/pgevolve-core/src/ir/table.rs`**

After the existing fields (last one is `grants`), add:

```rust
    /// `ROW LEVEL SECURITY` enabled flag. PG default: false.
    pub rls_enabled: bool,
    /// `FORCE ROW LEVEL SECURITY` flag (applies even to owner). PG default: false.
    pub rls_forced: bool,
    /// Policies attached to this table. Canonicalized in `ir::canon::policies`.
    pub policies: Vec<crate::ir::policy::Policy>,
```

- [ ] **Step 2: Extend the hand-rolled `Diff for Table` impl**

`Table` has a hand-rolled `impl Diff`. After the existing `diff_field` calls (after `grants`), add three more:

```rust
        out.extend(diff_field(
            "rls_enabled",
            &format!("{:?}", self.rls_enabled),
            &format!("{:?}", other.rls_enabled),
        ));
        out.extend(diff_field(
            "rls_forced",
            &format!("{:?}", self.rls_forced),
            &format!("{:?}", other.rls_forced),
        ));
        out.extend(diff_field(
            "policies",
            &format!("{:?}", self.policies),
            &format!("{:?}", other.policies),
        ));
```

(Coarse `via_debug`-style — Stage 5's differ does element-level work on `policies`.)

- [ ] **Step 3: Backfill `Table { ... }` literals workspace-wide**

```bash
cargo check --workspace --all-targets 2>&1 | grep -E "missing field" | head -20
```

For each missing-field error, add `rls_enabled: false, rls_forced: false, policies: vec![]` to the literal. Likely sites: `diff/tables.rs` test helpers, `catalog/assemble/tables.rs`, `parse/builder/create_stmt.rs`, `crates/pgevolve-testkit/src/ir_generator.rs`, `crates/pgevolve-core/src/render/table.rs` test fixtures, and any conformance test fixtures that inline-build `Table`.

Iterate `cargo check` → fix → re-check until clean. v0.3.1 Stage 2 backfilled ~230 sites for owner+grants; this backfill is much smaller (Table is one struct out of 8) but expect ~25 sites.

- [ ] **Step 4: Add Diff tests for the new fields**

In `crates/pgevolve-core/src/ir/table.rs::tests`, add:

```rust
    #[test]
    fn rls_enabled_change_diffs() {
        let mut b = base();
        b.rls_enabled = true;
        assert!(base().diff(&b).iter().any(|x| x.path == "rls_enabled"));
    }

    #[test]
    fn rls_forced_change_diffs() {
        let mut b = base();
        b.rls_forced = true;
        assert!(base().diff(&b).iter().any(|x| x.path == "rls_forced"));
    }

    #[test]
    fn policies_change_diffs() {
        use crate::ir::grant::GrantTarget;
        use crate::ir::policy::{Policy, PolicyCommand};
        let mut b = base();
        b.policies.push(Policy {
            name: id("p1"),
            permissive: true,
            command: PolicyCommand::All,
            roles: vec![GrantTarget::Public],
            using: None,
            with_check: None,
        });
        assert!(base().diff(&b).iter().any(|x| x.path == "policies"));
    }
```

Adapt `base()` and `id()` to whatever the existing test module uses.

- [ ] **Step 5: Run + commit**

```bash
cargo test -p pgevolve-core --lib ir::policy
cargo test -p pgevolve-core --lib ir::table
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add -p crates/
git commit -m "$(cat <<'EOF'
feat(ir): policy IR + RLS table flags

New ir::policy module with Policy + PolicyCommand. Table gains three
fields: rls_enabled, rls_forced, policies. Backfilled all Table
struct literals across the workspace.

USING / WITH CHECK expressions use NormalizedExpr — same canonicalization
path as check constraints. PolicyCommand::allows_with_check() encodes
PG's rule that WITH CHECK is invalid on FOR SELECT and FOR DELETE.

Stage 1 of docs/superpowers/plans/2026-05-22-rls-policies.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 2 — Canon

Sort policies on each table; sort each policy's roles. Reuse the v0.3.1 `Public < Role(name)` convention.

**Files created:** `crates/pgevolve-core/src/ir/canon/policies.rs`.
**Files modified:** `crates/pgevolve-core/src/ir/canon/mod.rs`.

### Task 2.1: Create the canon module

- [ ] **Step 1: Write `crates/pgevolve-core/src/ir/canon/policies.rs`**

```rust
//! Canon rules for table policies.

use crate::ir::policy::Policy;
use crate::ir::table::Table;

/// Sort each policy's roles + sort policies by name on a single table.
pub fn run_on_table(t: &mut Table) {
    for p in &mut t.policies {
        normalize_roles(p);
    }
    t.policies.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
}

/// Sort `roles` lexicographically (with `Public` first per v0.3.1's
/// `GrantTarget::Ord` impl).
fn normalize_roles(p: &mut Policy) {
    p.roles.sort();
    p.roles.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::grant::GrantTarget;
    use crate::ir::policy::{Policy, PolicyCommand};
    use crate::ir::table::Table;
    use crate::identifier::QualifiedName;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn policy(name: &str, roles: Vec<GrantTarget>) -> Policy {
        Policy {
            name: id(name),
            permissive: true,
            command: PolicyCommand::All,
            roles,
            using: None,
            with_check: None,
        }
    }

    fn empty_table(qname: QualifiedName) -> Table {
        Table {
            qname,
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
        }
    }

    #[test]
    fn sorts_policies_by_name() {
        let mut t = empty_table(qn("app", "users"));
        t.policies = vec![
            policy("zebra", vec![]),
            policy("alpha", vec![]),
            policy("middle", vec![]),
        ];
        run_on_table(&mut t);
        let names: Vec<_> = t.policies.iter().map(|p| p.name.as_str().to_owned()).collect();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }

    #[test]
    fn sorts_roles_with_public_first() {
        let mut t = empty_table(qn("app", "users"));
        t.policies = vec![policy("p", vec![
            GrantTarget::Role(id("zelda")),
            GrantTarget::Public,
            GrantTarget::Role(id("alice")),
        ])];
        run_on_table(&mut t);
        // Public sorts first per GrantTarget::Ord.
        assert!(matches!(t.policies[0].roles[0], GrantTarget::Public));
        assert_eq!(t.policies[0].roles.len(), 3);
    }

    #[test]
    fn dedupes_duplicate_roles() {
        let mut t = empty_table(qn("app", "users"));
        t.policies = vec![policy("p", vec![
            GrantTarget::Role(id("alice")),
            GrantTarget::Role(id("alice")),
            GrantTarget::Public,
            GrantTarget::Public,
        ])];
        run_on_table(&mut t);
        assert_eq!(t.policies[0].roles.len(), 2);
    }

    #[test]
    fn run_is_idempotent() {
        let mut t = empty_table(qn("app", "users"));
        t.policies = vec![
            policy("b", vec![GrantTarget::Role(id("alice"))]),
            policy("a", vec![GrantTarget::Public]),
        ];
        run_on_table(&mut t);
        let snap1 = format!("{t:?}");
        run_on_table(&mut t);
        let snap2 = format!("{t:?}");
        assert_eq!(snap1, snap2);
    }
}
```

### Task 2.2: Wire into the canon orchestrator

- [ ] **Step 1: Add to `crates/pgevolve-core/src/ir/canon/mod.rs`**

```rust
pub mod policies;
```

(Alphabetical position.)

In the existing `canonicalize(cat: &mut Catalog)` orchestrator function, after the existing per-family canon passes (where v0.3.1 added `grants::run_on_list` calls), add:

```rust
    for t in &mut cat.tables {
        policies::run_on_table(t);
    }
```

### Task 2.3: Run + commit

```bash
cargo test -p pgevolve-core --lib ir::canon::policies
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add -p crates/pgevolve-core/src/ir/canon/
git commit -m "$(cat <<'EOF'
feat(canon): policies — sort + dedupe per-table

Two normalizations:
  - Sort each Table.policies by name.
  - Sort each policy's roles (Public first per GrantTarget::Ord) and
    dedupe.

Catalog::canonicalize calls run_on_table for every table after the
existing per-family canon passes.

Stage 2 of docs/superpowers/plans/2026-05-22-rls-policies.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 3 — Catalog reader

Two additions: select `relrowsecurity` + `relforcerowsecurity` on the existing tables query; new `pg_policies` query + assembler.

**Files created:** `crates/pgevolve-core/src/catalog/queries/policies.rs`, `crates/pgevolve-core/src/catalog/assemble/policies.rs`.
**Files modified:** `crates/pgevolve-core/src/catalog/queries/{tables,mod}.rs`, `crates/pgevolve-core/src/catalog/assemble/{tables,mod}.rs`, `crates/pgevolve-core/src/catalog/mod.rs`.

### Task 3.1: Add `relrowsecurity` + `relforcerowsecurity` to tables query

- [ ] **Step 1: Extend the tables query**

Find the existing tables query (the one that selects `relkind`, `relowner`, etc.). Add two columns:

```sql
       c.relrowsecurity::bool        AS rls_enabled,
       c.relforcerowsecurity::bool   AS rls_forced,
```

- [ ] **Step 2: Wire into the assembler**

In `crates/pgevolve-core/src/catalog/assemble/tables.rs::build_tables` (the row-decoding loop for each table), populate the new fields:

```rust
let rls_enabled = row.get_bool(q, "rls_enabled")?;
let rls_forced = row.get_bool(q, "rls_forced")?;

// in the Table { ... } literal:
//    rls_enabled,
//    rls_forced,
//    policies: vec![],          // populated by Stage 3.3 assembler post-build
```

### Task 3.2: `pg_policies` query

- [ ] **Step 1: Create `crates/pgevolve-core/src/catalog/queries/policies.rs`**

```rust
//! `pg_policies` query — row-level security policies.

pub const POLICIES_QUERY: &str = r"
SELECT p.schemaname,
       p.tablename,
       p.policyname,
       (p.permissive = 'PERMISSIVE')::bool AS permissive,
       p.cmd,
       coalesce(p.roles::text[], '{}'::text[]) AS roles,
       p.qual::text       AS using_text,
       p.with_check::text AS with_check_text
FROM pg_policies p
WHERE p.schemaname = ANY($1::text[])
ORDER BY p.schemaname, p.tablename, p.policyname
";
```

- [ ] **Step 2: Add a `Policies` variant to `CatalogQuery`**

In `crates/pgevolve-core/src/catalog/queries/mod.rs`:

```rust
pub mod policies;
```

In the `CatalogQuery` enum (lives in `catalog/mod.rs` or `queries/mod.rs` — read to confirm), add:

```rust
Policies,
```

Wire it into `query_for`:

```rust
CatalogQuery::Policies => policies::POLICIES_QUERY,
```

And into `takes_text_array_param`:

```rust
// Policies takes the managed-schemas list as $1::text[].
matches!(self, /* existing schema-param variants */ | Self::Policies)
```

(Verify against the v0.3.1 Stage 4 rename `needs_schema_param → takes_text_array_param` — that's the current method name.)

### Task 3.3: Policies assembler

- [ ] **Step 1: Create `crates/pgevolve-core/src/catalog/assemble/policies.rs`**

```rust
//! Assemble policy rows into Policy structs and attach to their tables.

use crate::catalog::error::CatalogError;
use crate::catalog::queries::CatalogQuery;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::grant::GrantTarget;
use crate::ir::policy::{Policy, PolicyCommand};

const Q: CatalogQuery = CatalogQuery::Policies;

/// Decode policy rows and push each onto the matching Table's policies list.
/// Rows whose table is not in the managed-schemas list (and thus not present
/// in `tables`) are silently dropped.
pub(crate) fn attach_policies(
    rows: &[/* row type — see Stage 5 of v0.3.1 for the actual API */],
    tables: &mut [crate::ir::table::Table],
) -> Result<(), CatalogError> {
    for row in rows {
        let schema_str = row.get_text(Q, "schemaname")?;
        let table_str  = row.get_text(Q, "tablename")?;
        let qname = QualifiedName::new(
            Identifier::from_unquoted(&schema_str).map_err(|e| CatalogError::BadColumnType {
                query: Q, column: "schemaname",
                message: format!("invalid schema {schema_str:?}: {e}"),
            })?,
            Identifier::from_unquoted(&table_str).map_err(|e| CatalogError::BadColumnType {
                query: Q, column: "tablename",
                message: format!("invalid table {table_str:?}: {e}"),
            })?,
        );

        let Some(table) = tables.iter_mut().find(|t| t.qname == qname) else {
            continue; // table not managed; skip the policy silently.
        };

        let policy = decode_policy(row)?;
        table.policies.push(policy);
    }
    Ok(())
}

fn decode_policy(row: &/* row type */) -> Result<Policy, CatalogError> {
    let name_str = row.get_text(Q, "policyname")?;
    let name = Identifier::from_unquoted(&name_str).map_err(|e| CatalogError::BadColumnType {
        query: Q, column: "policyname",
        message: format!("invalid policy name {name_str:?}: {e}"),
    })?;

    let permissive = row.get_bool(Q, "permissive")?;

    let cmd_str = row.get_text(Q, "cmd")?;
    let command = PolicyCommand::from_pg_text(&cmd_str).ok_or_else(|| CatalogError::BadColumnType {
        query: Q, column: "cmd",
        message: format!("unknown PolicyCommand {cmd_str:?}"),
    })?;

    let role_strs: Vec<String> = row.get_text_array(Q, "roles")?;
    let mut roles = Vec::with_capacity(role_strs.len());
    for role_str in role_strs {
        if role_str == "public" || role_str == "PUBLIC" {
            roles.push(GrantTarget::Public);
        } else {
            let ident = Identifier::from_unquoted(&role_str).map_err(|e| CatalogError::BadColumnType {
                query: Q, column: "roles",
                message: format!("invalid role {role_str:?}: {e}"),
            })?;
            roles.push(GrantTarget::Role(ident));
        }
    }

    let using = row.get_opt_text(Q, "using_text")?.map(NormalizedExpr::from_canonical_text);
    let with_check = row.get_opt_text(Q, "with_check_text")?.map(NormalizedExpr::from_canonical_text);

    Ok(Policy {
        name,
        permissive,
        command,
        roles,
        using,
        with_check,
    })
}
```

Adapt the row API to whatever Stage 5 of v0.3.1 confirmed (`get_text(q, "col")`, `get_bool(q, "col")`, `get_opt_text(q, "col")`, `get_text_array(q, "col")`).

- [ ] **Step 2: Wire into `read_catalog`**

In `crates/pgevolve-core/src/catalog/mod.rs::read_catalog`, after the tables build and after the default_privileges fetch from v0.3.1 Stage 6, add:

```rust
let policy_rows = querier.fetch(CatalogQuery::Policies, &managed_schemas).await?;
crate::catalog::assemble::policies::attach_policies(&policy_rows, &mut catalog.tables)?;
```

The `policy_rows` storage may need to land on `RawRows` (the struct Stage 6 of v0.3.1 added `default_privileges` to). Mirror that pattern: add a `policies: Vec<Row>` field, fetch it alongside, and pass into `attach_policies` after the per-family assembles complete.

Wire `pub mod policies;` into `catalog/queries/mod.rs` and `catalog/assemble/mod.rs`.

### Task 3.4: Docker-gated integration tests

- [ ] **Step 1: Add to `crates/pgevolve-core/tests/catalog_policies.rs`** (new file)

```rust
//! Docker-gated read tests for RLS policies + RLS table flags.

use pgevolve_core::ir::policy::PolicyCommand;
use pgevolve_core::ir::grant::GrantTarget;
// Adapt imports to existing test infrastructure.

#[tokio::test]
async fn reads_rls_enabled_flag() {
    if !docker_available() { return; }
    let pg = ephemeral_pg().await;
    pg.exec("CREATE SCHEMA app").await;
    pg.exec("CREATE TABLE app.docs (id bigint, author text)").await;
    pg.exec("ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY").await;
    let cat = read_catalog(pg.querier(), &["app".to_string()]).await.unwrap();
    let t = cat.tables.iter().find(|t| t.qname.name.as_str() == "docs").unwrap();
    assert!(t.rls_enabled);
    assert!(!t.rls_forced);
}

#[tokio::test]
async fn reads_rls_forced_flag() {
    if !docker_available() { return; }
    let pg = ephemeral_pg().await;
    pg.exec("CREATE SCHEMA app").await;
    pg.exec("CREATE TABLE app.docs (id bigint)").await;
    pg.exec("ALTER TABLE app.docs FORCE ROW LEVEL SECURITY").await;
    let cat = read_catalog(pg.querier(), &["app".to_string()]).await.unwrap();
    let t = cat.tables.iter().find(|t| t.qname.name.as_str() == "docs").unwrap();
    assert!(t.rls_forced);
}

#[tokio::test]
async fn reads_simple_policy() {
    if !docker_available() { return; }
    let pg = ephemeral_pg().await;
    pg.exec("CREATE SCHEMA app").await;
    pg.exec("CREATE TABLE app.docs (id bigint, author text)").await;
    pg.exec("CREATE POLICY author_only ON app.docs USING (author = current_user)").await;
    let cat = read_catalog(pg.querier(), &["app".to_string()]).await.unwrap();
    let t = cat.tables.iter().find(|t| t.qname.name.as_str() == "docs").unwrap();
    assert_eq!(t.policies.len(), 1);
    let p = &t.policies[0];
    assert_eq!(p.name.as_str(), "author_only");
    assert!(p.permissive);
    assert_eq!(p.command, PolicyCommand::All);
    // Default TO clause is PUBLIC.
    assert_eq!(p.roles.len(), 1);
    assert!(matches!(p.roles[0], GrantTarget::Public));
    assert!(p.using.is_some());
    assert!(p.with_check.is_none());
}

#[tokio::test]
async fn reads_restrictive_policy_with_roles_and_with_check() {
    if !docker_available() { return; }
    let pg = ephemeral_pg().await;
    pg.exec("CREATE ROLE readers").await;
    pg.exec("CREATE ROLE writers").await;
    pg.exec("CREATE SCHEMA app").await;
    pg.exec("CREATE TABLE app.docs (id bigint, author text)").await;
    pg.exec("CREATE POLICY only_authors AS RESTRICTIVE FOR INSERT TO writers, readers WITH CHECK (author = current_user)").await;
    let cat = read_catalog(pg.querier(), &["app".to_string()]).await.unwrap();
    let t = cat.tables.iter().find(|t| t.qname.name.as_str() == "docs").unwrap();
    let p = t.policies.iter().find(|p| p.name.as_str() == "only_authors").unwrap();
    assert!(!p.permissive);
    assert_eq!(p.command, PolicyCommand::Insert);
    // Canon sorts roles — readers, writers (lexicographic).
    let role_names: Vec<&str> = p.roles.iter().filter_map(|r| match r {
        GrantTarget::Role(n) => Some(n.as_str()),
        GrantTarget::Public => None,
    }).collect();
    assert_eq!(role_names, vec!["readers", "writers"]);
    assert!(p.using.is_none());
    assert!(p.with_check.is_some());
}
```

Mirror the v0.3.1 Stage 5 Docker test pattern for the helper functions (`docker_available()`, `ephemeral_pg().await`, etc.).

### Task 3.5: Run + commit

```bash
cargo test -p pgevolve-core --lib catalog
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
# Docker tests if available:
cargo test -p pgevolve-core --tests catalog_policies
git add -p crates/pgevolve-core/src/catalog/ crates/pgevolve-core/tests/catalog_policies.rs
git commit -m "$(cat <<'EOF'
feat(catalog): read RLS flags + pg_policies

Two additions:
  - Tables query gains relrowsecurity + relforcerowsecurity columns;
    Table.rls_enabled + Table.rls_forced populated from them.
  - New CatalogQuery::Policies + assemble::policies::attach_policies.
    Decodes pg_policies into Policy structs and attaches to tables.
    Policies on unmanaged tables silently dropped.

Default TO clause (omitted in source) round-trips as
[GrantTarget::Public] since pg_policies emits the literal 'public'
string.

Stage 3 of docs/superpowers/plans/2026-05-22-rls-policies.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 4 — Source parser

Two new parse paths:
1. `CREATE POLICY` (pg_query `NodeEnum::CreatePolicyStmt`).
2. Four `ALTER TABLE ... { ENABLE | DISABLE | FORCE | NO FORCE } ROW LEVEL SECURITY` subcommands.

Reject `ALTER POLICY` and `DROP POLICY` in source.

**Files created:** `crates/pgevolve-core/src/parse/builder/policy_stmt.rs`.
**Files modified:** `crates/pgevolve-core/src/parse/builder/alter_table_stmt.rs`, `crates/pgevolve-core/src/parse/builder/mod.rs`, `crates/pgevolve-core/src/parse/mod.rs`, `crates/pgevolve-core/src/parse/statement.rs`.

### Task 4.1: `CreatePolicyStmt` parser

- [ ] **Step 1: Recon pg_query field names**

```bash
grep -rn "CreatePolicyStmt" ~/.cargo/registry/src/index.crates.io-*/pg_query-*/src/protobuf.rs 2>/dev/null | head -10
```

Expected fields (verify):
- `policy_name: String`
- `table: Option<RangeVar>`
- `cmd_name: String` — "all"/"select"/...
- `permissive: bool`
- `roles: Vec<Node>` — RoleSpec list
- `qual: Option<Node>` — USING expression
- `with_check: Option<Node>` — WITH CHECK expression

(pg_query may serialize the USING expression's canonical text via a helper; alternatively, fall back to a normalize-via-deparse approach. v0.3.1 Stage 7 dealt with similar expression decoding; mirror that.)

- [ ] **Step 2: Create `crates/pgevolve-core/src/parse/builder/policy_stmt.rs`**

```rust
//! `CREATE POLICY name ON tablename ...` — RLS policy declarations.

use pg_query::protobuf::CreatePolicyStmt;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::grant::GrantTarget;
use crate::ir::policy::{Policy, PolicyCommand};
use crate::parse::error::{ParseError, SourceLocation};

pub(crate) fn apply(
    s: &CreatePolicyStmt,
    cat: &mut Catalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    // Policy name.
    let name = Identifier::from_unquoted(&s.policy_name).map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("invalid policy name {:?}: {e}", s.policy_name),
    })?;

    // Target table — RangeVar { schemaname, relname }.
    let rv = s.table.as_ref().ok_or_else(|| ParseError::Structural {
        location: loc.clone(),
        message: "CREATE POLICY missing target table".into(),
    })?;
    let table_qname = qname_from_rangevar(rv, loc)?;

    // Command kind.
    let command = decode_cmd(&s.cmd_name, loc)?;

    // Permissive vs Restrictive.
    let permissive = s.permissive;

    // TO roles — empty list canonicalizes to [Public].
    let mut roles = decode_role_targets(&s.roles, loc)?;
    if roles.is_empty() {
        roles.push(GrantTarget::Public);
    }

    // USING expression.
    let using = s.qual.as_ref().map(|expr| decode_expr_node(expr, loc)).transpose()?;

    // WITH CHECK expression.
    let with_check = s.with_check.as_ref().map(|expr| decode_expr_node(expr, loc)).transpose()?;

    // Validation: WITH CHECK invalid on FOR SELECT / FOR DELETE.
    if with_check.is_some() && !command.allows_with_check() {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "WITH CHECK is invalid on FOR {} policies; PG rejects",
                command.sql_keyword()
            ),
        });
    }

    // Attach to the named table.
    let table = cat.tables.iter_mut().find(|t| t.qname == table_qname).ok_or_else(|| {
        ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "CREATE POLICY {name} ON {table_qname} — unknown table; declare with CREATE TABLE first"
            ),
        }
    })?;

    // Reject duplicate policy name.
    if table.policies.iter().any(|p| p.name == name) {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("policy {name} declared more than once on {table_qname}"),
        });
    }

    table.policies.push(Policy {
        name,
        permissive,
        command,
        roles,
        using,
        with_check,
    });
    Ok(())
}

fn decode_cmd(s: &str, loc: &SourceLocation) -> Result<PolicyCommand, ParseError> {
    match s.to_ascii_lowercase().as_str() {
        "all" => Ok(PolicyCommand::All),
        "select" => Ok(PolicyCommand::Select),
        "insert" => Ok(PolicyCommand::Insert),
        "update" => Ok(PolicyCommand::Update),
        "delete" => Ok(PolicyCommand::Delete),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("unknown policy command kind {other:?}"),
        }),
    }
}

fn decode_role_targets(
    nodes: &[pg_query::protobuf::Node],
    loc: &SourceLocation,
) -> Result<Vec<GrantTarget>, ParseError> {
    use pg_query::protobuf::role_spec::RoleSpecType;
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        let Some(pg_query::NodeEnum::RoleSpec(rs)) = n.node.as_ref() else {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!("expected RoleSpec in TO clause, got {n:?}"),
            });
        };
        let role_type = RoleSpecType::try_from(rs.roletype).unwrap_or(RoleSpecType::Undefined);
        if role_type == RoleSpecType::RolespecPublic {
            out.push(GrantTarget::Public);
        } else {
            let ident = Identifier::from_unquoted(&rs.rolename).map_err(|e| ParseError::Structural {
                location: loc.clone(),
                message: format!("invalid role name {:?}: {e}", rs.rolename),
            })?;
            out.push(GrantTarget::Role(ident));
        }
    }
    Ok(out)
}

fn qname_from_rangevar(rv: &pg_query::protobuf::RangeVar, loc: &SourceLocation) -> Result<QualifiedName, ParseError> {
    let schema = Identifier::from_unquoted(&rv.schemaname).map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("invalid schema {:?}: {e}", rv.schemaname),
    })?;
    let name = Identifier::from_unquoted(&rv.relname).map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("invalid table {:?}: {e}", rv.relname),
    })?;
    Ok(QualifiedName::new(schema, name))
}

/// Decode an expression node to a `NormalizedExpr`.
///
/// Reuses `crate::parse::normalize_expr::from_pg_node`, which wraps the
/// expression in a `SELECT <expr>` scaffold, deparses through `pg_query`,
/// strips the prefix, and lowercases reserved keywords before hashing.
/// This is the same canonicalizer used by check constraints and trigger
/// WHEN clauses (see `create_trigger_stmt.rs::node_to_normalized_expr`
/// for the existing call pattern).
fn decode_expr_node(
    expr: &pg_query::protobuf::Node,
    loc: &SourceLocation,
) -> Result<NormalizedExpr, ParseError> {
    let inner_enum = expr.node.as_ref().ok_or_else(|| ParseError::Structural {
        location: loc.clone(),
        message: "policy expression node has no inner node".into(),
    })?;
    crate::parse::normalize_expr::from_pg_node(inner_enum, None, loc).map_err(|e| {
        ParseError::Structural {
            location: loc.clone(),
            message: format!("failed to canonicalize policy expression: {e}"),
        }
    })
}
```

**Implementer notes:** the `decode_expr_node` placeholder needs to call the same canonicalizer that check constraints use. Search `crates/pgevolve-core/src/parse/builder/` for the existing helper. If the helper takes `&pg_query::protobuf::Node` and returns `Result<NormalizedExpr, ParseError>` directly, use it. Otherwise adapt. **Do not write a new canonicalizer** — reuse the existing one.

Add unit tests for the parser (in the file's `tests` module):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn parse_source(sql: &str) -> Result<Catalog, ParseError> {
        // Use whatever parse helper exists in tests today.
        // Likely: crate::parse::parse_directory or a one-shot parse helper.
        todo!()
    }

    #[test]
    fn create_simple_policy() {
        let sql = "
            CREATE SCHEMA app;
            CREATE TABLE app.docs (id bigint);
            CREATE POLICY p ON app.docs USING (true);
        ";
        let cat = parse_source(sql).unwrap();
        let t = &cat.tables[0];
        assert_eq!(t.policies.len(), 1);
        assert_eq!(t.policies[0].name.as_str(), "p");
        assert!(t.policies[0].permissive);
        assert_eq!(t.policies[0].command, PolicyCommand::All);
        // Default TO clause expands to PUBLIC.
        assert_eq!(t.policies[0].roles, vec![GrantTarget::Public]);
        assert!(t.policies[0].using.is_some());
    }

    #[test]
    fn restrictive_with_check_on_select_errors() {
        let sql = "
            CREATE SCHEMA app;
            CREATE TABLE app.docs (id bigint);
            CREATE POLICY p ON app.docs FOR SELECT WITH CHECK (true);
        ";
        let err = parse_source(sql).unwrap_err();
        assert!(err.to_string().contains("WITH CHECK"), "got: {err}");
    }

    #[test]
    fn create_policy_on_unknown_table_errors() {
        let sql = "CREATE POLICY p ON nonexistent.t USING (true);";
        let err = parse_source(sql).unwrap_err();
        assert!(err.to_string().contains("unknown table"), "got: {err}");
    }

    #[test]
    fn duplicate_policy_name_errors() {
        let sql = "
            CREATE SCHEMA app;
            CREATE TABLE app.docs (id bigint);
            CREATE POLICY p ON app.docs USING (true);
            CREATE POLICY p ON app.docs USING (false);
        ";
        let err = parse_source(sql).unwrap_err();
        assert!(err.to_string().contains("more than once"), "got: {err}");
    }

    #[test]
    fn restrictive_with_roles_round_trips() {
        let sql = "
            CREATE SCHEMA app;
            CREATE TABLE app.docs (id bigint);
            CREATE POLICY p AS RESTRICTIVE FOR INSERT TO PUBLIC, readers WITH CHECK (true);
        ";
        let cat = parse_source(sql).unwrap();
        let t = &cat.tables[0];
        let p = &t.policies[0];
        assert!(!p.permissive);
        assert_eq!(p.command, PolicyCommand::Insert);
        // Order isn't normalized at parse time; canon does that. But Public
        // and "readers" should both appear.
        assert_eq!(p.roles.len(), 2);
    }
}
```

### Task 4.2: ALTER TABLE RLS subcommands

- [ ] **Step 1: Find pg_query's `AT_*` enum names**

Search:

```bash
grep -rn "AT_EnableRowSecurity\|AT_DisableRowSecurity\|AT_ForceRowSecurity\|AT_NoForceRowSecurity\|AlterTableType" ~/.cargo/registry/src/index.crates.io-*/pg_query-*/src/protobuf.rs 2>/dev/null | head -10
```

The Rust binding usually camel-cases: `AlterTableType::AtEnableRowSecurity`, etc. Verify.

- [ ] **Step 2: Extend `crates/pgevolve-core/src/parse/builder/alter_table_stmt.rs`**

In the existing `match cmd.subtype` (or equivalent dispatch on `AlterTableType`), add 4 new arms:

```rust
AlterTableType::AtEnableRowSecurity => {
    let table = find_table(cat, target_qname, loc)?;
    table.rls_enabled = true;
}
AlterTableType::AtDisableRowSecurity => {
    let table = find_table(cat, target_qname, loc)?;
    table.rls_enabled = false;
}
AlterTableType::AtForceRowSecurity => {
    let table = find_table(cat, target_qname, loc)?;
    table.rls_forced = true;
}
AlterTableType::AtNoForceRowSecurity => {
    let table = find_table(cat, target_qname, loc)?;
    table.rls_forced = false;
}
```

`find_table` is the existing helper that locates a table by qname; reuse it (or whatever the per-DB ALTER handler already uses).

If the existing alter-table parser uses a pending-action enum (like v0.3.1's `PendingOwner`), add `PendingRlsToggle` analogously. Mirror v0.3.1 Stage 7's `AT_ChangeOwner` integration.

- [ ] **Step 3: Add ALTER tests**

```rust
#[test]
fn alter_table_enable_rls() {
    let sql = "
        CREATE SCHEMA app;
        CREATE TABLE app.docs (id bigint);
        ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
    ";
    let cat = parse_source(sql).unwrap();
    assert!(cat.tables[0].rls_enabled);
    assert!(!cat.tables[0].rls_forced);
}

#[test]
fn alter_table_force_rls() {
    let sql = "
        CREATE SCHEMA app;
        CREATE TABLE app.docs (id bigint);
        ALTER TABLE app.docs FORCE ROW LEVEL SECURITY;
    ";
    let cat = parse_source(sql).unwrap();
    assert!(cat.tables[0].rls_forced);
}

#[test]
fn alter_table_disable_after_enable() {
    let sql = "
        CREATE SCHEMA app;
        CREATE TABLE app.docs (id bigint);
        ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
        ALTER TABLE app.docs DISABLE ROW LEVEL SECURITY;
    ";
    let cat = parse_source(sql).unwrap();
    assert!(!cat.tables[0].rls_enabled);
}
```

### Task 4.3: Reject `ALTER POLICY` and `DROP POLICY`

- [ ] **Step 1: Add rejection arms to top-level statement dispatch**

In `crates/pgevolve-core/src/parse/mod.rs` (or wherever the top-level `match node` dispatches), add:

```rust
pg_query::NodeEnum::AlterPolicyStmt(_) => {
    return Err(ParseError::Structural {
        location: loc,
        message: "ALTER POLICY in source is not supported — policy modifications happen via diff; use CREATE POLICY in source".into(),
    });
}
pg_query::NodeEnum::DropStmt(s) if is_drop_policy(s) => {
    return Err(ParseError::Structural {
        location: loc,
        message: "DROP POLICY in source is not supported — drops happen via diff".into(),
    });
}
```

The `is_drop_policy` helper checks `DropStmt.remove_type` for `ObjectType::ObjectPolicy`. Adapt to actual pg_query field names.

Also wire `pg_query::NodeEnum::CreatePolicyStmt(s) => policy_stmt::apply(s, cat, &loc)?,` into the dispatch.

In `crates/pgevolve-core/src/parse/statement.rs`, find where `CreatePolicyStmt` is currently labeled (the recon showed line 137 has `"CREATE POLICY"`). This file probably has a list of "unsupported" statement kinds — remove `CreatePolicyStmt` from any reject-list.

- [ ] **Step 2: Add rejection tests**

```rust
#[test]
fn alter_policy_in_source_errors() {
    let sql = "
        CREATE SCHEMA app;
        CREATE TABLE app.docs (id bigint);
        CREATE POLICY p ON app.docs USING (true);
        ALTER POLICY p ON app.docs USING (false);
    ";
    let err = parse_source(sql).unwrap_err();
    assert!(err.to_string().contains("ALTER POLICY"), "got: {err}");
}

#[test]
fn drop_policy_in_source_errors() {
    let sql = "
        CREATE SCHEMA app;
        CREATE TABLE app.docs (id bigint);
        CREATE POLICY p ON app.docs USING (true);
        DROP POLICY p ON app.docs;
    ";
    let err = parse_source(sql).unwrap_err();
    assert!(err.to_string().contains("DROP POLICY"), "got: {err}");
}
```

### Task 4.4: Wire + commit

- [ ] **Step 1: Register `policy_stmt` in `parse/builder/mod.rs`**

```rust
pub mod policy_stmt;
```

- [ ] **Step 2: Run + commit**

```bash
cargo test -p pgevolve-core --lib parse
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add -p crates/pgevolve-core/src/parse/
git commit -m "$(cat <<'EOF'
feat(parse): policies + RLS alter-table subcommands

New parse::builder::policy_stmt handles CREATE POLICY with:
  - PERMISSIVE / RESTRICTIVE (AS clause)
  - All command kinds (FOR ALL | SELECT | INSERT | UPDATE | DELETE)
  - TO clause (defaults to [Public] when omitted at parse time)
  - USING expression
  - WITH CHECK expression (rejected on FOR SELECT / FOR DELETE per PG rules)

Four new alter-table subcommand handlers (ENABLE / DISABLE / FORCE /
NO FORCE ROW LEVEL SECURITY) mutate Table.rls_enabled and
Table.rls_forced. ALTER POLICY and DROP POLICY both rejected in
source (diff-driven only). Expression canonicalization reuses the
existing check-constraint canonicalizer.

Stage 4 of docs/superpowers/plans/2026-05-22-rls-policies.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 5 — Differ

Per-table policy diff + 5 new Change variants.

**Files created:** `crates/pgevolve-core/src/diff/policies.rs`.
**Files modified:** `crates/pgevolve-core/src/diff/{change.rs, tables.rs, mod.rs}`.

### Task 5.1: Add 5 new Change variants

- [ ] **Step 1: Extend `crates/pgevolve-core/src/diff/change.rs`**

Add (after the existing v0.3.1 grants/owner variants):

```rust
    /// Create a new policy on the named table.
    CreatePolicy {
        table: QualifiedName,
        policy: crate::ir::policy::Policy,
    },
    /// Drop a policy from the named table.
    DropPolicy {
        table: QualifiedName,
        name: Identifier,
    },
    /// Alter a policy's roles / USING / WITH CHECK (NOT the command kind —
    /// PG rejects that; the differ emits DROP + CREATE instead).
    AlterPolicy {
        table: QualifiedName,
        policy: crate::ir::policy::Policy,
    },
    /// Toggle a table's `ROW LEVEL SECURITY`.
    SetTableRowSecurity {
        qname: QualifiedName,
        enable: bool,
    },
    /// Toggle a table's `FORCE ROW LEVEL SECURITY`.
    SetTableForceRowSecurity {
        qname: QualifiedName,
        force: bool,
    },
```

Imports as needed.

### Task 5.2: Implement `diff_policies`

- [ ] **Step 1: Create `crates/pgevolve-core/src/diff/policies.rs`**

```rust
//! Per-table policy diffing + RLS toggles.

use std::collections::BTreeMap;

use crate::diff::change::Change;
use crate::ir::table::Table;

/// Compute policy + RLS-toggle changes for a single table pair.
/// All changes are non-destructive (`Destructiveness::Safe` in the
/// surrounding per-DB diff machinery — applied by the caller).
pub fn diff_policies(target: &Table, source: &Table, out: &mut Vec<Change>) {
    // RLS toggle.
    if target.rls_enabled != source.rls_enabled {
        out.push(Change::SetTableRowSecurity {
            qname: source.qname.clone(),
            enable: source.rls_enabled,
        });
    }
    if target.rls_forced != source.rls_forced {
        out.push(Change::SetTableForceRowSecurity {
            qname: source.qname.clone(),
            force: source.rls_forced,
        });
    }

    // Policy pair-by-name diff.
    let target_map: BTreeMap<_, _> = target.policies.iter().map(|p| (&p.name, p)).collect();
    let source_map: BTreeMap<_, _> = source.policies.iter().map(|p| (&p.name, p)).collect();

    // Adds + modifies.
    for (name, src_p) in &source_map {
        match target_map.get(name) {
            None => out.push(Change::CreatePolicy {
                table: source.qname.clone(),
                policy: (*src_p).clone(),
            }),
            Some(tgt_p) => {
                if tgt_p.command != src_p.command {
                    // PG can't ALTER POLICY the command kind — recreate.
                    out.push(Change::DropPolicy {
                        table: target.qname.clone(),
                        name: (*name).clone(),
                    });
                    out.push(Change::CreatePolicy {
                        table: source.qname.clone(),
                        policy: (*src_p).clone(),
                    });
                } else if *tgt_p != *src_p {
                    out.push(Change::AlterPolicy {
                        table: source.qname.clone(),
                        policy: (*src_p).clone(),
                    });
                }
            }
        }
    }

    // Drops.
    for (name, _) in &target_map {
        if !source_map.contains_key(name) {
            out.push(Change::DropPolicy {
                table: target.qname.clone(),
                name: (*name).clone(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::grant::GrantTarget;
    use crate::ir::policy::{Policy, PolicyCommand};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn empty_table(qname: QualifiedName) -> Table {
        Table {
            qname,
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
        }
    }

    fn policy(name: &str, cmd: PolicyCommand) -> Policy {
        Policy {
            name: id(name),
            permissive: true,
            command: cmd,
            roles: vec![GrantTarget::Public],
            using: None,
            with_check: None,
        }
    }

    #[test]
    fn enable_rls_emits_one_change() {
        let target = empty_table(qn("app", "t"));
        let mut source = empty_table(qn("app", "t"));
        source.rls_enabled = true;
        let mut out = vec![];
        diff_policies(&target, &source, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Change::SetTableRowSecurity { enable: true, .. }));
    }

    #[test]
    fn force_rls_emits_one_change() {
        let target = empty_table(qn("app", "t"));
        let mut source = empty_table(qn("app", "t"));
        source.rls_forced = true;
        let mut out = vec![];
        diff_policies(&target, &source, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Change::SetTableForceRowSecurity { force: true, .. }));
    }

    #[test]
    fn new_policy_emits_create() {
        let target = empty_table(qn("app", "t"));
        let mut source = empty_table(qn("app", "t"));
        source.policies.push(policy("p", PolicyCommand::All));
        let mut out = vec![];
        diff_policies(&target, &source, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Change::CreatePolicy { .. }));
    }

    #[test]
    fn removed_policy_emits_drop() {
        let mut target = empty_table(qn("app", "t"));
        target.policies.push(policy("p", PolicyCommand::All));
        let source = empty_table(qn("app", "t"));
        let mut out = vec![];
        diff_policies(&target, &source, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Change::DropPolicy { .. }));
    }

    #[test]
    fn changed_policy_emits_alter_only() {
        let mut target = empty_table(qn("app", "t"));
        target.policies.push(policy("p", PolicyCommand::All));
        let mut source = empty_table(qn("app", "t"));
        let mut p = policy("p", PolicyCommand::All);
        p.roles.push(GrantTarget::Role(id("readers")));
        source.policies.push(p);
        let mut out = vec![];
        diff_policies(&target, &source, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Change::AlterPolicy { .. }));
    }

    #[test]
    fn command_kind_change_recreates() {
        let mut target = empty_table(qn("app", "t"));
        target.policies.push(policy("p", PolicyCommand::Select));
        let mut source = empty_table(qn("app", "t"));
        source.policies.push(policy("p", PolicyCommand::Insert));
        let mut out = vec![];
        diff_policies(&target, &source, &mut out);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Change::DropPolicy { .. }));
        assert!(matches!(out[1], Change::CreatePolicy { .. }));
    }
}
```

### Task 5.3: Wire into table diff

- [ ] **Step 1: Update `crates/pgevolve-core/src/diff/tables.rs`**

In the existing per-table diff function (where v0.3.1's grants + owner additions land), add at the end:

```rust
    crate::diff::policies::diff_policies(target, source, out);
```

### Task 5.4: Wire into mod.rs

- [ ] **Step 1: Add `pub mod policies;` to `crates/pgevolve-core/src/diff/mod.rs`**

### Task 5.5: Run + commit

```bash
cargo test -p pgevolve-core --lib diff::policies
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add -p crates/pgevolve-core/src/diff/
git commit -m "$(cat <<'EOF'
feat(diff): policies + RLS toggles

New diff::policies module produces 5 Change kinds:
  CreatePolicy / DropPolicy / AlterPolicy
  SetTableRowSecurity / SetTableForceRowSecurity

Pair-by-name on (table, policy_name). Command-kind changes recreate
(DROP + CREATE) because PG rejects ALTER POLICY changing the command.
All changes are non-destructive — dropping a policy reveals data
(reverts to surrounding RLS state) rather than destroying it.

Stage 5 of docs/superpowers/plans/2026-05-22-rls-policies.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 6 — Render + emit

5 new `StepKind` variants + SQL helpers + emit handlers.

**Files created:** `crates/pgevolve-core/src/plan/rewrite/policies.rs`.
**Files modified:** `crates/pgevolve-core/src/plan/raw_step.rs`, `crates/pgevolve-core/src/plan/rewrite/mod.rs`, `crates/pgevolve-core/src/plan/plan.rs`.

### Task 6.1: StepKind variants

- [ ] **Step 1: Extend `crates/pgevolve-core/src/plan/raw_step.rs::StepKind`**

```rust
    CreatePolicy,
    DropPolicy,
    AlterPolicy,
    SetTableRowSecurity,
    SetTableForceRowSecurity,
```

Extend the round-trip serialization test (every variant must appear).

- [ ] **Step 2: Extend `kind_name` / `parse_kind_name` in `crates/pgevolve-core/src/plan/plan.rs`**

Add the 5 snake_case mappings (`"create_policy"`, `"drop_policy"`, `"alter_policy"`, `"set_table_row_security"`, `"set_table_force_row_security"`).

### Task 6.2: SQL helpers

- [ ] **Step 1: Create `crates/pgevolve-core/src/plan/rewrite/policies.rs`**

```rust
//! SQL rendering for policies + RLS toggles.

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::grant::GrantTarget;
use crate::ir::policy::Policy;

/// `CREATE POLICY name ON qname AS PERMISSIVE FOR ALL TO public USING (...) WITH CHECK (...);`
///
/// Always renders the explicit form for AS, FOR, TO — byte-stable round-trip,
/// unambiguous fixture diffs, clear human-readable output. USING and WITH CHECK
/// are omitted when absent on the policy.
#[must_use]
pub fn create_policy(table: &QualifiedName, p: &Policy) -> String {
    let mut sql = format!(
        "CREATE POLICY {} ON {} AS {} FOR {} TO {}",
        p.name.render_sql(),
        table.render_sql(),
        if p.permissive { "PERMISSIVE" } else { "RESTRICTIVE" },
        p.command.sql_keyword(),
        render_roles(&p.roles),
    );
    if let Some(u) = &p.using {
        sql.push_str(&format!(" USING ({})", u.canonical_text));
    }
    if let Some(c) = &p.with_check {
        sql.push_str(&format!(" WITH CHECK ({})", c.canonical_text));
    }
    sql.push(';');
    sql
}

/// `ALTER POLICY name ON qname TO ... USING (...) WITH CHECK (...);`
///
/// ALTER POLICY does NOT change the command kind (that's handled by
/// DROP + CREATE in the differ). It can change TO clause, USING, WITH CHECK.
#[must_use]
pub fn alter_policy(table: &QualifiedName, p: &Policy) -> String {
    let mut sql = format!(
        "ALTER POLICY {} ON {} TO {}",
        p.name.render_sql(),
        table.render_sql(),
        render_roles(&p.roles),
    );
    if let Some(u) = &p.using {
        sql.push_str(&format!(" USING ({})", u.canonical_text));
    }
    if let Some(c) = &p.with_check {
        sql.push_str(&format!(" WITH CHECK ({})", c.canonical_text));
    }
    sql.push(';');
    sql
}

/// `DROP POLICY name ON qname;`
#[must_use]
pub fn drop_policy(table: &QualifiedName, name: &Identifier) -> String {
    format!("DROP POLICY {} ON {};", name.render_sql(), table.render_sql())
}

/// `ALTER TABLE qname { ENABLE | DISABLE } ROW LEVEL SECURITY;`
#[must_use]
pub fn set_table_row_security(qname: &QualifiedName, enable: bool) -> String {
    let verb = if enable { "ENABLE" } else { "DISABLE" };
    format!("ALTER TABLE {} {} ROW LEVEL SECURITY;", qname.render_sql(), verb)
}

/// `ALTER TABLE qname { FORCE | NO FORCE } ROW LEVEL SECURITY;`
#[must_use]
pub fn set_table_force_row_security(qname: &QualifiedName, force: bool) -> String {
    let verb = if force { "FORCE" } else { "NO FORCE" };
    format!("ALTER TABLE {} {} ROW LEVEL SECURITY;", qname.render_sql(), verb)
}

fn render_roles(roles: &[GrantTarget]) -> String {
    let parts: Vec<String> = roles.iter().map(|r| match r {
        GrantTarget::Public => "PUBLIC".to_string(),
        GrantTarget::Role(id) => id.render_sql(),
    }).collect();
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::default_expr::NormalizedExpr;
    use crate::ir::policy::PolicyCommand;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn simple_policy() -> Policy {
        Policy {
            name: id("p1"),
            permissive: true,
            command: PolicyCommand::All,
            roles: vec![GrantTarget::Public],
            using: Some(NormalizedExpr::from_canonical_text("true")),
            with_check: None,
        }
    }

    #[test]
    fn renders_create_policy() {
        let sql = create_policy(&qn("app", "docs"), &simple_policy());
        assert_eq!(
            sql,
            "CREATE POLICY p1 ON app.docs AS PERMISSIVE FOR ALL TO PUBLIC USING (true);"
        );
    }

    #[test]
    fn renders_restrictive_with_check() {
        let mut p = simple_policy();
        p.permissive = false;
        p.command = PolicyCommand::Insert;
        p.with_check = Some(NormalizedExpr::from_canonical_text("author = current_user"));
        p.using = None;
        let sql = create_policy(&qn("app", "docs"), &p);
        assert_eq!(
            sql,
            "CREATE POLICY p1 ON app.docs AS RESTRICTIVE FOR INSERT TO PUBLIC WITH CHECK (author = current_user);"
        );
    }

    #[test]
    fn renders_multi_role_to_clause() {
        let mut p = simple_policy();
        p.roles = vec![GrantTarget::Public, GrantTarget::Role(id("readers"))];
        let sql = create_policy(&qn("app", "docs"), &p);
        assert!(sql.contains("TO PUBLIC, readers"), "got: {sql}");
    }

    #[test]
    fn renders_alter_policy() {
        let sql = alter_policy(&qn("app", "docs"), &simple_policy());
        assert_eq!(sql, "ALTER POLICY p1 ON app.docs TO PUBLIC USING (true);");
    }

    #[test]
    fn renders_drop_policy() {
        let sql = drop_policy(&qn("app", "docs"), &id("p1"));
        assert_eq!(sql, "DROP POLICY p1 ON app.docs;");
    }

    #[test]
    fn renders_enable_disable_rls() {
        assert_eq!(set_table_row_security(&qn("app", "docs"), true), "ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;");
        assert_eq!(set_table_row_security(&qn("app", "docs"), false), "ALTER TABLE app.docs DISABLE ROW LEVEL SECURITY;");
    }

    #[test]
    fn renders_force_no_force_rls() {
        assert_eq!(set_table_force_row_security(&qn("app", "docs"), true), "ALTER TABLE app.docs FORCE ROW LEVEL SECURITY;");
        assert_eq!(set_table_force_row_security(&qn("app", "docs"), false), "ALTER TABLE app.docs NO FORCE ROW LEVEL SECURITY;");
    }
}
```

### Task 6.3: Emit handlers

- [ ] **Step 1: Add to `crates/pgevolve-core/src/plan/rewrite/mod.rs`**

Add `pub mod policies;`. In the per-change match arm (where v0.3.1's grants/owner emit lives), add 5 new arms:

```rust
Change::CreatePolicy { table, policy } => {
    raw_steps.push(RawStep {
        step_no: 0,
        kind: StepKind::CreatePolicy,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![table.clone()],
        sql: policies::create_policy(table, policy),
        transactional: TransactionConstraint::InTransaction,
    });
}
Change::DropPolicy { table, name } => {
    raw_steps.push(RawStep {
        step_no: 0,
        kind: StepKind::DropPolicy,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![table.clone()],
        sql: policies::drop_policy(table, name),
        transactional: TransactionConstraint::InTransaction,
    });
}
Change::AlterPolicy { table, policy } => {
    raw_steps.push(RawStep {
        step_no: 0,
        kind: StepKind::AlterPolicy,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![table.clone()],
        sql: policies::alter_policy(table, policy),
        transactional: TransactionConstraint::InTransaction,
    });
}
Change::SetTableRowSecurity { qname, enable } => {
    raw_steps.push(RawStep {
        step_no: 0,
        kind: StepKind::SetTableRowSecurity,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![qname.clone()],
        sql: policies::set_table_row_security(qname, *enable),
        transactional: TransactionConstraint::InTransaction,
    });
}
Change::SetTableForceRowSecurity { qname, force } => {
    raw_steps.push(RawStep {
        step_no: 0,
        kind: StepKind::SetTableForceRowSecurity,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![qname.clone()],
        sql: policies::set_table_force_row_security(qname, *force),
        transactional: TransactionConstraint::InTransaction,
    });
}
```

Adapt to the actual `RawStep` field shape (read v0.3.1's emit code for the exact pattern).

### Task 6.4: Update `pgevolve/src/commands/diff.rs`

The CLI's diff display needs arms for the 5 new variants. Add brief human-readable descriptions:

```rust
Change::CreatePolicy { table, policy } => format!("+ CREATE POLICY {} ON {}", policy.name, table),
Change::DropPolicy { table, name } => format!("- DROP POLICY {} ON {}", name, table),
Change::AlterPolicy { table, policy } => format!("~ ALTER POLICY {} ON {}", policy.name, table),
Change::SetTableRowSecurity { qname, enable } => {
    let verb = if *enable { "ENABLE" } else { "DISABLE" };
    format!("~ ALTER TABLE {} {} ROW LEVEL SECURITY", qname, verb)
}
Change::SetTableForceRowSecurity { qname, force } => {
    let verb = if *force { "FORCE" } else { "NO FORCE" };
    format!("~ ALTER TABLE {} {} ROW LEVEL SECURITY", qname, verb)
}
```

### Task 6.5: Run + commit

```bash
cargo test -p pgevolve-core --lib plan
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add -p crates/pgevolve-core/src/plan/ crates/pgevolve/src/commands/diff.rs
git commit -m "$(cat <<'EOF'
feat(plan): policies + RLS toggles — render + emit

5 new StepKind variants:
  CreatePolicy / DropPolicy / AlterPolicy
  SetTableRowSecurity / SetTableForceRowSecurity

plan::rewrite::policies renders each. CREATE POLICY always renders
explicit AS / FOR / TO clauses for round-trip byte stability and
fixture-diff clarity. ALTER POLICY rebuilds the alterable subset
(roles + USING + WITH CHECK); command-kind changes go through
DROP + CREATE per Stage 5's differ.

All ops run InTransaction. None destructive.

Stage 6 of docs/superpowers/plans/2026-05-22-rls-policies.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 7 — Lint rules

Extend the v0.3.1 `grant-references-unknown-role` rule to cover policy `TO` clauses. Add one new rule: `force-rls-without-policies`.

**Files created:** `crates/pgevolve-core/src/lint/rules/force_rls_without_policies.rs`.
**Files modified:** `crates/pgevolve-core/src/lint/rules/grant_references_unknown_role.rs`, `crates/pgevolve-core/src/lint/rules/mod.rs`, `crates/pgevolve-core/src/lint/universal.rs`.

### Task 7.1: Extend `grant-references-unknown-role`

- [ ] **Step 1: Add policy-roles walk to `check` function**

In `crates/pgevolve-core/src/lint/rules/grant_references_unknown_role.rs::check`, after the existing per-family iterations and before the default-privileges block, add:

```rust
    // Policies on tables — TO clause references.
    for t in &cat.tables {
        for p in &t.policies {
            for role_target in &p.roles {
                if let crate::ir::grant::GrantTarget::Role(name) = role_target {
                    if !cluster_roles.contains(name) {
                        findings.push(Finding {
                            rule: RULE_ID,
                            severity: Severity::Error,
                            message: format!(
                                "policy {} on table {}: TO clause references role {} which is not declared in the linked cluster project",
                                p.name, t.qname, name,
                            ),
                            location: None,
                        });
                    }
                }
            }
        }
    }
```

Add a test:

```rust
#[test]
fn policy_to_clause_with_unknown_role_fires() {
    use crate::ir::policy::{Policy, PolicyCommand};
    let mut cat = Catalog::empty();
    let mut t = empty_table_helper(); // adapt to existing test helper
    t.policies.push(Policy {
        name: id("p1"),
        permissive: true,
        command: PolicyCommand::All,
        roles: vec![GrantTarget::Role(id("unknown_role"))],
        using: None,
        with_check: None,
    });
    cat.tables.push(t);
    let cluster_roles = cluster_with(&["readers"]);
    let f = check(&cat, Some(&cluster_roles));
    assert!(f.iter().any(|f| f.message.contains("policy p1") && f.message.contains("unknown_role")));
}
```

### Task 7.2: `force-rls-without-policies` rule

- [ ] **Step 1: Create `crates/pgevolve-core/src/lint/rules/force_rls_without_policies.rs`**

```rust
//! Warns when FORCE ROW LEVEL SECURITY is enabled on a table with no
//! policies. PG's behavior in that state is to deny every row — almost
//! always a configuration mistake (operator forgot to add policies).

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "force-rls-without-policies";

pub fn check(cat: &Catalog) -> Vec<Finding> {
    cat.tables.iter()
        .filter(|t| t.rls_forced && t.policies.is_empty())
        .map(|t| Finding {
            rule: RULE_ID,
            severity: Severity::Warning,
            message: format!(
                "table {}: FORCE ROW LEVEL SECURITY enabled but no policies defined — all rows will be denied",
                t.qname,
            ),
            location: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::policy::{Policy, PolicyCommand};
    use crate::ir::grant::GrantTarget;
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn table(qname: QualifiedName, rls_forced: bool, policies: Vec<Policy>) -> Table {
        Table {
            qname,
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: rls_forced, // force implies enabled in real PG, but for IR we model independently
            rls_forced,
            policies,
        }
    }

    fn dummy_policy() -> Policy {
        Policy {
            name: id("p"),
            permissive: true,
            command: PolicyCommand::All,
            roles: vec![GrantTarget::Public],
            using: None,
            with_check: None,
        }
    }

    #[test]
    fn force_without_policies_fires() {
        let mut cat = Catalog::empty();
        cat.tables.push(table(qn("app", "t"), true, vec![]));
        let f = check(&cat);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, RULE_ID);
        assert_eq!(f[0].severity, Severity::Warning);
    }

    #[test]
    fn force_with_policies_silent() {
        let mut cat = Catalog::empty();
        cat.tables.push(table(qn("app", "t"), true, vec![dummy_policy()]));
        assert!(check(&cat).is_empty());
    }

    #[test]
    fn no_force_no_policies_silent() {
        let mut cat = Catalog::empty();
        cat.tables.push(table(qn("app", "t"), false, vec![]));
        assert!(check(&cat).is_empty());
    }
}
```

### Task 7.3: Wire dispatcher

- [ ] **Step 1: Register in `crates/pgevolve-core/src/lint/rules/mod.rs`**

```rust
pub mod force_rls_without_policies;
```

- [ ] **Step 2: Call from `check_universal` in `crates/pgevolve-core/src/lint/universal.rs`**

Add (or extend the existing `check_universal_with_cluster` if you prefer to keep it cluster-aware-only):

```rust
out.extend(rules::force_rls_without_policies::check(&source.catalog));
```

(`force-rls-without-policies` doesn't need cluster context — it's pure source-tree.)

Extend the module-level doc-comment index in `universal.rs` to list `force-rls-without-policies` under the "Source-tree rules" heading.

### Task 7.4: Run + commit

```bash
cargo test -p pgevolve-core --lib lint
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add -p crates/pgevolve-core/src/lint/
git commit -m "$(cat <<'EOF'
feat(lint): policy-TO check + force-rls-without-policies

Two lint updates for RLS:

  grant-references-unknown-role (existing) now also walks
  cat.tables[*].policies[*].roles. The finding message distinguishes
  "policy P on table T" from grant findings.

  force-rls-without-policies (new, Warning, waivable): fires when a
  table has FORCE ROW LEVEL SECURITY enabled but no policies defined.
  PG denies every row in that state — almost always a configuration
  mistake.

Stage 7 of docs/superpowers/plans/2026-05-22-rls-policies.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 8 — Conformance fixtures

11 fixtures under `crates/pgevolve-conformance/tests/cases/objects/policies/`. Mirror v0.3.0 / v0.3.1 fixture patterns.

### Task 8.1: Create fixtures

For each fixture: directory with `before.sql`, `after.sql`, `fixture.toml`, empty `expected/`.

Use `authoring = "objects"` for all of them (per-DB fixtures; reuse existing harness).

**1. `policies/enable-rls/`**

```
before.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
after.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
  ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
fixture.toml:
  [meta]
  title = "ALTER TABLE ENABLE ROW LEVEL SECURITY"
  authoring = "objects"
  spec_refs = ["objects.policy.rls"]
  [pg]
  min = 14
  max = 17
  [expect.plan]
  steps = 1
```

**2. `policies/disable-rls/`**

```
before.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
  ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
after.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
fixture.toml: steps = 1
```

**3. `policies/force-rls-toggle/`**

```
before.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
  ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
after.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
  ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
  ALTER TABLE app.docs FORCE ROW LEVEL SECURITY;
fixture.toml: steps = 1  (only the FORCE toggle changed)
```

**4. `policies/simple-permissive-policy/`**

```
before.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint, author text);
after.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint, author text);
  CREATE POLICY author_only ON app.docs USING (author = current_user);
fixture.toml: steps = 1
```

**5. `policies/restrictive-policy/`**

```
after.sql adds:
  CREATE POLICY only_authors AS RESTRICTIVE FOR INSERT WITH CHECK (author = current_user);
fixture.toml: steps = 1
```

**6. `policies/policy-with-roles/`**

```
before.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
after.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
  CREATE POLICY p ON app.docs TO PUBLIC, readers USING (true);
fixture.toml: steps = 1, setup = "CREATE ROLE readers;"
```

(Use the `setup.sql` mechanism added in v0.3.1 Stage 12 to seed roles. If the conformance harness doesn't support `setup.sql`, mark this fixture as requiring it and document the gap.)

**7. `policies/policy-with-check/`**

```
after.sql:
  CREATE POLICY p ON app.docs FOR INSERT WITH CHECK (true);
```

**8. `policies/alter-policy-roles/`**

```
before.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
  CREATE POLICY p ON app.docs TO PUBLIC USING (true);
after.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
  CREATE POLICY p ON app.docs TO readers USING (true);
fixture.toml: steps = 1 (single ALTER POLICY)
```

**9. `policies/alter-policy-command-recreates/`**

```
before.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
  CREATE POLICY p ON app.docs FOR SELECT USING (true);
after.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
  CREATE POLICY p ON app.docs FOR INSERT WITH CHECK (true);
fixture.toml: steps = 2 (DROP + CREATE)
```

**10. `policies/drop-policy-on-source-removal/`**

```
before.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
  CREATE POLICY p ON app.docs USING (true);
after.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
fixture.toml: steps = 1
```

**11. `policies/lint/force-without-policies/`**

```
before.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
after.sql:
  CREATE SCHEMA app;
  CREATE TABLE app.docs (id bigint);
  ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
  ALTER TABLE app.docs FORCE ROW LEVEL SECURITY;
fixture.toml:
  [expect.advisory]
  rule_ids = ["force-rls-without-policies"]
  [expect.plan]
  steps = 2  (enable + force; no policies → lint fires)
```

### Task 8.2: Bless + verify

- [ ] **Step 1: Bless**

```bash
cargo xtask bless --conformance
```

- [ ] **Step 2: Spot-check the blessed output**

For each new fixture, inspect `expected/plan.sql` to confirm the rendered SQL matches the spec promise. The `policy-with-roles` fixture's plan SQL should preserve `TO PUBLIC, readers` exactly.

- [ ] **Step 3: Run full conformance**

```bash
cargo test -p pgevolve-conformance
```

All green. Pre-existing fixtures should not regress.

### Task 8.3: Commit

```bash
git add -p crates/pgevolve-conformance/tests/cases/
git commit -m "$(cat <<'EOF'
test(conformance): 11 policy + RLS fixtures

New fixture root: cases/objects/policies/. Covers:
  enable-rls / disable-rls / force-rls-toggle
  simple-permissive-policy / restrictive-policy
  policy-with-roles / policy-with-check
  alter-policy-roles / alter-policy-command-recreates (DROP+CREATE)
  drop-policy-on-source-removal
  lint/force-without-policies

Stage 8 of docs/superpowers/plans/2026-05-22-rls-policies.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 9 — Proptest + docs + v0.3.2 release

### Task 9.1: Property tests

- [ ] **Step 1: Extend `arbitrary_table` in `crates/pgevolve-testkit/src/ir_generator.rs`**

```rust
// In arbitrary_table (or arbitrary_non_pk_column's parent strategy):
// Add proptest strategies for rls_enabled, rls_forced, and policies.

fn arb_policy_command() -> impl Strategy<Value = PolicyCommand> {
    prop_oneof![
        Just(PolicyCommand::All),
        Just(PolicyCommand::Select),
        Just(PolicyCommand::Insert),
        Just(PolicyCommand::Update),
        Just(PolicyCommand::Delete),
    ]
}

fn arb_policy(role_pool: Vec<&'static str>) -> impl Strategy<Value = Policy> {
    use proptest::collection::vec;
    (
        arb_simple_identifier(),  // policy name
        any::<bool>(),            // permissive
        arb_policy_command(),
        vec(prop_oneof![Just(GrantTarget::Public), /* roles from pool */], 0..3),
    ).prop_flat_map(move |(name, permissive, command, roles)| {
        // For SELECT/DELETE, with_check must be None.
        let with_check_strategy = if command.allows_with_check() {
            prop_oneof![Just(None), Just(Some(NormalizedExpr::from_canonical_text("true")))]
        } else {
            Just(None).boxed()
        };
        let using_strategy = prop_oneof![Just(None), Just(Some(NormalizedExpr::from_canonical_text("true")))];
        (Just(name), Just(permissive), Just(command), Just(roles), using_strategy, with_check_strategy)
    }).prop_map(|(name, permissive, command, roles, using, with_check)| Policy {
        name, permissive, command, roles, using, with_check,
    })
}
```

Plumb into the existing `arbitrary_table` so generated tables get 0–3 policies + random `rls_enabled` / `rls_forced` flags.

- [ ] **Step 2: Run 10× per constitution §9**

```bash
for i in 1 2 3 4 5 6 7 8 9 10; do
    echo "=== Run $i ==="
    PROPTEST_CASES=512 cargo test --workspace --release 2>&1 | tail -3
done
```

All 10 green.

- [ ] **Step 3: Commit**

```
test(proptest): policies + RLS flags in arbitrary_table

arb_policy honors the WITH CHECK / FOR SELECT-or-DELETE
incompatibility — generated policies never produce invalid PG
syntax. rls_enabled + rls_forced added as independent bool
strategies. 10× per §9; all green.

Stage 9.1 of docs/superpowers/plans/2026-05-22-rls-policies.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

### Task 9.2: Docs

- [ ] **Step 1: Update `docs/spec/objects.md:250`**

Replace:

```markdown
| Row-level security policies (`POLICY`) | 📋 Planned, v0.3 | Including `ENABLE ROW LEVEL SECURITY` toggle on tables. |
```

with:

```markdown
| Row-level security policies (`POLICY`) | ✅ Supported | Per-table `rls_enabled` + `rls_forced` flags + embedded `policies: Vec<Policy>`. USING / WITH CHECK use NormalizedExpr canon (shared with check constraints). Command-kind changes go through DROP + CREATE. change_kinds: [create, alter, drop, set_table_row_security, set_table_force_row_security] |
```

- [ ] **Step 2: Create `docs/spec/policies.md`**

```markdown
# Row-level security policies

pgevolve manages Postgres RLS declaratively. Tables carry:

- `rls_enabled: bool` — `ALTER TABLE t ENABLE/DISABLE ROW LEVEL SECURITY`.
- `rls_forced: bool` — `ALTER TABLE t FORCE/NO FORCE ROW LEVEL SECURITY`
  (applies even to the table owner).
- `policies: Vec<Policy>` — embedded; policies can't exist orphan.

## Source surface

```sql
CREATE POLICY author_only ON app.docs
    AS PERMISSIVE              -- (default)
    FOR ALL                    -- (default)
    TO public                  -- (default)
    USING (author = current_user);

ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
ALTER TABLE app.docs FORCE ROW LEVEL SECURITY;
```

`ALTER POLICY` and `DROP POLICY` are **rejected in source** — both
come from the diff against the catalog.

## Command-kind changes recreate

PG's `ALTER POLICY` can change roles, USING, and WITH CHECK but
NOT the command kind. If source changes a policy from `FOR SELECT`
to `FOR INSERT`, pgevolve emits `DROP POLICY` + `CREATE POLICY` as
two separate plan steps.

## Cross-cluster role validation

Policy `TO` clauses reference roles. The v0.3.1 cross-cluster lint
`grant-references-unknown-role` extends to policy roles when
`[cluster].project` is set in `pgevolve.toml`.

## FORCE without policies = denial

PG's behavior: `FORCE ROW LEVEL SECURITY` with no policies defined
denies every row, including for the table owner. Almost always a
configuration mistake. The `force-rls-without-policies` lint warns
on this state.

## WITH CHECK validity

`WITH CHECK` is invalid on `FOR SELECT` and `FOR DELETE` policies
(PG rejects). The source parser pre-empts with a clear error.

## Out of scope

- `ALTER POLICY ... RENAME TO ...` in source — rejected.
  Operators can drop+create.
- `SECURITY LABEL` — ⛔ Not planned.
- `leakproof` / `security_barrier` on views — 🔮 Future.
```

- [ ] **Step 3: Update `CHANGELOG.md`**

Add a new `[0.3.2]` section above `[0.3.1]`:

```markdown
## [Unreleased]

## [0.3.2] — 2026-05-22

### Added

- **Row-level security policies** — `Table` gains `rls_enabled`, `rls_forced`, and `policies: Vec<Policy>`. Policies carry `permissive`, `command`, `roles`, `using`, `with_check`. USING / WITH CHECK reuse `NormalizedExpr` canonicalization shared with check constraints.
- **Source parser:** `CREATE POLICY` + four `ALTER TABLE ... { ENABLE | DISABLE | FORCE | NO FORCE } ROW LEVEL SECURITY` subcommands. `ALTER POLICY` and `DROP POLICY` rejected in source (diff-driven).
- **Differ:** 5 new Change variants (`CreatePolicy`, `DropPolicy`, `AlterPolicy`, `SetTableRowSecurity`, `SetTableForceRowSecurity`). Command-kind changes recreate (DROP + CREATE) because PG doesn't allow `ALTER POLICY` to change the command.
- **Catalog reader:** new `pg_policies` query + `relrowsecurity` / `relforcerowsecurity` on the tables query.
- **Two lint additions:**
  - `grant-references-unknown-role` (existing) now also walks policy `TO` clauses.
  - `force-rls-without-policies` (new, Warning) — fires when a table has FORCE RLS enabled but no policies defined (PG would deny all rows).
- **Conformance:** 11 new fixtures under `objects/policies/`.

### Closes

v0.3 security/permissions trilogy: roles (v0.3.0) → grants (v0.3.1) → policies (v0.3.2).

## [0.3.1] — 2026-05-22
```

### Task 9.3: Version bump

```bash
# Root Cargo.toml [workspace.package].version → "0.3.2"
# Each crate's Cargo.toml that has its own version → "0.3.2"
cargo build --workspace
```

Verify CHANGELOG sync gate:

```bash
v=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
echo "version: $v"
grep -q "^## \[$v\] — " CHANGELOG.md && echo OK || echo MISMATCH
```

Expected: OK.

### Task 9.4: §9 verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo doc --workspace --no-deps 2>&1 | grep -cE "^warning"  # expect 0
```

### Task 9.5: Re-bless conformance (plan-id depends on version)

```bash
cargo xtask bless --conformance
cargo test -p pgevolve-conformance
```

### Task 9.6: Release commit

```bash
git add docs/spec/objects.md docs/spec/policies.md CHANGELOG.md Cargo.toml Cargo.lock crates/*/Cargo.toml crates/pgevolve-conformance/tests/cases/
git commit -m "$(cat <<'EOF'
release: v0.3.2 — row-level security policies

Final v0.3 sub-spec. Tables gain rls_enabled, rls_forced, and
policies: Vec<Policy>. USING / WITH CHECK reuse NormalizedExpr from
check constraints. The cross-cluster lint from v0.3.1 extends to
cover policy TO clauses.

Closes the v0.3 security/permissions trilogy:
  v0.3.0 — cluster surface + ROLE/USER
  v0.3.1 — object grants + ownership + default privileges
  v0.3.2 — row-level security policies (this release)

Closes issue #4.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 9.7: STOP

Do NOT `git tag`, `git push`, or `gh issue close`. Those require the user's signing key. Report DONE.

---

## Done.

After Stage 9, v0.3.2 is committed locally and ready for tagging:

- Policy IR + RLS table flags
- Canon (sort policies, sort roles, dedupe)
- Catalog reader (pg_policies + relrowsecurity/relforcerowsecurity)
- Source parser (CREATE POLICY + 4 RLS subcommands; ALTER/DROP rejected)
- Differ (5 Change variants; command-kind change recreates)
- Render + emit (5 StepKind variants; explicit clause rendering)
- Lint extension + new `force-rls-without-policies` rule
- 11 conformance fixtures + property test coverage
- v0.3.2 release commit (no tag, no push)

After v0.3.2 ships, the v0.3 security/permissions trilogy is complete. Next plan target: **CREATE STATISTICS** (issue #5), which is small, independent, and the last v0.3 roadmap item.
