# Conformance Test Suite — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the `pgevolve-conformance` crate — fixture-driven, deterministic, end-to-end coverage of diff → plan → apply that becomes the only CI gate, alongside moving property tests off CI.

**Architecture:** A new workspace crate hosts a fixture tree (`tests/cases/`) and a single integration test that walks it. Each fixture is `before.sql` + `after.sql` + `fixture.toml`. The runner asserts four layers per fixture: diff invariants, plan structural invariants, plan-SQL golden (default-on, normalized), and apply roundtrip against ephemeral Postgres. CI gains a `conformance` job that runs the suite across the PG 14–17 matrix; property tests get `#[ignore]` and a nightly workflow.

**Tech Stack:** Rust 2024, tokio, `pgevolve-core` (planner library API), `pgevolve` (binary, used as a subprocess for apply), `pgevolve-testkit` (`EphemeralPostgres`, `docker_available`), `testcontainers`, `toml`, `walkdir`, `pretty_assertions`.

**Scope (this plan):** C0 (determinism), C1 (runner), C5 (CI cutover), plus one proof-of-life fixture. Out of scope and deferred to follow-up plans: C2 (port ~20 Tier-4 scenarios), C3 (`docs/spec/pg-versions/` delta docs), C4 (exhaustive fixture authoring across `docs/spec/`).

**Spec:** [`docs/superpowers/specs/2026-05-11-conformance-test-suite-design.md`](../specs/2026-05-11-conformance-test-suite-design.md).

---

## File Structure

**New files**
- `crates/pgevolve-conformance/Cargo.toml`
- `crates/pgevolve-conformance/src/lib.rs` — public API (re-exports)
- `crates/pgevolve-conformance/src/fixture.rs` — `Fixture` struct + TOML loader
- `crates/pgevolve-conformance/src/planning.rs` — in-process pipeline `before.sql` + `after.sql` → `Plan`
- `crates/pgevolve-conformance/src/normalize.rs` — strip nondeterministic bits from `plan.sql`
- `crates/pgevolve-conformance/src/assertions/mod.rs`
- `crates/pgevolve-conformance/src/assertions/diff.rs` — Layer 1
- `crates/pgevolve-conformance/src/assertions/plan.rs` — Layers 2 + 3
- `crates/pgevolve-conformance/src/assertions/apply.rs` — Layer 4 (docker-gated)
- `crates/pgevolve-conformance/tests/run.rs` — walker, the single integration test
- `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/{before.sql,after.sql,fixture.toml,expected/diff.txt,expected/plan.sql}` — proof-of-life fixture
- `.github/workflows/property-tests.yml` — nightly property/soak run
- `docs/superpowers/specs/2026-05-11-conformance-test-suite-design.md` — already exists

**Modified files**
- `Cargo.toml` — register the new crate
- `xtask/src/main.rs` — add `bless --conformance` subcommand
- `.github/workflows/ci.yml` — add `conformance` job; keep `pg-matrix` as the property-test gate it already is, then in C5 move property tests to `#[ignore]` and shrink `pg-matrix` to conformance-only
- `crates/pgevolve-core/tests/property_tests.rs` — `#[ignore]` on every test
- `crates/pgevolve/tests/pg_property_tests.rs` — `#[ignore]` on every test
- `crates/pgevolve/tests/chaos_apply.rs` — `#[ignore]` on every test
- `crates/pgevolve-core/src/plan/ordering.rs` — only if Task 1 reveals non-determinism; HashMap iteration audit

---

## Task 1: Determinism harness

**Files:**
- Test: `crates/pgevolve-core/tests/determinism.rs` (new)

This must land **before** anything else. If the planner is non-deterministic, plan-SQL goldens cannot work. The test runs the in-process planner pipeline 100 times on a non-trivial input and asserts byte equality across runs.

- [ ] **Step 1: Write the determinism test**

Create `crates/pgevolve-core/tests/determinism.rs`:

```rust
//! Determinism harness.
//!
//! The conformance suite (see docs/superpowers/specs/2026-05-11-conformance-test-suite-design.md)
//! relies on plan-SQL goldens. Plan-SQL goldens require the in-process
//! planner pipeline to produce byte-identical output for the same input
//! across runs. This test runs the pipeline 100 times and asserts byte
//! equality; any flake here is a determinism bug to fix at the source.

use pgevolve_core::diff::diff;
use pgevolve_core::parse::parse_directory;
use pgevolve_core::plan::{
    Plan, PlannerPolicy, Strategy, group_steps, order, rewrite, write_plan_sql,
};

/// A schema diff non-trivial enough to exercise topo-sort, rewrite, and
/// grouping — but small enough to read at a glance. Two related tables, a
/// PK, an FK, and a secondary index. The `after.sql` adds a NOT NULL
/// column to one table, which on `online` strategy expands into a
/// multi-step rewrite.
const BEFORE_SQL: &str = "\
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orgs (
  id bigint NOT NULL,
  CONSTRAINT orgs_pkey PRIMARY KEY (id)
);
CREATE TABLE app.users (
  id bigint NOT NULL,
  org_id bigint NOT NULL,
  CONSTRAINT users_pkey PRIMARY KEY (id),
  CONSTRAINT users_org_fk FOREIGN KEY (org_id) REFERENCES app.orgs(id)
);
CREATE INDEX users_org_idx ON app.users (org_id);
";

const AFTER_SQL: &str = "\
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orgs (
  id bigint NOT NULL,
  name text NOT NULL,
  CONSTRAINT orgs_pkey PRIMARY KEY (id)
);
CREATE TABLE app.users (
  id bigint NOT NULL,
  org_id bigint NOT NULL,
  email text NOT NULL,
  CONSTRAINT users_pkey PRIMARY KEY (id),
  CONSTRAINT users_org_fk FOREIGN KEY (org_id) REFERENCES app.orgs(id)
);
CREATE INDEX users_org_idx ON app.users (org_id);
CREATE INDEX users_email_idx ON app.users (email);
";

fn render_plan_once() -> String {
    let before_dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(before_dir.path().join("before.sql"), BEFORE_SQL).unwrap();
    let target = parse_directory(before_dir.path(), &[]).expect("parse before");

    let after_dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(after_dir.path().join("after.sql"), AFTER_SQL).unwrap();
    let source = parse_directory(after_dir.path(), &[]).expect("parse after");

    let changes = diff(&target, &source);
    let ordered = order(&target, &source, changes).expect("order");
    let policy = PlannerPolicy {
        strategy: Strategy::Online,
        ..PlannerPolicy::default()
    };
    let steps = rewrite(ordered, &target, &policy);
    let groups = group_steps(steps);
    let plan = Plan::from_grouped(
        groups,
        &source,
        &target,
        // Fixed target identity so plan IDs are reproducible. Any 32-byte
        // value works; we just need it stable across iterations.
        [0u8; 32],
        None,
        pgevolve_core::VERSION,
        policy.planner_ruleset_version,
    );

    let mut buf = Vec::new();
    write_plan_sql(&plan, &mut buf).expect("render plan.sql");
    String::from_utf8(buf).expect("plan.sql is utf-8")
}

#[test]
fn planner_pipeline_is_byte_deterministic() {
    let baseline = render_plan_once();
    for i in 0..99 {
        let next = render_plan_once();
        if next != baseline {
            // Render both to a tempdir so the user can diff them.
            let dir = tempfile::tempdir().expect("tempdir");
            std::fs::write(dir.path().join("baseline.sql"), &baseline).unwrap();
            std::fs::write(dir.path().join(format!("run-{i}.sql")), &next).unwrap();
            panic!(
                "planner output differed on iteration {i}. baseline written to {} for inspection.",
                dir.path().display()
            );
        }
    }
}
```

The exact `Plan::from_grouped` signature: confirm by reading `crates/pgevolve-core/src/plan/plan.rs:151` before adjusting argument order if needed. The fields and types above match the call site in `crates/pgevolve/src/commands/plan.rs`.

- [ ] **Step 2: Run the test**

Run: `cargo test -p pgevolve-core --test determinism -- --test-threads=1`
Expected: PASS — but it may fail if `HashMap` iteration affects output.

- [ ] **Step 3: If the test fails, audit `crates/pgevolve-core/src/plan/ordering.rs`**

Known suspect sites (from `grep HashMap`):
- `ordering.rs:127` — `target_indexes` is a lookup map; safe.
- `ordering.rs:150–151` — `collect_dropped_columns` returns a `HashSet`. Verify whether its iteration order escapes. If yes, change to `BTreeSet`.
- `ordering.rs:217` — `position` is a lookup map; safe.
- `ordering.rs:238` — `in_cycle` is a lookup set; safe.

If the test fails, locate the offending iteration with `RUST_LOG=trace` or by bisecting; replace with `BTreeMap` / `BTreeSet` or sort before iterating. Do not change types speculatively — only in response to a concrete failure. **Skip this step if Step 2 passed.**

- [ ] **Step 4: Run the test again to confirm it passes**

Run: `cargo test -p pgevolve-core --test determinism -- --test-threads=1`
Expected: PASS, 1 test.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/tests/determinism.rs
# plus any ordering.rs fix
git commit -m "test(planner): byte-determinism harness for plan.sql output

Prerequisite for the conformance suite's plan-SQL goldens. Runs the
in-process pipeline 100 times on a non-trivial diff and asserts byte
equality. Fixes any planner non-determinism revealed by the run."
```

---

## Task 2: Scaffold `pgevolve-conformance` crate

**Files:**
- Create: `crates/pgevolve-conformance/Cargo.toml`
- Create: `crates/pgevolve-conformance/src/lib.rs`
- Modify: `Cargo.toml` (workspace) — add the crate to `[workspace] members`

- [ ] **Step 1: Add to workspace members**

Edit `Cargo.toml` at the workspace root:

```toml
[workspace]
resolver = "2"
members = [
  "crates/pgevolve-core",
  "crates/pgevolve",
  "crates/pgevolve-testkit",
  "crates/pgevolve-conformance",
  "xtask",
]
```

- [ ] **Step 2: Create the crate's Cargo.toml**

Create `crates/pgevolve-conformance/Cargo.toml`:

```toml
[package]
name         = "pgevolve-conformance"
description  = "Deterministic fixture-driven conformance suite for pgevolve"
version      = { workspace = true }
edition      = { workspace = true }
rust-version = { workspace = true }
license      = { workspace = true }
repository   = { workspace = true }
authors      = { workspace = true }
publish      = false

[lints]
workspace = true

[dependencies]
pgevolve-core    = { path = "../pgevolve-core" }
pgevolve-testkit = { path = "../pgevolve-testkit" }

anyhow         = { workspace = true }
serde          = { workspace = true }
toml           = { workspace = true }
tokio          = { workspace = true }
tokio-postgres = { workspace = true }
tracing        = { workspace = true }
walkdir        = "2"

[dev-dependencies]
pretty_assertions = { workspace = true }
tempfile          = "3"
```

`walkdir` is not in `[workspace.dependencies]` because `xtask` declares it inline. Adding it inline here matches that pattern.

- [ ] **Step 3: Create the lib.rs skeleton**

Create `crates/pgevolve-conformance/src/lib.rs`:

```rust
//! `pgevolve-conformance` — fixture-driven deterministic test suite.
//!
//! Each fixture is a directory under `tests/cases/` containing
//! `before.sql`, `after.sql`, `fixture.toml`, and an `expected/` sub-tree.
//! See `docs/superpowers/specs/2026-05-11-conformance-test-suite-design.md`
//! for the assertion model.

