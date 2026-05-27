# PG 18 Catalog Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Extend pgevolve to read v0.3 IR from a Postgres 18 server. No new IR shapes; just the version variant, the dispatch table, the query module, and the conformance matrix.

**Architecture:** Mirror the existing `pg{14,15,16,17}.rs` pattern. Initial `pg18.rs` is a thin re-export of `shared` — divergences (if any) are discovered by running tier-2 round-trip tests under PG 18 and added incrementally.

**Tech Stack:** Rust 1.x, `pg_query = "6"`, ephemeral-Postgres testkit via Docker.

---

## File Structure

| Path | Action | Responsibility |
|---|---|---|
| `crates/pgevolve-core/src/catalog/version.rs` | Modify | Add `Pg18` variant + detection |
| `crates/pgevolve-core/src/catalog/queries/pg18.rs` | Create | PG 18 SQL strings (re-exports `shared` initially) |
| `crates/pgevolve-core/src/catalog/queries/mod.rs` | Modify | Dispatch `Pg18 =>` arms |
| `crates/pgevolve-testkit/src/ephemeral_pg.rs` | Modify | `default_pg_version` + `Pg18` case |
| `.github/workflows/ci.yml` (or equivalent) | Modify | Add `pg:18` to the matrix |
| `crates/pgevolve-conformance/...` | No code change | Tier-3/4 fixtures already run version-parametrically |

---

## Task 1: Add `PgVersion::Pg18` variant

**Files:**
- Modify: `crates/pgevolve-core/src/catalog/version.rs`

- [ ] **Step 1: Write a failing test for PG 18 detection**

Add to `tests` module in `version.rs`:

```rust
#[test]
fn detects_pg18() {
    assert_eq!(
        PgVersion::detect(&MockSingle(180_000)).unwrap(),
        PgVersion::Pg18,
    );
}
```

- [ ] **Step 2: Run test, verify failure**

Run: `cargo test -p pgevolve-core catalog::version::tests::detects_pg18`
Expected: FAIL — `PgVersion::Pg18` does not exist.

- [ ] **Step 3: Add the variant + all match arms**

In `version.rs`:
- Add `Pg18` to the `PgVersion` enum (after `Pg17`).
- Add `18 => Ok(Self::Pg18),` to the `from_major` match.
- Add `Self::Pg18 => "pg18",` to `as_str`.
- Add `Self::Pg18 => 18,` to `major`.
- Update the round-trip test array to include `(180_000, PgVersion::Pg18)`.

- [ ] **Step 4: Run all version tests**

Run: `cargo test -p pgevolve-core catalog::version`
Expected: all pass, including the new `detects_pg18`.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/catalog/version.rs
git commit -m "$(cat <<'EOF'
feat(catalog): add PgVersion::Pg18

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Create `queries/pg18.rs` as a `shared` re-export

**Files:**
- Create: `crates/pgevolve-core/src/catalog/queries/pg18.rs`
- Modify: `crates/pgevolve-core/src/catalog/queries/mod.rs`

- [ ] **Step 1: Inspect `pg17.rs` for the re-export pattern**

Run: `cat crates/pgevolve-core/src/catalog/queries/pg17.rs`
Expected: short file re-exporting `shared::` constants by name.

- [ ] **Step 2: Create `pg18.rs` mirroring `pg17.rs`**

Copy each `pub use shared::*` or `pub const ... = shared::...` line verbatim. For now, every query is identical to `shared`. Divergences (if discovered in Task 4) get inlined here.

- [ ] **Step 3: Add `pub mod pg18;` to `queries/mod.rs`**

Insert after `pub mod pg17;`.

- [ ] **Step 4: Add `Pg18 =>` dispatch arms in `query_for`**

For every `(PgVersion::Pg17, CatalogQuery::X) => pg17::X,` arm, add a matching `(PgVersion::Pg18, CatalogQuery::X) => pg18::X,` arm. The pattern is fully exhaustive — `cargo check` will reject any missing arm.

- [ ] **Step 5: Run `cargo check`**

Run: `cargo check -p pgevolve-core`
Expected: clean compile.

- [ ] **Step 6: Run the queries-module tests**

Run: `cargo test -p pgevolve-core catalog::queries`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/pgevolve-core/src/catalog/queries/pg18.rs crates/pgevolve-core/src/catalog/queries/mod.rs
git commit -m "$(cat <<'EOF'
feat(catalog): add pg18.rs queries module (re-exports shared)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Add PG 18 to the testkit ephemeral-Postgres helper

**Files:**
- Modify: `crates/pgevolve-testkit/src/ephemeral_pg.rs`

