# Object grants + ownership + default privileges (v0.3.1)

**Status:** Design accepted 2026-05-22. Second leg of the v0.3 security/permissions trilogy.
**Closes:** GitHub issue #3 (v0.3: implement GRANT / REVOKE / ALTER DEFAULT PRIVILEGES).
**Spec line touched:** `docs/spec/objects.md:249` (will move from 📋 Planned → ✅ Supported).
**Depends on:** v0.3.0 (cluster surface + roles). GRANTs target roles; roles must exist as a managed concept before grants can reference them.

## Summary

Manage Postgres object permissions declaratively. Each grantable object kind gains optional `owner: Option<Identifier>` and required `grants: Vec<Grant>` fields. A new top-level `default_privileges` collection on `Catalog` handles `ALTER DEFAULT PRIVILEGES`. The differ produces minimal GRANT/REVOKE/ALTER OWNER sequences. Drift to roles outside the source is *tolerated* (lenient policy) and surfaces as a lint warning. An optional `[cluster]` block in `pgevolve.toml` links a per-DB project to a cluster project so grantee role names can be cross-checked at plan time.

## Scope

**In scope:**

- 8 grantable object types gain `owner: Option<Identifier>` and `grants: Vec<Grant>`:
  - `Schema`, `Sequence`, `Table` (including partitioned + partitions), `View`, `MaterializedView`, `Function`, `Procedure`, `UserType` (composite, enum, domain).
- New shared IR module `ir::grant` with `Grant`, `Privilege`, `GrantTarget`.
- Column-level grants on `Table` / `View` / `MaterializedView` via `Grant.columns: Option<Vec<Identifier>>`.
- New top-level `Catalog.default_privileges: Vec<DefaultPrivilegeRule>` mirroring `pg_default_acl`.
- Source-parser additions: `GRANT`, `REVOKE` (rejected in source — see below), `ALTER ... OWNER TO`, `ALTER DEFAULT PRIVILEGES ... GRANT/REVOKE`.
- Catalog-reader additions: decode `*acl::text[]` arrays and the 4 `*owner` columns; pull `pg_default_acl`.
- Differ: minimal GRANT/REVOKE/ALTER OWNER step sequences; lenient drift policy.
- Render/emit: six new step kinds covering the cross product of (grant/revoke × object-level/column-level), plus owner change and default-privileges DDL.
- Lint rules: `grants-to-unmanaged-role` (warning), `revoke-from-owner` (error).
- Cross-cluster role verification (optional, via `[cluster]` block in `pgevolve.toml`).
- Conformance: ~10 fixtures spanning the grantable object types.

**Explicitly out of scope:**

- `DATABASE`, `TABLESPACE`, `LANGUAGE`, `FOREIGN TABLE`, `LARGE OBJECT` grants — cluster-level or unmanaged.
- Row-level security policies — separate sub-spec (v0.3.2).
- `GRANT ALL` round-tripping verbatim — parser accepts it; canon expands to the explicit privilege list.
- Predefined `pg_*` roles as grantees — allowed; the `grants-to-unmanaged-role` lint will warn about them since they're not in the cluster source either. Users can waive.
- Cluster-level role membership GRANTs — those are v0.3.0 territory.

User accepted all three scope decisions during 2026-05-22 brainstorming.

## Project-level config — `[cluster]` block in `pgevolve.toml`

New optional section:

```toml
[cluster]
# Path to the cluster project this DB belongs to. Relative paths resolve
# against pgevolve.toml's directory. When set, lint cross-checks that
# grantee role names appear in cluster_root/roles/*.sql. When absent, no
# cross-check (per-DB independence is preserved).
project = "../my-cluster"
```

`[cluster].project = "../my-cluster"` makes the per-DB project lint-aware of the cluster project at the given path. Apply still uses the per-DB connection DSN — no superuser escalation, no cluster apply triggered from per-DB apply.

## Common IR — `crates/pgevolve-core/src/ir/grant.rs`

New module:

```rust
/// One ACL entry on a grantable object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub grantee: GrantTarget,
    pub privilege: Privilege,
    /// `WITH GRANT OPTION` flag. PG bookkeeping. Default false.
    pub with_grant_option: bool,
    /// Column-level grants. `None` = object-level. `Some(cols)` = only those columns.
    /// Only meaningful on `Table` / `View` / `MaterializedView`; canon rejects
    /// `Some(_)` on other object kinds.
    pub columns: Option<Vec<Identifier>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantTarget {
    Role(Identifier),
    Public,                              // GRANT ... TO PUBLIC
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Privilege {
    Select, Insert, Update, Delete, Truncate, References, Trigger,  // tables/views/MVs
    Usage,                                                          // schemas, sequences, types
    Execute,                                                        // functions, procedures
    Create,                                                         // schemas (CREATE in schema)
    // Database-level (CONNECT, TEMPORARY) and cluster-level (SET, ALTER SYSTEM) NOT modeled.
}
```