#![warn(missing_docs)]
#![deny(unsafe_code)]

pub mod assertions;
pub mod fixture;
pub mod normalize;
pub mod planning;

pub use fixture::{Fixture, FixtureError, FixtureExpect, FixtureMeta};
```

- [ ] **Step 4: Add empty module files so `cargo check` passes**

Create `crates/pgevolve-conformance/src/fixture.rs`, `normalize.rs`, `planning.rs` each containing only:

```rust
//! Stub — see follow-up tasks.
```

Create `crates/pgevolve-conformance/src/assertions/mod.rs`:

```rust
//! Per-layer fixture assertions.
//!
//! See the design spec for the assertion contract.
```

- [ ] **Step 5: Verify the workspace still builds**

Run: `cargo check --workspace`
Expected: clean (warnings allowed for empty modules; no errors).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/pgevolve-conformance/
git commit -m "chore(conformance): scaffold pgevolve-conformance crate

Empty crate registered in the workspace. Module stubs only — runner,
fixture loader, normalizer, and assertions land in follow-up commits."
```

---

## Task 3: `Fixture` struct + TOML loader

**Files:**
- Modify: `crates/pgevolve-conformance/src/fixture.rs`
- Test: `crates/pgevolve-conformance/src/fixture.rs` (`#[cfg(test)]` module at the bottom)

- [ ] **Step 1: Write the failing test (unit test in the same file)**

Replace `crates/pgevolve-conformance/src/fixture.rs` contents with:

```rust
//! Fixture loading and validation.
//!
//! A fixture directory contains:
//! - `before.sql` — IR baseline ("what's in the DB already")
//! - `after.sql` — IR target ("desired state")
//! - `fixture.toml` — metadata, version range, expected assertions
//! - `expected/diff.txt` — diff substrings (one per line)
//! - `expected/plan.sql` — golden plan SQL (optional; default-on)
//! - `per-pg/pg<N>/plan.sql` — per-version golden override (optional)

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Errors loading or validating a fixture directory.
#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    /// IO error reading a required file.
    #[error("io error in {path}: {source}")]
    Io {
        /// Path the error happened on.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// TOML parse error in `fixture.toml`.
    #[error("invalid fixture.toml at {path}: {source}")]
    Toml {
        /// Path to fixture.toml.
        path: PathBuf,
        /// Parse error.
        source: toml::de::Error,
    },
    /// A required file is missing.
    #[error("fixture {dir} missing required file {file}")]
    Missing {
        /// Fixture root.
        dir: PathBuf,
        /// Relative path of the missing file.
        file: String,
    },
    /// `pg.min` greater than `pg.max`.
    #[error("fixture {dir}: pg.min ({min}) > pg.max ({max})")]
    BadVersionRange {
        /// Fixture root.
        dir: PathBuf,
        /// min field.
        min: u32,
        /// max field.
        max: u32,
    },
}

/// `[meta]` block.
#[derive(Debug, Clone, Deserialize)]
pub struct FixtureMeta {
    /// Human-readable title shown in failure output.
    pub title: String,
    /// References to `docs/spec/` capability entries this fixture covers.
    #[serde(default)]
    pub spec_refs: Vec<String>,
    /// Optional issue URL when this fixture is a regression capture.
    #[serde(default)]
    pub issue: Option<String>,
}

/// `[pg]` block.
#[derive(Debug, Clone, Deserialize)]
pub struct FixturePg {
    /// Inclusive minimum supported PG major. Defaults to 14.
    #[serde(default = "default_pg_min")]
    pub min: u32,
    /// Inclusive maximum supported PG major. Defaults to 17.
    #[serde(default = "default_pg_max")]
    pub max: u32,
}

fn default_pg_min() -> u32 {
    14
}
fn default_pg_max() -> u32 {
    17
}

impl Default for FixturePg {
    fn default() -> Self {
        Self {
            min: default_pg_min(),
            max: default_pg_max(),
        }
    }
}

/// `[expect]` block.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FixtureExpect {
    /// `[expect.diff]`.
    #[serde(default)]
    pub diff: ExpectDiff,
    /// `[expect.plan]`.
    #[serde(default)]
    pub plan: ExpectPlan,
    /// `[expect.apply]`.
    #[serde(default)]
    pub apply: ExpectApply,
}

/// `[expect.diff]`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExpectDiff {
    /// Substrings that must appear in the rendered diff.
    #[serde(default)]
    pub contains: Vec<String>,
}

/// `[expect.plan]`.
#[derive(Debug, Clone, Deserialize)]
pub struct ExpectPlan {
    /// Expected number of plan steps.
    #[serde(default)]
    pub steps: Option<usize>,
    /// Rewrite identifiers expected in the plan.
    #[serde(default)]
    pub rewrites_used: Vec<String>,
    /// Golden file path (relative to fixture dir). `None` opts out of
    /// golden-comparison; absent in TOML defaults to `expected/plan.sql`.
    #[serde(default = "default_golden")]
    pub golden: Option<String>,
}

fn default_golden() -> Option<String> {
    Some("expected/plan.sql".to_string())
}

impl Default for ExpectPlan {
    fn default() -> Self {
        Self {
            steps: None,
            rewrites_used: Vec::new(),
            golden: default_golden(),
        }
    }
}

/// `[expect.apply]`.
#[derive(Debug, Clone, Deserialize)]
pub struct ExpectApply {
    /// Whether the apply phase is expected to succeed. Defaults to true.
    #[serde(default = "default_true")]
    pub succeeds: bool,
    /// File whose parsed IR is compared against post-apply introspection.
    /// Defaults to `"after.sql"`.
    #[serde(default = "default_post_apply")]
    pub post_apply_equals_to: String,
    /// When `succeeds = false`, every substring here must appear in the
    /// error message from `pgevolve plan` or `pgevolve apply`.
    #[serde(default)]
    pub error_contains: Vec<String>,
}

fn default_true() -> bool {
    true
}
fn default_post_apply() -> String {
    "after.sql".to_string()
}

impl Default for ExpectApply {
    fn default() -> Self {
        Self {
            succeeds: default_true(),
            post_apply_equals_to: default_post_apply(),
            error_contains: Vec::new(),
        }
    }
}

/// `[intent]` and `[planner]` are forwarded verbatim to the planner. They
/// are deserialized as `toml::Table` so the runner can write them straight
/// to `intent.toml` / merge into config without us tracking every key.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FixturePassthrough {
    /// `[intent]` table written into `intent.toml`.
    #[serde(default)]
    pub intent: toml::Table,
    /// `[planner]` table merged into the test config.
    #[serde(default)]
    pub planner: toml::Table,
}

/// A loaded fixture, ready for the runner to operate on.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// Absolute path to the fixture directory.
    pub dir: PathBuf,
    /// `before.sql` contents.
    pub before_sql: String,
    /// `after.sql` contents.
    pub after_sql: String,
    /// `[meta]`.
    pub meta: FixtureMeta,
    /// `[pg]`.
    pub pg: FixturePg,
    /// `[intent]` + `[planner]` passthroughs.
    pub passthrough: FixturePassthrough,
    /// `[expect]`.
    pub expect: FixtureExpect,
}

#[derive(Debug, Deserialize)]
struct RawFixtureToml {
    meta: FixtureMeta,
    #[serde(default)]
    pg: FixturePg,
    #[serde(default)]
    intent: toml::Table,
    #[serde(default)]
    planner: toml::Table,
    #[serde(default)]
    expect: FixtureExpect,
}

impl Fixture {
    /// Load a fixture from its directory.
    pub fn load(dir: &Path) -> Result<Self, FixtureError> {
        let toml_path = dir.join("fixture.toml");
        let toml_bytes = std::fs::read_to_string(&toml_path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                FixtureError::Missing {
                    dir: dir.to_path_buf(),
                    file: "fixture.toml".to_string(),
                }
            } else {
                FixtureError::Io {
                    path: toml_path.clone(),
                    source,
                }
            }
        })?;
        let raw: RawFixtureToml = toml::from_str(&toml_bytes).map_err(|source| {
            FixtureError::Toml {
                path: toml_path,
                source,
            }
        })?;

        if raw.pg.min > raw.pg.max {
            return Err(FixtureError::BadVersionRange {
                dir: dir.to_path_buf(),
                min: raw.pg.min,
                max: raw.pg.max,
            });
        }

        let before_sql = read_required(dir, "before.sql")?;
        let after_sql = read_required(dir, "after.sql")?;

        Ok(Self {
            dir: dir.to_path_buf(),
            before_sql,
            after_sql,
            meta: raw.meta,
            pg: raw.pg,
            passthrough: FixturePassthrough {
                intent: raw.intent,
                planner: raw.planner,
            },
            expect: raw.expect,
        })
    }

    /// Returns the path to the plan-SQL golden for the given PG major.
    /// Resolves `per-pg/pg<N>/plan.sql` first, falling back to
    /// `expected/plan.sql`. Returns `None` when goldening is opted out.
    pub fn golden_path(&self, pg_major: u32) -> Option<PathBuf> {
        let rel = self.expect.plan.golden.as_ref()?;
        let per_pg = self.dir.join("per-pg").join(format!("pg{pg_major}")).join("plan.sql");
        if per_pg.exists() {
            Some(per_pg)
        } else {
            Some(self.dir.join(rel))
        }
    }

    /// Whether this fixture is supposed to run on the given PG major.
    pub fn applies_to(&self, pg_major: u32) -> bool {
        pg_major >= self.pg.min && pg_major <= self.pg.max
    }
}

fn read_required(dir: &Path, rel: &str) -> Result<String, FixtureError> {
    let path = dir.join(rel);
    std::fs::read_to_string(&path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            FixtureError::Missing {
                dir: dir.to_path_buf(),
                file: rel.to_string(),
            }
        } else {
            FixtureError::Io { path, source }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fixture(dir: &Path, toml_body: &str, before: &str, after: &str) {
        std::fs::write(dir.join("fixture.toml"), toml_body).unwrap();
        std::fs::write(dir.join("before.sql"), before).unwrap();
        std::fs::write(dir.join("after.sql"), after).unwrap();
    }

    #[test]
    fn loads_minimal_fixture() {
        let tmp = tempfile::tempdir().unwrap();
        write_fixture(
            tmp.path(),
            r#"
[meta]
title = "trivial"
"#,
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
        );
        let f = Fixture::load(tmp.path()).unwrap();
        assert_eq!(f.meta.title, "trivial");
        assert_eq!(f.pg.min, 14);
        assert_eq!(f.pg.max, 17);
        // Default golden path is set even when `[expect.plan]` is omitted.
        assert_eq!(
            f.expect.plan.golden.as_deref(),
            Some("expected/plan.sql"),
            "golden defaults to expected/plan.sql"
        );
        assert!(f.expect.apply.succeeds);
        assert!(f.applies_to(14));
        assert!(f.applies_to(17));
        assert!(!f.applies_to(13));
        assert!(!f.applies_to(18));
    }

    #[test]
    fn rejects_inverted_version_range() {
        let tmp = tempfile::tempdir().unwrap();
        write_fixture(
            tmp.path(),
            r#"
[meta]
title = "bad-range"
[pg]
min = 17
max = 14
"#,
            "",
            "",
        );
        let err = Fixture::load(tmp.path()).unwrap_err();
        match err {
            FixtureError::BadVersionRange { min: 17, max: 14, .. } => {}
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn missing_before_sql_is_diagnosed_clearly() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("fixture.toml"),
            r#"
[meta]
title = "no-before"
"#,
        )
        .unwrap();
        std::fs::write(tmp.path().join("after.sql"), "").unwrap();
        let err = Fixture::load(tmp.path()).unwrap_err();
        match err {
            FixtureError::Missing { file, .. } if file == "before.sql" => {}
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn golden_opt_out_propagates() {
        let tmp = tempfile::tempdir().unwrap();
        write_fixture(
            tmp.path(),
            r#"
[meta]
title = "opt-out"
[expect.plan]
golden = false
"#,
            "",
            "",
        );
        // `golden = false` in TOML deserializes to `Option<String>::None`
        // via the deserializer for `Option<String>` when the value is
        // boolean-false. Since serde's default impl treats this as a type
        // error, we accept either: a follow-up commit hardens this via a
        // custom deserializer.
        let f = Fixture::load(tmp.path());
        // For Task 3 we just need missing-key → default-on. The opt-out
        // path is tested in Task 8 after we wire the custom deserializer.
        let _ = f;
    }
}
```

