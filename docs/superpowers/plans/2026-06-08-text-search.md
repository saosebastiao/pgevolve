# TEXT SEARCH (CONFIGURATION + DICTIONARY) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add two managed schema-scoped object kinds — `TEXT SEARCH DICTIONARY` (template + ordered options) and `TEXT SEARCH CONFIGURATION` (parser + token→dictionary mappings) — with PARSER/TEMPLATE treated as unmanaged environment references.

**Architecture:** New `TsDictionary` / `TsConfiguration` IR on `Catalog`, parsed from `DefineStmt` (+ `AlterTsDictionaryStmt` / `AlterTsConfigurationStmt`), read from `pg_ts_dict` / `pg_ts_config` (+ `pg_ts_config_map`), diffed as **managed** objects (Create/Drop/Replace + AlterOptions / per-token mapping add-alter-drop + lenient owner + comment), rendered with new `StepKind`s, with a dep edge from each configuration to the managed dictionaries it maps to.

**Tech Stack:** Rust, `pg_query`, `pg_catalog` introspection, conformance harness.

**Design:** [`docs/superpowers/specs/2026-06-08-text-search-design.md`](../specs/2026-06-08-text-search-design.md)

**Closest templates:**
- **AGGREGATE** (`2026-06-06-aggregate.md`) — managed schema-scoped object parsed from a `DefineStmt`, with owner + comment + a closed-world-ish reference; the per-object scaffolding (IR/change/diff/render/parser/reader) is the template. Read its files: `ir/aggregate.rs`, `diff/aggregates.rs`, `diff/change.rs` (`AggregateChange`), `plan/rewrite/emit/aggregate.rs`, `parse/builder/aggregate_stmt.rs`, `catalog/assemble/aggregates.rs`, `plan/edges.rs` (`NodeId::Aggregate`).
- **Publication selective-table membership** (`diff/publications.rs:145` `diff_selective_tables`, `PublicationChange::{AddTable,DropTable}`) — the template for the per-token-type mapping ADD/ALTER/DROP diff.
- **COLLATION** — also a `DefineStmt`; its parser dispatch in `parse/statement.rs` shows how `DefineStmt.kind` is branched.

## Verified facts (pg_query 6.1.1)
- `CREATE TEXT SEARCH DICTIONARY` / `CONFIGURATION` are `DefineStmt` with `kind = ObjectType::ObjectTsdictionary` (47) / `ObjectTsconfiguration` (46). `defnames` = name; `definition` = `Vec<DefElem>` (`template`/`parser`/`copy`/option pairs).
- `ALTER TEXT SEARCH DICTIONARY … (opts)` = `AlterTsDictionaryStmt { dictname, options }`.
- `ALTER TEXT SEARCH CONFIGURATION …` = `AlterTsConfigurationStmt { kind: AlterTsConfigType, cfgname, tokentype, dicts, override, replace, missing_ok }`. `AlterTsConfigType`: `AddMapping=1`, `AlterMappingForToken=2`, `ReplaceDict=3`, `ReplaceDictForToken=4`, `DropMapping=5`.
- `ObjectType::ObjectTsparser`(48) / `ObjectTstemplate`(49) — we do NOT manage these; never created/dropped.
- Reader catalogs: `pg_ts_dict(dictname,dictnamespace,dicttemplate,dictinitoption,dictowner)`, `pg_ts_template(tmplname,tmplnamespace)`, `pg_ts_config(cfgname,cfgnamespace,cfgparser,cfgowner)`, `pg_ts_parser(prsname,prsnamespace)`, `pg_ts_config_map(mapcfg,maptokentype,mapseqno,mapdict)`. Token alias names via `ts_token_type(parser_oid)` → `(tokid int, alias text, description text)`.

Project rules: no `unwrap`/`expect`/`panic!`/`todo!` in non-test code; `cargo clippy --workspace --all-targets` ZERO; `cargo fmt`; `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` clean; **build/clippy/doc at WORKSPACE level each task**. Co-author trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Commit to `main`.

---

## Task 1: IR — `TsDictionary` + `TsConfiguration` + canon

