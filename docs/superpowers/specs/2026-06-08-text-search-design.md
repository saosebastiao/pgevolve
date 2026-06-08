---
status: design
target: v0.4.x
sub_spec: text-search
---

# `TEXT SEARCH` (CONFIGURATION + DICTIONARY) — design

Adds the two user-facing full-text-search object kinds — `TEXT SEARCH
DICTIONARY` and `TEXT SEARCH CONFIGURATION` — as managed schema-scoped objects.
The two remaining kinds, `TEXT SEARCH PARSER` and `TEXT SEARCH TEMPLATE`, require
C-language functions (START/GETTOKEN, INIT/LEXIZE) that pgevolve cannot manage
(same class as `BASE TYPE`); they are **unmanaged environment references** —
a configuration names its parser, a dictionary names its template, rendered by
name (almost always `pg_catalog` built-ins or extension-provided) but never
created or dropped. This mirrors how tables reference tablespaces and casts
reference built-in types.

Both managed kinds follow the established schema-scoped pattern (collation /
aggregate / cast): identity is the schema-qualified name, drop is **managed**
(auto-dropped when absent from source), owner is lenient.

Brainstorming decisions:
- **CONFIGURATION + DICTIONARY managed; PARSER + TEMPLATE unmanaged refs.**
- **`COPY=` on `CREATE TEXT SEARCH CONFIGURATION` is out of scope for source.**
  A `COPY`-created configuration reads back as `PARSER = …` plus explicit
  mappings, so source declares that canonical form (`PARSER=` + `ADD MAPPING`).
  A source `COPY=` is rejected with a structured error.
- **Parser/template are referenced by name with no closed-world lint** —
  environment infra, like tablespace references.

---

## §1. IR

New module tree `crates/pgevolve-core/src/ir/text_search/` with
`dictionary.rs` and `configuration.rs` (and a `mod.rs`).

```rust
// dictionary.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TsDictionary {
    pub qname: QualifiedName,
    /// Unmanaged template reference (e.g. `pg_catalog.snowball`).
    pub template: QualifiedName,
    /// Template options as ordered key/value pairs (e.g. `language='english'`).
    /// Canon sorts by key for stable comparison.
    pub options: Vec<(String, String)>,
    pub owner: Option<Identifier>,
    pub comment: Option<String>,
}

// configuration.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TsConfiguration {
    pub qname: QualifiedName,
    /// Unmanaged parser reference (e.g. `pg_catalog.default`).
    pub parser: QualifiedName,
    /// Token-type → ordered dictionary-chain mappings.
    pub mappings: Vec<TsMapping>,
    pub owner: Option<Identifier>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TsMapping {
    /// Token-type alias name (e.g. `word`, `asciiword`, `numword`).
    pub token_type: String,
    /// Ordered dictionary fallback chain for this token type.
    pub dictionaries: Vec<QualifiedName>,
}
```

`Catalog` gains `pub ts_dictionaries: Vec<TsDictionary>` and
`pub ts_configurations: Vec<TsConfiguration>`. Identity is `qname` for both.

## §2. Parser

`crates/pgevolve-core/src/parse/builder/` — text-search statement builders,
dispatched on:
- `DefineStmt` with `kind = ObjectType::ObjectTsdictionary` → `TsDictionary`:
  `defnames` → qname; `definition` `DefElem`s → `TEMPLATE` (required) + arbitrary
  `opt = value` pairs into `options`. (Same `DefineStmt` dispatch path as
  collation / aggregate.)
- `DefineStmt` with `kind = ObjectType::ObjectTsconfiguration` → `TsConfiguration`:
  `PARSER` DefElem → `parser`; `mappings` start empty (populated by ALTER). A
  `COPY` DefElem → structured `ParseError` (out of scope, see decisions).
