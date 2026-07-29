# Own the Parser Binding — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `pg_query` crate dependency with a binding we own and vendor, and close the correctness gap between pgevolve's advertised "Postgres 14–18" support and its actual PG-17 parser.

**Operating assumption (maintainer decision, 2026-07-28):** `pganalyze/pg_query.rs` is treated as **permanently unmaintained**. There is no "wait for upstream" branch in this plan and no reversibility hedge. We own the binding from Stage 3 onward, forever.

**Architecture:** One vendored binding tracking the **newest supported Postgres major** — not one per major. The five-major design is analysed and rejected in the spec; §11 there records the 50,970-statement experiment that killed it. Version awareness stays where it already lives (catalog readers + plan-time lints), *not* in `parse/`.

**Tech Stack:** Rust, vendored libpg_query C (BSD-3-Clause) + PostgreSQL sources (PostgreSQL License), `cc` for the C build, `prost` initially (removed in the follow-on plan).

**Spec:** [`../specs/2026-07-28-parser-multiversion-feasibility-design.md`](../specs/2026-07-28-parser-multiversion-feasibility-design.md)

---

## Pre-flight

Before starting any stage:

1. Read [`docs/CONSTITUTION.md`](../../CONSTITUTION.md) — binding principles.
2. Read [`CLAUDE.md`](../../../CLAUDE.md) — operating directives, including the commit co-author trailer (§10) and the release ceremony (§11).
3. Read the spec end-to-end, especially **§6 (lowering)**, **§11 (the decisive experiment)** and **§12.3 (silent-degradation sites)**.
4. Confirm the working branch is green before starting.

## Per-stage verify gate (run before every commit)

```sh
cargo fmt --check                                            # 0 diffs
cargo clippy --workspace --all-targets -- -D warnings        # 0 warnings
cargo test --lib -p pgevolve-core                            # all pass
cargo test --lib -p pgevolve                                 # all pass
cargo test --lib -p pgevolve-testkit                         # all pass
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace   # cargo doc clean
```

From Stage 3 onward, additionally:

```sh
cargo package -p pgevolve-pgquery                            # packaging is a gate, not a surprise
                                                             # (verification is the default; --verify is not a flag)
```

> **Why `cargo package` is a standing gate:** upstream `pg_query.rs` has no such check, and its `main` is currently unpublishable because `build.rs` copies a header its `include` globs do not ship. We do not inherit that failure mode.

---

## Stage sequence

| Stage | Deliverable | Effort | Gate |
|---|---|---|---|
| 0 | ✅ **done** — Policy + honesty: `deny.toml`, constitution §5/§6, README | 0.5 ew | GO — unconditional |
| 1 | ✅ **done** — Silent-degradation sites → typed errors; PG18 catalog preflight | 1 ew | GO — unconditional; live bugs today |
| 2 | ✅ **done** — Seal `pg_query` out of `pgevolve-core`'s public API | 1 ew | GO — unconditional; makes Stage 4 mechanical |
| 3 | 🔶 **partial** — `pgevolve-pgquery` crate built and packaged, vendoring libpg_query **17**; PG18 bump blocked on source access | 2.5 ew | **KILL GATE** — unrun (needs PG18 + 5 live servers) |
| 4 | ✅ **done** — Cut `pgevolve-core` over; `pg_query` dropped from the graph | 0.5 ew | Zero fixture re-blessing ✅ |
| 5 | `xtask pg-oracle` + four PG18 plan-time lints | 1 ew | Oracle reproduces the acceptance matrix |
| 6 | PG18 semantics with conformance fixtures | 3 ew | A fixture per claimed feature |
| 7 | *(separate plan)* srcdata-generated typed AST; drop prost/protoc/bindgen | ~6 ew | Deferred — see §Stage 7 |

**Total through Stage 6: ~9.5 engineer-weeks.**

Stages 0–2 are correct under every outcome and touch no parser code. **All three are done.** Stage 3 is built against PG17 (see its scope note); the PG18 bump and the kill gate are the outstanding work.

---

## Stage 0 — Policy and honesty

Nothing here is optional and none of it depends on later stages.