`Grant` has a canonical order: `(grantee, privilege, columns)` sorted via `Ord` on a tuple. Canon enforces this — same input always produces the same sorted list.

## Ownership: `owner: Option<Identifier>` on every grantable object

Added to `Schema`, `Sequence`, `Table`, `View`, `MaterializedView`, `Function`, `Procedure`, `UserType`. All `#[diff(via_debug)]`.

**Semantics:**

- `None` means "unmanaged" — the differ ignores ownership for this object regardless of catalog state.
- `Some(role)` means "managed" — diff emits `ALTER ... OWNER TO role` if catalog disagrees.

**Source declaration:** Users opt in per-object by writing standalone `ALTER TABLE app.t OWNER TO app_owner;` (or equivalent for other object kinds). The parser updates the prior-declared object's `owner` field. Omitting it leaves `owner: None`.

This matches the pattern users expect from Terraform/Atlas-style tools: explicit ownership is opt-in; absence means "don't touch what's there."

## Per-object grants

Each grantable IR struct gains `pub grants: Vec<Grant>` (always present, default empty). `#[diff(via_debug)]` for coarse diff path; the differ computes set differences directly.

**Canon:**

- Sort grants by `(grantee_key, privilege, columns)`. `grantee_key` orders `Public` < `Role(name lex order)`.
- Deduplicate: same `(grantee, privilege, columns)` collapsed to one entry; if any duplicate has `with_grant_option = true`, the survivor inherits `true`.
- For column-grants, sort the column list.

## Default privileges — `Catalog.default_privileges`

```rust
// in ir::catalog::Catalog
pub default_privileges: Vec<DefaultPrivilegeRule>,

pub struct DefaultPrivilegeRule {
    /// `FOR ROLE x` — whose future objects this applies to.
    pub target_role: Identifier,
    /// `IN SCHEMA y` — scope. `None` = "all schemas owned by `target_role`".
    pub schema: Option<Identifier>,
    pub object_type: DefaultPrivObjectType,
    pub grants: Vec<Grant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultPrivObjectType {
    Tables,       // includes views + MVs in PG's grouping
    Sequences,
    Functions,    // PG-side keyword is FUNCTIONS but covers procedures too
    Types,
    Schemas,      // PG 14+
}
```

Canon sorts default-privilege rules by `(target_role, schema, object_type)` and sorts grants within each.

## Source parser additions

Each grantable family's existing parser dispatch (`parse/builder/`) gains four kinds of statement:

1. **`GRANT priv [, priv]* ON object TO grantee [, grantee]* [WITH GRANT OPTION]`** — updates `grants` on the named object. Multi-privilege and multi-grantee forms expand to the cross-product entries.

2. **`ALTER <objtype> name OWNER TO role`** — updates the named object's `owner` field. The grammar for each object family already supports this; we just need to handle the `AlterTableStmt::AlterOwner` (and equivalents).

3. **`GRANT priv (col, col) ON TABLE t TO grantee`** — column-level grant. Sets `Grant.columns = Some(...)` and only valid for table-like objects.

4. **`ALTER DEFAULT PRIVILEGES [FOR ROLE x] [IN SCHEMA y] GRANT ... TO z`** — appends to `Catalog.default_privileges`.

**Rejected in source** (with clear `ParseError`):

- `REVOKE` of any flavor — revokes come from diff, not source. Consistent with cluster-roles convention.
- `GRANT` on objects pgevolve doesn't manage (DATABASE, TABLESPACE, LANGUAGE, FOREIGN TABLE).
- `GRANT` with grantor specified (`GRANTED BY x`) — unmodeled; warn-and-ignore.

**Special handling:**

- `GRANT ALL ON object TO role` — parser expands to the full privilege list applicable to the object type. Round-trips through canon.
- `GRANT ON SCHEMA pgevolve_internal …` (or any reserved schema) — rejected as a `ParseError`.

## Catalog reader additions

### ACL decoding (`crates/pgevolve-core/src/catalog/grants.rs` — new)

PG's ACL columns (`relacl`, `nspacl`, etc.) are `aclitem[]`. Each `aclitem` text form is `grantee=privileges/grantor` (e.g., `foo=arwd*/owner` — `*` flags `WITH GRANT OPTION`). Decoded shape:

```rust
pub(crate) fn decode_aclitem_array(raw: &[String]) -> Result<Vec<Grant>, CatalogError> { ... }
```

