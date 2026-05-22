# Row-level security policies (v0.3.2 — final v0.3 trilogy leg)

**Status:** Design accepted 2026-05-22. Final leg of the v0.3 security/permissions trilogy (roles → grants → RLS).
**Closes:** GitHub issue #4 (v0.3: implement POLICY).
**Spec line touched:** `docs/spec/objects.md:250` (will move from 📋 Planned → ✅ Supported).
**Depends on:** v0.3.0 (cluster roles — policy `TO` clauses reference roles) and v0.3.1 (grants infrastructure — reuses `GrantTarget`, cross-cluster lint).

## Summary

Manage Postgres Row-Level Security declaratively. Tables gain `rls_enabled`, `rls_forced`, and `policies: Vec<Policy>` fields. Policies embed in their table (no orphan possible). USING / WITH CHECK expressions use the existing `NormalizedExpr` canonicalization shared with check constraints. The differ produces minimal CREATE/ALTER/DROP POLICY sequences plus the four ROW LEVEL SECURITY toggles. The v0.3.1 cross-cluster lint extends to cover policy `TO` clauses.

## Scope

**In scope:**

- `Table` gains three new fields: `rls_enabled: bool`, `rls_forced: bool`, `policies: Vec<Policy>`.
- New IR module `ir::policy` with `Policy` + `PolicyCommand`.
- Source parser for `CREATE POLICY` and the four `ALTER TABLE ... { ENABLE | DISABLE | FORCE | NO FORCE } ROW LEVEL SECURITY` subcommands.
- Catalog reader queries `pg_policies` view + the `relrowsecurity` / `relforcerowsecurity` columns on `pg_class`.
- Differ: 5 new change kinds (`CreatePolicy`, `DropPolicy`, `AlterPolicy`, `SetTableRowSecurity`, `SetTableForceRowSecurity`).
- Render: SQL helpers + emit handlers for all 5 step kinds.
- Lint: extend the v0.3.1 `grant-references-unknown-role` rule to cover policy `roles`; add one new rule `force-rls-without-policies` (warning).
- Conformance: ~11 fixtures under `objects/policies/`.

**Explicitly out of scope:**

- `ALTER POLICY ... RENAME TO ...` in source — renaming policies via diff is rejected for v0.3.2 (would require a separate rename op kind; not worth the complexity for a feature with poor user demand). Operators can drop+create in source.
- `SECURITY LABEL` — already marked ⛔ Not planned in objects.md.
- `leakproof` / `security_barrier` on views — separate row in objects.md, `🔮 Future`.
- The v0.3.1 deferred cluster-link conformance harness extension and the 2 deferred fixtures — explicitly out of scope per user direction; tracked as separate follow-up.

User confirmed tight scope during 2026-05-22 brainstorming.

## IR

### `Table` extensions

In `crates/pgevolve-core/src/ir/table.rs`:

```rust
pub struct Table {
    // ... existing fields ...
    /// `ROW LEVEL SECURITY` enabled flag. PG default: false.
    #[diff(via_debug)]
    pub rls_enabled: bool,
    /// `FORCE ROW LEVEL SECURITY` flag (RLS applies even to owner). PG default: false.
    #[diff(via_debug)]
    pub rls_forced: bool,
    /// Policies attached to this table. Canonicalized in `ir::canon`.
    #[diff(via_debug)]
    pub policies: Vec<Policy>,
}
```

`#[diff(via_debug)]` for the same reason `grants` uses it: coarse path is fine; the differ does element-level comparison.

### New module — `crates/pgevolve-core/src/ir/policy.rs`

```rust
use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::grant::GrantTarget;

/// A row-level security policy attached to a table.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Policy {
    /// Policy name. Unique per table.
    pub name: Identifier,
    /// `AS PERMISSIVE` (any matching policy passes) vs `AS RESTRICTIVE`
    /// (all must pass). PG default: true (PERMISSIVE).
    pub permissive: bool,
    /// Which command(s) this policy applies to. `All` covers all DML.
    pub command: PolicyCommand,
    /// `TO roles` list. Source omission canonicalizes to
    /// `vec![GrantTarget::Public]` at parse time so source and catalog
    /// round-trip equally.
    pub roles: Vec<GrantTarget>,
    /// `USING (expr)` — row-visibility filter. Always allowed; PG default: absent.
    pub using: Option<NormalizedExpr>,
    /// `WITH CHECK (expr)` — write-time filter. Valid only on commands
    /// that write rows (Insert/Update/All); parser rejects on Select/Delete.
    pub with_check: Option<NormalizedExpr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyCommand {
    All,
    Select,
    Insert,
    Update,
    Delete,
}
```

