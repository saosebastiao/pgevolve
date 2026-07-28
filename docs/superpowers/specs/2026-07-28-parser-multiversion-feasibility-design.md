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

**The multi-version design is feasible — proved, not assumed — and it is the
wrong thing to build. Fork one parser at the newest major instead.**

The diagnosis of `pg_query.rs` is correct. The prescription is not.

- `pg_query.rs` is genuinely under-maintained: 334 days without a crates.io
  release, 8 unreleased commits on `main`, **0 of 8 open issues have any
  maintainer reply**, and its PG 18 pull request is a draft carrying its author's
  own unresolved typo. Its sibling Ruby binding's release PR has been open 68
  days. Treat the arrival date of a PG-18 `pg_query` crate as **unbounded**.
- It is also **structurally incapable** of what was asked for: one git submodule,
  one generated `protobuf.rs`. No amount of maintenance quality would change
  that. Neither can any other Rust crate.
- **`libpg_query` (the C library) is healthy** and publishes exactly the
  granularity the design wants: long-lived `14-latest` … `18-latest` branches,
  each self-contained, each shipping machine-readable `srcdata/*.json`.
- **Both hard technical risks were retired by building them.** Five Postgres
  majors were symbol-prefixed, linked into one 13 MB binary, and verified to each
  accept exactly their own grammar with flat memory over 25,000 interleaved
  parses (§4). A srcdata-driven Rust generator ingested 32,144 regression
  statements with zero errors (§5).
- **Then the premise itself was tested, and it failed.** Across 50,970
  statements, PG18's grammar accepts everything PG14–17 accept — the only five
  regressions are regression-suite *negative* tests. Meanwhile per-version
  deparsers would inject **2.27–2.43% byte divergence** into `NormalizedBody`,
  the exact hash the multi-version design was meant to protect, where today's
  single deparser is byte-identical to PG18's on **2,199/2,199** of pgevolve's
  own fixtures (§11).
- Worse, N=5 adds a failure mode N=1 cannot have: routing a parse tree to the
  wrong version's deparser **aborts the process** — `abort()`, not `Err` (§11.3).
- And it fixes none of the real pain. pgevolve's cross-version difficulty comes
  from the **server's `ruleutils.c`** emitting differently-formatted DDL text,
  not from grammar differences. Five parsers eliminate exactly zero of those
  workarounds (§11.4).
- The thing that looked easy — **lowering parse nodes into pgevolve's IR** — is
  the actual cost centre under any design, and it does **not** benefit from
  codegen (§6).

**Recommended: ~8.5 engineer-weeks across 5 phases (§12), the first two of which
are correct under every possible outcome including upstream shipping next week.
The multi-version binding is shelved behind a dated, evidence-based kill gate
that current evidence says will never fire.**

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

## 11. The decisive experiment: is one current grammar enough?

Everything above establishes that five co-resident parsers are *buildable*. It
does not establish that they are *wanted*. That question was settled by
measurement, not argument.

**Method.** Five independent scanner binaries, one per libpg_query major (PG14
from the glibc-patched tree), run over a **50,970-statement** corpus: 48,771
statements split from libpg_query's 233 `postgres_regress` files, plus 2,199
split from pgevolve's own 858 `.sql` fixtures.

### 11.1 Acceptance is strictly monotone

| | PG14 | PG15 | PG16 | PG17 | PG18 |
|---|---|---|---|---|---|
| statements accepted | 48,048 | 48,475 | 48,700 | 49,115 | **49,488** |

Statements accepted by *any* older major but rejected by PG18: **5 of 50,970
(0.0098%)**. All five are PostgreSQL regression-suite **negative tests** — invalid
SQL that older grammars accepted and rejected later in parse analysis
(`PARTITION BY MAGIC (a)`, `SELECT JSON()`, `JSON_SCALAR()`, `JSON_SERIALIZE()`,
`JSON_TABLE()` in a target list). PG18 rejects them *earlier*, which is better
behaviour, not a regression.

**Zero valid SQL regresses.** For every purpose pgevolve has, the PG18 grammar is
a superset of PG14–17.

### 11.2 Per-version deparsers would *break* the invariant they were meant to protect

This is the finding that inverts the case.

`NormalizedBody::from_sql` depends on byte-equality of deparsed output. Measured
cross-major deparse divergence on the same corpus, whitespace-collapsed exactly
as `NormalizedBody` does:

