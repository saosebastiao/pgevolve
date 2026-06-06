# TABLESPACE (cluster object) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the cluster-level `TABLESPACE` object (CREATE / ALTER OWNER / ALTER SET options / DROP / COMMENT), managed via the `pgevolve cluster …` surface.

**Architecture:** Modeled on the cluster `Role`: a `Tablespace` IR on `ClusterCatalog`, parsed from a `tablespaces/` source dir through the existing cluster statement router, introspected from `pg_tablespace`, diffed into `ClusterChange` variants, rendered as flat in-transaction steps. Location is immutable (drift → lint, never a change); owner and options are lenient; drop is intent-gated; no first-class rename. Filesystem layout stays out of scope — only the SQL object.

**Tech Stack:** Rust, `pg_query` (libpg_query), `pg_catalog` introspection, the in-repo cluster plan/apply + conformance harness.

**Design:** [`docs/superpowers/specs/2026-06-05-tablespace-design.md`](../specs/2026-06-05-tablespace-design.md)

**Closest template — copy its shape at every layer:** the cluster `Role` (`ir/cluster/role.rs`, `parse/cluster/create_role.rs`, `catalog/queries/cluster.rs`, `diff/cluster.rs`, `plan/cluster_rewrite/sql.rs`).

---

## Key verified facts (do not re-derive)
- pg_query nodes (pg_query 6.1.1): `CreateTableSpaceStmt { tablespacename: String, owner: Option<RoleSpec>, location: String, options: Vec<Node> }`, `AlterTableSpaceOptionsStmt { tablespacename: String, options: Vec<Node>, is_reset: bool }`, `DropTableSpaceStmt`, `AlterOwnerStmt { object_type, newowner }`, `RenameStmt`/`CommentStmt` with `ObjectType::ObjectTablespace`.
- Reader: `pg_tablespace.spclocation` does NOT exist (removed PG 9.2). Use `pg_tablespace_location(oid)` for the path and `spcoptions` (text[] of `key=value`, NULL when none) for options; owner via `pg_get_userbyid(spcowner)`. Exclude built-ins `pg_default`, `pg_global`.
- `CREATE TABLESPACE` requires the LOCATION directory to already exist (empty, postgres-owned). In tests, provision it over the superuser SQL connection with `COPY (SELECT 1) TO PROGRAM 'mkdir -p <dir> && chmod 700 <dir>'` (verified working) — no container exec needed.
- Cluster render is flat `InTransaction` steps (no dep-graph). `DROP ROLE`/`DROP TABLESPACE` in source are rejected at parse; drops come from the diff.

---

## Task 1: IR — `Tablespace` + `ClusterCatalog.tablespaces` + canon

**Files:**
- Create: `crates/pgevolve-core/src/ir/cluster/tablespace.rs`
- Modify: `crates/pgevolve-core/src/ir/cluster/mod.rs` (`pub mod tablespace;`)
- Modify: `crates/pgevolve-core/src/ir/cluster/catalog.rs` (add field; update `ClusterCatalog::empty()`/Default)
- Modify: `crates/pgevolve-core/src/ir/canon/cluster.rs` (sort tablespaces, reject dup names)

- [ ] **Step 1: Write `tablespace.rs`**

Read `crates/pgevolve-core/src/ir/cluster/role.rs` first for the `DiffMacro` + `#[diff(...)]` conventions. Then:
```rust
//! `TABLESPACE` IR — a cluster-level object (like `Role`). Only the SQL object
//! is modeled; filesystem layout is out of scope.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use pgevolve_core_macros::DiffMacro;

/// A `CREATE TABLESPACE` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct Tablespace {
    /// Cluster-global tablespace name.
    pub name: Identifier,
    /// `LOCATION '/path'` directory. Immutable in PG — a change surfaces as a
    /// lint, never an ALTER.
    #[diff(via_debug)]
    pub location: String,
    /// Owner (`pg_tablespace.spcowner`). Lenient: `None` = unmanaged.
    #[diff(via_debug)]
    pub owner: Option<Identifier>,
    /// Tablespace options (`seq_page_cost`, `random_page_cost`,
    /// `effective_io_concurrency`, `maintenance_io_concurrency`). Lenient.
    #[diff(via_debug)]
    pub options: BTreeMap<String, String>,
    /// Optional comment (`pg_shdescription`).
    #[diff(via_debug)]
    pub comment: Option<String>,
}
```
(Confirm the `DiffMacro` import path matches `role.rs`'s — use whatever `role.rs` uses. If `role.rs` doesn't import the macro explicitly, match that.)

