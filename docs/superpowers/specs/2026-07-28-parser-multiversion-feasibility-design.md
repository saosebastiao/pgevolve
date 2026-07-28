---
date: 2026-07-28
status: analysis
sub_spec: parser-multiversion-feasibility
---

# Feasibility: owning a multi-version, strongly-typed Postgres parser binding

An evaluation of the `pg_query` crate's maintenance state and of whether
pgevolve should build and own a replacement that exposes **one strongly-typed
binding per supported Postgres major**, all co-resident in a single binary,
rather than a single binding whose types are shared across versions.

This document is **analysis, not a plan**. It records what was measured, what
was disproved, and what the decision hinges on. Nothing here changes code.

> **Method.** Every number below was measured on 2026-07-28 against local clones
> of `pganalyze/pg_query.rs`, `pganalyze/libpg_query` (all remote branches),
> pgevolve itself, and against the live crates.io / GitHub APIs. Claims that were
> inferred rather than measured were put through an adversarial verification pass;
> several were refuted and are reported here in corrected form. Where a figure
> came from a compiler-assisted measurement rather than `grep`, that is stated,
> because in two cases `grep` was wrong by a factor of 4–50.

---

## 1. Bottom line

**The premise is right, one of its two motivations is wrong, and the technical
risk is lower than expected.**

- `pg_query.rs` is genuinely under-maintained — 334 days without a crates.io
  release, 8 unreleased commits on `main`, median open-issue age 650 days.
- But it is **not abandoned**, and it is **not the reason to build our own**.
  A PG 18 branch exists (`d3042ed`, 2026-07-16) as draft PR #79.
- The real reason is structural: `pg_query.rs` has **one** git submodule and
  **one** generated `protobuf.rs`. It cannot express per-major typed bindings at
  any level of maintenance quality. Neither can any other Rust crate.
- **`libpg_query` (the C library) is healthy** and already publishes exactly the
  granularity we need: long-lived `14-latest` … `18-latest` branches, each a
  complete self-contained vendoring, each shipping machine-readable
  `srcdata/*.json` node definitions.
- The two things that looked hardest — **linking five Postgres parsers into one
  binary**, and **generating a correct typed Rust AST per version** — were both
  *prototyped and proved* during this investigation.
- The thing that looked easy — **lowering parse nodes into pgevolve's IR** — is
  the actual cost centre, and it does **not** benefit from codegen.

---

## 2. Upstream state

### 2.1 `pg_query.rs` (the Rust binding) — slow and bus-factored

| Signal | Measurement |
|---|---|
| Latest crates.io release | `6.1.1`, published 2025-08-28 — **334 days ago** |
| Git tags | Stop at `v6.1.0`; **6.1.1 was published untagged** |
| Unreleased commits on `main` | 8, spanning 2025-12-05 → 2026-06-23 |
| Commits per year | 2021: 32 · 2022: 27 · 2023: 13 · 2024: 13 · 2025: 18 · **2026: 4** |
| Median open-issue age | 650 days |
| Governance | No `CONTRIBUTING`, no `GOVERNANCE`, no `CODE_OF_CONDUCT` |
| Issue #73 "PG 18 Support?" | Open since 2026-02-08, **zero maintainer comments** |

The published `6.1.1` artifact pins libpg_query `17-6.1.0` (commit `1c1a32ed`,
2025-04-01) — a **483-day-old C snapshot, four libpg_query releases behind**.
It therefore also misses the crash fixes shipped in `17-6.2.1` / `17-6.2.2`
(`pg_query_normalize` DefElem crash, deparse-comments init crash).

Two structural defects found by reading the crate rather than its metrics:

- **`build.rs` mutates the Cargo registry cache.** When `protoc` is on `PATH`,
  building *any* crate that depends on `pg_query` rewrites
  `~/.cargo/registry/src/.../pg_query-6.1.1/src/protobuf.rs`. Reproduced: mtime
  moved from 2006-07-24 to today. `build.rs:72-83` deliberately sets
  `OUT_DIR` to the manifest's `src/` and renames the prost output over it.
  This is incompatible with `cargo vendor` checksums and with any
  reproducible-build policy.