- [x] **0.1** `deny.toml`: add `"PostgreSQL"` to `licenses.allow`, with a comment explaining that vendored Postgres sources carry it, that it is OSI-approved and functionally BSD-2-Clause-equivalent, and that its current absence is a **false green** — cargo-deny passes today only because `pg_query.rs` declares `license = "MIT"` over ~250,000 lines of PostgreSQL-licensed C.
- [x] **0.2** `docs/CONSTITUTION.md` §5: rebind from a crate to a technology. Replace the literal `pg_query = "6"` clause with a policy: *we maintain a vendored binding to libpg_query, tracking the newest supported Postgres major; re-extraction from PostgreSQL source via libpg_query's checked-in `scripts/extract_source.rb` + `patches/` is the documented fallback if libpg_query itself is abandoned.* **Keep "Parsing is not reimplemented" verbatim** — it gets stronger, not weaker: we still link upstream's C grammar and its 12,220-line C deparser.
- [x] **0.3** `docs/CONSTITUTION.md` §6: add *"pgevolve does not claim support for a Postgres major until the conformance suite has a fixture for every feature that major added."* This is what would have prevented the current false claim — there are **zero** `VIRTUAL` fixtures in `crates/` today.
- [x] **0.4** `README.md`: qualify the PG18 claim until Stage 6 lands. The current text ("Current release: v0.4.6 (Postgres 14–18)", "All actively-maintained PG majors… PG 14, 15, 16, 17, 18 covered") is not true of the parser.
- [x] **0.5** `crates/pgevolve-core/src/catalog/error.rs:49` — the `UnsupportedPgVersion` message reads `(supported: 14, 15, 16, 17)` while `PgVersion::Pg18` exists and `from_server_version_num` accepts `18`. Fix the string; add a test asserting the message lists every `PgVersion` variant so it cannot drift again.
- [x] **0.6** Update `docs/superpowers/specs/2026-06-07-virtual-generated-columns-design.md`: its §2 asserts "pg_query 6.1.1 ships the PG 18 grammar, so `VIRTUAL` parses", which is false. Re-point `status:` at this plan rather than `blocked-upstream`.

**Gate:** verify gate green; `cargo deny check licenses` passes *with* the vendored-licence entry present.

---

## Stage 1 — Convert silent degradation to typed errors

Four confirmed sites where a PG18 construct, or a parse failure, yields plausible-but-wrong IR instead of an error. All verified in-tree. **These are live bugs today, independent of every later stage.**

- [x] **1.1** `catalog/assemble/tables.rs:245,256` — `attgenerated` is tested with `== "s"` and there is **no `"v"` arm anywhere in `catalog/`**. A PG18 virtual generated column is currently read as a plain column with a `DEFAULT`. Replace the stringly test with an exhaustive decoder over a new enum (`''` → none, `'s'` → stored, `'v'` → virtual, anything else → `CatalogError`). This closes a standing **§4 violation** ("closed sets are always Rust enums, never strings or integers").
  - Add `GeneratedKind::Virtual` to `crates/pgevolve-core/src/ir/column.rs:132` and fix the stale doc comment at `:123` ("PG only supports stored as of v17").
  - Until Stage 6 implements virtual-column *semantics*, the decoder may map `'v'` to a typed `CatalogError` rather than to IR — but it must never map it to `Stored` or to a plain column.
- [x] **1.2** `catalog/assemble/tables.rs:399-400` — `parse_fk_referenced_columns(&def).unwrap_or_else(|| placeholder_idents(fk_attnums.len()))`. A foreign key whose `pg_get_constraintdef` text does not parse silently becomes a FK against *placeholder column names*, which will diff and plan wrongly. Propagate a new `CatalogError::UnparseableConstraintDef { constraint, def }`.
- [x] **1.3** `catalog/assemble/views.rs:288` — `let Ok(parsed) = pg_query::parse(body_text) else { return vec![] }`. A view body that fails to parse silently produces **zero dependency edges**, which can reorder or omit DDL in the plan. Note the surrounding doc comment (`:278-281`) makes best-effort extraction *deliberate*, so this needs a small design decision, not just an error type: either poison the plan, or keep best-effort but emit a `Finding` so it is visible. **Recommend: poison.** Silent dep-edge loss is a correctness bug in a tool whose entire job is ordering DDL.
- [x] **1.4** `parse/normalize_body.rs:80` — `pg_query::deparse(&protobuf).unwrap_or_default()`, self-labelled "silent graceful degradation" at `:68-70`. On deparse failure the *original* SQL becomes the canonical text, so two semantically identical bodies hash differently and pgevolve emits a spurious `CREATE OR REPLACE VIEW`. Return `BodyError::Deparse`.
- [x] **1.5** Catalog preflight: reject with a named, actionable error when introspecting a server that carries constructs we cannot yet represent — `attgenerated = 'v'`, `conenforced = false`, temporal (`PERIOD` / `WITHOUT OVERLAPS`) constraints. Better to refuse than to emit wrong IR.
- [x] **1.6** Tests for every branch above, including a fixture per new error variant.