- [ ] **Step 2: Register module + catalog field**

`ir/cluster/mod.rs`: add `pub mod tablespace;`.
`ir/cluster/catalog.rs`: next to `pub roles: Vec<Role>,` add `pub tablespaces: Vec<crate::ir::cluster::tablespace::Tablespace>,`. If `ClusterCatalog` has a manual `empty()`/constructor listing fields, add `tablespaces: Vec::new(),`.

- [ ] **Step 3: Canon — sort + dup-reject**

In `ir/canon/cluster.rs run()`, after the role sort, add:
```rust
    cat.tablespaces.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    for w in cat.tablespaces.windows(2) {
        if w[0].name == w[1].name {
            return Err(/* mirror the duplicate-role IrError style used here; if roles
                          don't dup-check, add IrError::DuplicateTablespace(Identifier) */);
        }
    }
```
Read how the role canon reports duplicates (if it does) and mirror it; if there's no cluster dup error variant, add `IrError::DuplicateTablespace(Identifier)` following the `DuplicateEventTrigger` precedent in `ir/mod.rs`.

- [ ] **Step 4: Tests (in `tablespace.rs`)**

Unit-test the diff macro produces field-level diffs (mirror a `role.rs` diff test): build two `Tablespace`s differing in `owner`/`options`/`comment` and assert the `.diff()` output paths. Add a canon test (in `canon/cluster.rs` tests) asserting tablespaces sort by name and duplicates error.

- [ ] **Step 5: Verify + commit**

Run: `cargo test -p pgevolve-core --lib ir::cluster::tablespace ir::canon::cluster` → pass.
Run: `cargo build --workspace` → compiles (fix any exhaustive `ClusterCatalog { .. }` literals; `grep -rn "ClusterCatalog {" crates`).
Run: `cargo clippy --workspace --all-targets` → 0 warnings.
```bash
cargo fmt && git add -A && git commit -m "feat(ir): Tablespace + ClusterCatalog.tablespaces + canon

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Cluster parser

**Files:**
- Create: `crates/pgevolve-core/src/parse/cluster/create_tablespace.rs`, `alter_tablespace.rs` (ALTER … SET/RESET + ALTER … OWNER TO; can be one file `tablespace_stmt.rs`)
- Modify: `crates/pgevolve-core/src/parse/cluster/mod.rs` (router arms + module decls + comment routing)

Read `parse/cluster/create_role.rs`, `alter_role.rs`, and the `apply_file` router in `parse/cluster/mod.rs` (the `match NodeEnum` at ~line 60).

- [ ] **Step 1: Builders**

`create_tablespace::apply(stmt: &CreateTableSpaceStmt, cat: &mut ClusterCatalog, loc: &SourceLocation) -> Result<(), ParseError>`:
- name = `Identifier::from_unquoted(&stmt.tablespacename)`.
- owner = `stmt.owner` (a `RoleSpec`) → `Some(Identifier)` via the same RoleSpec→Identifier helper `create_role`/`alter_role_owner` use (find it: `grep -rn "RoleSpec" crates/pgevolve-core/src/parse/cluster`). If `owner` is `None`, leave `None`.
- location = `stmt.location.clone()` (error if empty).
- options = parse `stmt.options` (a `Vec<Node>` of `DefElem { defname, arg }`) into `BTreeMap<String,String>` (mirror how role/tablespace `WITH (...)` DefElems are read; a `DefElem`'s value is typically an `A_Const` → string/number). Write a small `def_elems_to_map(&[Node]) -> Result<BTreeMap<String,String>, ParseError>` helper.
- push `Tablespace { name, location, owner, options, comment: None }`; error on duplicate name in `cat.tablespaces`.

`alter_tablespace_set::apply(stmt: &AlterTableSpaceOptionsStmt, cat, loc)`: look up the tablespace by name (error if absent — ALTER before CREATE). If `stmt.is_reset`, remove each named option from the map; else merge each option into the map.

`alter_tablespace_owner::apply(stmt: &AlterOwnerStmt, cat, loc)`: only when `stmt.object_type == ObjectType::ObjectTablespace`; extract the bare tablespace name from `stmt.object` (a `String` node) and the new owner from `stmt.newowner` (RoleSpec); set `owner = Some(...)`.

`set_tablespace_comment(cat, name, comment)`: for `CommentStmt` with `ObjectType::ObjectTablespace`.

- [ ] **Step 2: Router + rejections**

In `parse/cluster/mod.rs apply_file`, add arms:
```rust
NodeEnum::CreateTableSpaceStmt(s) => create_tablespace::apply(s, cat, &loc)?,
NodeEnum::AlterTableSpaceOptionsStmt(s) => alter_tablespace::apply_set(s, cat, &loc)?,
NodeEnum::AlterOwnerStmt(s) if s.object_type == ObjectType::ObjectTablespace as i32
    => alter_tablespace::apply_owner(s, cat, &loc)?,