- **`main` may currently be unpublishable.** `build.rs` now copies
  `libpg_query/postgres_deparse.h`, which sits at the libpg_query *root* on the
  18-latest submodule pin, while `Cargo.toml`'s `include` globs only cover
  `libpg_query/{src,vendor}/**/*.{c,h}`. CI runs build + test + fmt + clippy but
  has **no `cargo package --verify` gate**.

### 2.2 `libpg_query` (the C library) — healthy, and the right foundation

| Signal | Measurement |
|---|---|
| HEAD age | 4 days (`7ece262`, 2026-07-24) |
| Commits in 2026 | 44 |
| Per-major branches | `9.4-latest` … `18-latest` (long-lived, complete trees) |
| Recent tags | `14-3.0.0`, `15-4.2.4`, `16-5.2.0`, `17-6.2.2`, `18.0.0` |
| Hardening | OSS-Fuzz targets added June 2026 (`#341`, `#343`) |
| Bus factor | Lukas Fittl = 83% of 2026 commits, but releases have been cut by others |

**Caveat that must be carried into any plan:** upstream actively maintains only
the newest one or two branches. `14-latest` was last touched 2024-02-16 and is
pinned at **PG 14.6**; `15-latest` at **15.1**; `16-latest` at **16.1** — while
current upstream minors are 14.23 / 15.18 / 16.14. And `14-latest` **does not
compile** on glibc 2.39 without a one-line `#define strchrnul pg_strchrnul`
patch that `18-latest` already carries.

Two readings of that fact, both true: vendoring inherits the staleness, *and*
vendoring is the only way to fix it — PG minor releases essentially never change
the grammar (`14-latest`'s `srcdata/struct_defs.json` has not changed since
2022-11-14), and owning the vendoring lets us cherry-pick a grammar fix without
waiting for an upstream release.

### 2.3 Corrections to the initial hypothesis

Three things the investigation expected to find, and did not:

1. **"Rust is being singled out for neglect"** — false. As of today *no*
   binding has published PG 18: Ruby `pg_query` 6.2.2 and Go `pg_query_go`
   v6.2.2 are both PG 17 (Jan 2026), and third-party `pglast` 8.4 is PG 17 too.
   Where Rust genuinely lags its siblings is the PG **17 patch line**: Ruby and
   Go each shipped `17-6.2.2` within 1–2 days of the C tag; Rust never shipped
   6.2.x at all.
2. **"The ecosystem is stuck on old versions, so there's no upgrade pressure"** —
   false, and inverted. All 34 crates.io reverse-dependents require `^6.x`, and
   6.x passed 5.x in trailing-30-day downloads (51.4% vs 48.6%).
3. **"Nobody has shipped PG 18, so we'd have to do it ourselves"** — false.
   Branch `origin/pg-18`, one commit by a maintainer, open as **draft PR #79**.
   The diff is 11 files / +997 −786, almost entirely regenerated `protobuf.rs`.

---

## 3. The pain that is real today

pgevolve's README advertises "Postgres 14–18" and its CI matrix runs all five
majors. The parser underneath is **PostgreSQL 17.4**. Measured hard failures on
PG 18 DDL:

```
GENERATED ALWAYS AS (x) VIRTUAL        -> syntax error at or near "VIRTUAL"
... CHECK (...) NOT ENFORCED           -> syntax error at or near "ENFORCED"
FOREIGN KEY (a, PERIOD b) ...          -> syntax error at or near "b"
PRIMARY KEY (id, valid_at WITHOUT OVERLAPS) -> syntax error at or near "OVERLAPS"
RETURNING WITH (OLD AS o, NEW AS n) *  -> syntax error at or near "WITH"
```

`docs/superpowers/specs/2026-06-07-virtual-generated-columns-design.md` is
already `status: blocked-upstream` for exactly this reason. (That doc is
internally stale — its §2 asserts "pg_query 6.1.1 ships the PG 18 grammar, so
`VIRTUAL` parses", which the keyword-list measurement disproves. Worth fixing
whoever touches it next.)