| pair | divergent statements | rate |
|---|---|---|
| PG16 vs PG18 | 1,093 / 48,202 | **2.268%** |
| PG15 vs PG18 | — | 2.324% |
| PG14 vs PG18 | — | 2.428% |
| PG14 vs PG17 | — | 2.355% |
| PG16 vs PG17 | — | 2.193% |
| **PG17 vs PG18, on pgevolve's own fixtures** | **0 / 2,199** | **0.000%** |
| PG17 vs PG18, regression corpus | 36 / 49,115 | 0.073% |

Dominant divergence classes: `PARTITION BY RANGE(a)` → `RANGE (a)` (~600),
`::json` → `::pg_catalog.json` (~150), `DEFERRABLE` rendering, and
`DEFAULT 'foo'::text` → `DEFAULT ('foo'::text)`.

So: **today's single deparser produces byte-identical output to PG18's on
2,199/2,199 of pgevolve's fixtures, while five deparsers would inject 2.27–2.43%
divergence into the exact hash that decides whether two view bodies are the
same.** N=5 does not protect `NormalizedBody`; it is the thing that would break it.

A proposed repair — double round-trip (`parse@16 → deparse@16 → parse@18 →
deparse@18`) — does reduce divergence to 0.044%, but only because its terminal
canonicaliser is PG18. It works by silently reverting to the single-parser
architecture. It also introduces a new silent-data-loss class that N=1 does not
have: 7 of 48,202 statements fail to re-parse at all, and
`ALTER TABLE t DETACH PARTITION p FINALIZE` round-trips to
`... DETACH PARTITION any_namefinalize` — PG16/17's deparser omits a space (an
upstream bug fixed in PG18), and PG18 then re-parses the result as one
identifier, **silently swallowing the `FINALIZE` keyword**.

### 11.3 A process-abort hazard unique to N=5

`crates/pgevolve-core/src/parse/normalize_expr.rs:319-321`, verbatim:

> *"The `protobuf::ParseResult.version` field must match `libpg_query`'s embedded
> `PG_VERSION_NUM`, otherwise the C deparser asserts and aborts the process."*

Under N=5 every parse tree carries a version tag, and routing a tree to the wrong
version's deparser is an `abort()` — not a `Result::Err`. Unrecoverable,
uncatchable, and directly contrary to the constitution's no-panic posture. It is
the same cross-version mixing hazard as the enum-discriminant shifts, with a
worse failure mode. **N=1 cannot have this bug: only one version number exists.**

### 11.4 The real cross-version pain is not grammar pain

`crates/pgevolve-core/src/parse/normalize_body.rs:73-77` documents the actual
problem: *"PG14's `pg_get_viewdef` keeps the qualifier even when unambiguous,
while PG17 strips it."*

That difference originates in the **server's `ruleutils.c`**, not in libpg_query.
`strip_redundant_qualifiers` and `strip_redundant_string_casts` are workarounds
for *server text formatting*. **Per-version parsers eliminate exactly zero of
them.** This is the deepest structural reason the multi-version design does not
pay: it installs a version axis in the one layer where the versions do not
meaningfully differ.

### 11.5 Correction: pgevolve is *already* a multi-version program

The claim that "the version axis does not exist in pgevolve" is false, and
correcting it strengthens the case against N=5.
`crates/pgevolve-core/src/catalog/queries/` already contains `pg14.rs`, `pg15.rs`,
`pg16.rs`, `pg17.rs` and `pg18.rs`, alongside ~120 version-conditional paths
across `catalog/`, `plan/rewrite/`, `render/`. The version axis is deliberately
absent from **`parse/` only** — verified: `PgVersion` appears 155 times across 26
files and **zero** times under `crates/pgevolve-core/src/parse/`.

That is not an oversight. It is the correct design, and the 50,970-statement scan
is why.

*(A figure from the first pass — "`PgVersion` is used 337 times" — was not
reproducible; the real count is 155. The conclusion drawn from it was right, the
number was not.)*

---

## 12. Recommendation

**Feasible, but don't build it. Fork one parser at the newest major instead.**

The multi-version binding is technically achievable — that is now proved rather
than assumed. But the evidence says it would cost roughly 20+ engineer-weeks to
*introduce* a correctness regression in the deparse invariant, an unrecoverable
`abort()` failure mode, and a version axis in the only layer that doesn't need
one, while fixing none of pgevolve's actual cross-version pain.

The pain that *is* real — pgevolve advertising PG 18 support it does not have —
is fixed by a single vendored fork at PG 18.

### 12.1 Sequenced plan (~8.5 engineer-weeks)