- [ ] **Step 1: Locate version handling in testkit**

Run: `grep -n "Pg17\|pg:17\|17.0" crates/pgevolve-testkit/src/ephemeral_pg.rs`
Expected: matches for each call site that knows about supported versions.

- [ ] **Step 2: Mirror every `Pg17` case for `Pg18`**

For each match arm or version mapping that handles `Pg17`, add a parallel `Pg18` arm. Docker image tag is `postgres:18` (or `postgres:18-alpine` if the project uses alpine elsewhere — check `Pg17`'s tag).

- [ ] **Step 3: Build the testkit**

Run: `cargo build -p pgevolve-testkit`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-testkit/src/ephemeral_pg.rs
git commit -m "$(cat <<'EOF'
feat(testkit): add Pg18 ephemeral-postgres support

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Run tier-2 round-trip tests against PG 18; capture divergences

**Files:**
- Possibly modify: `crates/pgevolve-core/src/catalog/queries/pg18.rs`
- Possibly modify: `crates/pgevolve-core/src/catalog/queries/shared.rs`

- [ ] **Step 1: Run the full tier-2 catalog round-trip suite under PG 18**

Run: `PGEVOLVE_PG_VERSION=18 cargo test -p pgevolve-core --features pg-tests catalog`
Expected: ideally all pass with no divergences. If any fail with column-not-found / function-not-found / type-cast errors, those are the PG 18 divergences.

- [ ] **Step 2: Catalog each divergence**

For each test failure, identify the query and the divergence root cause (column renamed, function gone, new column needed for v0.3 IR). Record:
- File: which query (e.g., `SELECT_PUBLICATIONS`)
- Cause: e.g., `pg_publication.pubviaroot` renamed in PG 18 (hypothetical)
- Fix: inline a PG-18-specific variant in `pg18.rs`, leaving `shared.rs` unchanged.

- [ ] **Step 3: For each divergence, write a failing test then fix**

Per divergence:
1. Add a tier-2 fixture or assertion that specifically exercises the affected query under PG 18.
2. Run, confirm failure.
3. Replace the `re-exported` constant in `pg18.rs` with an inline PG-18-specific SQL string.
4. Re-run, confirm pass.
5. Commit with `fix(catalog): adapt SELECT_X for PG 18`.

If no divergences are found, this task is a one-line commit:

```bash
git commit --allow-empty -m "$(cat <<'EOF'
test(catalog): tier-2 round-trip clean against PG 18; no divergences

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Add PG 18 to the CI matrix

**Files:**
- Modify: `.github/workflows/ci.yml` (or the equivalent file for the `pg-matrix` job)

- [ ] **Step 1: Locate the `pg-matrix` job**

Run: `grep -rn "pg-matrix\|postgres:17" .github/`
Expected: one job with a version matrix listing `14, 15, 16, 17`.

- [ ] **Step 2: Add `18` to the version matrix**

Edit the matrix definition so the version list reads `[14, 15, 16, 17, 18]`.

- [ ] **Step 3: Verify locally that the workflow YAML parses**

Run: `yamllint .github/workflows/ci.yml` (if installed) — or just `cat` the file and visually verify.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
ci: add Postgres 18 to the pg-matrix job

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Bump `min_pg_version` upper bound in CLI config validation

**Files:**
- Modify: wherever `min_pg_version` is parsed/validated in `crates/pgevolve/src/`

- [ ] **Step 1: Locate the validator**

Run: `grep -rn "min_pg_version" crates/pgevolve/src/ crates/pgevolve-core/src/`
Expected: parser + validator that rejects values outside `14..=17`.

- [ ] **Step 2: Update the validation range to `14..=18`**

Edit the call site so the inclusive upper bound is `18`. Update any error-message string that lists supported versions to include `18`.

- [ ] **Step 3: Add a unit test that `min_pg_version = 18` parses cleanly**

Add a test alongside the existing `min_pg_version` parsing tests.

- [ ] **Step 4: Run config tests**

Run: `cargo test -p pgevolve-core -p pgevolve config min_pg_version`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/ crates/pgevolve/src/
git commit -m "$(cat <<'EOF'
feat(config): accept min_pg_version = 18

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Verification (end of plan)

- [ ] `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace` all green.
- [ ] `PGEVOLVE_PG_VERSION=18 cargo test --workspace --features pg-tests` green.
- [ ] CI's `pg-matrix` job exercises PG 18.
- [ ] `pgevolve.toml` with `min_pg_version = 18` parses without error.
- [ ] Constitution §6 reads "14, 15, 16, 17, and 18".