Compounding it: pgevolve **re-parses server-emitted DDL** — `pg_get_viewdef`,
`pg_get_indexdef`, `pg_get_constraintdef`, `pg_get_triggerdef`, `pg_get_expr`,
`pg_get_partkeydef`, `pg_get_functiondef` — from PG 14–18 servers, at 12
production sites in `catalog/assemble/`, all through the one PG 17 parser.

---

## 4. Is multi-version co-residency even possible? — Yes, proved

This was the make-or-break question, and it was answered by building it.

### 4.1 The hazard is real and silent

`libpg_query` does **zero** symbol namespacing. Measured global symbol counts:
2,538 (18-latest), 2,442 (17), 2,270 (16), 2,129 (15) — with **2,428 names
colliding between 17 and 18** and 2,101 common to 15/16/17/18. The collisions
include `palloc`, `pfree`, `MemoryContextAlloc`, `raw_parser`, `base_yyparse`,
`core_yylex`, `errstart`, `TopMemoryContext`, `CurrentMemoryContext`,
`error_context_stack` and every `pg_query_*` entry point. The extract step even
`#undef`s `PGDLLIMPORT`/`PGDLLEXPORT` with the comment *"Don't mark anything as
visible based on how Postgres defines it."*

Worse than a link error: **the naive link silently succeeds.**

```
cc -o naive main.c 17-latest/libpg_query.a 18-latest/libpg_query.a -pthread
# exit 0, no warnings
./naive   ->  {"version":170007,...}     # every symbol resolved from the FIRST archive
```

Not one member of the 18 archive was pulled in. Forcing both in with
`--whole-archive` produces exactly 2,428 `multiple definition` errors — matching
the measured overlap precisely. A naive multi-version build would ship a binary
claiming PG 14–18 while parsing everything with one grammar, **with green CI**.

### 4.2 The fix: compile-time symbol prefixing

Generate, per major, a header of `#define <sym> pfxNN_<sym>` for every global,
and compile with `-include prefix.h` (`/FI` on MSVC; the `cc` crate abstracts
this).

- 2,538 / 2,538 globals prefixed, **zero** compile errors, zero unprefixed
  remaining. Repeated cleanly on 15-latest.
- **libpg_query's own full upstream test suite passes unmodified on the prefixed
  build** — all 17 test binaries including deparse, fingerprint, concurrency and
  plpgsql, in 10 s.
