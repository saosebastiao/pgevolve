# Cluster surface — ROLE / CREATE USER (v0.3 first sub-spec)

**Status:** Design accepted 2026-05-21. First leg of the v0.3 security/permissions trilogy (roles → grants → RLS).
**Closes:** GitHub issue #2 (v0.3: implement ROLE / CREATE USER).
**Spec line touched:** `docs/spec/objects.md:248` (will move from 📋 Planned → ✅ Supported on the role row).
**Architectural reference:** `docs/superpowers/specs/2026-05-15-v0.2-architecture-review-design.md` §17 (Decision 23).

## Summary

Introduce pgevolve's **cluster-level surface** — a separate project type, separate CLI command family, separate executor running with a superuser DSN — and ship its first managed object: **roles**, with full attribute matrix and role-membership edges. Passwords are explicitly out of scope (set out-of-band).

This is the foundation for v0.3. GRANT/REVOKE on objects extends the per-DB Catalog; role membership GRANTs land here. RLS policies extend per-DB. Both follow-on sub-specs will assume the cluster scaffold exists.

## Scope

**In scope:**
- Cluster project layout (`pgevolve-cluster.toml`, `roles/` directory).
- `ClusterCatalog { roles }` IR (just roles, not yet tablespaces / cluster_settings / foreign_servers / etc.).
- Role IR with full PG attribute matrix.
- Role membership (`GRANT r TO target` and `CREATE ROLE … IN ROLE x` forms).
- Source parser, catalog reader, canon, differ, render, plan, apply, two lint rules.
- New CLI command family: `pgevolve cluster init/diff/plan/apply/status`.
- Six conformance fixtures.

**Explicitly out of scope:**
- Object-level GRANTs (`GRANT SELECT ON TABLE foo TO role`) — separate sub-spec.
- ALTER DEFAULT PRIVILEGES — separate sub-spec.
- RLS policies — separate sub-spec.
- Passwords — not stored in source; not managed (per spec note).
- Other cluster objects (tablespaces, cluster_settings/GUCs, foreign servers, user mappings, databases list) — separate sub-specs after roles ships.
- `pgevolve cluster lint` — universal lint dispatch lives in core; the two new rules ride on `check_changeset` for the cluster ChangeSet without a dedicated CLI subcommand.
- Cross-references from per-DB projects to roles (object ownership, grants) — lands when grants do.

User accepted both scope decisions during 2026-05-21 brainstorming.

## Project layout

```
my-cluster/
  pgevolve-cluster.toml       # connection config + project metadata; NO role content
  roles/
    app.sql                   # CREATE ROLE app_user; CREATE ROLE app_admin; ...
    ops.sql                   # CREATE ROLE oncall; GRANT oncall TO app_admin;
```

`roles/` files load alphabetically (matches the per-DB `objects/` convention). Order across files doesn't matter — the parser builds a complete `ClusterCatalog` after reading every file.

`pgevolve-cluster.toml` shape (minimum):

```toml
[project]
name = "my-cluster"

[connection]
dsn = "postgresql://superuser@localhost:5432/postgres"  # or env-var ref

[bootstrap]
# Roles that pgevolve treats as PG-owned and never diffs into existence or out.
# Defaults to ["postgres"]; override per-environment.
roles = ["postgres"]
```

A cluster project and per-DB projects coexist independently. No cross-references in this sub-spec.

## IR — `crates/pgevolve-core/src/ir/cluster/`

New module tree (parallel to existing `ir/`):

```
ir/cluster/
  catalog.rs       — ClusterCatalog
  role.rs          — Role, RoleAttributes
  mod.rs           — re-exports
```