`Policy` derives `Ord` with the natural tuple order (name first). `permissive: true` is the default; source `AS RESTRICTIVE` flips it to false. `command: PolicyCommand` encodes the FOR clause; default in source = `All` (matching PG).

## Canon

In `crates/pgevolve-core/src/ir/canon/policies.rs` (new):

- Sort each `Table.policies` by `name`.
- Sort each policy's `roles` lexicographically (`Public` < `Role(name)`).
- Deduplicate identical `Grant`-style role entries (canon collapses any source duplicates).

`Catalog::canonicalize` calls `policies::run` after the existing per-family canon passes:

```rust
for t in &mut cat.tables {
    canon::policies::run_on_table(t);
}
```

`NormalizedExpr` canonicalization for `using` / `with_check` is inherited from the constraint canon path. No new expression-canon work needed.

## Source parser

### `CREATE POLICY` — new builder

`crates/pgevolve-core/src/parse/builder/policy_stmt.rs`. Handles `pg_query::NodeEnum::CreatePolicyStmt`.

Decode:
- `policy_name`, `table` (RangeVar)
- `permissive: bool` from `s.permissive`
- `cmd_name: String` ("all"/"select"/"insert"/"update"/"delete") → `PolicyCommand`
- `roles: Vec<RoleSpec>` → `Vec<GrantTarget>`; if empty, canonicalize to `vec![GrantTarget::Public]`
- `qual: Option<Expr>` → `NormalizedExpr` for `using`
- `with_check: Option<Expr>` → `NormalizedExpr` for `with_check`

**Parser validation:**

- `WITH CHECK` on `FOR SELECT` policy → `ParseError::Structural` ("WITH CHECK is invalid on FOR SELECT policies; PG rejects").
- `WITH CHECK` on `FOR DELETE` policy → `ParseError::Structural` (same).
- `CREATE POLICY ON unknown_table` → `ParseError::Structural` ("unknown table — declare with CREATE TABLE first").

### `ALTER TABLE` RLS subcommands

Extend `crates/pgevolve-core/src/parse/builder/alter_table_stmt.rs` to recognize 4 new subcommand types:

- `AT_EnableRowSecurity` → set `table.rls_enabled = true`
- `AT_DisableRowSecurity` → set `table.rls_enabled = false`
- `AT_EnableRowSecurity` with force variant (verify pg_query field name) → `rls_forced = true`
- `AT_NoForceRowSecurity` → `rls_forced = false`

The exact pg_query subcommand identifiers may differ — implementer verifies during recon. Pattern mirrors v0.2.1's `AT_SetStorage` / `AT_SetCompression` additions and v0.3.1's `AT_ChangeOwner` work.

### Rejected in source

- `ALTER POLICY` — rejected with clear error ("policy modifications happen via diff; use `CREATE POLICY` in source").
- `DROP POLICY` — rejected (drops come from absence in source).

## Catalog reader

### `pg_policies` query

New `crates/pgevolve-core/src/catalog/queries/policies.rs`:

```sql
SELECT p.schemaname,
       p.tablename,
       p.policyname,
       (p.permissive = 'PERMISSIVE')::bool AS permissive,
       p.cmd,                                  -- ALL/SELECT/INSERT/UPDATE/DELETE
       coalesce(p.roles::text[], '{}'::text[]) AS roles,
       p.qual::text       AS using_text,       -- NULL if absent
       p.with_check::text AS with_check_text
FROM pg_policies p
WHERE p.schemaname = ANY($1::text[])
ORDER BY p.schemaname, p.tablename, p.policyname
```