**Files:** Create `crates/pgevolve-core/src/ir/text_search/{mod.rs,dictionary.rs,configuration.rs}`; modify `ir/mod.rs`, `ir/catalog.rs`, `ir/canon/mod.rs`; create `ir/canon/text_search.rs`; add `IrError::DuplicateTsDictionary` / `DuplicateTsConfiguration`.

- [ ] **Step 1** — `ir/text_search/dictionary.rs`: `TsDictionary { qname: QualifiedName, template: QualifiedName, options: Vec<(String, String)>, owner: Option<Identifier>, comment: Option<String> }`. `ir/text_search/configuration.rs`: `TsConfiguration { qname, parser: QualifiedName, mappings: Vec<TsMapping>, owner, comment }` + `TsMapping { token_type: String, dictionaries: Vec<QualifiedName> }`. All derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`. `ir/text_search/mod.rs` re-exports them. Serde round-trip unit tests for each.
- [ ] **Step 2** — `ir/mod.rs`: `pub mod text_search;`. `ir/catalog.rs`: add `pub ts_dictionaries: Vec<…TsDictionary>` and `pub ts_configurations: Vec<…TsConfiguration>` next to `aggregates`; add `Vec::new()` to every `Catalog` literal / `Catalog::empty()` (grep `aggregates:` for the sites, including testkit).
- [ ] **Step 3** — `IrError::DuplicateTsDictionary(QualifiedName)` + `DuplicateTsConfiguration(QualifiedName)` mirroring `DuplicateAggregate`.
- [ ] **Step 4** — `ir/canon/text_search.rs` `run(cat) -> Result<(), IrError>`: sort `ts_dictionaries` by `qname.render_sql()`; sort `ts_configurations` by `qname.render_sql()`; within each config sort `mappings` by `token_type`; within each dictionary sort `options` by key. Reject duplicate identity for each kind. Wire into `ir/canon/mod.rs canonicalize()` after `aggregates::run`. Tests: sort determinism + dup rejection + mapping/option ordering.
- [ ] **Step 5** — Verify `cargo test -p pgevolve-core --lib ir::text_search ir::canon::text_search`; `cargo build --workspace`; clippy 0; fmt. Commit `feat(ir): TsDictionary + TsConfiguration + canon`.

---

## Task 2: Change enums

**Files:** `crates/pgevolve-core/src/diff/change.rs` (+ `diff/mod.rs` re-exports).

- [ ] **Step 1** — Add `TsDictionaryChange` (mirror `AggregateChange`): `Create(TsDictionary)`, `Replace { from, to }`, `Drop { qname }`, `AlterOptions { qname, options: Vec<(String,String)> }`, `AlterOwner { qname, owner: Identifier }`, `CommentOn { qname, comment: Option<String> }`.
- [ ] **Step 2** — Add `TsConfigurationChange`: `Create(TsConfiguration)`, `Replace { from, to }`, `Drop { qname }`, `AddMapping { qname, token_type: String, dictionaries: Vec<QualifiedName> }`, `AlterMapping { qname, token_type, dictionaries }`, `DropMapping { qname, token_type: String }`, `AlterOwner { qname, owner }`, `CommentOn { qname, comment }`.
- [ ] **Step 3** — Add `Change::TsDictionary(TsDictionaryChange)` + `Change::TsConfiguration(TsConfigurationChange)`. `cargo build -p pgevolve-core` → fix exhaustive matches: real arms for Display/destructiveness (all Safe except… all Safe — TS objects carry no data; mirror Aggregate which is all Safe); temporary `// TODO(textsearch Task 5/6/7/10)` arms for ordering/emit/CLI implemented later. List sites touched. Commit `feat(diff): TsDictionary/TsConfiguration change enums`.

---

## Task 3: Dictionary differ

**Files:** Create `crates/pgevolve-core/src/diff/ts_dictionaries.rs`; wire into `diff/mod.rs`.

- [ ] **Step 1** — `diff_ts_dictionaries(target, source, out)` paired by `qname` (BTreeMap keyed by `render_sql`, mirror `diff/aggregates.rs`). Managed: source-only → `Create`; target-only → `Drop`; both: `template` differs → `Replace`; else `options` differ → `AlterOptions { options: source.options.clone() }`; `owner` differs & source `Some` → `AlterOwner`; `comment` differs → `CommentOn`. Wire call into `diff/mod.rs` after `aggregates`.
- [ ] **Step 2** — Tests: create; drop; replace on template change; AlterOptions on options change; lenient owner (source None → nothing); comment. Verify `cargo test -p pgevolve-core --lib diff::ts_dictionaries`; clippy 0. Commit `feat(diff): text-search dictionary differ`.