| Phase | Deliverable | Effort | Gate |
|---|---|---|---|
| **0 — Stop the silent degradation** | Convert four confirmed silent-failure sites to typed hard errors; add a catalog preflight rejecting `attgenerated='v'`, `conenforced=false`, temporal constraints. Qualify the README's PG18 claim. | 1 ew | **GO unconditionally.** Correct under every future, including upstream shipping next week. |
| **1 — Seal the public API** | Remove `pg_query` from `pgevolve-core`'s public surface (`parse::Statement`'s 33 raw protobuf payloads). | 1 ew | **GO unconditionally.** This is what makes Phase 2 a one-day symbol rename — and makes abandoning the fork equally cheap. |
| **2 — `pgevolve-pgquery` 18.x** | Vendored fork: libpg_query `18.0.0` C sources as files (never a submodule), pg_query.rs 6.1.1's hand-written Rust, plus PR #79's delta with its author's own unresolved `AtalterConstraint` typo fixed. `protoc` path deleted; generated `protobuf.rs` checked in. Keep only the 4 entry points actually used. | 2.5 ew | **THE REAL KILL GATE.** Binary: full test suite + all 770 conformance fixtures pass against all five live PG servers with **zero fixture re-blessing**. Predicted PASS (PG17/PG18 deparse byte-identical on 2,199/2,199 fixtures). On unexplained failure: stop, keep the PG17 crate, 2.5 ew sunk. |
| **3 — Version oracle as *tooling*** | `xtask pg-oracle` links all five majors in CI and reports the minimum major accepting each statement; fails the build when a fixture's floor exceeds its claimed lint floor. Plus 4 new plan-time lints for the PG18 features. | 1 ew | GO if Phase 2 passed. Version rejection stays at **lint** time — moving it into the parser would make parse results depend on config. |
| **4 — PG18 semantics** | `VIRTUAL` generated columns, `NOT ENFORCED`, temporal PK/FK, named `NOT NULL`, `RETURNING WITH (OLD/NEW)` — each with IR, diff, render, lint, conformance fixture. | 3 ew | GO if Phase 2 passed. **Required identically even if upstream shipped tomorrow** — "just wait for upstream" argues against Phase 2, not against this. |
| **5 — Multi-version kill gate** | This document stays analysis and ships no code. One review on 2026-11-01 (also PG14 EOL, which shrinks the matrix to 15–18). | 0 (0.5 ew review) | **NO-GO by default.** Build N=5 only if: (a) a reproduced case where PG18 *silently mis-parses* — not rejects — DDL emitted by a PG14/15/16 server; (b) PG19 removes or retypes something pgevolve reads such that one grammar cannot serve the matrix; (c) maintaining the fork exceeds 1 ew per Postgres major, measured on the real PG19 bump. Trigger (a) currently stands at **zero of 50,970**. |

Note the elegant property of this sequence: **Phases 0 and 1 are correct under
every outcome**, including upstream shipping a PG18 release next week. They are
also precisely what makes switching back to upstream a one-day change. There is
no branch of the decision tree where they are wasted.

### 12.2 Why not simply wait for upstream

Because the arrival date is unbounded, and the live evidence is worse than the
commit graph suggests:

- PR #79 is a **draft with an empty description** carrying its author's own
  **unresolved** self-flagged typo (`AtalterConstraint` → should be
  `AtAlterConstraint`), 12 days old. That is consistent with unfinished work, not
  with a PR parked awaiting review.
- A direct question on it — kabudu, 2026-07-27, *"Any chance of this PR being
  reviewed and merged soon?"* — has **no maintainer reply**.
