# Constitution Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve every HIGH and MEDIUM finding from the 2026-05-21 constitution audit, plus the cosmetic LOW items, so that the codebase materially complies with `docs/CONSTITUTION.md` end-to-end.

**Architecture:** Six sequential stages. Each stage is independently shippable and produces a coherent commit set. Earlier stages have higher impact-to-effort ratio and minimal architectural risk; later stages are larger refactors. Stages 1-2 can ship in a single session; 3-4 are a session each; 5-6 are multi-session efforts.

**Audit reference:** Findings from the 2026-05-21 constitution audit (this session's analysis output). 18 production `unwrap`/`expect` sites, 9 HIGH findings across §3/§5/§7/§8/§9/§10, multiple MEDIUM and LOW items.

---

## Stage 1 — Quick wins, CI hygiene, docs (~2 hours)

Highest impact-to-effort ratio. No architectural risk. Touches CI workflows, top-level docs, and a handful of small code changes.

### Task 1.1: CI — add `cargo deny check` + `cargo doc` jobs

**Files:** `.github/workflows/ci.yml`

Add two jobs to `ci.yml` so the §2/§3/§9 policy is mechanically enforced on every PR:

```yaml
  deny:
    name: cargo deny check
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: EmbarkStudios/cargo-deny-action@v2
        with:
          command: check

  doc:
    name: cargo doc
    runs-on: ubuntu-latest
    env:
      RUSTDOCFLAGS: "-D warnings"
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { toolchain: "1.95", components: "rustfmt,clippy" }
      - run: cargo doc --workspace --no-deps
```

Verify the workflow file parses: `actionlint .github/workflows/ci.yml` if available, or manually inspect.

Commit message:
```
ci: enforce cargo deny + cargo doc on every PR

Closes the §9 gap surfaced by the constitution audit: cargo deny
check (license + advisory policy from deny.toml) and cargo doc
(rustdoc warnings as errors) were configured locally but never
run on PRs. A PR adding a GPL dep or breaking docs.rs builds would
have passed CI undetected.
```

### Task 1.2: `SECURITY.md`

**Files:** `.github/SECURITY.md` (new)

Per constitution §10 line 91. Use a tight template:

```markdown
# Security Policy

## Reporting a vulnerability

Email **security@<DOMAIN>** with the following:

- A description of the issue and its potential impact.
- Steps to reproduce (proof-of-concept code is ideal).
- Affected versions and configurations.
- Whether you intend to publish your findings, and on what timeline.

We will acknowledge your report within **7 days**. We aim to provide a fix or mitigation within **30 days** for medium-severity issues, and faster for criticals.

After a fix is available we will publish the issue with a CVE where applicable. Credit is given to the reporter unless they ask to remain anonymous.

We do not pursue legal action against good-faith security researchers.

## Supported versions

| Version | Supported |
|---|---|
| 0.2.x | ✅ |
| 0.1.x | ❌ (please upgrade) |
```

Replace `<DOMAIN>` with whatever email channel the maintainer wants (placeholder OK; user can edit before merge).

### Task 1.3: `CODEOWNERS` + release runbook

**Files:** `.github/CODEOWNERS` (new), `docs/RELEASING.md` (new)

`CODEOWNERS`:
```
* @saosebastiao
```

`docs/RELEASING.md`:
```markdown
# Releasing pgevolve

## Checklist

1. Verify `cargo test --workspace --all-targets` passes locally.
2. Verify `cargo clippy --workspace --all-targets -- -D warnings`.
3. Verify `cargo fmt --all -- --check`.
4. Verify `cargo deny check` (install: `cargo install cargo-deny`).
5. Update `[workspace.package].version` in `Cargo.toml`.
6. Update the matching version in `crates/pgevolve-core-macros/Cargo.toml`.
7. Add a date-stamped section in `CHANGELOG.md` for the new version. Move items out of `[Unreleased]`.
8. Run `cargo build --workspace` so `Cargo.lock` updates with the new version.
9. Commit: `git commit -am "release: vX.Y.Z"`.
10. Tag with **signing**: `git tag -s vX.Y.Z -m "pgevolve vX.Y.Z"`.
11. Push: `git push origin main && git push origin vX.Y.Z`.
12. (Optional, when ready) `cargo publish -p pgevolve-core` then `cargo publish -p pgevolve`.

## Why signed tags

Constitution §9 requires it. `git tag -s` uses your GPG/SSH signing key as configured via `git config user.signingkey` and `git config gpg.format`.
```

### Task 1.4: Track v0.3 Planned features as GitHub issues

**Tool:** `gh issue create` for each.

Open one issue per Planned row in `docs/spec/objects.md` (per constitution §5: gaps must be tracked):

1. ROLE / CREATE USER — `objects.md:248`
2. GRANT / REVOKE / ALTER DEFAULT PRIVILEGES — `objects.md:249`
3. POLICY (RLS) + ENABLE ROW LEVEL SECURITY — `objects.md:250`
4. CREATE STATISTICS — `objects.md:285`
5. Toast column storage strategy — `objects.md:270`

Each issue: title "v0.3: implement <feature>", body referencing the objects.md line and constitution §5.

### Task 1.5: Trivial code cleanups

**Files:** `crates/pgevolve-core/src/lint/universal.rs`, `crates/pgevolve-core/src/plan/policy.rs`, `Cargo.toml`

- Remove the two `#[inline]` markers from `is_id_start` / `is_id_char` in `lint/universal.rs:504,509` (no profiler evidence per §8).
- Rename `check_not_valid_then_validate` in `plan/policy.rs:99` to `is_check_not_valid_rewrite_enabled` (predicate naming per §8).
- Update the `bincode` version-pin comment in `Cargo.toml` to reflect that bincode 3.0.0 is published.
- Normalize lint rule function names in `lint/universal.rs`: strip `_rule` suffix from all 11 functions OR add it to the other 7. Recommend stripping (shorter).

### Task 1.6: Hoist `walkdir` + `tempfile` to workspace

**Files:** `Cargo.toml`, `crates/pgevolve-core/Cargo.toml`, `crates/pgevolve-conformance/Cargo.toml`, `crates/pgevolve/Cargo.toml`, `xtask/Cargo.toml`

Move `walkdir = "2"` and `tempfile = "3"` into `[workspace.dependencies]`. Change per-crate refs to `{ workspace = true }`.

### Task 1.7: Stage 1 verify + commit

```bash
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo doc --workspace --no-deps 2>&1 | grep -cE "^warning"   # should be 0
```

One commit per task. Push at end of stage.

---

## Stage 2 — Release-process hardening (~30 min)

### Task 2.1: Sign-from-now-on policy

The historical v0.1.0 and v0.2.0 tags are unsigned. Two options — pick one:

**Option A (clean):** Force-push retagged signed versions:
```bash
git tag -d v0.1.0
git tag -s v0.1.0 <v0.1.0-sha> -m "..."
git push origin --force v0.1.0
# same for v0.2.0
```
Risk: anyone who fetched the old unsigned tag locally needs to re-fetch.

**Option B (conservative):** Document the historical violation in `docs/RELEASING.md`, commit to signing v0.3.0 forward. Add a CHANGELOG entry under `[Unreleased]` noting the policy change.

User chooses. Default to Option B unless they explicitly ask for A.

### Task 2.2: CI check that `[workspace.package].version` matches the latest `CHANGELOG.md` section

**Files:** `.github/workflows/ci.yml` (extend the existing `doc` or `fmt` job)

Add a step:

```yaml
  changelog:
    name: CHANGELOG version sync
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Verify Cargo.toml version has a CHANGELOG entry
        run: |
          v=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
          if [ "$v" = "0.2.0" ] || [ "$v" = "*-dev" ]; then exit 0; fi
          grep -q "^## \[$v\] — " CHANGELOG.md || {
            echo "::error::Cargo.toml version $v has no CHANGELOG entry"
            exit 1
          }
```

Adjust per the actual `Cargo.toml` shape. The point is: a PR bumping the version but forgetting to date-stamp the CHANGELOG entry fails CI.

---

## Stage 3 — Dependency modernization (~half day)

Touches code but is bounded. Each task is a single coherent refactor.

### Task 3.1: Drop `async-trait`

**Files:** `crates/pgevolve/src/shadow/*.rs`, `crates/pgevolve-testkit/src/test_pg_backend.rs`, the two `Cargo.toml` files declaring `async-trait`

At MSRV 1.95, native async fn in traits (AFIT, stable since 1.75) makes `async-trait` redundant.

For each `#[async_trait]` use site:
- Remove the attribute on both the trait and its impls.
- For traits where `Send`-bound futures are needed at the call site, callers may need an explicit `+ Send` on the trait's associated future type, or the trait may need to be marked `#[trait_variant::make(Send)]` if a `Send`-bound version is genuinely required. Most pgevolve call sites don't need this.

Verify `cargo test -p pgevolve --tests` and `cargo test -p pgevolve-testkit --tests` both pass.

Remove `async-trait` from both `Cargo.toml` files.

### Task 3.2: Replace `bincode` with `serde_json` in `PlanId::compute`

**Files:** `crates/pgevolve-core/src/plan/plan.rs`, `Cargo.toml`

`PlanId::compute` calls `bincode::encode_to_vec(...)` twice solely to produce deterministic bytes for BLAKE3. `serde_json::to_vec` is also deterministic for Serde-derived types and is already a workspace dep.

```rust
// Before
let source_bytes = bincode::encode_to_vec(source, config::standard())
    .expect("Catalog is bincode-serializable");

// After
let source_bytes = serde_json::to_vec(source)?;
```

`PlanId::compute` becomes `fn compute(...) -> Result<PlanId, SerializeError>`. Caller-site updates flow naturally.

Drop `bincode` from `[workspace.dependencies]` and from `crates/pgevolve-core/Cargo.toml`. **Also closes** the §7 `expect("Catalog is bincode-serializable")` violation (HIGH from Audit-2 #7).

### Task 3.3: Replace `glob` with `globset` (or inline regex)

**Files:** `crates/pgevolve-core/src/catalog/filter.rs`, `lint/mod.rs`, `parse/mod.rs`, `catalog/error.rs`, plus the test using `glob::Pattern`

`globset` (BurntSushi) is the better-maintained equivalent. Drop-in is roughly:

```rust
use globset::{Glob, GlobMatcher};

let pattern: GlobMatcher = Glob::new(pat)?.compile_matcher();
let matched = pattern.is_match(path);
```

Replace every `glob::Pattern::new(...)` and `glob::Pattern::matches_path(...)`. Drop `glob` from `Cargo.toml`, add `globset = "0.4"` to `[workspace.dependencies]`.

### Task 3.4: Migrate `parse_fk_referenced_columns` to `pg_query`

**Files:** `crates/pgevolve-core/src/catalog/assemble.rs:488`

The function does hand-written string scanning over `pg_get_constraintdef` output to extract the `(col, col)` list after `REFERENCES`. Per constitution §5: "parsing is not reimplemented."

Replace with a `pg_query::parse` call wrapping the constraint def in a synthetic `ALTER TABLE t ADD CONSTRAINT c <constraintdef>`, then navigating the AST to find the `Constraint` node's `pktable.fk_attrs` field.

### Task 3.5: Stage 3 verify + push

```bash
cargo build --workspace
cargo test --workspace --lib
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
```

---

## Stage 4 — §7 `unwrap`/`expect` purge (~2-3 hours)

18 production sites across 10 files. Each gets converted to typed-error propagation or restructured so the invariant is type-enforced rather than runtime-asserted.

Order tasks by API impact (smallest first):

### Task 4.1: Pure-internal restructures (no API change)

- `identifier.rs:49` and `:95` — restructure `chars.next().expect(...)` after `is_empty()` checks using `if let Some(...)` to make the empty-string path explicit.
- `parse/normalize_expr.rs:99,103,106,151` — add `// SAFETY:` comments + restructure with `if let Some(...)` rather than `expect()`.
- `catalog/rows.rs:139` — restructure the double `chars()` call.
- `parse/builder/create_stmt.rs:747,748` — replace `parts.pop().unwrap()` with `let [s, n]: [_; 2] = parts.try_into()...`.
- `plan/rewrite/set_not_null_check_pattern.rs:29` — make the synthetic identifier a `const` constructed at compile time.
- `plan/rewrite/emit/extension.rs:109` — make `Identifier::from_unquoted("pg_extension")` a `const` or `LazyLock`.

### Task 4.2: API-changing fixes

- `plan/plan.rs:63,65` — already addressed by Stage 3.2 (serde_json migration).
- `executor/status.rs:183` — propagate `serde_json::to_string_pretty` error.
- `catalog/assemble.rs:350,462` — propagate `CatalogError` from helper functions instead of expecting on missing OIDs / valid synthesized identifiers.
- `pg_querier.rs:59,64` — Mutex poison `expect()` is idiomatic; document with `// SAFETY:` comment.

### Task 4.3: Verify count is 0

```bash
grep -rn "\.unwrap()\|\.expect(" \
  crates/pgevolve-core/src crates/pgevolve-core-macros/src crates/pgevolve/src \
  --include='*.rs' | grep -v "#\[cfg(test)\]" | grep -v "/tests/" | wc -l
```

Should be 0 (or only sites with `// SAFETY:` comments that document why they're acceptable).

---

## Stage 5 — Large-file splits (~1-2 sessions)

The two large files identified in Audit-4. Each split is its own coherent refactor; they don't interact, so they can land independently.

### Task 5.1: Split `catalog/assemble.rs` (2018 → ~600 + 3×400)

**Files:** `crates/pgevolve-core/src/catalog/assemble/{mod.rs, tables.rs, functions.rs, views.rs}` (mod restructure)

Plan:
- `assemble/mod.rs` keeps the top-level orchestrator (`fn assemble`) and the small parsers (`parse_referential_action`, `parse_match_type`).
- `assemble/tables.rs` — `build_tables`, `parse_check_expression`, `parse_default_expr_text`, `parse_index_def`, and table-level helpers.
- `assemble/functions.rs` — `build_functions_and_procedures`, `parse_arg_full`, `extract_body_from_functiondef`, `find_as_dollar_quote_pos`, `parse_return_type_from_string`, `parse_table_return_columns`.
- `assemble/views.rs` — `build_views_and_mvs`, `walk_node_for_deps`, `extract_deps_from_body`.
- `assemble/partitions.rs` — the partition metadata merge added in PART4 (already factored into helper functions).

The file already has natural seams; the split is mechanical. Each helper becomes `pub(super)` or `pub(crate)`. Verify no public-API changes leak outside `catalog/`.

### Task 5.2: Split `lint/universal.rs` (2672 → many per-rule files)

**Files:** `crates/pgevolve-core/src/lint/rules/<rule-id>.rs` + restructured `lint/universal.rs`

Replicate the proven `plan/rewrite/emit/` pattern: one file per lint rule (or one file per logical cluster — e.g., all "unmanaged reference" rules together). `universal.rs` becomes a thin dispatcher.

Naming convention: `lint/rules/trigger_references_unmanaged_table.rs` exports a `pub(super) fn check(catalog: &Catalog) -> Vec<Finding>` (or similar — match what works). `lint/universal.rs::check_universal` calls each.

Move the test modules along with their rules.

### Task 5.3: (Optional) Split `plan/rewrite/sql.rs` (850 lines)

If it's already over 1000 lines by the time you get here, do it. Otherwise defer to a follow-up. Group by DDL family (`table.rs`, `schema.rs`, `index.rs`, etc.).

---

## Stage 6 — Conformance fixture expansion (~half day)

Per Audit-3 finding: `columns` family has 1 fixture, `tables` family has 2. Two of the most foundational object families are the most under-tested directly.

### Task 6.1: Add `columns/` fixtures

**Files:** `crates/pgevolve-conformance/tests/cases/objects/columns/*` (new directories)

Each fixture is `before.sql` + `after.sql` + `fixture.toml` + `expected/` (bless-generated).

Targets:
1. `drop-column-nullable/` — drop a nullable column. Steps = 1, intent-gated.
2. `drop-column-with-data-loss/` — drop a column with a default; verify intent path.
3. `alter-column-type-widening/` — int → bigint, IS the safe path; no intent.
4. `alter-column-type-narrowing/` — bigint → int, requires intent.
5. `add-column-with-default/` — verify the default is rendered + the rewrite is online.
6. `drop-default/` and `set-default/` — paired test for default semantics.
7. `generated-column-add/` — `GENERATED ALWAYS AS (expr) STORED`.
8. `identity-column-add/` — `GENERATED ALWAYS AS IDENTITY`.
9. `column-reorder/` — verify the column-position drift lint fires.

### Task 6.2: Add `tables/` fixtures

**Files:** `crates/pgevolve-conformance/tests/cases/objects/tables/*` (new)

Targets:
1. `create-simple/` — `CREATE TABLE app.t (id bigint primary key);`. Steps = 1.
2. `drop-simple/` — `DROP TABLE`; intent-gated.
3. `add-constraint-check/` — `ALTER TABLE … ADD CONSTRAINT … CHECK (...)`.
4. `add-constraint-unique/` — add unique constraint.
5. `add-constraint-foreign-key/` — add FK; verify online rewrite NOT VALID + VALIDATE pattern.
6. `drop-constraint/` — drop check constraint.
7. `comment-on-table/` — verify comment diff.
8. `comment-on-column/` — verify column-level comment diff.

### Task 6.3: Bless + verify

```bash
cargo xtask bless --conformance
cargo test -p pgevolve-conformance
```

All new fixtures pass; pre-existing don't regress.

---

## Stage acceptance

After each stage, run:
```bash
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo doc --workspace --no-deps 2>&1 | grep -cE "^warning"   # expect 0
cargo deny check
```

Push to origin/main at the end of each stage. Each stage's commits should be reviewable as a coherent set.

---

## Tracking

Mark stage progress in this plan by checking off the `- [ ]` boxes per task. The plan is checked into the repo at `docs/superpowers/plans/2026-05-21-constitution-cleanup.md` so progress survives session boundaries.