---

## Task 4: Configuration differ (mappings)

**Files:** Create `crates/pgevolve-core/src/diff/ts_configurations.rs`; wire into `diff/mod.rs`.

- [ ] **Step 1** — `diff_ts_configurations(target, source, out)` paired by `qname`. Managed: source-only → `Create` (the renderer emits the config + ADD MAPPING for each mapping — see Task 7); target-only → `Drop`; both: `parser` differs → `Replace`; else diff mappings by `token_type` (mirror `diff/publications.rs:145 diff_selective_tables`): source-only token → `AddMapping`; both but `dictionaries` differ → `AlterMapping`; target-only token → `DropMapping`. Then `owner` (lenient) + `comment`.
- [ ] **Step 2** — Tests: create; drop; replace on parser change; add mapping; alter mapping (dict chain change); drop mapping; owner; comment. Verify `cargo test -p pgevolve-core --lib diff::ts_configurations`; clippy 0. Commit `feat(diff): text-search configuration differ (mappings)`.

---

## Task 5: Dep graph + ordering

**Files:** `crates/pgevolve-core/src/plan/edges.rs`, `plan/ordering.rs`.

- [ ] **Step 1** — `NodeId::TsDictionary(QualifiedName)` + `NodeId::TsConfiguration(QualifiedName)` (mirror `NodeId::Collation(QualifiedName)` at `edges.rs:81`). Fix exhaustive `match NodeId` sites (labels: `ts_dictionary:<q>`, `ts_configuration:<q>`; ASCII only).
- [ ] **Step 2** — Register each dictionary + configuration node. For each configuration, add an edge `TsConfiguration → TsDictionary` for every **managed** dictionary referenced in its `mappings` (look up the dict qname in `catalog.ts_dictionaries`; skip unmanaged/built-in dict names not present — like the aggregate sfunc edge skips unmanaged). No edges for parser/template (unmanaged). Mirror the collation/aggregate node-registration block (`edges.rs:325-336`).
- [ ] **Step 3** — `plan/ordering.rs change_node`: map each `TsDictionaryChange`/`TsConfigurationChange` to its `NodeId` (Create/Replace → to.qname; others → carried qname). Clear Task-2 ordering TODOs. Add `partition()` arms (Create→creates, Drop/Replace→drops, AlterOptions/AddMapping/AlterMapping/DropMapping/AlterOwner/CommentOn→modifies).
- [ ] **Step 4** — Tests: config node depends on its managed mapped dictionary. `grep -rn "TODO(textsearch Task 5)" crates` empty. Verify; clippy 0. Commit `feat(plan): NodeId::TsDictionary/TsConfiguration + config→dict edges`.

---

## Task 6: Render + StepKind — dictionary

**Files:** Create `crates/pgevolve-core/src/plan/rewrite/emit/ts_dictionary.rs`; modify `emit/mod.rs`, `rewrite/mod.rs`, `plan/raw_step.rs`, `plan/plan.rs`, `plan/rewrite/sql.rs`.

- [ ] **Step 1** — `StepKind`: `CreateTsDictionary`, `DropTsDictionary`, `AlterTsDictionary`, `AlterTsDictionaryOwner`, `CommentOnTsDictionary` (+ serde round-trip + kind_name/parse_kind_name).
- [ ] **Step 2** — `emit/ts_dictionary.rs` (mirror `emit/aggregate.rs`): 
  - `CREATE TEXT SEARCH DICTIONARY <q> (TEMPLATE = <template>[, <opt> = '<val escaped>', …]);` (options in canon order).
  - `ALTER TEXT SEARCH DICTIONARY <q> (<opt> = '<val>', …);`
  - `DROP TEXT SEARCH DICTIONARY <q>;`
  - `ALTER TEXT SEARCH DICTIONARY <q> OWNER TO <owner>;`
  - `COMMENT ON TEXT SEARCH DICTIONARY <q> IS '…' | IS NULL;`
  `Create` with owner/comment → follow-up steps. `Replace` → Drop + Create. All `InTransaction`, Safe.