Implementation: parse each aclitem text, map PG privilege letters (`r` → Select, `w` → Update, `a` → Insert, `d` → Delete, `D` → Truncate, `x` → References, `t` → Trigger, `X` → Execute, `U` → Usage, `C` → Create, …) to our `Privilege` enum. Implicit owner-grants (rows where grantee == grantor with all privileges) are emitted as-is — canon won't dedupe owner self-grants because they're meaningful for round-trip.

**Column-level grants:** for tables/views/MVs, query `pg_attribute.attacl::text[]` per column. Each non-null entry produces one `Grant { columns: Some(vec![colname]), ... }`. Canon then groups grants by `(grantee, privilege, with_grant_option)` and merges every group's column lists into a single `Grant` with `columns: Some(sorted_union)`. Object-level grants (`columns: None`) are never merged with column-level grants.

### Owner columns

Existing per-family queries gain a join to `pg_authid` for the owner. Decoded into the new `owner: Option<Identifier>` field as `Some(name)` (never `None` from catalog — always populated).

### Default privileges

New query against `pg_default_acl`:

```sql
SELECT a.rolname AS target_role, n.nspname AS schema_name,
       d.defaclobjtype AS object_type, d.defaclacl::text[] AS acl
FROM pg_default_acl d
JOIN pg_authid a ON a.oid = d.defaclrole
LEFT JOIN pg_namespace n ON n.oid = d.defaclnamespace
WHERE a.rolname NOT LIKE 'pg\_%' ESCAPE '\'
```

Decoded with the same `decode_aclitem_array` helper. `defaclobjtype` is a char: `r` Tables, `S` Sequences, `f` Functions, `T` Types, `n` Schemas.

### Canon — owner round-trip when source is `None`

When source `owner = None` (unmanaged), the differ ignores the field. But canon sees both source IR and catalog IR. We need both sides to compare equal even though catalog always has a value. Solution: the **differ** handles the asymmetry, not canon. Canon leaves both sides as-read; the differ skips the owner check when `source.owner.is_none()`.

## Differ

For each grantable object that exists on both sides:

1. **Owner:** if `source.owner.is_some()` and `source.owner != target.owner` → `AlterObjectOwner { object_kind, qname, from, to }`. If `source.owner.is_none()`, skip.
2. **Grants:** set-diff between `source.grants` and the **managed subset of `target.grants`**:
   - Managed subset = grants whose grantee is in `source_managed_roles`, where `source_managed_roles` is the set of all role names mentioned in this `Catalog`'s grant lists, owner fields, and `default_privileges`.
   - In source not in managed-catalog → `GrantObjectPrivilege` (or `GrantColumnPrivilege` if `columns: Some`).
   - In managed-catalog not in source → `RevokeObjectPrivilege` (or `RevokeColumnPrivilege`).
   - In catalog but grantee unmanaged → accumulate for `grants-to-unmanaged-role` lint; do not emit any DDL.

3. **Default privileges:** pair-by-`(target_role, schema, object_type)`. Per-pair set-diff grants. Cross-managed-role filter applies as above.

**Destructiveness:** all grant/revoke/owner-change ops are `Destructiveness::Safe`. Revoke doesn't lose data, just access; ownership changes are catalog-only.

## Render / emit

Six new `StepKind` variants:

- `AlterObjectOwner` — `ALTER <objkind> qname OWNER TO role;`
- `GrantObjectPrivilege` — `GRANT priv ON <objkind> qname TO grantee [WITH GRANT OPTION];`
- `RevokeObjectPrivilege` — `REVOKE priv ON <objkind> qname FROM grantee;`
- `GrantColumnPrivilege` — `GRANT priv (col1, col2) ON TABLE qname TO grantee [WITH GRANT OPTION];`
- `RevokeColumnPrivilege` — `REVOKE priv (col1, col2) ON TABLE qname FROM grantee;`
- `AlterDefaultPrivileges` — `ALTER DEFAULT PRIVILEGES FOR ROLE x [IN SCHEMA y] {GRANT|REVOKE} priv ON {TABLES|SEQUENCES|FUNCTIONS|TYPES|SCHEMAS} {TO|FROM} grantee;`

All `transactional: InTransaction`. SQL helpers live in a new `plan/rewrite/grants.rs`.

The `<objkind>` token in SQL output is `TABLE`, `SCHEMA`, `SEQUENCE`, `VIEW`, `MATERIALIZED VIEW`, `FUNCTION`, `PROCEDURE`, or `TYPE` per the object's IR family. PG's GRANT syntax accepts these uniformly.

**Special case for functions/procedures:** GRANT on these needs the argument signature: `GRANT EXECUTE ON FUNCTION app.foo(int, text) TO role`. Renderer pulls signature from the IR.

## Lint rules