- **All 8 open issues have zero maintainer comments. A 0% response rate**,
  including a substantive architecture proposal with working forks (#72) and a
  build-debuggability bug filed with an offered patch (#80).
- The sibling bindings are stuck too, which is the strongest timeline signal:
  pganalyze's **flagship Ruby** release PR #346 ("Release 18.0.0"), opened by the
  founder on 2026-05-21, is **still unmerged 68 days later**. Go has no v18
  module path at all. A Rust release landing soon would require Rust to overtake
  the maintainer's primary binding.
- libpg_query 18.0.0 shipped **2026-05-21** — upstream has been ready for over
  two months.
- One open upstream risk to carry into Phase 2's gate: libpg_query issue #337,
  *"`pg_query_parse_plpgsql()` regressions in 18.0.0"*, is still **open**.
  pgevolve depends on plpgsql **analyzer semantics** (it selects `SETOF` vs
  `void` wrappers because the analyzer rejects `RETURN QUERY` in a non-`SETOF`
  wrapper), so this is a correctness risk, not a cosmetic one.

Waiting is not free — it is the status quo in which pgevolve keeps advertising
PG 18 support it does not have.

### 12.3 Silent-degradation sites to fix first (verified in-tree)

These exist **today**, independent of any parser decision:

- `catalog/assemble/tables.rs:245,256` — `attgenerated == "s"` with **no `"v"`
  arm anywhere** in `catalog/`. A PG18 virtual generated column is read as a
  plain column. Standing constitution §4 violation (stringly-typed closed set).
- `catalog/assemble/tables.rs:399-400` — `parse_fk_referenced_columns(&def)
  .unwrap_or_else(|| placeholder_idents(...))`.
- `catalog/assemble/views.rs:288` — `let Ok(parsed) = pg_query::parse(body_text)
  else { return vec![] }`. (The surrounding doc comment makes the skip
  deliberate, so this needs a small design decision, not just an error type.)
- `parse/normalize_body.rs:80` — `pg_query::deparse(&protobuf)
  .unwrap_or_default()`, self-labelled "silent graceful degradation".

---

## 13. Decisions that are the maintainer's, not mine

1. **Fork vs. wait.** The one genuine judgement call. The evidence removes the
   technical risk — migration re-bless cost measures at **zero** on pgevolve's
   own fixtures — but cannot bound upstream's arrival date. Suggested framing:
   start Phases 0 and 1 now (correct under every future), set a hard 4-week watch
   on `pganalyze/pg_query.rs`, and if 6.2.0 ships before Phase 2 begins, take it
   and skip Phase 2 entirely.
2. **Constitution §5 must be amended under every option, including doing
   nothing** — it names `pg_query = "6"` literally. Suggested rewording: rebind
   from a *crate* to a *technology* — "the official Postgres grammar and deparser
   via libpg_query, vendored per supported major" — and keep *"Parsing is not
   reimplemented"* verbatim. That clause gets **stronger** here, not weaker: we
   still link upstream's C grammar and upstream's hand-written C deparser.
3. **`deny.toml` needs `licenses.allow += "PostgreSQL"` today**, independent of
   this decision. Vendored Postgres sources carry the PostgreSQL License; it is
   absent from the allow-list with `exceptions = []`, and is invisible only
   because `pg_query.rs` declares `license = "MIT"` over ~250,000 lines of
   PostgreSQL-licensed C. **cargo-deny is currently returning a false green on a
   §2 assertion.** The licence is OSI-approved and functionally BSD-2-Clause
   equivalent — this is disclosure, not risk — but permanent allow-list entry vs.
   scoped exception is a policy call.
4. **Whether §6 adopts:** *"pgevolve does not claim support for a Postgres major
   until the conformance suite has a fixture for every feature that major
   added."* It has teeth — **zero `VIRTUAL` fixtures exist in `crates/` today** —
   and it is exactly what would have prevented the current false claim. It also
   binds future majors.
5. **Phase ordering** — whether Phase 4 (PG18 semantics) ships before or after
   the Phase 2 parser swap. Either is defensible once Phase 0 makes the README
   honest.
6. **After PG14 EOL (Nov 2026)** — whether the Phase 3 oracle keeps a PG14
   archive for historical lint verification or drops it.
7. **One free hour:** comment on PR #79 confirming the `AtalterConstraint` typo
   and offering to co-maintain. Costs nothing, delays nothing, and if upstream
   revives it, saves Phase 2 entirely.

---

## 14. Caveats on this analysis

- The 50,970-statement corpus is **authored DDL and regression SQL, not
  server-emitted `pg_get_viewdef` text**, and no live PG servers were available.
  The five enumerated PG17→PG18 deparser divergence classes are the complete list
  of what to look for, but Phase 2's gate must run against real servers.
- The codegen prototype proves **ingestion**, not round-trip losslessness
  (§5.2). Fields that are always zero across the corpus were never exercised.
- In a long-running harness parsing and deparsing tens of thousands of statements
  in one process, the **PG14/15/16 builds segfaulted around statement ~6,300**
  where PG17/18 did not. Statement-by-statement bisection found **no individual
  reproducer**, so this is cumulative and most likely a harness allocation-pattern
  artefact — **do not cite it as a libpg_query defect.** It is worth one hour of
  soak testing to rule out, since it is the class of bug that only appears in a
  long-lived process.
- Figures corrected during adversarial review: `PgVersion` usage (337 → 155);
  distinct protobuf fields read (198 → **182**, by compiler-assisted measurement —
  `grep` overcounted hot fields by up to 53×); deparse call sites (4 → **6**).