- Pure preprocessor transformation: portable to GCC / Clang / MSVC on Linux,
  macOS and Windows, with **no** `objcopy` / binutils dependency (which matters —
  Apple's toolchain ships `nm`/`otool`/`strip`, not `objcopy`).

This is also the remedy pganalyze themselves reached for: `pg_query_go` commit
`51c94ef` (2026-01-27), *"Fix xxhash symbol conflict by using namespace."*

### 4.3 Runtime co-residency is safe, not merely link-clean

The obvious objection — global mutable C state — does not apply. libpg_query has
already made every hazard thread-local: **99 `__thread` declarations** covering
`CurrentMemoryContext`, `TopMemoryContext`, `ErrorContext`, `PG_exception_stack`,
`error_context_stack`, the `errordata[]` stack, the aset free lists, and the
encoding globals. Each prefixed major therefore gets its own TLS slots and its
own `sigsetjmp` error stack: **a parse error in PG 14 cannot longjmp into PG 18's
handler.**

### 4.4 End-to-end proof

All five majors linked into one binary — **13.0 MB stripped**:

| | PG14 | PG15 | PG16 | PG17 | PG18 |
|---|---|---|---|---|---|
| reported `PG_VERSION_NUM` | 140006 | 150001 | 160001 | 170007 | 180004 |
| `SELECT 1` | ok | ok | ok | ok | ok |
| `MERGE … WHEN MATCHED` | **ERR** | ok | ok | ok | ok |
| `… IS JSON SCALAR` | **ERR** | **ERR** | ok | ok | ok |
| `MERGE … NOT MATCHED BY SOURCE` | **ERR** | **ERR** | **ERR** | ok | ok |
| `… GENERATED ALWAYS AS (a*2) VIRTUAL` | **ERR** | **ERR** | **ERR** | **ERR** | ok |

Each version deparsed through its own deparser, with visible version-specific
behaviour (PG14/15 emit `PARTITION BY range(a)`, PG16 `RANGE(a)`, PG17/18
`RANGE (a)`). Concurrency: 4 threads × 200 iterations × 7 queries interleaved
across all five, including error/`sigsetjmp` paths — clean, with **RSS flat at
12.3 MB after 25,000 parses**.

That version staircase is exactly the property pgevolve needs, and it converts
directly into a conformance test. Whatever strategy is adopted, **a link-time
staircase assertion is mandatory** — it is the only thing standing between us
and the silent single-grammar trap.

### 4.5 Build story

Each branch is fully self-contained: `make build` needs **no** Postgres source
tree, **no** `protoc`, **no** Ruby, **no** network. All five build from clean in
**58 s wall on 4 cores**. A `build.rs` using the `cc` crate can drive it
directly — glob `src/*.c` + `src/postgres/*.c`, set six include dirs and three
flags. The C API surface to wrap is small and stable: 30 functions in PG 17/18,
and `pg_query.h` for 17 vs 18 differs in **exactly three lines** (the version
macros).

---

## 5. Is per-version typed codegen possible? — Yes, prototyped

### 5.1 The input already exists

`libpg_query` ships five machine-readable files on **every** version branch —
`srcdata/{struct_defs,enum_defs,nodetypes,typedefs,all_known_enums}.json` — with
the same schema on all five. These are the source of truth its own Ruby
generator consumes, and they are strictly richer than the `.proto` (they retain C
types, field order, comments, and the parsenodes/primnodes grouping the proto
flattens away).

They are also **frozen once a major is cut**: `14-latest`'s `struct_defs.json`
has not changed since 2022-11-14. A per-version generated module is a
write-once artifact per PG major, not an ongoing maintenance tax.

| | PG14 | PG15 | PG16 | PG17 | PG18 |
|---|---|---|---|---|---|
| node structs | 224 | 229 | 243 | 260 | 263 |
| struct fields | 1,372 | 1,407 | 1,471 | 1,580 | 1,620 |
| enums / variants | 56 / 820 | 58 / 840 | 63 / 871 | 70 / 925 | 72 / 941 |
| `NodeTag` entries | 431 | 437 | 454 | 474 | 479 |

### 5.2 It was built and it works

A prototype srcdata-driven Rust generator was written during this investigation
and run against the real parser:

> **32,144 statements** from the PostgreSQL regression corpus deserialized with
> **zero errors** under `#[serde(deny_unknown_fields)]` on all 266 generated
> structs — 397,103 nodes walked, 189 distinct node kinds. Plus **858/858** of
> pgevolve's own `.sql` fixtures.

Exactly **two** hand-written serde shims are required, and both are stable across
all five majors:

1. `A_Const` — libpg_query's `_outAConst` inlines the inner value as
   `ival`/`fval`/`boolval`/`sval`/`bsval` rather than emitting a tagged node.
2. `Node` — libpg_query emits a bare `{}` for a NULL element inside a `List`,
   which serde's derived externally-tagged enum cannot represent. (Found
   empirically: 1,377 failures reading `expected value`, all fixed by a
   `visit_map` returning `Node::Absent`.)

Two per-version naming hazards were also found *by compiling*, which is exactly
the class of surprise that makes "just write a codegen" estimates wrong: PG14 has
a real node type named `Null` (collides with the obvious sentinel — rename it
`Absent`), and `nodes/value::String` generates an infinitely self-referential
`pub struct String { pub sval: String }` unless `char*` maps to
`::std::string::String`.

**Adversarial caveat, and it matters:** `deny_unknown_fields` acceptance is a
*weaker* property than losslessness. libpg_query's JSON outfuncs elide every
zero/false/empty field behind `if (node->fldname != 0)` guards, so any field that
is always zero across the corpus was never exercised; no re-serialize round-trip
was run; and JSON is keyed by field *name* while protobuf is keyed by field
*number*. The prototype proves ingestion, not round-trip, and says nothing about
the protobuf encoding the deparse path needs.

### 5.3 Size is a non-issue for Rust, and the binding constraint for C