NodeEnum::DropTableSpaceStmt(_) => return Err(ParseError::Structural {
    location: loc, message: "DROP TABLESPACE in source is not supported — drops happen via diff".into() }),
NodeEnum::RenameStmt(s) if s.rename_type == ObjectType::ObjectTablespace as i32
    => return Err(ParseError::Structural {
        location: loc, message: "ALTER TABLESPACE … RENAME is not supported — rename is drop+create".into() }),
```
Extend the existing `CommentStmt` arm to route `ObjectType::ObjectTablespace` to `set_tablespace_comment`. (Match how the role `AlterOwnerStmt`/`CommentStmt` discriminants are compared — the existing code shows whether to compare against `as i32` or a decoded enum; mirror it exactly.)

- [ ] **Step 3: Tests + verify + commit**

Tests (mirror `create_role.rs` tests — they parse SQL via the cluster parse entry and assert the `ClusterCatalog`): simple create; create with OWNER + WITH options; ALTER SET option; ALTER RESET; ALTER OWNER TO; COMMENT ON; duplicate-name error; ALTER-before-CREATE error; DROP-in-source rejected; RENAME rejected.
Run: `cargo test -p pgevolve-core --lib parse::cluster` → pass. `cargo clippy --workspace --all-targets` → 0. `cargo build --workspace` → clean.
```bash
cargo fmt && git add -A && git commit -m "feat(parse): CREATE/ALTER/COMMENT TABLESPACE (cluster source)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Catalog reader

**Files:**
- Modify: `crates/pgevolve-core/src/catalog/queries/cluster.rs` (new query const)
- Modify: `crates/pgevolve-core/src/catalog/mod.rs` (`CatalogQuery::ClusterTablespaces` + SQL mapping + param group)
- Modify: `crates/pgevolve-core/src/catalog/cluster.rs` (fetch + decode + attach)

- [ ] **Step 1: Query**