**Gate (binary):** verify gate green, **and** a PG18 server carrying a `VIRTUAL` column, a `NOT ENFORCED` check and a temporal FK produces a named typed error at introspection rather than any IR at all. Zero code paths remain where a PG18 construct yields a silently-wrong `Column` or `Constraint`.

---

## Stage 2 — Seal `pg_query` out of the public API

`parse::Statement` is a 36-variant enum whose payloads **are** 33 raw `pg_query::protobuf::*Stmt` structs, and `lib.rs:25` re-exports `pub mod parse;`. So `pg_query` is part of `pgevolve-core`'s public API and any parser change is a semver break. **No caller outside `parse/` consumes those payloads** — this is fixable now and it is what makes Stage 4 a rename rather than a rewrite.

- [x] **2.1** Seal the module. Taken as the facade option, not the lowered-struct one: every submodule is now `pub(crate)` and the public surface is seven items — `parse_directory`, `parse_cluster_directory`, `parse_cluster_sources`, `parse_routine_body`, `ParseError`, `SourceLocation`, `NormalizedBody`/`BodyError`. `Statement` is `pub(crate)`. Replacing its payloads is Stage 7's job and doing it here would have collided with the Stage 4 cutover.
- [x] **2.2** Close the outbound sites. All 72 are gone; the count was 73 with one double-counted line. Landed as `parse::from_catalog` (rebuilding IR from server-emitted definition text) plus `parse::syntax` (syntax-only checks for render tests). The predicted shared helper was real: `parameter_list` replaced three near-identical 40-line walks in `functions.rs`, `casts.rs`, and `aggregates.rs` that differed only in error wording.
- [ ] **2.3** ~40 of the 102 used `NodeEnum` variants appear once or twice as `_ => Err(unsupported)` rejection arms; those need only a discriminant, not a struct. **Deferred to Stage 7** — this is an input to the srcdata-generated AST design, not a change that stands on its own. Nothing in Stage 4 depends on it.

**Gate (binary):** zero `pg_query` types reachable from `pgevolve-core`'s public surface; `cargo doc` clean; test suite unchanged and green.

**Result:** gate met. Zero parser references outside `src/parse/`, enforced by `tests/parser_containment.rs` (which also guards against passing vacuously). 2468 tests pass, 0 failures — up 4 from the 2464 at Stage 1, all new. Sealing also surfaced four dead items that `pub` had been masking from `dead_code` analysis, and ~57 lints that `avoid-breaking-exported-api` had been suppressing; both are noted in Stage 4's notes below since they are the same effect that will surface again if any module is re-exported.

---

## Stage 3 — `pgevolve-pgquery`: the vendored binding

A new workspace crate, published to crates.io, versioned to track the Postgres major it vendors.

> ### Scope adaptation — PG17 vendored first (maintainer decision, 2026-07-29)
>
> **The PG18 sources are unreachable from a Claude Code remote session.** Routes checked, all closed:
>
> | Route | Result |
> |---|---|
> | `codeload.github.com` tarball | HTTP 403 |
> | `api.github.com` repo/tags | *"GitHub access to this repository is not enabled for this session. Use add_repo…"* |
> | `add_repo pganalyze/libpg_query` | *"cross-tier adds are not supported in v1: session already has repos from owner(s) [saosebastiao]"* |
> | crates.io published `pg_query` | tops out at **6.1.1 = libpg_query 17**; PG18 lives only in unmerged draft PR #79 |
> | `ftp.postgresql.org` (re-extraction route) | unreachable |
>
> GitHub egress is mediated per-repository and only same-owner repos can be attached, so `pganalyze/*` is out of scope for this session by policy. This is an environment limit, not a plan defect.
>
> **Decision: vendor PG17 now, bump to 18 later.** Rationale: the plan's operating assumption is that `pg_query.rs` is *permanently unmaintained*, so getting off it is valuable on its own and is half the stated goal. The vendoring, build, stripping, and packaging work (3.2, 3.4, 3.5, 3.6, 3.7) is identical whichever major is inside `libpg_query/`, so none of it is wasted. The 17→18 bump reduces to: drop in the new tree, regenerate `protobuf.rs`, apply the PR #79 delta, update `VENDORED_PG_MAJOR`. Procedure recorded in the crate README.
>
> **Consequence, stated plainly:** Stage 3's kill gate is about PG17→PG18 deparser diffs, so it **remains unrun**. Vendoring PG17 does not close the PG14–18 support gap; it removes the unmaintained-dependency risk. The gap stays open until the bump plus Stage 6.
>
> **Kill gate is separately CI work.** It needs five live PG servers; `docker info` fails in this container and only PG16 binaries are present. Per maintainer decision it is delegated to CI regardless of the source-access question.

