---
status: design
target: v0.4.0
sub_spec: cluster-tablespace
---

# `TABLESPACE` (cluster object) — design

Adds the cluster-level `TABLESPACE` object, a v0.4.0 roadmap row. This
reverses the previous "out of scope" stance in
[`objects.md`](../../spec/objects.md): `CREATE TABLESPACE` was held out as a
cluster-admin object outside the schema-management remit, but the
`pgevolve cluster …` surface already manages cluster-level objects (roles),
and tablespaces fit the same model. **Only the SQL object is managed** —
filesystem-layout concerns (creating the directory, mount points, backup
relocation, validating the path exists on disk) stay out of scope.

Tablespaces are modeled on the existing cluster `Role`
(`crates/pgevolve-core/src/ir/cluster/`): a top-level `ClusterCatalog`
collection, parsed from cluster source files, introspected from
`pg_tablespace`, diffed into `ClusterChange` entries, rendered as flat
in-transaction steps. They differ from roles in three ways that drive the
design: they carry an **owner** (`pg_tablespace.spcowner`), their
**`LOCATION` is immutable** (Postgres has no `ALTER TABLESPACE … SET
LOCATION`), and they have a **filesystem dependency** (the location
directory must exist, be empty, and be owned by the Postgres OS user).

Brainstorming decisions:
- **Location drift → lint, never a change.** A live tablespace whose path
  differs from source emits an advisory finding; pgevolve never moves it
  (you cannot relocate a tablespace's files without emptying it and
  recreating, which is destructive and usually impossible while objects
  use it).
- **No first-class rename.** Match by name (the role convention); a renamed
  tablespace reads as drop-old + create-new, governed by the drop policy.
- **Owner and options are lenient.** Source `owner = None` never emits an
  owner change; only options the source declares are `SET` — live-only
  options are not `RESET`.
- **Conformance/test LOCATIONs** are provided by a harness helper that
  creates an empty, postgres-owned directory inside the ephemeral container
  and substitutes a `${TABLESPACE_DIR}` placeholder in fixture SQL.

`DROP TABLESPACE` is destructive and intent-gated (mirroring `DropRole`),
and — like every other source-side object — a `DROP` written in source is
rejected; drops only arise from the diff.

---

## §1. IR

New module `crates/pgevolve-core/src/ir/cluster/tablespace.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct Tablespace {
    /// Cluster-global tablespace name.
    pub name: Identifier,
    /// The `LOCATION '/path'` directory. Immutable in Postgres — a change
    /// here is surfaced as a lint, never an ALTER (see §5).
    #[diff(via_debug)]
    pub location: String,
    /// Owner (`pg_tablespace.spcowner`). Lenient: `None` = unmanaged.
    #[diff(via_debug)]
    pub owner: Option<Identifier>,
    /// Tablespace options (`seq_page_cost`, `random_page_cost`,
    /// `effective_io_concurrency`, `maintenance_io_concurrency`). Sorted by
    /// key (BTreeMap). Lenient: only source-declared options are managed.
    #[diff(via_debug)]
    pub options: BTreeMap<String, String>,
    /// Optional comment (`pg_shdescription`).
    #[diff(via_debug)]
    pub comment: Option<String>,
}
```

`ClusterCatalog` (`ir/cluster/catalog.rs`) gains
`pub tablespaces: Vec<Tablespace>`. Canon (`ir/canon/cluster.rs`) sorts
tablespaces by `name` and rejects duplicate names (mirroring roles).
`DiffMacro` is used as on `Role` so field-level diffs are precise.

## §2. Source layout + parser

Tablespaces are declared in a `tablespaces/` directory parallel to the
existing `roles/` cluster-source directory, configured via a new
`ClusterConfig.tablespaces_dir` field (defaulting to `tablespaces`). The
cluster parser (`parse/cluster/`) gains a `create_tablespace` builder and
an `alter_tablespace` builder, dispatched from the cluster `apply_file`
router on `CreateTableSpaceStmt` / `AlterTableSpaceOptionsStmt` /
`AlterTableSpaceOwnerStmt` / `CommentStmt(OBJECT_TABLESPACE)`. Supported
source forms:

- `CREATE TABLESPACE name [OWNER role] LOCATION '/path' [WITH (option = value, …)]`
- `ALTER TABLESPACE name OWNER TO role`
- `ALTER TABLESPACE name SET (option = value, …)` (and `RESET` folded into the option map)
- `COMMENT ON TABLESPACE name IS '…'`

`DROP TABLESPACE` and `ALTER TABLESPACE … RENAME TO` in source are rejected
with a `ParseError` (drops come from the diff; rename is not modeled —
mirrors the role/event-trigger policy). The parser folds CREATE + ALTER
statements into one `Tablespace` per name (the role accumulator pattern).

## §3. Catalog reader