Generated Rust for **all five** versions: 42,646 LOC / 1.67 MB raw / **230 KB
gzipped** — roughly the same as **one** version of `pg_query.rs`'s prost output
(9,306 LOC / 364 KB). All five compile **co-resident in one crate**: 65 s clean
release build, 45.7 s `cargo check`.

The C is the constraint. Only **8 of 662** union C files (1.2%) are byte-identical
across all five branches, so there is no shareable core, and gzip's 32 KB window
gives **zero** cross-version dedup.

| packaging | gzipped | vs 10 MiB crates.io limit |
|---|---|---|
| all five in one crate | 8,924,450 B | **85%** — no headroom |
| `pg14-sys` | 1,630,690 B | 15% |
| `pg15-sys` | 1,700,198 B | 16% |
| `pg16-sys` | 1,700,264 B | 16% |
| `pg17-sys` | 1,810,334 B | 17% |
| `pg18-sys` | 2,082,461 B | 19% |

85% with no headroom is a trap: one PG minor bump and the release is blocked
pending a crates.io limit exception. **Five `-sys` crates is the only safe
shape** — at the cost of taking pgevolve's release ceremony from 2 crates to 7.

Two related constraints: **docs.rs builds with network access disabled**, so
`build.rs` may not download anything; and submodules are fine for crates.io
(cargo packages the *files*, not the reference — the published `pg_query-6.1.1`
`.crate` is 2,013,829 B containing 478 files under `libpg_query/src/postgres/`)
but are a release footgun. Vendoring in-tree per `-sys` crate is the correct
answer.

### 5.4 The per-version delta validates "don't unify"

| across all five majors | count |
|---|---|
| union of node structs | 265 |
| present in all five | 223 |
| **byte-identical in all five** | **139 (52.5%)** |
| identical after normalizing the PG17 `int`→`ParseLoc` rename | 188 (70.9%) |
| union of enums / identical in all five | 73 / **46** |

Adjacent-major deltas are small in count but land on the highest-traffic nodes:
`Constraint`, `Query`, `RangeTblEntry`, `IndexStmt`, `ColumnDef`, and the four
DML statement nodes. (16→17 looks like 81 changed structs but 69 of those are the
mechanical `int location` → `ParseLoc location` rename; only 12 are genuine.)

The decisive point is not struct shape but **enum discriminants**: `NodeTag`,
`ObjectType`, `AlterTableType`, `ConstrType`, `RTEKind`, `JoinType` and `CmdType`
all renumber between majors. A shared cross-version enum is not a convenience —
it is a silent-corruption hazard in a tool whose entire job is deciding whether
two schema objects are the same. **Separate types per version is the correct
call, and it is correct for a stronger reason than ergonomics.**

---

## 6. The part that does *not* get easier: lowering

This is the finding that should drive the decision, and it emerged only under
adversarial verification — the first-pass analysis got it wrong.

Codegen eliminates the per-version cost of **decoding** (C/JSON → typed Rust
node). It does **not** touch **lowering** (typed node → pgevolve IR), because
lowering is not a field-to-field mapping.

Measured against pgevolve's actual code:

- Of 105 IR fields across the 12 principal IR structs, only **17 (16%)** share a
  name with any field of the corresponding parse node. `ir::View` 0/11,
  `ir::Sequence` 0/12, `ir::Schema` 0/4, `ir::Extension` 0/4, `ir::Table` 2/14,
  `ir::Constraint` 1/4.
- At least **4 of those 17 name matches are provably wrong** — name matching
  succeeds and silently yields wrong IR. Generated lowering would fail *loudly*
  only where a field set diverges; these do not diverge, so it stays silent.
- **84%** of pgevolve's 206 node-consuming functions need context beyond the node
  (parent qualified name, default schema, a `&mut TakenNames` order-dependent
  allocator, `SourceLocation`). 19% take a `&mut` context. Only 48% return
  `Result<single T>`; 18% return `Result<(), _>` and mutate shared catalog state.
  A representative builder, `build_column`, takes 5 parameters and returns
  `Result<(Column, Vec<Constraint>, Option<Identifier>), ParseError>`.
- Of the 190 structurally-identical nodes, pgevolve touches only **79**. There is
  no `ir::Var` / `ir::Aggref` / `ir::SubPlan`, so there is no lowering to
  generate for the other ~111.