Note: the `golden = false` case is intentionally lenient in Task 3 — the proper opt-out deserializer lands in Task 8 alongside the golden assertion. The current `Option<String>` field interprets a missing key as default-on (because of `#[serde(default = "default_golden")]`), which is the only path Task 3 needs to verify.

- [ ] **Step 2: Add `thiserror` to the conformance crate's dependencies**

Edit `crates/pgevolve-conformance/Cargo.toml`:

```toml
[dependencies]
pgevolve-core    = { path = "../pgevolve-core" }
pgevolve-testkit = { path = "../pgevolve-testkit" }

anyhow         = { workspace = true }
serde          = { workspace = true }
thiserror      = { workspace = true }
toml           = { workspace = true }
tokio          = { workspace = true }
tokio-postgres = { workspace = true }
tracing        = { workspace = true }
walkdir        = "2"
```

- [ ] **Step 3: Run the unit tests**

Run: `cargo test -p pgevolve-conformance --lib`
Expected: 3 passing tests (`loads_minimal_fixture`, `rejects_inverted_version_range`, `missing_before_sql_is_diagnosed_clearly`); 1 inert (`golden_opt_out_propagates`).

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-conformance/
git commit -m "feat(conformance): fixture loader and TOML schema

Loads fixture.toml + before.sql + after.sql, with defaults for the pg
version range, expectations, and the default-on golden path. Rejects
inverted version ranges and reports missing required files clearly."
```

---

## Task 4: In-process planner pipeline

**Files:**
- Modify: `crates/pgevolve-conformance/src/planning.rs`

The conformance runner reproduces the same pipeline as `pgevolve plan` for the diff and plan layers — purely in-process, without Postgres or the binary. Apply roundtrip uses the binary as a subprocess (Task 8).

- [ ] **Step 1: Write the pipeline function with a test**

Replace `crates/pgevolve-conformance/src/planning.rs`:

```rust
//! Pure in-process diff+plan pipeline.
//!
//! Parses `before.sql` and `after.sql` into IRs and runs them through
//! `diff` → `order` → `rewrite` → `group_steps` → `Plan::from_grouped`
//! with fixed (test) values for everything that would otherwise be
//! nondeterministic (target identity, git rev, timestamps).

use std::path::Path;

use pgevolve_core::diff::{ChangeSet, diff};
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::parse::{ParseError, parse_directory};
use pgevolve_core::plan::{
    Plan, PlanError, PlannerPolicy, Strategy, group_steps, order, rewrite, write_plan_sql,
};

/// Fixed 32-byte target identity used by the conformance pipeline. Plan
/// IDs depend on this; using a fixed value keeps plan IDs reproducible.
pub const TEST_TARGET_IDENTITY: [u8; 32] = [0u8; 32];

/// Errors produced by the pipeline.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// Parse error in before.sql or after.sql.
    #[error("parse error in {label}: {source}")]
    Parse {
        /// "before" or "after"
        label: &'static str,
        /// underlying parser error
        source: ParseError,
    },
    /// Planner error.
    #[error("plan error: {0}")]
    Plan(#[from] PlanError),
    /// Tempdir / IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Parse `sql` into a `Catalog` using `parse_directory`. We use a tempdir
/// rather than calling the parser directly because `parse_directory` owns
/// the only public filesystem-walk entry point and gives us the same
/// behavior the binary would.
pub fn parse_sql(sql: &str, label: &'static str) -> Result<Catalog, PipelineError> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("fixture.sql");
    std::fs::write(&path, sql)?;
    parse_directory(tmp.path(), &[]).map_err(|source| PipelineError::Parse { label, source })
}

/// Compute the diff between `before.sql` and `after.sql`.
pub fn compute_changes(before_sql: &str, after_sql: &str) -> Result<(Catalog, Catalog, ChangeSet), PipelineError> {
    let target = parse_sql(before_sql, "before")?;
    let source = parse_sql(after_sql, "after")?;
    let changes = diff(&target, &source);
    Ok((target, source, changes))
}

/// Run the full pipeline. Returns the rendered `plan.sql` plus the
/// underlying `Plan` for structural assertions (step count, rewrites).
pub fn render_plan(
    before_sql: &str,
    after_sql: &str,
    strategy: Strategy,
) -> Result<(Plan, String), PipelineError> {
    let (target, source, changes) = compute_changes(before_sql, after_sql)?;
    let ordered = order(&target, &source, changes)?;
    let policy = PlannerPolicy {
        strategy,
        ..PlannerPolicy::default()
    };
    let steps = rewrite(ordered, &target, &policy);
    let groups = group_steps(steps);
    let plan = Plan::from_grouped(
        groups,
        &source,
        &target,
        TEST_TARGET_IDENTITY,
        None, // git_rev
        pgevolve_core::VERSION,
        policy.planner_ruleset_version,
    );
    let mut buf = Vec::new();
    write_plan_sql(&plan, &mut buf)?;
    let sql = String::from_utf8(buf).expect("plan.sql is utf-8");
    Ok((plan, sql))
}

/// Unused parameter, kept on the signature so callers can pass a path
/// without `_p` warnings on the call site.
#[allow(dead_code)]
fn _path_marker(_p: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_diff_produces_empty_plan() {
        let sql = "-- @pgevolve schema=app\nCREATE SCHEMA app;\n";
        let (plan, rendered) = render_plan(sql, sql, Strategy::Online).unwrap();
        let step_count: usize = plan.groups.iter().map(|g| g.steps.len()).sum();
        assert_eq!(step_count, 0, "no diff → no steps");
        // plan.sql may still emit a header even with zero steps; only
        // assert that it didn't fail.
        assert!(rendered.len() < 4096, "header-only plan should be short");
    }

    #[test]
    fn add_column_produces_at_least_one_step() {
        let before = "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL);\n";
        let after = "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL, name text);\n";
        let (plan, rendered) = render_plan(before, after, Strategy::Online).unwrap();
        let step_count: usize = plan.groups.iter().map(|g| g.steps.len()).sum();
        assert!(step_count >= 1, "add column → at least one step, got {step_count}");
        assert!(rendered.contains("ADD COLUMN"), "plan.sql contains ADD COLUMN; got:\n{rendered}");
    }
}
```

Note: confirm the actual `Strategy::Online` variant name and `PlannerPolicy::default()` field set by reading `crates/pgevolve-core/src/plan/policy.rs` before this task lands. The names above match the call site in `crates/pgevolve/src/commands/plan.rs`.

- [ ] **Step 2: Run the unit tests**

Run: `cargo test -p pgevolve-conformance --lib planning`
Expected: 2 passing tests.

- [ ] **Step 3: Commit**

```bash
git add crates/pgevolve-conformance/src/planning.rs
git commit -m "feat(conformance): in-process diff+plan pipeline

Wraps parse → diff → order → rewrite → group → Plan::from_grouped with
fixed test inputs (target identity, no git rev), exposing render_plan()
that returns the Plan plus rendered plan.sql for downstream assertions."
```

---

## Task 5: `plan.sql` normalizer

**Files:**
- Modify: `crates/pgevolve-conformance/src/normalize.rs`

Goldens require byte-equal compare. Real `plan.sql` output contains the plan ID and a `-- generated at <timestamp>` header (confirm by reading `crates/pgevolve-core/src/plan/serialize.rs` for the exact format before writing the regexes). The normalizer strips these.

- [ ] **Step 1: Inspect the real plan.sql header format**

Run: `grep -n "fn write_plan_sql\|generated at\|plan id\|Plan-Id" crates/pgevolve-core/src/plan/serialize.rs`
Read the header-emit section. Note the *exact* string format of any nondeterministic field. Update the regexes in Step 2 to match.

- [ ] **Step 2: Implement and test the normalizer**

Replace `crates/pgevolve-conformance/src/normalize.rs`:

```rust
//! Normalize a rendered `plan.sql` for golden comparison.
//!
//! Strips fields whose values are intentionally nondeterministic across
//! runs even when planner logic is byte-stable. After Task 1 there should
//! be very few of these; the normalizer exists so adding more later is a
//! single-line change.

use std::sync::OnceLock;

use regex::Regex;

/// Apply all normalization passes. Idempotent.
pub fn normalize(plan_sql: &str) -> String {
    // Order matters: longest matches first so we don't overlap.
    let mut s = plan_sql.to_string();
    for pat in patterns() {
        s = pat.regex.replace_all(&s, pat.replacement).to_string();
    }
    s
}

struct Pattern {
    regex: &'static Regex,
    replacement: &'static str,
}

fn patterns() -> &'static [Pattern] {
    static PATTERNS: OnceLock<Vec<Pattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            // Plan ID — adjust regex if Task 5 Step 1 reveals a different
            // emission format. Conservative default: hex plan id at any
            // header line starting with `-- Plan-Id:` or `-- plan id:`.
            Pattern {
                regex: leak_re(r"(?m)^-- [Pp]lan[- ][Ii]d:?\s*[0-9a-fA-F]+\s*$"),
                replacement: "-- Plan-Id: <NORMALIZED>",
            },
            // Generated-at timestamp.
            Pattern {
                regex: leak_re(r"(?m)^-- generated at\s+.*$"),
                replacement: "-- generated at <NORMALIZED>",
            },
            // pgevolve crate version (so a version bump doesn't churn
            // every golden).
            Pattern {
                regex: leak_re(r"(?m)^-- pgevolve\s+\S+\s*$"),
                replacement: "-- pgevolve <NORMALIZED>",
            },
        ]
    })
}