- `AlterTsDictionaryStmt` → replace the target dictionary's `options` with the
  given option list (PG's `ALTER … (opt=val)` sets/overrides those options).
- `AlterTsConfigurationStmt` (`kind: AlterTsConfigType`) → mutate the target
  configuration's `mappings`:
  - `AddMapping` / `AlterMappingForToken` → set `token_type → dicts` for each
    listed token type.
  - `DropMapping` → remove the listed token types.
  - (`ReplaceDict` / `ReplaceDictForToken` substitute one dict for another across
    mappings — apply by rewriting the affected `dictionaries` entries.)
- `ALTER … OWNER TO` (`AlterOwnerStmt`, `ObjectTsconfiguration`/`ObjectTsdictionary`)
  → `owner`. `CommentStmt` → `comment`. `DropStmt` in source → `ParseError`
  (drops come from the diff; mirror aggregate/cast DROP-in-source rejection).

## §3. Catalog reader

`crates/pgevolve-core/src/catalog/assemble/text_search.rs` + queries:
- **Dictionaries** (`pg_ts_dict` ⋈ `pg_ts_template` ⋈ `pg_namespace`): `dictname`/
  schema → qname; `dicttemplate` → template qname; `dictinitoption` (a text blob
  like `language = 'english', stopwords = 'english'`) parsed into ordered
  `options`; `dictowner` → owner; comment via `pg_description`.
- **Configurations** (`pg_ts_config` ⋈ `pg_ts_parser` ⋈ `pg_namespace`):
  `cfgname`/schema → qname; `cfgparser` → parser qname; `cfgowner` → owner;
  comment. **Mappings** from `pg_ts_config_map` (`mapcfg = cfg.oid`): group rows
  by `maptokentype`, order each group's dicts by `mapseqno`, resolve
  `maptokentype` (int) → alias name via `ts_token_type(cfg.cfgparser)` (which
  returns `(tokid, alias, description)`), and `mapdict` → dictionary qname.
- Exclude extension-owned objects (`pg_depend deptype='e'`), mirroring the
  aggregate/collation readers.

## §4. Canon

`crates/pgevolve-core/src/ir/canon/text_search.rs`: sort `ts_dictionaries` and
`ts_configurations` by qname; within a configuration, sort `mappings` by
`token_type`; sort each dictionary's `options` by key. Reject duplicate identity
(`IrError::DuplicateTsDictionary` / `DuplicateTsConfiguration`). The dictionary
 option **values** and the per-token dictionary **chain order** are significant
and preserved.

## §5. Diff (managed)

`crates/pgevolve-core/src/diff/text_search.rs`, paired by `qname`:

**Dictionary** (`TsDictionaryChange`):
- source-only → `Create` (Safe). target-only → `Drop` (Safe).
- both: `template` differs → `Replace` (DROP+CREATE — PG has no `ALTER … TEMPLATE`).
  Else `options` differ → `AlterOptions { options }` (`ALTER TEXT SEARCH
  DICTIONARY … (opt=val, …)`). `owner` differs & source `Some` → `AlterOwner`
  (lenient). `comment` → `CommentOn`.

**Configuration** (`TsConfigurationChange`):
- source-only → `Create` (+ its mappings as ADD MAPPING follow-ups). target-only
  → `Drop`.
- both: `parser` differs → `Replace` (DROP+CREATE — no `ALTER … PARSER`; the new
  config re-adds all mappings). Else per-token-type mapping diff:
  source-only token → `AddMapping`; both but `dictionaries` differ →
  `AlterMapping`; target-only token → `DropMapping`. `owner`/`comment` as above.

## §6. Render + dependency graph

- `CREATE TEXT SEARCH DICTIONARY q (TEMPLATE = t[, opt = 'v', …]);`
  `ALTER TEXT SEARCH DICTIONARY q (opt = 'v', …);`
  `CREATE TEXT SEARCH CONFIGURATION q (PARSER = p);`
  `ALTER TEXT SEARCH CONFIGURATION q ADD MAPPING FOR tok WITH d1, d2;`
  `ALTER … q ALTER MAPPING FOR tok WITH d1, d2;`
  `ALTER … q DROP MAPPING IF EXISTS FOR tok;`
  `DROP TEXT SEARCH {DICTIONARY|CONFIGURATION} q;`
  `ALTER … q OWNER TO r;` `COMMENT ON TEXT SEARCH … q IS '…';`
  New `StepKind`s; `Replace` = drop-then-create (configuration re-adds mappings).
- Dep graph: `NodeId::TsDictionary(qname)`, `NodeId::TsConfiguration(qname)`.
  Edge: configuration → each **managed** dictionary referenced in its mappings
  (so dictionaries are created before the configuration's mappings, dropped
  after). Dictionary→template and configuration→parser are unmanaged references
  → no edge. The graph is acyclic (configs depend on dicts; dicts depend on
  nothing managed).

## §7. Parser / template references (no managed constraint)

A configuration's `parser` and a dictionary's `template` are rendered by name
verbatim; there is **no** closed-world check that they are managed and **no**
new lint. They are environment infra (almost always `pg_catalog` built-ins or
extension-provided). If absent at apply, Postgres errors.

## §8. Tests

- **Conformance** `objects/text_search/`: `create-dictionary` (snowball +
  `language`), `alter-dictionary-options`, `create-configuration` (PARSER =
  default), `add-mapping` (`FOR word, asciiword WITH <managed dict>`),
  `alter-mapping`, `drop-mapping`, `drop`, `comment-on`.
- **E2E** (real PG): create a managed dictionary + a configuration mapping a
  token type to it + a `tsvector` GIN index that uses the configuration;
  introspect; assert round-trip convergence (and that the FTS index still works).
- **Unit:** parser (each DefElem; ADD/ALTER/DROP MAPPING; ALTER dict options;
  reject `COPY=`, reject DROP-in-source); reader decode (incl. mapping grouping +
  token alias resolution, option parsing); canon (sort + dup + option/mapping
  ordering); diff (dictionary create/drop/replace/options; configuration
  create/drop/replace/mapping add/alter/drop; lenient owner; comment); render
  strings; dep-graph edge config→managed-dictionary.

## §9. Out of scope / non-goals

- `CREATE TEXT SEARCH PARSER` / `TEXT SEARCH TEMPLATE` — read as unmanaged
  references only (need C functions).
- `COPY=` on `CREATE TEXT SEARCH CONFIGURATION` — source declares `PARSER=` +
  explicit `ADD MAPPING` (reader normalizes COPY-created configs to that form).
- Extension-owned text-search objects (excluded from introspection via
  `pg_depend`).