And the safety argument runs backwards for the dominant change mode: for
**17 → 18, zero** of the 8 removed or retyped fields appear anywhere in
pgevolve's identifier set. A field-map generator would compile **cleanly** on
PG 18 while silently dropping every new PG 18 feature. The cost driver for a new
major is new **semantics** — `NOT ENFORCED`, temporal PK/FK, virtual generated
columns, named `NOT NULL` constraints — and no amount of codegen turns those into
a compile error.

**Conclusion: "new PG majors become a compile error to fix rather than a silent
behaviour change" is not a property this architecture delivers.** Any plan that
rests its ROI on that claim is mispriced.

---

## 7. pgevolve's coupling surface

Measured with a compiler-assisted method (vendoring `pg_query`, marking all 1,386
protobuf fields `#[deprecated]`, and reading rustc's diagnostics) rather than
`grep` — because `grep` overcounted the hot fields by up to 53×.

| | |
|---|---|
| `pg_query` token sites | 538–552 across 64 of 454 Rust files; 53 files hold real code |
| Inside `pgevolve-core/src/parse/` | **465 of 538 (86.4%)** |
| Outbound leakage | 73 sites (13.6%): `catalog/assemble` 41 (9 files), `render` 18 (**all** `#[cfg(test)]`), `lint` 8, `ir` 4, `identifier.rs` 1 |
| Crates declaring the dep | `pgevolve-core` only — CLI, testkit, conformance, xtask are clean |
| Top-level API used | **four**: `parse` (73), `deparse` (6 production), `parse_plpgsql` (1), plus `Error`/`ParseResult` |
| API **not** used | `normalize`, `fingerprint`, `scan`, `split`, `truncate`, `NodeRef` walking, `.tables()` — all zero |
| Node types | **102** distinct `NodeEnum` variants of 268 (38%); 503 references |
| Enums | 18 types, 111 distinct variants (`ObjectType` alone: 34 variants / 121 refs) |
| Fields | **182** distinct names read, 211 touched; only 96 of 272 structs touched at all; 19.6% of field slots |
| Types differing across PG14–18 | only **17 of 105** used types (20 counting `A_Const`/`A_Expr`/`A_ArrayExpr`). PG16↔17: 3. PG17↔18: 8 |
| Mechanical migration size | 2,108 lines naming a `pg_query` symbol (1,754 production + 354 test, 455 of them comments) inside a 19,784-LOC translation layer with 10,435 lines of in-file tests and 770 conformance fixtures |

Four structural observations:

1. **The abstraction leaks through a public type.** `parse::Statement` is a
   36-variant enum whose payloads *are* 33 raw `pg_query::protobuf::*Stmt`
   structs, and it is `pub use`d from the crate root — so `pg_query` is part of
   `pgevolve-core`'s public API and a `pg_query` major bump is a pgevolve
   breaking change. The good news: **no caller outside `parse/` consumes it**,
   so this is fixable independently of any wrapper decision, and doing it first
   shrinks whichever migration is chosen.
2. **Deparse is 6 call sites and load-bearing.** `NormalizedBody::from_sql` does
   parse → `strip_redundant_qualifiers` → `deparse` → `collapse_whitespace` →
   BLAKE3, and is the byte-equality key for every view and SQL-function body.
   Its own doc comment states why: *"PG14's `pg_get_viewdef` keeps the qualifier
   even when unambiguous, while PG17 strips it; canonicalize to the unqualified
   form so source and catalog texts match."* **Today one PG 17 deparser
   normalizes text from all five server versions to one byte-form.** Per-version
   deparsers would require re-proving that cross-version byte-equality rather
   than assuming it — this is the sharpest argument *against* fully independent
   per-version bindings.
3. **The deparser cannot be rewritten.** It is hand-written libpg_query-specific
   C that Postgres upstream does not ship: 10,322 LOC (14-latest) growing to
   12,220 (18-latest). We must **link** it. That reframes "build our own parser"
   as "own the *bindings*", which is a far smaller claim.
4. **There is no version axis in `parse/` today.** `PgVersion` is used 337 times
   and **zero** times under `crates/pgevolve-core/src/parse/`. The parser is
   deliberately version-agnostic; version constraints are enforced as *plan-time
   lints* keyed on `[managed].min_pg_version`. Introducing per-version bindings
   means threading a version through 36 `Statement` variants and ~38 builder
   entry points — architectural work **not** covered by the 2,108-line estimate —
   and deciding whether version rejection moves from lint-time to parse-time.

---

## 8. Alternatives considered

| Option | Verdict |
|---|---|
| **Wait for PR #79** | PG 18 work is already written by a maintainer. Near-zero cost, unbounded latency — the crate's own history is 334 days between releases and 650-day median issue age. |
| **`pg_parse` 0.15.0** (MIT) | Empirically *better* maintained than `pg_query.rs` — shipped PG 18 **one day** after libpg_query tagged it. But: JSON/serde AST not protobuf, **no binding to the C deparser at all**, and its optional Rust deparser has 155 `unsupported!` arms covering most DDL. Against 503 `NodeEnum` references and a load-bearing deparse path, this is a rewrite, not a swap. |
| **Fork & vendor `pg_query.rs`** | Tractable: only ~1,450 of its 15,027 lines are hand-written; the rest is generated. This is exactly what **Supabase** did (`crates/pgls_query`, submodule `branch=17-latest`, version `0.0.0`, unpublished). Caution: they never solved publishing, because theirs is internal-only. pgevolve *is* published. |
| **Hand-write a parser** (Squawk's path) | Squawk deleted libpg_query in v2.0.0 and hand-wrote a rust-analyzer-style parser: **~92.5k lines over 15 months**, and it now ships PG 19 syntax ahead of libpg_query. Proof it is possible; also proof of the price. Rules out by constitution §5 anyway. |
| **`sqlparser-rs`** | Disqualified outright. Its own README: *"provides only a syntax parser, and tries to avoid applying any SQL semantics."* Zero occurrences of `CreateAggregate`, `CreateOpclass`, `CreateRule`, `CreateCast`, `CreateStatistics`, `CreatePublication`, `CreateSubscription`, `ExclusionConstraint`. Incompatible with "full Postgres support". |
| **`pgrx`** | Not a path, but useful prior art. Its per-version `pgNN.rs` module skeleton is what we want; its mutual exclusivity is **not** a Rust limitation — a pgrx extension resolves symbols against the one host backend at `dlopen` (it emits `rustc-link-search` with **no** `rustc-link-lib`). That constraint does not apply to us. |

Notably, **no other libpg_query binding on crates.io is alive** —
`libpg_query-sys`, `libpg_query2-sys`, `libpgquery-sys` and `postgres-parser` are
all 2+ years stale. The real option set is small.

### 8.1 Prior art for the target architecture

The exact architecture — N majors, per-version types, co-resident and
runtime-selectable — **already ships**, but only in JavaScript/WASM:

- **`@pgsql/parser`** v1.5.0 (MIT): PG 13–18, subpath exports `/v13`…`/v18`,
  per-version WASM + per-version `.d.ts`.
- **`@supabase/pg-parser`** v0.1.7 (MIT): PG 15–17 selectable **at runtime**,
  with precisely the type design to port —
  `type NodeVersionMap = { 15: Node15; 16: Node16; 17: Node17 }` plus a generic
  `Node<Version> = NodeVersionMap[Version]`. The Rust translation is a sealed
  `trait PgMajor { type Node; type ParseResult; }` with `struct Pg14`…`struct Pg18`,
  making `fn lower<V: PgMajor>(n: &V::Node) -> Ir` the generic form — and making
  it a type error to pass a PG14 node where a PG18 node is expected, which is
  what the constitution's "illegal states unrepresentable" asks for.

WASM sidesteps symbol collision by construction but costs **4–5×** throughput
(24.8 µs vs 6.6 µs per `wasilibs/go-pgquery`). Native prefixing already works, so
WASM is the documented contingency, not the plan.

Also worth noting: the JS precedents solve *parsing* per version but **not
deparsing** — `pgsql-parser` hand-wrote an 11,634-line single-shape TypeScript
deparser. Going native-per-version, we inherit libpg_query's C deparser per
version at **zero marginal cost**, which is strictly better than any precedent.

And **no schema-diff competitor does this**: Atlas hand-writes parsers with no
libpg_query at all, pgroll uses a PG17-only fork, migra loads into a temp database
and introspects, zombodb/pg-schema-diff depends on a 2021-era crate. Shipping
per-version typed parsing would make pgevolve unique — and means there is no
implementation to crib the cross-version diff semantics from.

---

## 9. Constitution and policy consequences

Any option here, **including doing nothing**, requires an amendment:

1. **§5 names the dependency literally** — *"We use the official Postgres parser
   made available by the `pg_query` crate (`pg_query = "6"`)… Parsing is not
   reimplemented."* Owning the bindings does not reimplement parsing (we still
   link libpg_query's C), but the clause as written must be reworded.
2. **`deny.toml` has no `PostgreSQL` entry** and `exceptions = []`. Vendoring
   Postgres sources ourselves surfaces the **PostgreSQL License** directly in our
   dependency graph. It is permissive and unambiguously compatible with
   MIT OR Apache-2.0 — this is a *disclosure* issue, not a licensing risk — but
   it needs an explicit allow-list addition, which is a constitution-level change
   requiring sign-off, not a silent `deny.toml` edit. Today it is invisible only
   because `pg_query.rs` declares `license = "MIT"` in its manifest while
   vendoring ~250,000 lines of PostgreSQL-licensed C.
3. **`unknown-git = "deny"`** and `cargo publish`'s ban on git dependencies mean
   "just depend on `pg_query` `main`" is not available as a stopgap.
4. **Generated code vs workspace lints.** The generated modules produce ~1,700
   `clippy::pedantic`/`nursery` warnings (765 missing-backticks in doc comments
   carried over from raw C comments, 324 derivable, 242 tabs-in-doc, 60
   non-camel-case variants from `A_Const`/`A_Expr`, 35 non-camel-case types, 31
   >3-bools). A `#![allow(...)]` header on a *generated* module is the standard
   defensible exception (prost does exactly this) and should be written into the
   plan with justification, per CLAUDE.md directive 8 — not discovered at review
   time. Two cheap generator improvements cut most of it: strip tabs and
   backtick-wrap identifiers when converting C comments to doc comments.
5. **Release ceremony.** Five `-sys` crates take the publish sequence from 2
   crates to 7, against a CLAUDE.md §11 rule that already forbids publishing
   before CI is green across all five PG majors.

---

## 10. Effort and risk

### 10.1 What is already retired

| Risk | Status |
|---|---|
| Can five majors link into one binary? | **Retired** — proved, 13 MB stripped, correct version staircase |
| Is co-residency runtime-safe? | **Retired** — 99 `__thread` decls; flat RSS over 25,000 interleaved parses |
| Can we generate typed Rust per version? | **Retired for ingestion** — 32,144/32,144 under `deny_unknown_fields` |
| Is the generated code too big? | **Retired** — five versions ≈ one version of prost output |
| Do we need `protoc`/prost? | **Retired** — all 255 protobuf field numbers reproduced from `srcdata` exactly |
| Does deparse need byte-exact protobuf? | **Retired** — 1,989/1,989 statements re-encoded byte-differently deparsed *identically* |

### 10.2 What is not

| Risk | Why it is open |
|---|---|
| **Lowering** (§6) | Codegen cannot generate it. 16% field-name overlap; 84% of builders need context; 4 known silently-wrong name matches. This is the real cost centre. |
| **Cross-version deparse equality** (§7.2) | Byte-equality of view/function bodies currently works *because* one deparser normalizes everything. Five deparsers must re-prove it across the 5×5 server/parser matrix. |
| **Version threading** (§7.4) | `parse/` has no version axis. Adding one touches 36 `Statement` variants and ~38 builders, and forces a decision about lint-time vs parse-time rejection. |
| **Round-trip losslessness** (§5.2) | Proven for ingestion only. Zero/false/empty fields were never exercised; no re-serialize comparison was run. |
| **Stale upstream branches** (§2.2) | We would own re-vendoring 14/15/16 and carrying the glibc patch. Low likelihood of grammar drift, but it becomes our job. |

---