fn leak_re(pattern: &str) -> &'static Regex {
    // Each regex is built once and intentionally leaked into 'static for
    // cheap use in patterns(). The set of patterns is tiny and bounded;
    // this is the same approach used by once_cell-based regex caches.
    let re = Regex::new(pattern).expect("valid regex");
    Box::leak(Box::new(re))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_plan_id_line() {
        let s = "-- Plan-Id: deadbeefcafebabe\nSELECT 1;\n";
        let n = normalize(s);
        assert_eq!(n, "-- Plan-Id: <NORMALIZED>\nSELECT 1;\n");
    }

    #[test]
    fn strips_timestamp_line() {
        let s = "-- generated at 2026-05-11T10:00:00Z\nSELECT 1;\n";
        let n = normalize(s);
        assert_eq!(n, "-- generated at <NORMALIZED>\nSELECT 1;\n");
    }

    #[test]
    fn strips_pgevolve_version() {
        let s = "-- pgevolve 0.1.0-dev\nSELECT 1;\n";
        let n = normalize(s);
        assert_eq!(n, "-- pgevolve <NORMALIZED>\nSELECT 1;\n");
    }

    #[test]
    fn is_idempotent() {
        let s = "-- Plan-Id: abc123\n-- generated at 2026-05-11\nSELECT 1;\n";
        let once = normalize(s);
        let twice = normalize(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn leaves_normal_sql_alone() {
        let s = "CREATE TABLE app.users (id bigint NOT NULL);\n";
        assert_eq!(normalize(s), s);
    }
}
```

- [ ] **Step 3: Add `regex` to the crate's dependencies**

Edit `crates/pgevolve-conformance/Cargo.toml` `[dependencies]`:

```toml
regex = { workspace = true }
```

- [ ] **Step 4: Run the unit tests**

Run: `cargo test -p pgevolve-conformance --lib normalize`
Expected: 5 passing tests.

- [ ] **Step 5: If real `plan.sql` headers don't match the patterns**

If Step 1 revealed an emission format that the regexes in Step 2 miss, update both the regex and the corresponding test in `normalize.rs`. Do not commit a normalizer that doesn't actually match real planner output.

- [ ] **Step 6: Commit**

```bash
git add crates/pgevolve-conformance/Cargo.toml crates/pgevolve-conformance/src/normalize.rs
git commit -m "feat(conformance): plan.sql normalizer for golden compare

Strips plan id, generated-at timestamp, and pgevolve version from
rendered plan.sql so byte-equal golden comparisons survive across
runs. Idempotent. Pattern set is intentionally small."
```

---

## Task 6: Diff assertion layer

**Files:**
- Create: `crates/pgevolve-conformance/src/assertions/diff.rs`
- Modify: `crates/pgevolve-conformance/src/assertions/mod.rs`

- [ ] **Step 1: Inspect how the existing parser corpus renders diffs**

Read `crates/pgevolve-core/tests/parser_corpus.rs:render_diffs` (around line 35). It renders each difference as `"{path}: {from} -> {to}\n"`. The conformance diff assertion uses the same rendering so fixture authors can write `expect.diff.contains` in the same syntax they already know.

- [ ] **Step 2: Write the assertion + test**

Create `crates/pgevolve-conformance/src/assertions/diff.rs`:

```rust
//! Layer 1: diff invariants.
//!
//! Asserts that the diff between `before.sql` and `after.sql` contains
//! every substring listed in `fixture.expect.diff.contains`. Uses the
//! same `path: from -> to` rendering as the parser corpus so fixture
//! authors can transfer the syntax directly.

use std::fmt::Write as _;

use pgevolve_core::ir::difference::Difference;

use crate::fixture::Fixture;
use crate::planning::{PipelineError, compute_changes};

/// Result of running the diff assertion.
#[derive(Debug)]
pub struct DiffOutcome {
    /// Rendered diff (one entry per line).
    pub rendered: String,
    /// Substrings that were not found.
    pub missing: Vec<String>,
}

impl DiffOutcome {
    /// True when every expected substring was found.
    pub fn is_ok(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Render the diff between `before` and `after` and check each
/// `expect.diff.contains` entry.
pub fn check(fixture: &Fixture) -> Result<DiffOutcome, PipelineError> {
    let (target, source, changes) = compute_changes(&fixture.before_sql, &fixture.after_sql)?;
    // diff() returns a ChangeSet; we want the human Difference list. The
    // canonical_eq helper on Catalog returns those — but we already have
    // the changes; use Catalog::diff which produces a Vec<Difference>.
    // (Confirm by reading `pgevolve_core::ir::eq::Diff` trait.)
    let differences: Vec<Difference> = target.diff(&source);
    let _ = changes; // ChangeSet is for the plan layer; not used here.
    let _ = source;
    let rendered = render(&differences);
    let missing = fixture
        .expect
        .diff
        .contains
        .iter()
        .filter(|needle| !rendered.contains(needle.as_str()))
        .cloned()
        .collect();
    Ok(DiffOutcome { rendered, missing })
}

fn render(diffs: &[Difference]) -> String {
    let mut s = String::new();
    for d in diffs {
        let _ = writeln!(s, "{}: {} -> {}", d.path, d.from, d.to);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{
        ExpectDiff, ExpectPlan, ExpectApply, FixtureExpect, FixtureMeta, FixturePassthrough,
        FixturePg,
    };
    use std::path::PathBuf;

    fn fixture(before: &str, after: &str, expect_contains: Vec<&str>) -> Fixture {
        Fixture {
            dir: PathBuf::from("/dev/null"),
            before_sql: before.to_string(),
            after_sql: after.to_string(),
            meta: FixtureMeta {
                title: "test".into(),
                spec_refs: vec![],
                issue: None,
            },
            pg: FixturePg::default(),
            passthrough: FixturePassthrough::default(),
            expect: FixtureExpect {
                diff: ExpectDiff {
                    contains: expect_contains.into_iter().map(str::to_string).collect(),
                },
                plan: ExpectPlan::default(),
                apply: ExpectApply::default(),
            },
        }
    }

    #[test]
    fn passes_when_expected_substring_appears() {
        let f = fixture(
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL);\n",
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL, name text);\n",
            vec!["app.t.name"],
        );
        let out = check(&f).unwrap();
        assert!(out.is_ok(), "expected match; rendered:\n{}", out.rendered);
    }

    #[test]
    fn reports_missing_substrings() {
        let f = fixture(
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
            vec!["this.will.not.match"],
        );
        let out = check(&f).unwrap();
        assert!(!out.is_ok());
        assert_eq!(out.missing, vec!["this.will.not.match"]);
    }
}
```

The `target.diff(&source)` call assumes `Catalog::diff(&Self) -> Vec<Difference>` from `pgevolve_core::ir::eq::Diff`. Confirm the trait name and import path against `crates/pgevolve-core/tests/parser_corpus.rs` line 35 (already in the spec context).

- [ ] **Step 3: Register the module**

Edit `crates/pgevolve-conformance/src/assertions/mod.rs`:

```rust
//! Per-layer fixture assertions.
//!
//! See the design spec for the assertion contract.

pub mod diff;
```

- [ ] **Step 4: Run the unit tests**

Run: `cargo test -p pgevolve-conformance --lib assertions::diff`
Expected: 2 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-conformance/src/assertions/
git commit -m "feat(conformance): Layer 1 diff invariant assertion

Renders the diff between before.sql and after.sql in the same
'path: from -> to' format used by the parser corpus, then checks that
every fixture.expect.diff.contains substring appears."
```

---

## Task 7: Plan structural-invariant assertion layer

**Files:**
- Create: `crates/pgevolve-conformance/src/assertions/plan.rs`
- Modify: `crates/pgevolve-conformance/src/assertions/mod.rs`

This implements Layer 2 (step count + rewrites). Layer 3 (golden compare) is added in Task 8.

- [ ] **Step 1: Confirm how rewrites are identified in the Plan**

Run: `grep -n "rewrites\|kind:\|StepKind" crates/pgevolve-core/src/plan/raw_step.rs crates/pgevolve-core/src/plan/plan.rs | head -30`

The fixture field `rewrites_used` matches against rewrite kind names (the same identifiers `OnlineRewrites` uses: `create_index_concurrent`, `fk_not_valid_then_validate`, `check_not_valid_then_validate`, `not_null_via_check_pattern`). Look at how rewrites are recorded on each `RawStep` / on the `Plan` manifest. If steps don't carry rewrite IDs, this assertion needs a different signal — e.g., matching SQL substrings like `NOT VALID`. **Whichever path matches reality, document it in a comment in plan.rs.**

- [ ] **Step 2: Implement and test**

Create `crates/pgevolve-conformance/src/assertions/plan.rs`:

```rust
//! Layer 2: plan structural invariants — step count and rewrite kinds
//! used. Layer 3 (golden plan.sql compare) lives alongside in this file
//! and is added in Task 8.

use pgevolve_core::plan::{Plan, Strategy};

use crate::fixture::Fixture;
use crate::planning::{PipelineError, render_plan};

/// Result of running Layer 2.
#[derive(Debug)]
pub struct PlanInvariantOutcome {
    /// The plan that was rendered.
    pub plan: Plan,
    /// Rendered plan.sql, available for Layer 3 to use.
    pub rendered_sql: String,
    /// Actual step count.
    pub actual_steps: usize,
    /// `Some(expected)` when `[expect.plan].steps` was set and didn't match.
    pub step_mismatch: Option<usize>,
    /// Rewrites named in fixture.expect that did not appear in the plan.
    pub missing_rewrites: Vec<String>,
}

impl PlanInvariantOutcome {
    /// True when every assertion held.
    pub fn is_ok(&self) -> bool {
        self.step_mismatch.is_none() && self.missing_rewrites.is_empty()
    }
}

/// Run Layer 2.
pub fn check(fixture: &Fixture) -> Result<PlanInvariantOutcome, PipelineError> {
    let strategy = parse_strategy(&fixture).unwrap_or(Strategy::Online);
    let (plan, rendered_sql) = render_plan(&fixture.before_sql, &fixture.after_sql, strategy)?;

    let actual_steps: usize = plan.groups.iter().map(|g| g.steps.len()).sum();
    let step_mismatch = match fixture.expect.plan.steps {
        Some(expected) if expected != actual_steps => Some(expected),
        _ => None,
    };

    // Find rewrites used. Two strategies depending on what Task 7 Step 1
    // discovered:
    //  - if RawStep carries a rewrite_kind field, match by id.
    //  - else, match by SQL substring on rendered_sql (e.g. "NOT VALID"
    //    implies fk_not_valid_then_validate / check_not_valid_then_validate).
    // For the initial landing, we go with the SQL-substring approach
    // since it has no dependency on internal types and is what `cli_e2e`
    // already does. Replace with structured matching in a follow-up if
    // brittleness shows up.
    let missing_rewrites = fixture
        .expect
        .plan
        .rewrites_used
        .iter()
        .filter(|kind| !plan_uses(&rendered_sql, kind))
        .cloned()
        .collect();

    Ok(PlanInvariantOutcome {
        plan,
        rendered_sql,
        actual_steps,
        step_mismatch,
        missing_rewrites,
    })
}

fn plan_uses(plan_sql: &str, rewrite_kind: &str) -> bool {
    match rewrite_kind {
        "create_index_concurrent" => plan_sql.contains("CREATE INDEX CONCURRENTLY")
            || plan_sql.contains("CREATE UNIQUE INDEX CONCURRENTLY"),
        "fk_not_valid_then_validate" => {
            plan_sql.contains("NOT VALID") && plan_sql.contains("VALIDATE CONSTRAINT")
        }
        "check_not_valid_then_validate" => {
            plan_sql.contains("NOT VALID") && plan_sql.contains("VALIDATE CONSTRAINT")
        }
        "not_null_via_check_pattern" => {
            plan_sql.contains("CHECK") && plan_sql.contains("NOT VALID")
        }
        // Unknown kinds match nothing; the assertion fails clearly.
        _ => false,
    }
}

fn parse_strategy(fixture: &Fixture) -> Option<Strategy> {
    let raw = fixture.passthrough.planner.get("strategy")?.as_str()?;
    match raw {
        "atomic" => Some(Strategy::Atomic),
        "online" => Some(Strategy::Online),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{
        ExpectApply, ExpectDiff, ExpectPlan, FixtureExpect, FixtureMeta, FixturePassthrough,
        FixturePg,
    };
    use std::path::PathBuf;

    fn fixture(before: &str, after: &str, expect_plan: ExpectPlan) -> Fixture {
        Fixture {
            dir: PathBuf::from("/dev/null"),
            before_sql: before.to_string(),
            after_sql: after.to_string(),
            meta: FixtureMeta {
                title: "test".into(),
                spec_refs: vec![],
                issue: None,
            },
            pg: FixturePg::default(),
            passthrough: FixturePassthrough::default(),
            expect: FixtureExpect {
                diff: ExpectDiff::default(),
                plan: expect_plan,
                apply: ExpectApply::default(),
            },
        }
    }

    #[test]
    fn step_count_mismatch_is_reported() {
        let before =
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL);\n";
        let after =
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL, name text);\n";
        let f = fixture(
            before,
            after,
            ExpectPlan {
                steps: Some(999),
                rewrites_used: vec![],
                golden: None,
            },
        );
        let out = check(&f).unwrap();
        assert!(!out.is_ok());
        assert_eq!(out.step_mismatch, Some(999));
    }

    #[test]
    fn unrecognized_rewrite_id_is_reported_missing() {
        let before = "-- @pgevolve schema=app\nCREATE SCHEMA app;\n";
        let after = "-- @pgevolve schema=app\nCREATE SCHEMA app;\n";
        let f = fixture(
            before,
            after,
            ExpectPlan {
                steps: None,
                rewrites_used: vec!["totally_made_up_rewrite".into()],
                golden: None,
            },
        );
        let out = check(&f).unwrap();
        assert!(!out.is_ok());
        assert_eq!(out.missing_rewrites, vec!["totally_made_up_rewrite"]);
    }
}
```

If Task 7 Step 1 revealed that `RawStep` carries a structured `rewrite_kind` field, replace the substring-based `plan_uses` with a structured lookup over `plan.groups[].steps[].rewrite_kind` and document the change in the file header.

- [ ] **Step 3: Register the module**

Edit `crates/pgevolve-conformance/src/assertions/mod.rs`:

```rust
pub mod diff;
pub mod plan;
```

- [ ] **Step 4: Run the unit tests**

Run: `cargo test -p pgevolve-conformance --lib assertions::plan`
Expected: 2 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-conformance/src/assertions/
git commit -m "feat(conformance): Layer 2 plan structural-invariant assertion

Checks expected step count and rewrite-kind usage (matched on plan.sql
substrings for now; promote to structured RawStep field if substring
matching proves brittle)."
```

---

## Task 8: Plan-SQL golden assertion (Layer 3)

**Files:**
- Modify: `crates/pgevolve-conformance/src/assertions/plan.rs` (extend with Layer 3)
- Modify: `crates/pgevolve-conformance/src/fixture.rs` (custom deserializer for `golden = false`)

- [ ] **Step 1: Add custom deserializer for `golden`**

In `crates/pgevolve-conformance/src/fixture.rs`, replace the `ExpectPlan` struct (find it via the `#[derive(Debug, Clone, Deserialize)]\npub struct ExpectPlan {` block) with:

```rust
/// `[expect.plan]`.
#[derive(Debug, Clone, Deserialize)]
pub struct ExpectPlan {
    /// Expected number of plan steps.
    #[serde(default)]
    pub steps: Option<usize>,
    /// Rewrite identifiers expected in the plan.
    #[serde(default)]
    pub rewrites_used: Vec<String>,
    /// Golden file path. Accepts string (custom path), `true` (default
    /// `expected/plan.sql`), or `false` (opt out). Missing key →
    /// default-on. See `deserialize_golden`.
    #[serde(default = "default_golden", deserialize_with = "deserialize_golden")]
    pub golden: Option<String>,
}

fn deserialize_golden<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct GoldenVisitor;
    impl<'de> Visitor<'de> for GoldenVisitor {
        type Value = Option<String>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a string path, `true`, or `false`")
        }
        fn visit_bool<E: de::Error>(self, v: bool) -> Result<Option<String>, E> {
            Ok(if v { default_golden() } else { None })
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Option<String>, E> {
            Ok(Some(v.to_string()))
        }
        fn visit_string<E: de::Error>(self, v: String) -> Result<Option<String>, E> {
            Ok(Some(v))
        }
    }
    d.deserialize_any(GoldenVisitor)
}
```

- [ ] **Step 2: Replace the `golden_opt_out_propagates` test with a proper assertion**

In `crates/pgevolve-conformance/src/fixture.rs` `mod tests`, replace `golden_opt_out_propagates` with:

```rust
    #[test]
    fn golden_opt_out_via_false() {
        let tmp = tempfile::tempdir().unwrap();
        write_fixture(
            tmp.path(),
            r#"
[meta]
title = "opt-out"
[expect.plan]
golden = false
"#,
            "",
            "",
        );
        let f = Fixture::load(tmp.path()).unwrap();
        assert!(f.expect.plan.golden.is_none(), "false → None");
    }

    #[test]
    fn golden_custom_path() {
        let tmp = tempfile::tempdir().unwrap();
        write_fixture(
            tmp.path(),
            r#"
[meta]
title = "custom"
[expect.plan]
golden = "expected/custom.sql"
"#,
            "",
            "",
        );
        let f = Fixture::load(tmp.path()).unwrap();
        assert_eq!(f.expect.plan.golden.as_deref(), Some("expected/custom.sql"));
    }
```

- [ ] **Step 3: Append the Layer 3 outcome and check to `assertions/plan.rs`**

Append to `crates/pgevolve-conformance/src/assertions/plan.rs`:

```rust
/// Layer 3 result.
#[derive(Debug)]
pub struct GoldenOutcome {
    /// Path of the golden compared against. `None` when goldening is
    /// opted out for this fixture.
    pub golden_path: Option<std::path::PathBuf>,
    /// Normalized plan.sql produced from the in-process pipeline.
    pub actual_normalized: String,
    /// Normalized contents of the golden file. `None` when goldening is
    /// opted out or the golden file is missing entirely.
    pub expected_normalized: Option<String>,
    /// Set when actual ≠ expected.
    pub mismatch: Option<String>,
}

impl GoldenOutcome {
    /// True if the golden compare passed or was opted out.
    pub fn is_ok(&self) -> bool {
        self.mismatch.is_none()
    }
}

/// Compare the rendered plan.sql against the fixture's golden file. Uses
/// `pg_major` to select `per-pg/pg<N>/plan.sql` when present.
pub fn check_golden(
    fixture: &crate::fixture::Fixture,
    rendered_sql: &str,
    pg_major: u32,
) -> std::io::Result<GoldenOutcome> {
    let golden_path = fixture.golden_path(pg_major);

    let actual_normalized = crate::normalize::normalize(rendered_sql);
    let Some(path) = golden_path else {
        return Ok(GoldenOutcome {
            golden_path: None,
            actual_normalized,
            expected_normalized: None,
            mismatch: None,
        });
    };

    if !path.exists() {
        // Treat a missing golden as a *mismatch* (with helpful message)
        // rather than a hard IO error — failing this way means the bless
        // command is the natural fix path.
        return Ok(GoldenOutcome {
            golden_path: Some(path.clone()),
            actual_normalized: actual_normalized.clone(),
            expected_normalized: None,
            mismatch: Some(format!(
                "golden file {} does not exist; run `cargo xtask bless --conformance` to create it",
                path.display()
            )),
        });
    }

    let raw = std::fs::read_to_string(&path)?;
    let expected_normalized = crate::normalize::normalize(&raw);
    let mismatch = if actual_normalized == expected_normalized {
        None
    } else {
        Some(format!(
            "plan.sql mismatch vs {}. Run `cargo xtask bless --conformance` if intentional.",
            path.display()
        ))
    };

    Ok(GoldenOutcome {
        golden_path: Some(path),
        actual_normalized,
        expected_normalized: Some(expected_normalized),
        mismatch,
    })
}

#[cfg(test)]
mod golden_tests {
    use super::*;
    use crate::fixture::{
        ExpectApply, ExpectDiff, ExpectPlan, FixtureExpect, FixtureMeta, FixturePassthrough,
        FixturePg,
    };
    use std::path::PathBuf;

    fn fixture_with_golden_in(dir: PathBuf, golden_body: &str) -> crate::fixture::Fixture {
        std::fs::create_dir_all(dir.join("expected")).unwrap();
        std::fs::write(dir.join("expected/plan.sql"), golden_body).unwrap();
        crate::fixture::Fixture {
            dir,
            before_sql: String::new(),
            after_sql: String::new(),
            meta: FixtureMeta {
                title: "g".into(),
                spec_refs: vec![],
                issue: None,
            },
            pg: FixturePg::default(),
            passthrough: FixturePassthrough::default(),
            expect: FixtureExpect {
                diff: ExpectDiff::default(),
                plan: ExpectPlan {
                    steps: None,
                    rewrites_used: vec![],
                    golden: Some("expected/plan.sql".into()),
                },
                apply: ExpectApply::default(),
            },
        }
    }

    #[test]
    fn passes_when_golden_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fixture_with_golden_in(tmp.path().to_path_buf(), "SELECT 1;\n");
        let out = check_golden(&f, "SELECT 1;\n", 17).unwrap();
        assert!(out.is_ok(), "{:?}", out.mismatch);
    }

    #[test]
    fn fails_when_golden_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fixture_with_golden_in(tmp.path().to_path_buf(), "SELECT 1;\n");
        let out = check_golden(&f, "SELECT 2;\n", 17).unwrap();
        assert!(!out.is_ok());
        assert!(out.mismatch.unwrap().contains("bless"));
    }

    #[test]
    fn missing_golden_is_flagged() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = fixture_with_golden_in(tmp.path().to_path_buf(), "SELECT 1;\n");
        std::fs::remove_file(tmp.path().join("expected/plan.sql")).unwrap();
        // golden_path() now returns a non-existent path — that's the
        // case we want to assert.
        let out = check_golden(&f, "SELECT 1;\n", 17).unwrap();
        assert!(!out.is_ok());
        let _ = &mut f;
    }

    #[test]
    fn opt_out_returns_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = fixture_with_golden_in(tmp.path().to_path_buf(), "SELECT 1;\n");
        f.expect.plan.golden = None;
        let out = check_golden(&f, "anything", 17).unwrap();
        assert!(out.is_ok());
        assert!(out.golden_path.is_none());
    }
}
```

- [ ] **Step 4: Run the unit tests**

Run: `cargo test -p pgevolve-conformance --lib`
Expected: all existing tests still pass; 4 new golden tests pass; 2 new fixture tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-conformance/src/
git commit -m "feat(conformance): Layer 3 plan.sql golden compare

Default-on byte-equal compare of normalized plan.sql against
expected/plan.sql (or per-pg/pg<N>/plan.sql when present). Supports
golden = false in fixture.toml to opt out. Missing golden produces a
clear bless instruction rather than an opaque IO error."
```

---

## Task 9: Apply-roundtrip assertion (Layer 4)

**Files:**
- Create: `crates/pgevolve-conformance/src/assertions/apply.rs`
- Modify: `crates/pgevolve-conformance/src/assertions/mod.rs`

Layer 4 spins up `EphemeralPostgres`, seeds `before.sql` raw, then runs the real `pgevolve` binary for plan+apply, then introspects.

- [ ] **Step 1: Inspect `cli_e2e.rs` for the canonical end-to-end sequence**

Read `crates/pgevolve/tests/cli_e2e.rs`. The conformance apply layer follows the same shape: build a temp project dir with `pgevolve.toml` pointing at the ephemeral DSN, write `schema/<fixture>.sql` containing `after.sql`, optionally write `intent.toml` from the fixture passthrough, then run the `pgevolve` binary.

- [ ] **Step 2: Implement Layer 4**

Create `crates/pgevolve-conformance/src/assertions/apply.rs`:

```rust
//! Layer 4: apply roundtrip against ephemeral Postgres.
//!
//! Seeds `before.sql` directly into an EphemeralPostgres (bypassing
//! pgevolve), constructs a temp project with `after.sql` as the source,
//! invokes the real pgevolve binary for plan+apply, then introspects the
//! post-apply DB and compares the resulting IR against `after.sql`
//! parsed independently.
//!
//! Docker-gated. Skipped (not failed) when docker_available() is false,
//! consistent with the rest of the workspace.

use std::path::{Path, PathBuf};
use std::process::Command;

use pgevolve_core::catalog::{CatalogFilter, read_catalog};
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::parse::parse_directory;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, PgVersion, docker_available};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;

use crate::fixture::Fixture;

/// Outcome of an apply roundtrip.
#[derive(Debug)]
pub enum ApplyOutcome {
    /// Docker unavailable; layer skipped.
    Skipped,
    /// Apply succeeded; IRs were equal.
    Ok,
    /// Apply succeeded but introspected IR diverged from after.sql.
    IrMismatch(String),
    /// `pgevolve plan` or `pgevolve apply` failed.
    ApplyFailed {
        /// stderr from the failing command.
        stderr: String,
        /// "plan" or "apply".
        stage: &'static str,
    },
    /// The fixture expected `apply.succeeds = false`, and the failure
    /// did not contain every required `error_contains` substring.
    UnexpectedSuccess,
}

impl ApplyOutcome {
    /// True for any non-failure variant the runner should treat as pass.
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok | Self::Skipped)
    }
}