- [ ] **Step 3** — Register `pub mod ts_dictionary;` in `emit/mod.rs`; dispatch `Change::TsDictionary(c) => emit::ts_dictionary::emit(c, …)` in `rewrite/mod.rs`.
- [ ] **Step 4** — Unit-test each SQL string. `grep -rn "TODO(textsearch Task 6)" crates` empty. Verify; clippy 0. Commit `feat(render): emit text-search dictionary DDL`.

---

## Task 7: Render + StepKind — configuration

**Files:** Create `crates/pgevolve-core/src/plan/rewrite/emit/ts_configuration.rs`; modify `emit/mod.rs`, `rewrite/mod.rs`, `plan/raw_step.rs`, `plan/plan.rs`, `plan/rewrite/sql.rs`.

- [ ] **Step 1** — `StepKind`: `CreateTsConfiguration`, `DropTsConfiguration`, `AddTsConfigMapping`, `AlterTsConfigMapping`, `DropTsConfigMapping`, `AlterTsConfigurationOwner`, `CommentOnTsConfiguration` (+ serde + kind_name).
- [ ] **Step 2** — `emit/ts_configuration.rs`:
  - `CREATE TEXT SEARCH CONFIGURATION <q> (PARSER = <parser>);` then, for `Create`, a follow-up `ALTER … ADD MAPPING FOR <tok> WITH <d1>, <d2>;` per mapping (canon order).
  - `ALTER TEXT SEARCH CONFIGURATION <q> ADD MAPPING FOR <tok> WITH <dicts joined ", ">;`
  - `ALTER … <q> ALTER MAPPING FOR <tok> WITH <dicts>;`
  - `ALTER … <q> DROP MAPPING IF EXISTS FOR <tok>;`
  - `DROP TEXT SEARCH CONFIGURATION <q>;`
  - `ALTER … <q> OWNER TO <owner>;` `COMMENT ON TEXT SEARCH CONFIGURATION <q> IS …;`
  `Replace` → Drop + Create (Create re-adds all mappings). Render token_type as a bare identifier (it's a parser token alias like `word` — render via `Identifier`-style quoting if needed; token aliases are simple lowercase names, but quote defensively). All `InTransaction`, Safe.
- [ ] **Step 3** — Register + dispatch. 
- [ ] **Step 4** — Unit-test each SQL string (incl. Create with multiple mappings → CREATE + N ADD MAPPING). `grep -rn "TODO(textsearch Task 7)" crates` empty. Verify; clippy 0. Commit `feat(render): emit text-search configuration DDL`.

---

## Task 8: Parser

**Files:** Create `crates/pgevolve-core/src/parse/builder/text_search_stmt.rs`; modify `parse/statement.rs`, `parse/builder/mod.rs`, `parse/mod.rs`.

- [ ] **Step 1** — `statement.rs`: branch `DefineStmt.kind` for `ObjectTsdictionary` / `ObjectTsconfiguration` (alongside the existing collation/aggregate `DefineStmt` branches) → new `Statement` variants. Route `AlterTsDictionaryStmt`, `AlterTsConfigurationStmt`, `AlterOwnerStmt`/`CommentStmt`/`DropStmt` with `ObjectTsconfiguration`/`ObjectTsdictionary`.
- [ ] **Step 2** — `text_search_stmt.rs`:
  - dictionary `DefineStmt`: `defnames` → qname; `definition` DefElems → `template` (from `template` elem, → QualifiedName) + remaining elems → `options` (`(name, value-as-string)`); reject if no template.
  - configuration `DefineStmt`: `parser` elem → parser qname; `copy` elem → `ParseError` (out of scope); mappings empty.
  - `AlterTsDictionaryStmt` → set the target dict's `options` (replace with the given list).
  - `AlterTsConfigurationStmt`: `AddMapping`/`AlterMappingForToken` → set `token_type → dicts` for each token in `tokentype` (dicts from `dicts`); `DropMapping` → remove those tokens; `ReplaceDict`/`ReplaceDictForToken` → substitute dict qnames in affected mappings.
  - `ALTER … OWNER TO` / `COMMENT ON` apply by identity; `DROP …` in source → `ParseError`.
  - Flow both kinds into `catalog.ts_dictionaries` / `ts_configurations` via the parse accumulator (mirror aggregate flow).
- [ ] **Step 3** — Wire modules. Tests: create dictionary (template + options); alter dict options; create configuration (parser); add/alter/drop mapping; owner; comment; reject COPY=; reject DROP-in-source; duplicate identity. Verify `cargo test -p pgevolve-core --lib parse`; clippy 0. Commit `feat(parse): CREATE/ALTER text-search dictionary + configuration`.

---

## Task 9: Catalog reader — dictionary

**Files:** Create `crates/pgevolve-core/src/catalog/assemble/ts_dictionaries.rs` + query; modify `catalog/mod.rs`, `catalog/assemble/mod.rs`.

- [ ] **Step 1** — Query `pg_ts_dict d JOIN pg_namespace n ON n.oid=d.dictnamespace JOIN pg_ts_template t ON t.oid=d.dicttemplate JOIN pg_namespace tn ON tn.oid=t.tmplnamespace`: select dict schema+name, template schema+name, `d.dictinitoption`, `pg_get_userbyid(d.dictowner)` owner, comment via `pg_description` (`classoid='pg_ts_dict'::regclass`). Exclude extension-owned (`pg_depend deptype='e'`). Register as a global query (mirror the collation/event-trigger global query registration — these are schema-scoped but read cluster-wide like collations).
- [ ] **Step 2** — `assemble_ts_dictionaries`: decode; parse `dictinitoption` (format: `key = 'value', key2 = 'value2'` — a comma-separated option list; parse into `Vec<(String,String)>`, stripping quotes) into `options`. Build `TsDictionary`.
- [ ] **Step 3** — Tests: decode dict with template + options; option-string parsing (incl. quoted values with commas inside — handle carefully). Verify; clippy 0; cargo doc clean. Commit `feat(catalog): read pg_ts_dict`.

---

## Task 10: Catalog reader — configuration (mappings)

**Files:** Create `crates/pgevolve-core/src/catalog/assemble/ts_configurations.rs` + query; modify `catalog/mod.rs`, `catalog/assemble/mod.rs`.

- [ ] **Step 1** — Query `pg_ts_config c JOIN pg_namespace n JOIN pg_ts_parser p ON p.oid=c.cfgparser JOIN pg_namespace pn`: select cfg schema+name, parser schema+name, owner, comment. Exclude extension-owned. Plus a **second** query (or a lateral) for mappings: `SELECT m.mapcfg, m.maptokentype, m.mapseqno, dn.nspname AS dict_schema, d.dictname AS dict_name, tt.alias FROM pg_ts_config_map m JOIN pg_ts_dict d ON d.oid=m.mapdict JOIN pg_namespace dn ON dn.oid=d.dictnamespace JOIN pg_ts_config c ON c.oid=m.mapcfg, LATERAL ts_token_type(c.cfgparser) tt WHERE tt.tokid = m.maptokentype ORDER BY m.mapcfg, m.maptokentype, m.mapseqno`. (Confirm `ts_token_type` lateral join shape against a live PG during impl.)
- [ ] **Step 2** — `assemble_ts_configurations`: build each `TsConfiguration`; group mapping rows by `(mapcfg, alias)`, order dicts by `mapseqno`, attach as `TsMapping { token_type: alias, dictionaries: [qnames] }`. Resolve mapping rows to their config by oid.
- [ ] **Step 3** — Tests: decode config with parser + a multi-dict mapping (chain order preserved) + multiple token types. Verify; clippy 0; cargo doc clean. Commit `feat(catalog): read pg_ts_config + pg_ts_config_map`.

---

## Task 11: CLI exhaustive matches + conformance + e2e

**Files:** `crates/pgevolve/src/commands/diff.rs`, `commands/graph.rs`, `pgevolve-conformance/src/assertions/dep_graph.rs`, `shadow/validate.rs`, `pgevolve-testkit/src/ir_mutator.rs`; `crates/pgevolve-conformance/tests/cases/objects/text_search/**`; `crates/pgevolve/tests/text_search_e2e.rs`.

- [ ] **Step 1** — `cargo build --workspace`; add `Change::TsDictionary`/`TsConfiguration` describe arms + `NodeId::TsDictionary`/`TsConfiguration` labels + testkit `Catalog` field arms, mirroring Aggregate. Clear any `TODO(textsearch Task 10)` markers. Build + clippy 0.
- [ ] **Step 2** — Conformance fixtures `objects/text_search/`: `create-dictionary` (`TEMPLATE = pg_catalog.snowball, language = 'english'`), `alter-dictionary-options`, `create-configuration` (`PARSER = pg_catalog.default`), `add-mapping` (config + a managed dict + `ADD MAPPING FOR word, asciiword WITH <dict>`), `alter-mapping`, `drop-mapping`, `drop`, `comment-on`. `bless --conformance`; inspect plans (config plan: CREATE CONFIGURATION then ADD MAPPING; dep graph orders the dictionary before the configuration's mappings). `cargo test -p pgevolve-conformance`. If config orders before its mapped dict, STOP — Task 5 bug.
- [ ] **Step 3** — E2E (`text_search_e2e.rs`, mirror `aggregate_e2e.rs`): apply a catalog with a managed dictionary + a configuration mapping a token to it + a table with a `tsvector` column and a GIN index using the configuration; introspect; `assert_convergent`. Docker-guarded. MUST converge. If it diverges (option parsing, mapping order, token alias), STOP and report.
- [ ] **Step 4** — Commit `test: text-search conformance + e2e`.

---

## Task 12: docs + full gate

**Files:** `docs/spec/objects.md`, `docs/spec/roadmap.md`, `CHANGELOG.md`, `git rm docs/superpowers/plans/_skeleton/text-search.md`.

- [ ] **Step 1** — `objects.md`: flip `TEXT SEARCH CONFIGURATION`/`DICTIONARY` to ✅ Supported (note: managed; PARSER/TEMPLATE are unmanaged references). `roadmap.md`: move the `TEXT SEARCH` row to the Shipped table, version `Unreleased`, plan link `2026-06-08-text-search.md`. `CHANGELOG.md`: add a fresh `## [Unreleased]` above the latest released section with an `### Added` text-search bullet (CONFIGURATION + DICTIONARY managed; mappings; PARSER/TEMPLATE unmanaged refs; COPY out of scope). `git rm` the skeleton.
- [ ] **Step 2** — Full gate: `cargo test --workspace`; `cargo clippy --workspace --all-targets` 0; `cargo fmt --check`; `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` clean; `cargo deny check`. Tier-3 `catalog_round_trip` snapshots gain `"ts_dictionaries": []` + `"ts_configurations": []` — re-bless via `cargo run -p xtask -- bless` (Docker), verify additive-only. Commit `feat(text-search): mark shipped`.

---

## Self-review notes
- §1 IR → T1. §2 parser → T8. §3 reader → T9 (dict) + T10 (config). §4 canon → T1. §5 diff → T3 (dict) + T4 (config). §6 render → T6 (dict) + T7 (config). §7 parser/template refs (no constraint) → nothing to build (absence). §8 tests → T11 + unit across tasks. §9 non-goals: PARSER/TEMPLATE not created (T8 parser only handles dict/config DefineStmt kinds; reader resolves them as name refs); COPY rejected (T8); extension-owned excluded (T9/T10 readers).
- **Type consistency:** `TsDictionary { qname, template, options: Vec<(String,String)>, owner, comment }`, `TsConfiguration { qname, parser, mappings: Vec<TsMapping>, owner, comment }`, `TsMapping { token_type: String, dictionaries: Vec<QualifiedName> }` used identically T1–T11. Change/StepKind/NodeId names consistent.
- **Watch:** (1) `dictinitoption` parsing (T9) — values can contain commas inside quotes (`stopwords = 'a,b'`); parse the option list quote-aware, don't naive-split on comma. (2) mapping token alias resolution (T10) — the `ts_token_type(parser)` lateral must map `maptokentype` int → alias; verify the join against live PG. (3) config→dict dep edge (T5) — only for MANAGED dicts (a mapping may reference a built-in/extension dict like `pg_catalog.simple`, which has no node → skip the edge). (4) build sequencing: dictionary tasks (T3/T6/T9) before configuration tasks (T4/T7/T10) since configs reference dicts, but all land before T11.