```rust
pub struct ClusterCatalog {
    pub roles: Vec<Role>,
}

pub struct Role {
    pub name: Identifier,
    pub attributes: RoleAttributes,
    /// Roles this role is a member of (the `IN ROLE x` direction).
    /// Canon resolves both source forms (`CREATE ROLE r IN ROLE x` and
    /// `GRANT x TO r`) to the same `member_of` edge. The reverse is derivable.
    pub member_of: Vec<Identifier>,
    pub comment: Option<String>,
}

pub struct RoleAttributes {
    pub superuser:        bool,             // default false
    pub createdb:         bool,             // default false
    pub createrole:       bool,             // default false
    pub inherit:          bool,             // default true (PG default)
    pub login:            bool,             // default false; CREATE USER → LOGIN=true
    pub replication:      bool,             // default false
    pub bypass_rls:       bool,             // default false
    pub connection_limit: Option<i64>,      // None means unlimited (PG stores -1)
    pub valid_until:      Option<String>,   // RFC 3339; opaque to differ
}
```

**Membership direction.** Stored as `member_of` on each role (parents, not children). Rationale: matches the `CREATE ROLE r IN ROLE x` source form linearly; the catalog reader emits one edge per `pg_auth_members` row from the `member` side. The reverse listing (who-is-a-member-of-me) is derivable on demand and not part of the canonical IR.