- [x] **3.1** Vendored **libpg_query 17 (PG 17.4, `PG_VERSION_NUM 170004`)** as files in-tree — see the scope note above for why 17 and not 18. Not a submodule, not a download. **Measured on the real artifact: 524 files, 11.5 MiB uncompressed, 1.9 MiB compressed — 19% of the 10 MiB crates.io limit**, confirming the plan's 2.08 MB estimate.
- [x] **3.2** `build.rs` using `cc` with the `parallel` feature: 69 objects (13 `src/*.c` + 53 `src/postgres/*.c` + protobuf-c + xxhash + `pg_query.pb-c.c`), four include dirs plus two more on Windows, `-fno-strict-aliasing` and `-fwrapv` via `flag_if_supported`. No Make, no Ruby, no protoc, no network. Also dropped upstream's `fs_extra` copy of the whole 11 MB tree into `OUT_DIR` — `cc` writes objects there regardless, so the copy bought nothing.
- [x] **3.3** Ported 6.1.1's hand-written Rust, rewritten rather than copied: `error.rs`, `query.rs`, `parse_result.rs`, `node.rs`, `ffi.rs`, `lib.rs`. **The PG18 delta from draft PR #79 (including the `AtalterConstraint` → `AtAlterConstraint` typo fix) is deferred with the major bump** — it cannot be applied without the PR, which is in the unreachable repo.
- [x] **3.4** **Deleted the protoc path entirely.** Upstream's `build.rs:72-83` sets `OUT_DIR` to its own `src/` and renames prost output over `src/protobuf.rs`, which mutates the Cargo registry cache on any machine with `protoc` on `PATH` (reproduced: mtime changed). Check the generated `protobuf.rs` in as an ordinary source file.
- [x] **3.5** Keep only what pgevolve uses: `parse` (73 sites), `deparse` (6 production sites), `parse_plpgsql` (1 site), `Error`, `ParseResult`, `protobuf::*`, `NodeEnum`, `NodeRef::deparse`. **Delete** `nodes()` (covers 39 of 268 node types, zero uses here), `normalize`, `fingerprint`, `scan`, `split`, `truncate`, `.tables()`, `summary`, and the `NodeMut` raw-pointer machinery that only `truncate` needed.
- [x] **3.6** MSRV (`rust-version` from the workspace), `[lints] workspace = true`, `[package.metadata.docs.rs]`, keywords/categories, and an explicit `include` list. Dropped `itertools`, the dead `clippy = "0.0.302"` optional build-dep, `prost-build`, and `fs_extra`.
- [x] **3.7** Justified `#![allow(...)]` header on `src/protobuf.rs` and `src/ffi.rs` only; every hand-written module is held to the full workspace lint set and passes `-D warnings`. **Measured 3,394 warnings, roughly double the plan's ~1,700 estimate** — ~2,700 are `missing_docs` on the fields and variants of 272 generated message types, ~220 are missing backticks from C comments carried into Rustdoc, the rest style lints on generated code.
- [~] **3.8** **Re-extraction drill — procedure recorded, not executed.** Run libpg_query's own `make extract_source` pipeline once (`scripts/extract_source.rb` + `extract_headers.rb` + `extract_pg_types.rb` + `generate_protobuf_and_funcs.rb` + the 11 patches, downloading `postgresql-18.4.tar.bz2`) and record the procedure in the crate's README. This proves the bottom of our dependency stack is **PostgreSQL itself**, not libpg_query — worth knowing given libpg_query's own 83%-one-person bus factor. Needs Ruby on a maintainer machine; **not** a consumer build dependency.
- [x] **3.9** Soak test: parse+deparse ≥50,000 statements in one long-lived process. Landed as `parse::soak` (`#[ignore]`d; `cargo test -p pgevolve-core --lib -- --ignored soak`). **Result: 50,577 statements over 23 passes of 858 SQL files — 0 parse failures, 0 deparse failures, no crash, 10/10 consecutive runs identical.**
  - Cross-validation: the corpus yields exactly **2,199 statements per pass**, matching the figure the spec's own experiment recorded. The harness is measuring what the spec measured.
  - **Scope limit, stated because it is easy to overclaim here:** the crate linked today is the **PG17** build — one of the builds the spec says did *not* crash. So this rules out nothing about PG14/15/16 (not linkable without a per-major binding, the architecture the spec rejected) and nothing about the vendored PG18 build. **Re-run after the Stage 4 cutover**; that is when it becomes evidence about the binding pgevolve ships.
  - Value delivered now regardless: the harness exists, is deterministic, asserts against silent degradation (a run that quietly stopped parsing would still survive to the end and report "no crash"), and gives Stage 4 a before/after baseline instead of a first-ever measurement.