/// Run Layer 4.
pub async fn check(fixture: &Fixture, pg_major: u32) -> anyhow::Result<ApplyOutcome> {
    if !docker_available() {
        return Ok(ApplyOutcome::Skipped);
    }

    let version = pg_version_from_major(pg_major)?;
    let pg = EphemeralPostgres::start(version).await?;

    // 1. Seed before.sql directly via the postgres client.
    seed_before(&pg, &fixture.before_sql).await?;

    // 2. Build temp project with after.sql as the source schema.
    let project = tempfile::tempdir()?;
    let project_path = project.path();
    write_project(project_path, &pg.dsn(), fixture, pg_major)?;

    // 3. bootstrap, plan, apply via the binary.
    if let Err(e) = run_pgevolve(project_path, &["bootstrap", "--db", "dev"]) {
        return Ok(ApplyOutcome::ApplyFailed {
            stderr: e,
            stage: "plan", // bootstrap failures are treated as plan-stage.
        });
    }

    let plan_dir = match plan_and_locate(project_path) {
        Ok(p) => p,
        Err(stderr) => {
            return Ok(check_failure_expectation(fixture, &stderr, "plan"));
        }
    };

    if let Err(stderr) = run_pgevolve(project_path, &["apply", &plan_dir.display().to_string(), "--db", "dev"]) {
        return Ok(check_failure_expectation(fixture, &stderr, "apply"));
    }

    // 4. The fixture expected failure but we got here — that's a fail.
    if !fixture.expect.apply.succeeds {
        return Ok(ApplyOutcome::UnexpectedSuccess);
    }

    // 5. Introspect; compare to after.sql parsed independently.
    let post_apply_ir = introspect(&pg, fixture).await?;
    let expected_ir = parse_post_apply_target(fixture)?;

    if post_apply_ir.canonical_eq(&expected_ir) {
        Ok(ApplyOutcome::Ok)
    } else {
        let diffs = expected_ir.diff(&post_apply_ir);
        let rendered = diffs
            .iter()
            .map(|d| format!("{}: {} -> {}", d.path, d.from, d.to))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(ApplyOutcome::IrMismatch(rendered))
    }
}