**Passwords.** Not modeled. The catalog reader skips `rolpassword`. The source parser, on encountering `PASSWORD '...'` in `CREATE ROLE`/`ALTER ROLE`, emits a `ParseWarning` and proceeds without setting it (the plan won't touch passwords).

**Predefined roles.** Filtered at catalog-read time. Skip any role with `rolname LIKE 'pg\_%' ESCAPE '\\'`. Also skip the roles named in `pgevolve-cluster.toml`'s `[bootstrap].roles` list — defaults to `["postgres"]`.

## Source parser

Handles four statement kinds in `roles/*.sql`:

1. `CREATE ROLE r [WITH] [option…]`
2. `CREATE USER r [WITH] [option…]` — exactly `CREATE ROLE r WITH LOGIN`; the parser desugars
3. `ALTER ROLE r [option…]` — apply the listed options on top of whatever the source has already declared for `r` (alphabetical file order means later files can extend or override)
4. `GRANT role TO target` — adds `role` to `target.member_of`
5. `COMMENT ON ROLE r IS 'text'`

Attribute parsing covers the full PG option list (`SUPERUSER`/`NOSUPERUSER`, `CREATEDB`/`NOCREATEDB`, etc., `CONNECTION LIMIT n`, `VALID UNTIL 'ts'`).

**Rejected forms** (clear `ParseError`):
- `DROP ROLE` — drops happen via diff, never declared in source.
- `REVOKE role FROM target` — same.
- `GRANT … ON TABLE/SCHEMA/etc.` — object-level grants belong in per-DB sub-spec.
- `SET ROLE`/`RESET ROLE` — runtime, not source.

**Warned forms** (parse succeeds, content silently ignored):
- `PASSWORD '…'` clause anywhere — pgevolve never plans password changes; the spec note says "set out-of-band."
- `ENCRYPTED PASSWORD` — same.

The parser lives in `crates/pgevolve-core/src/parse/cluster/` (new module tree), invoked by a sibling of `parse_directory` named `parse_cluster_directory`.

## Catalog reader

New file `crates/pgevolve-core/src/catalog/cluster.rs`. Queries `pg_authid` and `pg_auth_members`:

```sql
-- roles
SELECT r.rolname,
       r.rolsuper, r.rolcreatedb, r.rolcreaterole, r.rolinherit,
       r.rolcanlogin, r.rolreplication, r.rolbypassrls,
       r.rolconnlimit, r.rolvaliduntil,
       d.description AS comment
FROM pg_authid r
LEFT JOIN pg_shdescription d
  ON d.objoid = r.oid AND d.classoid = 'pg_authid'::regclass
WHERE r.rolname NOT LIKE 'pg\_%' ESCAPE '\'
  AND r.rolname <> ALL($1::text[])  -- $1 = bootstrap.roles list

-- memberships
SELECT memb.rolname AS member, parent.rolname AS member_of
FROM pg_auth_members am
JOIN pg_authid memb   ON memb.oid = am.member
JOIN pg_authid parent ON parent.oid = am.roleid
WHERE memb.rolname NOT LIKE 'pg\_%' ESCAPE '\'
  AND parent.rolname NOT LIKE 'pg\_%' ESCAPE '\'
  AND memb.rolname <> ALL($1::text[])
  AND parent.rolname <> ALL($1::text[]);
```

`pg_authid` requires superuser. The `pgevolve-cluster.toml` `[connection]` block must point to a superuser DSN. If the connect succeeds but `pg_authid` denies access, surface a clear error.

Decode `rolconnlimit = -1 → None`, decode `rolvaliduntil` as RFC 3339 string, drop nulls/`infinity`.

## Canon

The only canon rule needed: **sort `member_of` lists deterministically** (lexicographic by role name). The reader emits them in PG's catalog order which is OID-dependent; canon normalizes.

No type-default stripping (unlike Stage 1 of v0.2.1) — every role attribute has a fixed PG default (false/true/-1), and the IR uses concrete bools / `Option<i64>` already.

## Differ

`crates/pgevolve-core/src/diff/cluster.rs` produces a `ClusterChangeSet`:

```rust
pub enum ClusterChange {
    CreateRole(Role),
    DropRole { name: Identifier, destructiveness: Destructiveness },
    AlterRoleAttributes {
        name: Identifier,
        from: RoleAttributes,
        to:   RoleAttributes,
    },
    GrantRoleMembership {
        role:   Identifier,   // the parent (member_of target)
        member: Identifier,
    },
    RevokeRoleMembership {
        role:   Identifier,
        member: Identifier,
    },
    CommentOnRole {
        name:    Identifier,
        comment: Option<String>,
    },
}
```

Pair-by-name on `roles`. `AlterRoleAttributes` compares the whole `RoleAttributes` block and emits one entry per role with the *full* new attribute set; render emits one `ALTER ROLE r WITH …` statement listing only changed attributes. Membership diff is per-edge.

**Destructiveness:**
- `DropRole`: `RequiresApprovalAndDataLossWarning` — dropping a role with grants in other DBs orphans those grants (PG will fail with a clear error if grants exist, but our intent gate makes it explicit).
- All others: `Safe`.

## Render / emit

`crates/pgevolve-core/src/plan/cluster_rewrite/sql.rs` — sibling to the per-DB `rewrite/sql.rs`. Helpers:

- `create_role(role)` — emits `CREATE ROLE r WITH <options>; ALTER ROLE r WITH NOLOGIN` etc., one statement.
- `drop_role(name)` — `DROP ROLE r;`
- `alter_role_attributes(name, only_changed_attrs)` — `ALTER ROLE r WITH <only changed>;`
- `grant_role_membership(role, member)` — `GRANT role TO member;`
- `revoke_role_membership(role, member)` — `REVOKE role FROM member;`
- `comment_on_role(name, comment)` — `COMMENT ON ROLE r IS '…';`

Emit handlers in `crates/pgevolve-core/src/plan/cluster_rewrite/emit.rs`. All steps are `InTransaction`-compatible (PG's role DDL is transactional).

`StepKind` gains: `CreateRole`, `DropRole`, `AlterRole`, `GrantRoleMembership`, `RevokeRoleMembership`, `CommentOnRole`.

## Lint rules

Two new rules under `lint/rules/`:

### `role-loses-superuser` (warning)
Fires when `AlterRoleAttributes` flips `superuser: true → false`. Rationale: losing superuser is rarely a routine config change; usually intentional but worth surfacing.

### `role-membership-cycle` (error)
Fires when a `GrantRoleMembership { role: a, member: b }` would create a cycle (i.e., the projected post-apply membership graph contains `b → … → a → b`). PG rejects cycles at apply time; catching pre-plan saves a round trip and produces a better error message.

Both register through a new `check_cluster_changeset(&ClusterChangeSet) -> Vec<Finding>` dispatcher in `lint/universal.rs`, sibling to the existing `check_changeset`.

## CLI

New command family in `crates/pgevolve/src/commands/cluster/`:

- `pgevolve cluster init [path]` — scaffold a new cluster project (creates `pgevolve-cluster.toml`, `roles/` empty dir, `.gitignore`).
- `pgevolve cluster diff` — IR vs. live cluster (read-only).
- `pgevolve cluster plan` — write `cluster-plans/<plan_id>/{plan.sql, intent.toml, manifest.toml}`. Same plan-id hashing scheme; cluster plans are stored in a separate `cluster-plans/` directory so cluster and per-DB plans never collide.
- `pgevolve cluster apply [<plan_id>]` — execute. Loads connection from `pgevolve-cluster.toml`. Reuses the per-step transactional runner from per-DB `executor/`.
- `pgevolve cluster status` — list applied/pending cluster plans.

Per-DB `pgevolve apply` does not touch cluster state. `pgevolve cluster apply` does not touch per-DB schema state. Strict separation per Decision 23.

`pgevolve cluster lint` is deferred — universal cluster lints surface in `cluster plan` output (advisory findings via the new dispatcher), matching how `check_changeset` surfaces v0.2.1's lint findings.

## Plan format

`cluster-plans/<plan_id>/` is structurally identical to per-DB `plans/<plan_id>/`:
- `plan.sql` — concatenated DDL steps
- `intent.toml` — destructive-step approval gates
- `manifest.toml` — plan metadata (id, source hash, target catalog hash, generated_at, etc.)

Plan-id hash uses the same BLAKE3 scheme over the canonical IR (which for cluster plans is `ClusterCatalog`).

## Conformance

New fixture root: `crates/pgevolve-conformance/tests/cases/cluster/roles/`.

Six fixtures:

1. **`create-simple-role/`** — `CREATE ROLE app_user` with defaults. Verifies one safe `CREATE ROLE` step.
2. **`create-login-user/`** — `CREATE USER app_user` (sugar form) renders as `CREATE ROLE app_user WITH LOGIN` after canon.
3. **`alter-role-attributes/`** — before: `CREATE ROLE r;` (defaults); after: `CREATE ROLE r WITH CREATEDB CONNECTION LIMIT 50;`. Verifies one `ALTER ROLE` with only the changed attrs.
4. **`add-membership/`** — before: two roles, no membership; after: `GRANT readers TO app_user;` added. One `GRANT … TO …` step.
5. **`drop-role-intent-gated/`** — role removed from source. Plan has one `DROP ROLE` step gated behind intent approval.
6. **`comment-on-role/`** — `COMMENT ON ROLE` round-trips.

The conformance harness needs minor extension to support cluster fixtures (new `authoring = "cluster"` mode in `fixture.toml`). The dispatcher in `tests/run.rs` adds a `run_cluster` function paralleling `run_objects`.

## Property test addition

Extend testkit with `arbitrary_role()` and `arbitrary_cluster_catalog()`. The diff round-trip property tests cover the new IR. Run 10× per constitution §9.

## Documentation updates

- `docs/spec/objects.md` row at line 248 → ✅ Supported. Update notes to reflect what's actually modeled.
- `CHANGELOG.md` → new `[0.3.0]` section. Each trilogy sub-spec ships its own minor release (roles → `v0.3.0`, grants → `v0.3.1`, RLS → `v0.3.2`), matching the v0.2.x pattern (v0.2.0 = architecture, v0.2.1 = TOAST). Avoids a multi-month `[Unreleased]` block and lets each capability ship independently.
- New `docs/spec/cluster.md` — overview of the cluster surface (what it is, what it manages, what's deferred).

## Non-goals reconfirmed

- No password management. Set passwords with `psql` or your secrets manager.
- No per-DB grants in this sub-spec. They land in the next one.
- No tablespaces, GUCs, foreign servers, user mappings — separate sub-specs.
- No automatic detection of per-DB grant orphans on `DROP ROLE`. The intent gate makes the operator responsible; PG's apply-time error is the safety net.

## Open questions resolved during brainstorming

- **Membership direction:** `member_of` only. Reverse is derivable.
- **Bundle lints now:** yes (role-loses-superuser + role-membership-cycle).
- **Bootstrap role filtering:** yes, configurable in `pgevolve-cluster.toml [bootstrap].roles`, defaults to `["postgres"]`.

No remaining open questions.