- [~] **3.10** plpgsql check — **baseline captured on PG17, PG18 check deferred with the bump.** `query.rs` has two characterization tests pinning the analyzer behaviour pgevolve actually depends on: it must reject `RETURN QUERY` in a non-`SETOF` function, and accept a valid `void` body. Those are the assertions a PG18 build has to keep passing. Issue **#337** (`pg_query_parse_plpgsql()` regressions in 18.0.0) is still **open**. pgevolve depends on plpgsql *analyzer semantics* — it selects `SETOF` vs `void` wrappers because the analyzer rejects `RETURN QUERY` in a non-`SETOF` wrapper — so this is a correctness risk, not cosmetic. Explicit exit-criterion line item, not a footnote.

**Result so far (2026-07-29):** the crate exists, builds, packages, and passes. 10 unit tests + 5 doctests green; `cargo package` verifies (it compiles from the generated tarball — upstream's `main` does not); workspace `fmt`, `clippy -D warnings`, and `cargo doc -D warnings` clean; full suite 2,478 pass / 0 fail.

Two findings worth carrying forward:

- **`NodeRef` was a round trip to nowhere.** Upstream reached single-node deparsing via `NodeEnum::to_ref()` → `NodeRef::deparse()` → `NodeRef::to_enum()` — a clone into a borrowed view and a clone straight back — to arrive at a five-line wrapper. That cost ~4,100 lines of generated conversion tables (`node_ref.rs`, `node_structs.rs`, and the `to_ref`/`to_mut` half of `node_enum.rs`) to serve **two** call sites in pgevolve. `NodeEnum::deparse` is called directly now.
- **The deparser canonicalizes SQL-standard type spellings but not Postgres-internal aliases.** Measured: `integer`→`int`, `character varying(10)`→`varchar(10)`, `timestamp without time zone`→`timestamp`, `decimal(5,2)`→`numeric(5, 2)`; but `int4`, `bool`, and `float8` pass through unchanged, so `bool` and `boolean` do **not** converge. Deparsing is therefore necessary but not sufficient for type equality — which is exactly why `ColumnType` normalization exists. Pinned by a characterization test so a change in either direction is visible.

### Stage 3 kill gate (binary)

> The entire existing test suite **plus all 770 conformance fixtures** pass against all five live PG servers with **zero fixture re-blessing**, and every observed diff is attributable to one of the five measured PG17→PG18 deparser classes: `ALTER CONSTRAINT` deferrability; `SET timezone`/`xmloption`; `ARRAY` subscript parenthesisation; `COPY FREEZE`; the `DETACH PARTITION FINALIZE` space fix. **Zero unexplained diffs, zero new parse failures.**

**Predicted PASS** — PG17 and PG18 deparsers are byte-identical on 2,199/2,199 pgevolve fixture statements, and only one of the five classes (`ARRAY[1,2][i]` → `(ARRAY[1,2])[i]`) can appear inside a view body, default expression or CHECK constraint.

**On unexplained failure: STOP.** Keep the PG17 crate, re-open the decision with 2.5 ew sunk.

---

## Stage 4 — Cut over

- [x] **4.1** Replaced in `[workspace.dependencies]` (`path` + `version`, so the published crate resolves from crates.io) and in `crates/pgevolve-core/Cargo.toml`. **No `pgevolve` dev-dependency to change** — `tests/cast_e2e.rs`'s only mention of the parser was a comment, so that crate never depended on it. `pg_query` is now absent from `cargo tree` and has zero entries in `Cargo.lock`.
- [x] **4.2** Renamed across 46 files, all under `crates/pgevolve-core/src/parse/` — Stage 2's containment held, so this was the rename it was supposed to be. **One non-mechanical change:** `NodeRef` is gone from the new crate, so the two `.to_ref().deparse()` sites became `.deparse()` directly (identical code path — upstream's `NodeRef::deparse` cloned back to a `NodeEnum` and built the same wrapper). Prose was also corrected: ~110 comments describing AST encoding now say `libpg_query` (accurate — it is the C parser's encoding, and that library is still what is vendored), version-pinned claims like "pg_query 6.x" became "libpg_query 17", and the user-facing `BodyError::Parse` message no longer names a crate pgevolve does not depend on.
- [x] **4.3** CLAUDE.md §11 updated: three crates, `pgevolve-pgquery` first (core depends on it by `version`, so the index must carry it before core resolves). The never-publish-before-green rule is preserved verbatim, plus two rules specific to the new crate — `cargo package` before tagging, and never regenerate `src/protobuf.rs` as part of a release.
- [x] **4.4** `PARSER_CRATE` now spells `pgevolve_pgquery`; both containment tests pass, including the one that fails if `parse/` stops naming the parser (so the guard cannot pass vacuously against a stale constant).

**Gate: met, with one item deferred.** Verify gate green — fmt, `clippy --workspace --all-targets -D warnings`, `cargo doc -D warnings`, 2,478 tests pass / 0 fail, 5 doctests pass. **Zero fixture re-blessing, literally:** no file matching `fixture`/`.snap`/`expected`/`golden` changed, and the test count and results are identical to the pre-cutover run.

Strongest evidence the swap is behaviour-preserving: the soak test reports **byte-identical figures before and after** — 50,577 statements, 23 passes, 2.4 MiB deparsed, 0 parse failures, 0 deparse failures. Same vendored C, same output.

`cargo deny check` **not run — cargo-deny is not installed in this environment.** It is a CI gate and Stage 0 already added the `PostgreSQL` licence to `deny.toml`'s allow-list in anticipation of exactly this vendoring, so the expectation is green; but it is unverified here and should not be reported otherwise.

### Notes carried from Stage 2

Two effects surfaced when `parse`'s submodules went from `pub` to `pub(crate)`, and both will surface again for any module that changes visibility:

- **`dead_code` starts seeing the module.** A `pub` item in a `pub` module is reachable by definition, so the lint never fires. Sealing revealed four genuinely-dead items: the whole `builder::Builder` accumulator (superseded by `ParseContext`, zero references), `AttachPartition::parent` (duplicated `partition_of.parent`), and `IndexNameKind::Exclude` (kept with a justification — it mirrors PG's closed set of name suffixes).
- **`avoid-breaking-exported-api` stops suppressing clippy.** ~57 `redundant_pub_crate` findings plus one `struct_field_names` appeared at once, none of them new code. `cargo clippy --fix` handled all but the last.

Neither is a reason to avoid sealing; both are reasons to expect a lint burst and budget for it rather than reading it as breakage.

---

## Stage 5 — Version oracle and PG18 lints

The one genuinely valuable idea from the multi-version proposals, extracted from the runtime and moved to CI where it costs almost nothing.

- [ ] **5.1** `xtask pg-oracle`: link all five libpg_query majors (they build clean in 58 s on 4 cores) and report, for any statement, the **minimum major that accepts it**. Runs in CI over the conformance corpus and **fails the build** when a fixture's minimum accepting major exceeds the lint floor claimed for it.
- [ ] **5.2** Four plan-time lints on the pattern already proven four times in-tree (see `lint/rules/builtin_provider_requires_pg_17.rs`): `virtual_generated_column_requires_pg_18`, `not_enforced_constraint_requires_pg_18`, `temporal_key_requires_pg_18`, `returning_old_new_requires_pg_18`.

> **Version rejection stays at LINT time.** The PG18 grammar accepts the superset; the lint tells the user their *target server* cannot run it. Do **not** move rejection into the parser — that would make parse results depend on config and destroy parse-once-plan-for-many-targets.

**Gate:** the oracle reproduces the measured acceptance matrix (PG14 48,048 / PG15 48,475 / PG16 48,700 / PG17 49,115 / PG18 49,488 over the 50,970-statement corpus), and CI fails on a deliberately mis-floored fixture.

---

## Stage 6 — PG18 semantics

Each with IR, diff, render, lint **and a conformance fixture**. This work is required identically under every strategy — anyone arguing "just wait for upstream" is arguing against Stage 3, not against this.

- [ ] **6.1** `VIRTUAL` generated columns — `GeneratedKind::Virtual` gains real semantics; the Stage 1.1 decoder gains its `'v'` → IR arm.
- [ ] **6.2** `NOT ENFORCED` constraints (zero `conenforced` references exist in `crates/` today).
- [ ] **6.3** Temporal PK/FK — `PERIOD`, `WITHOUT OVERLAPS`.
- [ ] **6.4** Named `NOT NULL` constraints.
- [ ] **6.5** `RETURNING WITH (OLD AS o, NEW AS n)`.
- [ ] **6.6** Restore the unqualified PG18 claim in `README.md` and constitution §6 — **only** once every one of these has a passing fixture (the §0.3 rule).

**Gate (binary):** a conformance fixture exists and passes for every feature PG18 added that pgevolve claims, and the Stage 5 oracle confirms each new fixture's minimum accepting major is 18 with a matching lint floor.

---

## Stage 7 — *(Deferred to a separate plan)* srcdata-generated typed AST

Under permanent ownership we maintain a generation pipeline either way, so the question becomes which pipeline — not which is closest to upstream. The evidence favours replacing prost:

- `contype: i32`, `fk_del_action: String`: 496 of 1,404 protobuf fields (35%) are untyped node references and 118 more are `i32`-with-a-hint — a standing **§4 violation** inherited from upstream.
- `protobuf::Node` is **584 bytes** (prost emits non-recursive variants unboxed); a uniformly-boxed binding reaches ~16.
- JSON parse is **3.2× faster** than protobuf decode (11.7 vs 37.9 µs/parse).
- Codegen from `srcdata/*.json` is proven: 32,144/32,144 regression statements ingested clean under `deny_unknown_fields`, with exactly **two** hand-written shims (`A_Const` value inlining; bare `{}` for NULL list elements).
- Deparse needs protobuf **input only**; its 255 field numbers are 100% derivable from srcdata (reproduced exactly), and it does **not** require byte-exact encoding (1,989/1,989 deparsed identically from deliberately-different bytes).

Net: JSON in through a srcdata-generated strongly-typed AST, generated write-only protobuf encoder out for deparse — dropping `prost`, `prost-build`, `protoc` **and** `bindgen` (and libclang with it).

**Why deferred:** this changes the AST shape, so it is a genuine rewrite of the translation layer rather than a rename. Do it as an independently-testable change behind the Stage 2 boundary, never concurrently with Stage 4. Two open risks to retire first (spec §5.2): the codegen proof covers **ingestion, not round-trip losslessness** — fields always zero across the corpus were never exercised — and no re-serialise comparison was run.

---

## Explicitly out of scope

The **five-major co-resident binding**. It is buildable — proved end-to-end (spec §4) — and it is rejected on evidence, not on effort:

- PG18's grammar is a superset for every purpose pgevolve has: 50,970 statements, strictly monotone acceptance, and the only 5 regressions are regression-suite *negative* tests.
- Per-version deparsers would inject **2.27–2.43%** byte divergence into `NormalizedBody` — the exact invariant N=5 was meant to protect — where today's single deparser is byte-identical to PG18's on 2,199/2,199 fixtures.
- N=5 adds a failure mode N=1 cannot have: a tree routed to the wrong version's deparser **aborts the process** (`parse/normalize_expr.rs:319`), not `Err`.
- pgevolve's real cross-version pain is the **server's `ruleutils.c`** emitting differently-formatted DDL. Five parsers eliminate zero of those workarounds.

**Reconsider only if** one of these fires: (a) a reproduced case where the PG18 grammar *silently mis-parses* — not rejects — DDL emitted by a PG14/15/16 server at one of the 12 `catalog/assemble` re-parse sites; (b) PG19 removes or retypes something pgevolve reads such that one grammar cannot serve the matrix; (c) maintaining `pgevolve-pgquery` exceeds 1 ew per Postgres major, measured on the real PG19 bump.

Trigger (a) currently stands at **zero of 50,970**.