fn pg_version_from_major(major: u32) -> anyhow::Result<PgVersion> {
    match major {
        14 => Ok(PgVersion::Pg14),
        15 => Ok(PgVersion::Pg15),
        16 => Ok(PgVersion::Pg16),
        17 => Ok(PgVersion::Pg17),
        other => Err(anyhow::anyhow!("unsupported PG major: {other}")),
    }
}

async fn seed_before(pg: &EphemeralPostgres, before_sql: &str) -> anyhow::Result<()> {
    // `before.sql` may contain pgevolve schema directives (-- @pgevolve);
    // these are comments to Postgres so executing the file directly is
    // safe.
    let (client, conn) = tokio_postgres::connect(&pg.dsn(), tokio_postgres::NoTls).await?;
    tokio::spawn(conn);
    if !before_sql.trim().is_empty() {
        client.batch_execute(before_sql).await?;
    }
    Ok(())
}

fn write_project(
    project_path: &Path,
    dsn: &str,
    fixture: &Fixture,
    pg_major: u32,
) -> std::io::Result<()> {
    let _ = pg_major;
    let schemas = collect_managed_schemas(&fixture.after_sql);
    let schema_list = schemas
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");

    let strategy = fixture
        .passthrough
        .planner
        .get("strategy")
        .and_then(|v| v.as_str())
        .unwrap_or("online");

    let cfg = format!(
        "[project]\nname = \"conformance\"\nschema_dir = \"schema\"\nplan_dir = \"plans\"\nlayout_profile = \"schema-mirror\"\n\n\
         [managed]\nschemas = [{schema_list}]\n\n\
         [planner]\nstrategy = \"{strategy}\"\n\n\
         [environments.dev]\nurl = \"{dsn}\"\n"
    );
    std::fs::write(project_path.join("pgevolve.toml"), cfg)?;

    // schema-mirror layout wants schema/<schema>/<kind>/<name>.sql, but
    // for conformance fixtures we use a single `schema/0001-fixture.sql`
    // (legal under the schema-mirror profile when files declare schema
    // via the @pgevolve directive). Confirm by re-reading the parser's
    // layout enforcement.
    std::fs::create_dir_all(project_path.join("schema"))?;
    std::fs::write(project_path.join("schema/0001-fixture.sql"), &fixture.after_sql)?;

    // Intent passthrough.
    if !fixture.passthrough.intent.is_empty() {
        let intent_path = project_path.join("intent.toml");
        let body = toml::to_string(&fixture.passthrough.intent).map_err(io_err)?;
        std::fs::write(intent_path, body)?;
    }
    Ok(())
}

fn io_err(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}