Extend `catalog/cluster.rs` + `catalog/queries/cluster.rs` with a
`CLUSTER_TABLESPACES_QUERY` over `pg_tablespace`:

- `spcname` → `name`; `pg_tablespace_location(oid)` → `location` (the
  function returns the path safely; empty for the built-ins);
  `pg_authid.rolname` (via `spcowner`) → `owner`; `spcoptions` (text[] of
  `key=value`) → `options` map; `pg_shdescription` → `comment`.
- **Exclude the built-in tablespaces** `pg_default` and `pg_global` (they
  are not user-managed) — `WHERE spcname NOT IN ('pg_default','pg_global')`.

Read into `ClusterCatalog.tablespaces` in `read_cluster_catalog`, then
canonicalize.

## §4. Diff

`ClusterChange` (`diff/cluster.rs`) gains:

```rust
CreateTablespace(Tablespace),
DropTablespace { name: Identifier },
AlterTablespaceOwner { name: Identifier, owner: Identifier },
SetTablespaceOptions { name: Identifier, options: BTreeMap<String, String> },
CommentOnTablespace { name: Identifier, comment: Option<String> },
```

`diff_cluster` pairs tablespaces by name:
- **source-only** → `CreateTablespace` (Safe).
- **live-only** → `DropTablespace` (`RequiresApprovalAndDataLossWarning`,
  reason "drops tablespace … — objects using it will fail"), intent-gated
  exactly like `DropRole`.
- **both present:**
  - `location` differs → **no change**; emit a `tablespace-location-drift`
    advisory finding (§6) instead. Postgres cannot relocate a tablespace.
  - `owner` differs **and source declares one** (`Some`) → `AlterTablespaceOwner`.
  - `options` — for each option in source whose value differs from live (or
    is absent in live), include it in a single `SetTablespaceOptions`
    (lenient: live-only options are not reset).
  - `comment` differs → `CommentOnTablespace`.

## §5. Render + plan

`plan/cluster_rewrite/sql.rs` gains:
- `create_tablespace(ts)` → `CREATE TABLESPACE name [OWNER role] LOCATION '<path>' [WITH (k = v, …)];` (options sorted; path single-quote-escaped).
- `drop_tablespace(name)` → `DROP TABLESPACE name;`
- `alter_tablespace_owner(name, owner)` → `ALTER TABLESPACE name OWNER TO owner;`
- `alter_tablespace_set(name, options)` → `ALTER TABLESPACE name SET (k = v, …);`
- `comment_on_tablespace(name, comment)` → `COMMENT ON TABLESPACE name IS '…';` / `IS NULL;`

New `StepKind` variants (`CreateTablespace`, `DropTablespace`,
`AlterTablespaceOwner`, `SetTablespaceOptions`, `CommentOnTablespace`).
Cluster ops remain flat `InTransaction` steps (no dependency graph, like
roles). `create_tablespace` follow-ups: CREATE sets owner inline via
`OWNER`, so no separate owner step is needed on create; options ride inline
in the `WITH (…)`; comment, if any, is a follow-up `COMMENT ON` step.

Cluster ops run before per-DB ops in the apply pipeline, so a table whose
`TABLESPACE` attribute names a managed tablespace finds it already created.

## §6. Lint

New rule `tablespace-location-drift` (cluster lint, alongside the existing
role lints in `lint/`): when a tablespace exists in both live and source
but the `LOCATION` differs, emit an advisory `Finding` ("tablespace `name`
location differs: live=`/a`, source=`/b` — pgevolve does not relocate
tablespaces; recreate manually if intended"). Informational, never blocks.

## §7. Tests

- **Conformance** under `crates/pgevolve-conformance/tests/cases/cluster/tablespaces/`:
  `create-simple`, `create-with-options`, `alter-owner`, `alter-set-option`,
  `comment-on`, `drop-intent` (drop requires approval), and
  `location-drift-lint` (location mismatch → 0 steps + the lint finding).
  Fixtures use a `${TABLESPACE_DIR}` placeholder; a new conformance harness
  helper creates an empty postgres-owned directory inside the ephemeral
  container and substitutes the real path before applying `before.sql`.
- **E2E**: extend `crates/pgevolve/tests/cluster_apply_e2e.rs` (or a new
  sibling) with a tablespace round-trip — create a temp dir, apply a
  cluster plan that creates a tablespace, verify it appears in
  `pg_tablespace`.

## §8. Out of scope / non-goals

- Filesystem management: creating/validating the `LOCATION` directory,
  mount points, backup relocation. The user owns the directory; pgevolve
  emits only the SQL.
- `ALTER TABLESPACE … RENAME TO` as a first-class change (name is identity).
- Relocating a tablespace on `LOCATION` drift (lint only — §6).
- Per-partition `TABLESPACE` overrides — a separate roadmap row.
- Resetting live-only tablespace options (lenient option management — §4).