`p.roles` is a `name[]` of role names. The literal string `public` decodes to `GrantTarget::Public`; other names to `GrantTarget::Role(Identifier)`. Stage 5 of v0.3.1's ACL decoder doesn't apply here directly (no aclitem grammar), but the role-list shape is simpler — straight identifier decoding.

### `pg_class` extensions

Extend the existing tables query (Stage 5 of v0.3.1 already added owner+acl; same shape) to also select:

```sql
       c.relrowsecurity::bool   AS rls_enabled,
       c.relforcerowsecurity::bool AS rls_forced,
```

Populate `Table.rls_enabled` + `Table.rls_forced` in `catalog/assemble/tables.rs::build_tables`.

### Assembler — `crates/pgevolve-core/src/catalog/assemble/policies.rs`

Reads `pg_policies` rows, decodes each into `Policy`, and attaches to the matching table (by `(schema, table)` pair). For tables not in the managed schemas list, policies are silently dropped (matches how grants on filtered objects are handled).

The `using` / `with_check` text from PG is the canonicalized form (PG normalizes during catalog write). Wrap in `NormalizedExpr::from_canonical_text(...)` directly.

## Differ

In `crates/pgevolve-core/src/diff/policies.rs` (new), per-table:

```rust
pub fn diff_policies(
    target: &Table,
    source: &Table,
    out: &mut Vec<Change>,
) {
    // RLS toggle.
    if target.rls_enabled != source.rls_enabled {
        out.push(Change::SetTableRowSecurity {
            qname: target.qname.clone(),
            enable: source.rls_enabled,
        });
    }
    // FORCE toggle.
    if target.rls_forced != source.rls_forced {
        out.push(Change::SetTableForceRowSecurity {
            qname: target.qname.clone(),
            force: source.rls_forced,
        });
    }
    // Policies — pair by name.
    let target_map: BTreeMap<_, _> = target.policies.iter().map(|p| (&p.name, p)).collect();
    let source_map: BTreeMap<_, _> = source.policies.iter().map(|p| (&p.name, p)).collect();

    for (name, src_p) in &source_map {
        match target_map.get(name) {
            None => out.push(Change::CreatePolicy {
                table: source.qname.clone(),
                policy: (*src_p).clone(),
            }),
            Some(tgt_p) => {
                if tgt_p.command != src_p.command {
                    // PG can't ALTER command — must recreate.
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
```

All RLS ops are `Destructiveness::Safe`. Dropping a policy reveals data (not destroys it); the table reverts to whatever the surrounding RLS state allows. No intent gate needed.

## Render / emit

New `crates/pgevolve-core/src/plan/rewrite/policies.rs`:

- `create_policy(table_qname, &Policy) -> String` — full `CREATE POLICY ... ON ... [AS ...] [FOR ...] TO ... [USING (...)] [WITH CHECK (...)];`. **Always renders the explicit form** for all clauses (including `AS PERMISSIVE`, `FOR ALL`, and `TO PUBLIC` even when they match PG defaults). Reasons: catalog round-trip is byte-stable, fixture diffs are unambiguous, and human readers don't have to remember which defaults are implicit. The `USING` and `WITH CHECK` clauses are still omitted when absent on the policy (since `None` is meaningfully distinct from `Some(true_expr)`).
- `alter_policy(table_qname, &Policy)` — `ALTER POLICY name ON qname TO roles USING (...) WITH CHECK (...);` (only the alterable subset; PG doesn't allow ALTER POLICY to change the command — diff handles that via DROP+CREATE).
- `drop_policy(table_qname, name)` — `DROP POLICY name ON qname;`
- `enable_row_security(qname)` — `ALTER TABLE qname ENABLE ROW LEVEL SECURITY;`
- `disable_row_security(qname)` — `ALTER TABLE qname DISABLE ROW LEVEL SECURITY;`
- `force_row_security(qname)` — `ALTER TABLE qname FORCE ROW LEVEL SECURITY;`
- `no_force_row_security(qname)` — `ALTER TABLE qname NO FORCE ROW LEVEL SECURITY;`

5 new `StepKind` variants:

- `CreatePolicy`
- `DropPolicy`
- `AlterPolicy`
- `SetTableRowSecurity` (the Change carries an `enable: bool`; renderer picks ENABLE vs DISABLE)
- `SetTableForceRowSecurity` (carries `force: bool`)

All `transactional: InTransaction`, `destructive: false`.

## Lint rules

### Extend `grant-references-unknown-role`

The v0.3.1 lint at `crates/pgevolve-core/src/lint/rules/grant_references_unknown_role.rs` currently walks all grantable objects' grants + owners + default-privilege grantees. Extend it to also walk `cat.tables[*].policies[*].roles`:

```rust
// in grant_references_unknown_role::check
for t in &cat.tables {
    for p in &t.policies {
        for role_target in &p.roles {
            if let GrantTarget::Role(name) = role_target {
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

No new RULE_ID — the existing rule covers the new surface. The finding message distinguishes "policy P on table T" from "GRANT on X" via the object_label-style prefix.

### New rule — `force-rls-without-policies`

`crates/pgevolve-core/src/lint/rules/force_rls_without_policies.rs`:

```rust
pub const RULE_ID: &str = "force-rls-without-policies";

pub(crate) fn check(cat: &Catalog) -> Vec<Finding> {
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
```

PG behavior: FORCE + no policies = default-deny on every row (including for the owner). Almost always a mistake. Warning is right; waivable.

Wire into the source-tree-level dispatcher (`check_universal`, not `check_changeset`) since it operates on the source IR alone.

## Conformance fixtures

Under `crates/pgevolve-conformance/tests/cases/objects/policies/`:

1. `enable-rls/` — `ALTER TABLE app.users ENABLE ROW LEVEL SECURITY;` → 1 step.
2. `disable-rls/` — toggle off → 1 step.
3. `force-rls-toggle/` — `FORCE` then `NO FORCE` → 2 steps over 2 fixtures or 1 fixture with both transitions.
4. `simple-permissive-policy/` — `CREATE POLICY p ON t USING (current_user = author);`.
5. `restrictive-policy/` — `AS RESTRICTIVE` round-trips.
6. `policy-with-roles/` — `TO readers, writers` → 1 step preserving roles.
7. `policy-with-check/` — INSERT policy with `WITH CHECK`.
8. `alter-policy-roles/` — change `TO` clause only → single `ALTER POLICY` step.
9. `alter-policy-command-recreates/` — change `FOR SELECT` → `FOR INSERT` → DROP + CREATE (2 steps).
10. `drop-policy-on-source-removal/` — policy in catalog, not in source → DROP step.
11. `lint/force-without-policies/` — `FORCE` enabled, no policies → `force-rls-without-policies` warning fires.

All use `authoring = "objects"` mode (per-DB). The harness already supports `[expect.advisory]` from v0.3.0.

## Property test

Extend `arbitrary_table` to include `rls_enabled`, `rls_forced`, and `policies: Vec<Policy>` (0–3 policies per table). Each policy gets random name, permissive, command, 0–3 roles from a fixed pool, and synthetic `USING`/`WITH CHECK` text (e.g., `"true"` literal) to keep the search space small.

Run 10× per constitution §9.

## Documentation updates

- `docs/spec/objects.md:250` — move row from 📋 Planned → ✅ Supported.
- New `docs/spec/policies.md` — overview of the surface, command-vs-recreate semantics, force-rls warning, cross-cluster lint extension.
- `CHANGELOG.md` — new `[0.3.2]` section.

## Release shape

v0.3.2 — completes the v0.3 security/permissions trilogy.

After v0.3.2 ships, the natural next sub-spec is `CREATE STATISTICS` (issue #5), which is small and independent. The deferred v0.3.1 cluster-link conformance harness extension is tracked separately.

## Open questions resolved during brainstorming

- **Tight scope (RLS only, no harness work):** confirmed by user.
- **Cross-cluster lint approach:** extend `grant-references-unknown-role` (don't add a sibling rule).
- **Expression canonicalization:** `NormalizedExpr` — same path as check constraints.
- **Default TO clause:** parser canonicalizes omission to `vec![GrantTarget::Public]`.
- **Command change semantics:** PG can't `ALTER POLICY` the command kind — the differ emits DROP + CREATE.
- **`ALTER POLICY ... RENAME TO` in source:** rejected (drop + create model).

No remaining open questions.