### `grants-to-unmanaged-role` (warning, waivable)

Fires when the catalog has a grant whose grantee is not in `source_managed_roles`. Surfaces the role name and the object so operators can decide whether to:
- (a) add the role to source — bring it under management
- (b) waive — accept the out-of-band grant as intentional

Severity: `Warning`. Standard `@pgevolve waive` directive applies.

### `revoke-from-owner` (error, non-waivable)

Fires when the diff would emit a `RevokeObjectPrivilege` whose grantee equals the object's owner. PG silently rejects (owner has implicit privileges); we pre-empt with a clear error.

Severity: `Error`. Plan construction fails until the source's grant list is corrected.

### `grant-references-unknown-role` (error, when `[cluster].project` is set)

Cross-cluster check. Fires when a grantee in source isn't defined in the linked cluster project's `roles/*.sql`. Only runs when `[cluster].project` is present; otherwise silently skipped.

Severity: `Error`. Catches typos and missing role declarations before apply.

## Cross-cluster role verification

When `[cluster].project = "../my-cluster"` is set in `pgevolve.toml`:

1. At plan time, after parsing the per-DB Catalog, **also parse the linked cluster project's `roles/`** via `parse_cluster_directory`.
2. Build a `HashSet<Identifier>` of all role names declared in the cluster source.
3. The `grant-references-unknown-role` rule checks every grantee in source against this set.

Cluster project loading is read-only: pgevolve does NOT diff or apply anything from the cluster project during per-DB operations. It's source-of-truth-for-role-names only.

## Conformance fixtures

New fixture root: `crates/pgevolve-conformance/tests/cases/objects/grants/` plus extensions to existing per-family fixtures.

- `grants/table/grant-select` — basic table grant.
- `grants/table/revoke-on-drop-from-source` — drop the grant in source → REVOKE step.
- `grants/table/column-level-grant` — column-level GRANT round-trips.
- `grants/table/grant-all-expands` — `GRANT ALL` expands via canon.
- `grants/schema/grant-usage-and-create` — USAGE + CREATE on schema.
- `grants/function/grant-execute-with-signature` — function GRANT with argument types.
- `grants/sequence/grant-usage` — sequence USAGE.
- `grants/owner/alter-owner-emits-one-step` — single ALTER OWNER step.
- `grants/owner/unmanaged-owner-skipped` — source.owner = None, no diff.
- `grants/default-privs/in-schema-tables` — `ALTER DEFAULT PRIVILEGES IN SCHEMA app GRANT SELECT ON TABLES TO readers`.
- `grants/default-privs/global-functions` — global default privs for functions.
- `grants/lint/grants-to-unmanaged-role` — catalog has a grant to a role not in source; warning fires.
- `grants/lint/revoke-from-owner-error` — source omits an owner-grant; error fires.
- `grants/cluster-link/role-mention-validated` — `[cluster].project` set; valid role passes.
- `grants/cluster-link/role-mention-rejected` — `[cluster].project` set; missing role → error.

(15 fixtures total — comparable to v0.3.0's 7 cluster fixtures + the per-family grants spread across 8 object types.)

## Documentation updates

- `docs/spec/objects.md:249` — move row from 📋 Planned → ✅ Supported.
- New `docs/spec/grants.md` — overview of the grants/ownership surface, drift policy, cluster-link option.
- `docs/spec/cluster.md` — add a cross-reference: "Per-DB projects can lint-check grantee role names against this cluster project by setting `[cluster].project`."
- `CHANGELOG.md` — new `[0.3.1]` section.

## Release shape

v0.3.1 — one minor bump after v0.3.0. Aligns with the trilogy convention (each sub-spec ships its own minor; roles → 0.3.0, grants → 0.3.1, RLS → 0.3.2).

## Non-goals reconfirmed

- No automatic owner inference from `current_user`. Source must declare explicitly.
- No grant-grantor (`GRANTED BY`) modeling. PG tracks it; pgevolve treats grantor as "whoever runs the apply."
- No row-level security policies. v0.3.2.
- No retroactive revoke when a role is dropped from source. The drop-role flow in cluster apply handles its own cleanup; per-DB diffs only touch grants for managed roles.

## Open questions resolved during brainstorming

- **Owner is `Option<Identifier>`:** lets projects opt in per-object. Required ownership would break existing source files.
- **Drift policy is lenient + lint warning:** never silently revokes someone else's grants.
- **Cluster-link is opt-in via `[cluster].project`:** preserves per-DB independence as the default; provides a cleaner UX when configured.
- **`revoke-from-owner` is error severity:** prevents a no-op DDL that PG silently rejects; a clearer signal than waiting for `cargo run -- apply` to fail.

No remaining open questions.