/// Crude scan: any line containing `CREATE SCHEMA <name>` adds <name>.
/// Sufficient for the fixture layout where after.sql declares its own
/// schemas explicitly.
fn collect_managed_schemas(after_sql: &str) -> Vec<String> {
    let re = regex::Regex::new(r"(?i)CREATE\s+SCHEMA\s+(?:IF\s+NOT\s+EXISTS\s+)?(\w+)").unwrap();
    let mut out: Vec<String> = re
        .captures_iter(after_sql)
        .map(|c| c[1].to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn cargo_bin() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_BIN_EXE_pgevolve"));
    if !p.exists() {
        p = PathBuf::from("target/debug/pgevolve");
    }
    p
}

fn run_pgevolve(cwd: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new(cargo_bin())
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|e| format!("spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    Ok(())
}

fn plan_and_locate(cwd: &Path) -> Result<PathBuf, String> {
    let out = Command::new(cargo_bin())
        .current_dir(cwd)
        .args(["plan", "--db", "dev"])
        .output()
        .map_err(|e| format!("spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find(|l| l.starts_with("Wrote plan"))
        .ok_or_else(|| format!("no 'Wrote plan' in stdout:\n{stdout}"))?;
    let rel = line
        .split(" to ")
        .nth(1)
        .and_then(|s| s.split(' ').next())
        .ok_or_else(|| format!("could not parse plan dir from: {line}"))?;
    Ok(cwd.join(rel))
}

fn check_failure_expectation(
    fixture: &Fixture,
    stderr: &str,
    stage: &'static str,
) -> ApplyOutcome {
    if fixture.expect.apply.succeeds {
        return ApplyOutcome::ApplyFailed {
            stderr: stderr.to_string(),
            stage,
        };
    }
    let all_match = fixture
        .expect
        .apply
        .error_contains
        .iter()
        .all(|s| stderr.contains(s));
    if all_match {
        ApplyOutcome::Ok
    } else {
        ApplyOutcome::ApplyFailed {
            stderr: format!(
                "fixture expected failure with substrings {:?}; got stderr:\n{stderr}",
                fixture.expect.apply.error_contains,
            ),
            stage,
        }
    }
}

async fn introspect(pg: &EphemeralPostgres, fixture: &Fixture) -> anyhow::Result<Catalog> {
    let (client, conn) = tokio_postgres::connect(&pg.dsn(), tokio_postgres::NoTls).await?;
    tokio::spawn(conn);
    let querier = PgCatalogQuerier::new(client)?;
    let schemas = collect_managed_schemas(&fixture.after_sql);
    let filter = CatalogFilter::new(schemas, vec![])?;
    tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await?
        .map_err(Into::into)
}

fn parse_post_apply_target(fixture: &Fixture) -> anyhow::Result<Catalog> {
    let rel = &fixture.expect.apply.post_apply_equals_to;
    let body = std::fs::read_to_string(fixture.dir.join(rel))
        .map_err(|e| anyhow::anyhow!("read {rel}: {e}"))?;
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("after.sql"), body)?;
    parse_directory(tmp.path(), &[]).map_err(|e| anyhow::anyhow!("parse {rel}: {e}"))
}
```

Note: this layer is async; the runner will dispatch one fixture at a time per PG version (parallelism via the CI matrix, not within a single run).

- [ ] **Step 3: Register the module**

Edit `crates/pgevolve-conformance/src/assertions/mod.rs`:

```rust
pub mod apply;
pub mod diff;
pub mod plan;
```

- [ ] **Step 4: Verify the crate still builds**

Run: `cargo check -p pgevolve-conformance --tests`
Expected: clean. (No new unit tests in this task; Layer 4 is exercised end-to-end by Task 10's `tests/run.rs` via the proof-of-life fixture.)

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-conformance/src/assertions/
git commit -m "feat(conformance): Layer 4 apply roundtrip against ephemeral PG

Seeds before.sql raw into EphemeralPostgres, drives plan+apply through
the real pgevolve binary, then introspects and compares against
after.sql parsed independently. Docker-gated; skipped (not failed)
when docker is unavailable, matching existing tier-3/4 convention."
```

---

## Task 10: Top-level walker + proof-of-life fixture

**Files:**
- Create: `crates/pgevolve-conformance/tests/run.rs`
- Create: `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/before.sql`
- Create: `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/after.sql`
- Create: `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/fixture.toml`
- Create: `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/expected/diff.txt` (informational only; not currently read by the runner — kept for human readability)

The walker is a single `#[tokio::test]` that iterates every fixture directory and runs all four layers. Per-fixture failures point at the directory so failure messages are actionable.

- [ ] **Step 1: Build the proof-of-life fixture first**

This is the *simplest possible* fixture that exercises all four layers. We use it to drive the runner's implementation.

Create `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/before.sql`:

```sql
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
  id bigint NOT NULL,
  CONSTRAINT users_pkey PRIMARY KEY (id)
);
```

Create `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/after.sql`:

```sql
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
  id bigint NOT NULL,
  email text,
  CONSTRAINT users_pkey PRIMARY KEY (id)
);
```

Create `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/fixture.toml`:

```toml
[meta]
title     = "ADD COLUMN nullable — proof of life"
spec_refs = ["objects.column.add"]

[pg]
min = 14
max = 17

[expect.diff]
contains = [
  "app.users.email",
]

[expect.plan]
steps  = 1
golden = false   # bless this in Task 11 after the runner produces output

[expect.apply]
succeeds             = true
post_apply_equals_to = "after.sql"
```

Goldening is deliberately off at fixture-authoring time. Task 11's bless command produces the golden, and a follow-up commit flips `golden = false` to default-on (deleting that line is equivalent).

Create `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/expected/diff.txt` (one substring per line; informational mirror of `[expect.diff].contains`):

```
app.users.email
```

- [ ] **Step 2: Implement the walker**

Create `crates/pgevolve-conformance/tests/run.rs`:

```rust
//! Conformance suite entry point.
//!
//! Walks `tests/cases/**/fixture.toml`, runs all four assertion layers
//! against each fixture, and aggregates failures. Each fixture failure
//! is reported with its directory path so failures are immediately
//! actionable.
//!
//! Driven by env var `PGEVOLVE_TEST_PG_VERSION` (default: 17) so the
//! same suite runs once per major in the CI matrix.

use std::path::{Path, PathBuf};

use pgevolve_conformance::assertions::{apply, diff, plan};
use pgevolve_conformance::fixture::Fixture;

fn cases_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cases")
}

fn active_pg_major() -> u32 {
    std::env::var("PGEVOLVE_TEST_PG_VERSION")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(17)
}

fn discover_fixtures(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let Ok(entry) = entry else { continue };
        if entry.file_name() == "fixture.toml" {
            if let Some(parent) = entry.path().parent() {
                out.push(parent.to_path_buf());
            }
        }
    }
    out.sort();
    out
}

#[derive(Debug, Default)]
struct Report {
    failures: Vec<String>,
}

impl Report {
    fn fail(&mut self, fixture: &Path, layer: &str, detail: impl AsRef<str>) {
        self.failures.push(format!(
            "[{}] {}: {}",
            layer,
            fixture
                .strip_prefix(cases_root())
                .unwrap_or(fixture)
                .display(),
            detail.as_ref()
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conformance_suite() {
    let pg_major = active_pg_major();
    let fixtures = discover_fixtures(&cases_root());
    assert!(
        !fixtures.is_empty(),
        "no fixtures discovered under {}",
        cases_root().display()
    );

    let mut report = Report::default();
    let mut ran = 0usize;
    let mut skipped = 0usize;

    for dir in &fixtures {
        let fixture = match Fixture::load(dir) {
            Ok(f) => f,
            Err(e) => {
                report.fail(dir, "load", e.to_string());
                continue;
            }
        };
        if !fixture.applies_to(pg_major) {
            skipped += 1;
            continue;
        }
        ran += 1;

        // Layer 1.
        match diff::check(&fixture) {
            Ok(out) if out.is_ok() => {}
            Ok(out) => report.fail(
                dir,
                "diff",
                format!(
                    "missing substrings {:?}; rendered diff:\n{}",
                    out.missing, out.rendered
                ),
            ),
            Err(e) => report.fail(dir, "diff", e.to_string()),
        }

        // Layer 2.
        let plan_outcome = match plan::check(&fixture) {
            Ok(o) => o,
            Err(e) => {
                report.fail(dir, "plan", e.to_string());
                continue;
            }
        };
        if let Some(expected) = plan_outcome.step_mismatch {
            report.fail(
                dir,
                "plan",
                format!(
                    "expected {} step(s), got {}",
                    expected, plan_outcome.actual_steps
                ),
            );
        }
        if !plan_outcome.missing_rewrites.is_empty() {
            report.fail(
                dir,
                "plan",
                format!("missing rewrites {:?}", plan_outcome.missing_rewrites),
            );
        }

        // Layer 3.
        match plan::check_golden(&fixture, &plan_outcome.rendered_sql, pg_major) {
            Ok(out) if out.is_ok() => {}
            Ok(out) => {
                let detail = out.mismatch.unwrap_or_default();
                // When the golden exists, surface a unified diff snippet.
                let extras = match (out.expected_normalized.as_ref(), out.golden_path.as_ref()) {
                    (Some(expected), Some(_path)) => {
                        format!(
                            "\n--- expected (normalized) ---\n{}\n--- actual (normalized) ---\n{}",
                            expected, out.actual_normalized
                        )
                    }
                    _ => String::new(),
                };
                report.fail(dir, "golden", format!("{detail}{extras}"));
            }
            Err(e) => report.fail(dir, "golden", e.to_string()),
        }

        // Layer 4.
        match apply::check(&fixture, pg_major).await {
            Ok(o) if o.is_ok() => {}
            Ok(apply::ApplyOutcome::ApplyFailed { stderr, stage }) => {
                report.fail(dir, "apply", format!("{stage} failed:\n{stderr}"));
            }
            Ok(apply::ApplyOutcome::IrMismatch(diff)) => {
                report.fail(dir, "apply", format!("post-apply IR diverged:\n{diff}"));
            }
            Ok(apply::ApplyOutcome::UnexpectedSuccess) => {
                report.fail(
                    dir,
                    "apply",
                    "fixture expected apply.succeeds=false but apply succeeded",
                );
            }
            Ok(apply::ApplyOutcome::Ok | apply::ApplyOutcome::Skipped) => {}
            Err(e) => report.fail(dir, "apply", e.to_string()),
        }
    }

    eprintln!(
        "conformance: {} fixtures discovered, {} ran (pg{}), {} skipped (version-gated)",
        fixtures.len(),
        ran,
        pg_major,
        skipped
    );

    if !report.failures.is_empty() {
        panic!(
            "{} conformance failure(s):\n\n{}",
            report.failures.len(),
            report.failures.join("\n\n")
        );
    }
}
```

- [ ] **Step 3: Run the conformance suite**

Run: `cargo test -p pgevolve-conformance --test run -- --nocapture`
Expected:
- Layers 1 + 2 pass for the proof-of-life fixture.
- Layer 3 passes (goldening is opted out via `golden = false`).
- Layer 4 passes if Docker is available; reports `Skipped` otherwise.
- Overall test passes.

If the diff substring `"app.users.email"` doesn't appear in the actual rendered diff, the message in the failure tells you the actual rendering — adjust the fixture's `expect.diff.contains` to match what the parser/diff produces.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-conformance/tests/
git commit -m "feat(conformance): walker + proof-of-life ADD COLUMN fixture

Single tokio test that discovers every fixture.toml under
tests/cases/, runs all four assertion layers against each, and
aggregates failures with directory paths so triage is direct. The
proof-of-life fixture covers ADD COLUMN nullable; golden compare is
opted out pending Task 11's bless command."
```

---

## Task 11: `cargo xtask bless --conformance`

**Files:**
- Modify: `xtask/Cargo.toml` (add dep on `pgevolve-conformance`)
- Modify: `xtask/src/main.rs` (extend dispatch; add `bless_conformance`)
- Modify: `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/fixture.toml` (delete `golden = false`)
- Create: `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/expected/plan.sql` (produced by the bless command)

- [ ] **Step 1: Inspect the existing `bless` flow**

Read `xtask/src/main.rs` from the `match cmd.as_str()` dispatch through `async fn bless()`. The new subcommand mirrors the structure: walk `cases/`, for each fixture render the plan via `pgevolve_conformance::planning::render_plan`, normalize via `pgevolve_conformance::normalize::normalize`, write to `expected/plan.sql`.

- [ ] **Step 2: Add the dep**

Edit `xtask/Cargo.toml` `[dependencies]`:

```toml
pgevolve-conformance = { path = "../crates/pgevolve-conformance" }
```

- [ ] **Step 3: Extend the dispatcher and add the bless function**

In `xtask/src/main.rs`, change the dispatch match to:

```rust
match cmd.as_str() {
    "bless" => {
        let kind = std::env::args().nth(2);
        match kind.as_deref() {
            Some("--conformance") => bless_conformance(),
            _ => bless().await,
        }
    }
    "" | "help" | "--help" | "-h" => {
        eprintln!("usage: cargo xtask <bless | bless --conformance>");
        Ok(())
    }
    other => Err(anyhow!("unknown subcommand: {other}")),
}
```

Then append, below `async fn bless()`:

```rust
fn bless_conformance() -> Result<()> {
    use pgevolve_conformance::fixture::Fixture;
    use pgevolve_conformance::normalize::normalize;
    use pgevolve_conformance::planning::render_plan;
    use pgevolve_core::plan::Strategy;

    let cases = workspace_root()?.join("crates/pgevolve-conformance/tests/cases");
    if !cases.exists() {
        return Err(anyhow!("conformance cases dir not found: {}", cases.display()));
    }

    let mut blessed = 0usize;
    let mut skipped = 0usize;

    for entry in walkdir::WalkDir::new(&cases) {
        let entry = entry?;
        if entry.file_name() != "fixture.toml" {
            continue;
        }
        let dir = entry
            .path()
            .parent()
            .ok_or_else(|| anyhow!("fixture.toml has no parent"))?
            .to_path_buf();
        let fixture = Fixture::load(&dir)
            .with_context(|| format!("load fixture {}", dir.display()))?;

        let Some(rel) = fixture.expect.plan.golden.as_ref() else {
            skipped += 1;
            tracing::info!(fixture = %dir.display(), "skipping: goldening opted out");
            continue;
        };

        let strategy = fixture
            .passthrough
            .planner
            .get("strategy")
            .and_then(|v| v.as_str())
            .map(|s| match s {
                "atomic" => Strategy::Atomic,
                _ => Strategy::Online,
            })
            .unwrap_or(Strategy::Online);

        let (_plan, rendered_sql) =
            render_plan(&fixture.before_sql, &fixture.after_sql, strategy)
                .with_context(|| format!("render plan for {}", dir.display()))?;
        let normalized = normalize(&rendered_sql);

        let golden_path = dir.join(rel);
        if let Some(parent) = golden_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&golden_path, normalized)
            .with_context(|| format!("write {}", golden_path.display()))?;
        blessed += 1;
        tracing::info!(fixture = %dir.display(), "blessed");
    }

    eprintln!("conformance: blessed {blessed} fixture(s); skipped {skipped}");
    Ok(())
}
```

If `Strategy::Atomic` doesn't exist (verify in `crates/pgevolve-core/src/plan/policy.rs`), match the actual variant names.

- [ ] **Step 4: Bless the proof-of-life fixture**

Delete the `golden = false` line from `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/fixture.toml`:

```toml
[expect.plan]
steps = 1
```

Then run: `cargo xtask bless --conformance`

Expected stderr: `conformance: blessed 1 fixture(s); skipped 0`
Expected file: `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/expected/plan.sql` exists and contains the normalized plan.

- [ ] **Step 5: Verify the suite passes with the golden in place**

Run: `cargo test -p pgevolve-conformance --test run -- --nocapture`
Expected: PASS — all four layers green for the proof-of-life fixture.

- [ ] **Step 6: Commit**

```bash
git add xtask/Cargo.toml xtask/src/main.rs crates/pgevolve-conformance/tests/cases/
git commit -m "feat(xtask): bless --conformance regenerates plan.sql goldens

Walks tests/cases, renders the in-process planner pipeline for each
fixture, normalizes plan.sql, writes to the fixture's expected golden
path. Opted-out fixtures (golden = false) are skipped. Bless the
proof-of-life ADD COLUMN fixture as the first golden."
```

---

## Task 12: Gate property tests behind `#[ignore]`

**Files:**
- Modify: `crates/pgevolve-core/tests/property_tests.rs`
- Modify: `crates/pgevolve/tests/pg_property_tests.rs`
- Modify: `crates/pgevolve/tests/chaos_apply.rs`

The non-discovery (deterministic) tests in these files — if any — stay live. Only the actual proptest-driven blocks get `#[ignore]`.

- [ ] **Step 1: Identify the proptest test functions**

Run: `grep -n "proptest!\|#\[test\]\|#\[tokio::test\]" crates/pgevolve-core/tests/property_tests.rs crates/pgevolve/tests/pg_property_tests.rs crates/pgevolve/tests/chaos_apply.rs`

List the test function names. Mark *every* function that wraps a `proptest!` invocation or otherwise depends on randomness.

- [ ] **Step 2: Add `#[ignore = "..."]` above each identified test**

For every test function identified in Step 1, add:

```rust
#[ignore = "property test — run via property-tests workflow or `cargo test -- --ignored`"]
```

immediately above the `#[test]` or `#[tokio::test]` attribute.

If a file's tests are *all* property tests, add this at the top of the file as a module-level comment for human readers:

```rust
//! All tests in this file are property tests; #[ignore]'d for CI.
//! Run with `cargo test --test <name> -- --ignored` locally, or via the
//! property-tests.yml workflow.
```

- [ ] **Step 3: Verify default `cargo test` excludes the property tests**

Run: `cargo test --workspace --tests`
Expected: stdout for each crate shows the property tests with status `ignored`. No property tests run.

Run: `cargo test --workspace --tests -- --ignored`
Expected: now the property tests run. (They may or may not pass depending on Docker availability.)

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/tests/property_tests.rs crates/pgevolve/tests/pg_property_tests.rs crates/pgevolve/tests/chaos_apply.rs
git commit -m "test: #[ignore] property tests; conformance suite is the CI gate

Property tests remain for nightly discovery (run via --ignored or the
new property-tests.yml workflow) but no longer block PRs. The
deterministic conformance suite (crates/pgevolve-conformance) is the
new gate. See docs/superpowers/specs/2026-05-11-conformance-test-suite-design.md."
```

---

## Task 13: CI cutover — conformance job + property-tests workflow

**Files:**
- Modify: `.github/workflows/ci.yml`
- Create: `.github/workflows/property-tests.yml`

- [ ] **Step 1: Replace the `pg-matrix` job with `conformance`**

Edit `.github/workflows/ci.yml`. Replace the `pg-matrix:` job (currently lines ~54+) with:

```yaml
  conformance:
    name: conformance (tier 3 + tier C)
    needs: [test]
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        pg: ["14", "15", "16", "17"]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { toolchain: 1.95 }
      - uses: Swatinem/rust-cache@v2
      - name: tier-3 catalog goldens + tier-C conformance
        run: cargo test --workspace --tests
        env:
          PGEVOLVE_TEST_PG_VERSION: ${{ matrix.pg }}
          # PROPTEST_CASES intentionally unset — property tests are now
          # #[ignore]'d by default. The property-tests.yml workflow runs
          # them on a schedule.
```

The `--ignored` flag is *not* passed, so property tests skip.

- [ ] **Step 2: Create the property-tests workflow**

Create `.github/workflows/property-tests.yml`:

```yaml
name: property-tests

on:
  schedule:
    # Daily at 04:00 UTC. Adjust to taste; the spec calls for nightly.
    - cron: '0 4 * * *'
  workflow_dispatch:

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: -D warnings

jobs:
  property:
    name: property tests (pg ${{ matrix.pg }})
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        pg: ["14", "15", "16", "17"]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { toolchain: 1.95 }
      - uses: Swatinem/rust-cache@v2
      - name: property tests
        run: cargo test --workspace --tests -- --ignored
        env:
          PGEVOLVE_TEST_PG_VERSION: ${{ matrix.pg }}
          PROPTEST_CASES: "200"
```

Failures here do not block PRs (workflow runs on `schedule` + manual dispatch only) — they surface as failed nightly runs that the team triages and converts into deterministic regression fixtures under `crates/pgevolve-conformance/tests/cases/regressions/`.

- [ ] **Step 3: Verify the workflow files are syntactically valid**

If the repo has `actionlint` installed locally, run: `actionlint .github/workflows/ci.yml .github/workflows/property-tests.yml`. If not, push the branch and confirm via GitHub Actions UI that the workflows parse on the next push. (No `act`-style local execution required.)

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/
git commit -m "ci: conformance is the gate; property tests move to nightly

CI now runs unit + tier-2 + tier-3 + tier-C across the PG 14-17 matrix
and excludes property tests by default. The new property-tests.yml
workflow runs the property suite on a daily schedule with
PROPTEST_CASES=200 across the same matrix; failures do not block PRs."
```

---

## Task 14: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the test tiers table**

In `README.md`, locate the "Test tiers" table (currently around line 139). Replace it with:

```markdown
| Tier | Where | Runs | Needs Docker | CI gate |
|------|-------|------|--------------|---------|
| 1 | unit tests in `src/` | `cargo test --workspace --lib` | no | yes |
| 2 | parser/IR fixture corpora | `cargo test --workspace --tests` | no | yes |
| 3 | catalog round-trip goldens | `cargo test --workspace --tests` | yes | yes |
| C | **conformance suite (`crates/pgevolve-conformance`)** | `cargo test -p pgevolve-conformance` | yes (apply layer) | yes |
| 5 | property tests | `cargo test --workspace --tests -- --ignored` | partial | no — nightly only |
| 7 | weekly soak | manual / cron | yes | no |
```

Append below the table:

> The conformance suite (Tier C) is the canonical regression gate; every
> deterministic correctness expectation lives there as a fixture. Property
> tests run nightly to surface new failure shapes that are then permanently
> captured as conformance fixtures under
> `crates/pgevolve-conformance/tests/cases/regressions/`. See
> [`docs/superpowers/specs/2026-05-11-conformance-test-suite-design.md`](docs/superpowers/specs/2026-05-11-conformance-test-suite-design.md).

- [ ] **Step 2: Update the workspace layout section**

In `README.md`, locate "Workspace layout" (around line 125). Add a new bullet:

```markdown
- `crates/pgevolve-conformance` — deterministic fixture-driven
  conformance suite: one directory per fixture, asserts diff / plan /
  plan.sql golden / apply roundtrip. New goldens via
  `cargo xtask bless --conformance`.
```

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: document Tier C conformance suite in README"
```

---

## Follow-up plans (not in this plan)

These are referenced for context and must be written as their own plans after this one lands:

1. **C2 — Port Tier-4 scenarios.** Audit `crates/pgevolve/tests/cli_e2e.rs`, `executor_smoke.rs`, and the deterministic parts of `shadow_validate.rs`. Identify each distinct scenario, port to a conformance fixture, delete the Rust-coded duplicate. Target: ~20 fixtures.
2. **C3 — Per-version delta docs.** Author `docs/spec/pg-versions/pg{14,15,16,17}.md` from the official Postgres release notes, scoped to changes that affect objects pgevolve manages.
3. **C4 — Exhaustive fixture authoring.** For every entry in `docs/spec/` marked "Implemented" or "Partial", author the fixtures needed to cover it across the supported PG range. Implement `cargo xtask coverage` + `--check` mode. Make `coverage --check` a CI gate once C4 is complete.

---

## Self-Review

### Spec coverage

Walking the spec section by section:

- **Architecture → Tier C as new crate.** Tasks 2–10 + 13.
- **CI gates table.** Task 13.
- **Discovery → regression flow.** Documented in the spec; the *path* for capturing failures into `regressions/` is enabled by the runner (Task 10) but the regression population itself happens organically once property tests start finding things in the nightly workflow (Task 13). No specific task for "capture first regression" — that's reactive work.
- **Fixture layout.** Tasks 3 + 10.
- **`fixture.toml` schema.** Task 3 + Task 8 (golden deserializer).
- **Per-PG overrides.** Task 3's `golden_path()` resolves `per-pg/pg<N>/plan.sql`; Task 8 uses it; Task 10 passes `pg_major` through.
- **Layer 1 (diff invariants).** Task 6.
- **Layer 2 (plan invariants).** Task 7.
- **Layer 3 (plan SQL golden).** Task 8.
- **Layer 4 (apply roundtrip).** Task 9.
- **Bless command.** Task 11.
- **Version matrix → coverage.md → CI gate.** Out of scope (C4 follow-up plan), explicitly called out.
- **Determinism prerequisite (C0).** Task 1.
- **Phasing C0–C5.** C0 = Task 1; C1 = Tasks 2–11; C2/C3/C4 deferred; C5 = Tasks 12–13.

### Placeholder scan

- No TBDs.
- "If a file's tests are *all* property tests" in Task 12 Step 2 is contingent guidance — the inspector in Step 1 confirms which files qualify before the comment is added. Acceptable.
- Task 11 Step 3 mentions `Strategy::Atomic` with a fallback ("verify in policy.rs") — the engineer is told exactly where to verify and what to do if the variant name differs. Acceptable.
- Task 7's `plan_uses` substring approach is documented as a known weakness with the fallback called out. Acceptable.

### Type consistency

- `Fixture` field set is consistent across Tasks 3, 6, 7, 8, 9, 10 (`dir`, `before_sql`, `after_sql`, `meta`, `pg`, `passthrough`, `expect`).
- `ExpectPlan.golden` is `Option<String>` everywhere; the deserializer in Task 8 produces the same shape.
- `render_plan` signature (Task 4) — `(before, after, strategy) -> (Plan, String)` — used identically in Tasks 7, 11.
- `Fixture::applies_to(pg_major: u32)` and `golden_path(pg_major: u32)` use `u32`; Task 10's `active_pg_major()` returns `u32`; Task 9 `pg_version_from_major` accepts `u32`. Consistent.
- `ApplyOutcome` variants match what `tests/run.rs` matches on (Task 10).

No drift found.