In `catalog/queries/cluster.rs` (mirror `CLUSTER_ROLES_QUERY`'s bootstrap-filter style; `$1` = bootstrap names array):
```rust
pub const CLUSTER_TABLESPACES_QUERY: &str = "\
SELECT
    t.spcname::text                                    AS name,
    pg_catalog.pg_get_userbyid(t.spcowner)::text       AS owner,
    pg_catalog.pg_tablespace_location(t.oid)::text     AS location,
    t.spcoptions::text[]                               AS options,
    d.description::text                                AS comment
FROM pg_catalog.pg_tablespace t
LEFT JOIN pg_catalog.pg_shdescription d
       ON d.objoid = t.oid AND d.classoid = 'pg_tablespace'::regclass
WHERE t.spcname NOT IN ('pg_default','pg_global')
  AND t.spcname <> ALL($1::text[])
ORDER BY t.spcname";
```
(`spcoptions` is a `text[]` of `key=value`, NULL when none.)

- [ ] **Step 2: Register CatalogQuery**

`catalog/mod.rs`: add `ClusterTablespaces` variant near `ClusterRoles`; map it to `CLUSTER_TABLESPACES_QUERY`; it takes the bootstrap-names text-array param like `ClusterRoles` (add to the same param group, NOT the no-param group).

- [ ] **Step 3: Decode + attach**

In `catalog/cluster.rs`, add `decode_tablespace(row) -> Result<Tablespace, CatalogError>` (mirror `decode_role`): name; owner via `row.get_opt_text`/`get_text` → `Some(Identifier)` (None if empty); location via `get_text`; options: parse the `text[]` (use the same array accessor `decode_role`/the codebase uses — `get_text_array` guarded by `is_null`) splitting each `key=value` on the first `=` into the `BTreeMap`; comment via `get_opt_text` → None if empty. In `read_cluster_catalog`, fetch `CatalogQuery::ClusterTablespaces` (passing the bootstrap array like roles) and set `cat.tablespaces` before the final canonicalize.

- [ ] **Step 4: Tests + verify + commit**

Tests (mirror `decode_role` tests using `Row::new().with(...)`): decode simple; with owner+options (`Value::TextArray(["seq_page_cost=2.0"])`); NULL options → empty map; NULL comment → None. Run `cargo test -p pgevolve-core --lib catalog::cluster` → pass; clippy 0; build clean.
```bash
cargo fmt && git add -A && git commit -m "feat(catalog): read pg_tablespace into ClusterCatalog (excl. built-ins)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Diff

**Files:** Modify `crates/pgevolve-core/src/diff/cluster.rs`.

- [ ] **Step 1: Extend `ClusterChange`**

Add variants (after the role ones):
```rust
    CreateTablespace(crate::ir::cluster::tablespace::Tablespace),
    DropTablespace { name: Identifier },
    AlterTablespaceOwner { name: Identifier, owner: Identifier },
    SetTablespaceOptions { name: Identifier, options: std::collections::BTreeMap<String, String> },
    CommentOnTablespace { name: Identifier, comment: Option<String> },
```

- [ ] **Step 2: Diff logic + location-drift signal**

In `diff_cluster`, after the role diffing, pair `target.tablespaces`/`source.tablespaces` by name (BTreeMap):
- source-only → `CreateTablespace` (Destructiveness::Safe).
- target-only → `DropTablespace` with `RequiresApprovalAndDataLossWarning` (reason e.g. `"drops tablespace {name} — objects using it will fail"`), mirroring `DropRole`.
- both present:
  - `owner` differs AND source `owner.is_some()` → `AlterTablespaceOwner { name, owner: src }` (Safe).
  - options: collect `{ k: v for (k,v) in source.options if target.options.get(k) != Some(v) }`; if non-empty → `SetTablespaceOptions { name, options }` (Safe). (Lenient: do not reset live-only options.)
  - `comment` differs → `CommentOnTablespace` (Safe).
  - `location` differs → **emit nothing here**; the lint (Task 6) surfaces it. Add a code comment explaining PG can't relocate a tablespace.

- [ ] **Step 3: Tests + build**

Add `diff_cluster` tests (mirror role diff tests): create; drop is `RequiresApprovalAndDataLossWarning`; owner-lenient (source None → no change); options SET only differing keys; live-only option not reset; location change emits NO ClusterChange. Run `cargo test -p pgevolve-core --lib diff::cluster` → pass. `cargo build --workspace` → expect exhaustive-match breaks in emit (Task 5) and CLI; leave those for their tasks but DO add a temporary nothing-arm with `// TODO(tablespace Task 5)` in `plan/cluster_rewrite/emit.rs` if needed to keep this commit compiling — or defer the commit to after Task 5. Prefer: commit after Task 5 so emit is real. If committing now, mark the temporary emit arm clearly.

- [ ] **Step 4: Commit** (`feat(diff): tablespace ClusterChange variants + lenient diff`).

---

## Task 5: StepKind + render + emit

**Files:**
- Modify: `crates/pgevolve-core/src/plan/raw_step.rs` (StepKind variants)
- Modify: `crates/pgevolve-core/src/plan/plan.rs` (`kind_name` + `parse_kind_name` round-trip)
- Modify: `crates/pgevolve-core/src/plan/cluster_rewrite/sql.rs` (builders)
- Modify: `crates/pgevolve-core/src/plan/cluster_rewrite/emit.rs` (`emit_one` arms)

- [ ] **Step 1: StepKind + round-trip**

`raw_step.rs`: add `CreateTablespace, DropTablespace, AlterTablespaceOwner, SetTablespaceOptions, CommentOnTablespace` near the role kinds. Update the serde round-trip test there if it enumerates kinds.
`plan.rs`: add both `kind_name` arms (`"create_tablespace"`, `"drop_tablespace"`, `"alter_tablespace_owner"`, `"set_tablespace_options"`, `"comment_on_tablespace"`) and the inverse `parse_kind_name` arms.

- [ ] **Step 2: SQL builders (`sql.rs`)**

```rust
// CREATE TABLESPACE name [OWNER owner] LOCATION '<loc>' [WITH (k = v, ...)];
fn create_tablespace(ts: &Tablespace) -> String { /* options sorted by key; loc single-quote-escaped */ }
fn drop_tablespace(name: &Identifier) -> String { format!("DROP TABLESPACE {};", name.render_sql()) }
fn alter_tablespace_owner(name: &Identifier, owner: &Identifier) -> String { /* ALTER TABLESPACE n OWNER TO o; */ }
fn alter_tablespace_set(name: &Identifier, options: &BTreeMap<String,String>) -> String { /* ALTER TABLESPACE n SET (k = v, ...); */ }
fn comment_on_tablespace(name: &Identifier, comment: Option<&str>) -> String { /* IS '...' | IS NULL; escape ' */ }
```
Use the same identifier/comment helpers `create_role`/`comment_on_role` use. Single-quote-escape the location and comment (`replace('\'', "''")`).

- [ ] **Step 3: Emit dispatch (`emit.rs`)**

In `emit_one`, add arms mapping each tablespace `ClusterChange` → a `RawStep` with the matching `StepKind`, `TransactionConstraint::InTransaction`, destructive flag from the change entry (drop = destructive), and `sql` from the builder. For `CreateTablespace`, if `ts.comment.is_some()`, emit the CREATE then a follow-up `comment_on_tablespace` step (owner + options ride inline in CREATE). Remove any Task-4 temporary arm.

- [ ] **Step 4: Tests + verify + commit**

Unit-test each SQL builder string exactly (mirror `sql.rs` role tests): create simple; create with owner+options+location; drop; alter owner; alter set; comment set/clear. Run `cargo test -p pgevolve-core --lib plan::cluster_rewrite plan::` → pass. `grep -rn "TODO(tablespace" crates` → empty. `cargo clippy --workspace --all-targets` → 0. `cargo build --workspace` → clean (fix any CLI exhaustive matches on `ClusterChange`/`StepKind` mirroring the role arms — `grep -rn "ClusterChange::" crates/pgevolve/src`).
```bash
cargo fmt && git add -A && git commit -m "feat(plan): render + emit tablespace cluster changes

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: `tablespace-location-drift` lint

**Files:**
- Create: `crates/pgevolve-core/src/lint/rules/tablespace_location_drift.rs`
- Modify: `crates/pgevolve-core/src/lint/rules/mod.rs` (module decl)
- Modify: `crates/pgevolve-core/src/lint/universal.rs` (`check_cluster_changeset` — call the rule)

Read the existing cluster lints `lint/rules/role_loses_superuser.rs` + `check_cluster_changeset` in `lint/universal.rs` for the signature/`Finding` shape.

- [ ] **Step 1: Rule**

The rule needs both catalogs (source + live) since location drift is a both-present comparison, not a changeset entry. Check the `check_cluster_changeset` signature — it takes `(source, changeset)`. Location drift isn't in the changeset (Task 4 emits nothing), so either (a) pass the live `target` catalog into the lint too, or (b) compute drift from source vs the changeset is impossible. Preferred: add a sibling cluster lint entry point that takes `(source, target)` catalogs — check how `check_cluster_changeset` is called in `api/cluster.rs` (`build_cluster_plan`) and whether `target` is in scope there (it is — `read_cluster_catalog` result). Extend the cluster lint call to also run a catalog-based rule:
```rust
pub const RULE_ID: &str = "tablespace-location-drift";

/// Advisory: a tablespace exists in both source and live with a different
/// LOCATION. PG cannot relocate a tablespace; pgevolve never auto-changes it.
pub fn check(source: &ClusterCatalog, target: &ClusterCatalog) -> Vec<Finding> {
    let mut out = Vec::new();
    for s in &source.tablespaces {
        if let Some(t) = target.tablespaces.iter().find(|t| t.name == s.name) {
            if t.location != s.location {
                out.push(Finding::advisory(RULE_ID, format!(
                    "tablespace {} location differs: live={:?}, source={:?} — \
                     pgevolve does not relocate tablespaces; recreate manually if intended",
                    s.name.as_str(), t.location, s.location)));
            }
        }
    }
    out
}
```
Use the real `Finding` constructor (mirror how `role_loses_superuser` builds findings — severity/advisory API).

- [ ] **Step 2: Wire in**

In `build_cluster_plan` (`crates/pgevolve/src/api/cluster.rs`), where `check_cluster_changeset(&source, &changes)` produces `advisory_findings`, also extend with `tablespace_location_drift::check(&source, &target)`. (Or thread it through `check_cluster_changeset` if cleaner — but that fn only gets `source`+changeset; passing `target` separately at the call site is simplest.)

- [ ] **Step 3: Tests + verify + commit**

Test the rule directly: two `ClusterCatalog`s with same-named tablespace differing in location → one finding; same location → none; tablespace only in source → none. Run `cargo test -p pgevolve-core --lib lint::rules::tablespace_location_drift` + `cargo test -p pgevolve --lib` → pass. clippy 0; build clean.
```bash
cargo fmt && git add -A && git commit -m "feat(lint): tablespace-location-drift advisory

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: ClusterConfig `tablespaces` dir + `build_cluster_plan`

**Files:**
- Modify: `crates/pgevolve/src/cluster_config.rs` (config struct)
- Modify: `crates/pgevolve/src/api/cluster.rs` (`build_cluster_plan` parse both dirs)
- Modify: `crates/pgevolve-core/src/parse/cluster/mod.rs` (a parse fn that reads a tablespaces dir into the same catalog, OR reuse `parse_cluster_directory` twice — see below)

- [ ] **Step 1: Config**

`cluster_config.rs`: add
```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TablespacesConfig {
    /// Directory of tablespace .sql files; default `<project>/tablespaces`.
    pub dir: Option<std::path::PathBuf>,
}
```
and `#[serde(default)] pub tablespaces: TablespacesConfig,` on `ClusterConfig`. Update the e2e `cluster_cfg_for` test helpers that construct `ClusterConfig` literally to add `tablespaces: Default::default()`.

- [ ] **Step 2: Parse both dirs**

The cluster `apply_file` router already handles tablespace statements (Task 2), so parsing is dir-agnostic. Add `parse_cluster_sources(roles_dir: &Path, tablespaces_dir: &Path) -> Result<ClusterCatalog, ParseError>` in `parse/cluster/mod.rs` that runs the existing per-file apply over BOTH dirs (roles first, then tablespaces; skip a dir that doesn't exist) into one `ClusterCatalog`, then canonicalizes. Keep `parse_cluster_directory(roles_dir)` for back-compat (have it delegate with an empty/absent tablespaces dir).
In `build_cluster_plan`, replace the `parse_cluster_directory(&roles_dir)` call with `parse_cluster_sources(&roles_dir, &tablespaces_dir)`, resolving `tablespaces_dir` from `cfg.tablespaces.dir` (default `project_root.join("tablespaces")`).

- [ ] **Step 3: Tests + verify + commit**

Add a parse test for `parse_cluster_sources` with both a roles file and a tablespaces file → catalog has both. Run `cargo test -p pgevolve-core --lib parse::cluster` + `cargo test -p pgevolve --lib` → pass. clippy 0; build clean.
```bash
cargo fmt && git add -A && git commit -m "feat(cluster): tablespaces/ source dir wired into build_cluster_plan

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Conformance — `${TABLESPACE_DIR}` provisioning + fixtures

**Files:**
- Modify: `crates/pgevolve-conformance/src/assertions/apply.rs` (`seed_before` — provision dir + substitute)
- Create: `crates/pgevolve-conformance/tests/cases/cluster/tablespaces/**` (fixtures)

- [ ] **Step 1: Provision + substitute in `seed_before`**

The conformance harness applies `before.sql` via `client.batch_execute`. Before executing, if the SQL contains `${TABLESPACE_DIR}`:
1. choose a unique path `let dir = format!("/tmp/pgev_ts_{}", <unique>);` (use a counter or the fixture name hash — NOT `Date`/random which are banned; the fixture path or an atomic counter works).
2. provision it over the same connection BEFORE the batch: `client.batch_execute(&format!("COPY (SELECT 1) TO PROGRAM 'mkdir -p {dir} && chmod 700 {dir}';")).await?;` (verified: works as the superuser bootstrap connection).
3. `let sql = before_sql.replace("${TABLESPACE_DIR}", &dir);` then execute.
Also apply the SAME substitution to `after.sql` where it's parsed/applied for the plan comparison (the source parse path) — confirm where after.sql flows (`render_cluster_plan`/`parse_one_cluster_source`) and substitute there too, so the planned `CREATE TABLESPACE … LOCATION '<dir>'` matches. Read `crates/pgevolve-conformance/src/planning.rs` + `assertions/apply.rs` to thread the substitution through both before and after consistently. Keep the chosen `dir` stable within one fixture run so before/after agree.

- [ ] **Step 2: Fixtures**

Read a real cluster fixture first: `for f in fixture.toml before.sql after.sql expected/plan.sql; do echo "== $f =="; cat crates/pgevolve-conformance/tests/cases/cluster/roles/<a-case>/$f; done`. Then create under `cluster/tablespaces/`:
- `create-simple/` — after: `CREATE TABLESPACE ts_app LOCATION '${TABLESPACE_DIR}';` → 1 step.
- `create-with-options/` — `... LOCATION '${TABLESPACE_DIR}' WITH (seq_page_cost = 2.0, random_page_cost = 3.0);`
- `alter-owner/` — before creates ts owned by postgres; after `ALTER TABLESPACE ts_app OWNER TO app_owner;` (ensure `app_owner` role exists in before.sql).
- `alter-set-option/` — before creates ts; after `ALTER TABLESPACE ts_app SET (random_page_cost = 1.5);`
- `comment-on/` — after `COMMENT ON TABLESPACE ts_app IS 'app data';`
- `drop-intent/` — before creates ts; after omits it → `DROP TABLESPACE` step gated by intent (encode `[[expect.intent]]` like the role drop fixture; read a role-drop fixture for the shape).
- `location-drift-lint/` — before creates ts at `${TABLESPACE_DIR}`; after declares the same ts at a DIFFERENT literal path → expect 0 plan steps + the `tablespace-location-drift` advisory. (Encode the lint expectation as the role-lint fixtures do.)

- [ ] **Step 3: Bless + inspect + run**

`cargo run -p xtask -- bless --conformance`. Inspect each generated `expected/plan.sql` by hand (cross-check against the Task 5 render test strings). Run `cargo test -p pgevolve-conformance` (Docker) → all pass + no regressions. If a generated plan looks wrong (e.g. drift emits a DROP/CREATE instead of nothing+lint), STOP — that's a Task 4/6 bug.

- [ ] **Step 4: Commit** (`test(conformance): TABLESPACE fixtures + ${TABLESPACE_DIR} harness`).

---

## Task 9: E2E + docs + full gate

**Files:**
- Create/extend: `crates/pgevolve/tests/cluster_apply_e2e.rs` (or a sibling) — tablespace round-trip
- Modify: `docs/spec/objects.md`, `docs/spec/roadmap.md`, `CHANGELOG.md`; `git rm docs/superpowers/plans/_skeleton/cluster-tablespace.md`

- [ ] **Step 1: E2E test**

Mirror the role e2e in `cluster_apply_e2e.rs`. Before applying, provision the dir over the client: `client.batch_execute("COPY (SELECT 1) TO PROGRAM 'mkdir -p /tmp/pgev_e2e_ts && chmod 700 /tmp/pgev_e2e_ts';").await?;`. Write a `tablespaces/t.sql` with `CREATE TABLESPACE ts_e2e LOCATION '/tmp/pgev_e2e_ts';`, build+apply the cluster plan, and assert `SELECT 1 FROM pg_tablespace WHERE spcname='ts_e2e'` returns a row. `#[ignore]` + Docker-guard like the existing cluster e2e.

- [ ] **Step 2: Docs**

`objects.md`: flip the `TABLESPACE` row to `✅ Supported` (mirror EVENT TRIGGER's recently-shipped marker); reconcile the v0.4.0-vs-v0.4.2 wording to v0.4.0. `roadmap.md`: move TABLESPACE from the Active matrix to Shipped (v0.4.0), plan link → `2026-06-05-tablespace.md`. `CHANGELOG.md` `[Unreleased] → Added`: a TABLESPACE bullet (cluster object; CREATE/ALTER OWNER/ALTER SET/DROP/COMMENT; lint-only location drift; lenient owner+options; filesystem layout out of scope). `git rm` the skeleton.

- [ ] **Step 3: Full gate + commit**

`cargo test --workspace` (all pass), `cargo clippy --workspace --all-targets` (0 warnings), `cargo fmt --check`, `cargo deny check`. Run the new e2e: `cargo test -p pgevolve --test cluster_apply_e2e -- --ignored` (Docker) → pass. If `Catalog`/`ClusterCatalog` field additions re-blessed any tier-3 snapshots, verify the diffs are only the additive `tablespaces: []` line (mirror the EVENT TRIGGER snapshot-refresh check).
```bash
cargo fmt && git add -A
git commit -m "feat(tablespace): mark shipped — e2e, objects.md, roadmap, CHANGELOG

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-review notes (coverage vs spec)
- §1 IR → T1. §2 source layout/parser → T2 (+ T7 dir wiring). §3 reader → T3. §4 diff (lenient owner/options, intent-gated drop, location→lint) → T4 (+ T6 lint). §5 render/plan → T5. §6 lint → T6. §7 tests → T8 (conformance + harness) + T9 (e2e). §8 non-goals: rename rejected (T2), location not relocated (T4 emits nothing + T6 lint), options lenient (T4), filesystem out (no dir creation in production code — only test harness provisions dirs).
- **Cross-task type consistency:** `ClusterChange::{CreateTablespace, DropTablespace, AlterTablespaceOwner, SetTablespaceOptions, CommentOnTablespace}` and the matching `StepKind` names are used identically in T4/T5. `Tablespace` fields `{name, location, owner, options, comment}` consistent T1→T5.
- **Known risk:** the conformance `${TABLESPACE_DIR}` substitution must reach BOTH the applied `before.sql` and the parsed `after.sql`/plan so the rendered LOCATION matches (T8 Step 1) — the one spot most likely to need iteration.
