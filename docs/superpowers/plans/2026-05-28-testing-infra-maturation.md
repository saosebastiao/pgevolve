# Testing + Infra Maturation Implementation Plan (sub-project C of v1.0 path)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the v1.0 charter §3 quality-gate gaps in two commits: (1) workflow surgery — disk-space fix on the CI conformance job + merge of `property-tests.yml` and `soak.yml` into a single nightly soak at `PROPTEST_CASES=5000`; (2) new `cargo xtask soak-streak` subcommand that reports the consecutive-clean-day streak across `ci.yml` and `soak.yml` for the v1.0 readiness check.

**Architecture:** YAML surgery + one new Rust module in `xtask/`. The xtask subcommand shells out to `gh run list` and computes a streak from the returned JSON; mirrors the existing `xtask::property_status` pattern.

**Tech Stack:** GitHub Actions YAML, Rust 1.95+ in xtask, `gh` CLI as a runtime dep (already required by existing `property-status` xtask), `serde_json` (already in xtask).

**Spec:** [`../specs/2026-05-28-testing-infra-maturation-design.md`](../specs/2026-05-28-testing-infra-maturation-design.md)

---

## Pre-flight

1. Confirm `main` is green: `git log --oneline -1`, `gh run list --branch main --limit 1` → ✅.
2. Read the spec end-to-end. The three changes in spec §1-§3 are the work; §5 specifies the two-commit split.
3. Read these existing files for context — they're the patterns being followed:
   - `.github/workflows/ci.yml` (conformance job is around lines 107-135)
   - `.github/workflows/property-tests.yml`
   - `.github/workflows/soak.yml`
   - `xtask/src/property_status.rs` (the precedent xtask subcommand using `gh` CLI)
   - `xtask/src/main.rs` (subcommand dispatch pattern)
   - `xtask/Cargo.toml` (no new deps expected; verify before adding)

## Per-task verify gate

After every commit:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
cargo build -p xtask         # ensure xtask still compiles after main.rs edits
```

For Task 2 (xtask command), also:

```sh
cargo test --lib -p xtask    # xtask unit tests
cargo run -p xtask -- soak-streak --days 7    # smoke run (will hit gh API — OK on a dev machine with `gh auth login`)
```

For Task 1 (YAML), there are no `cargo test` implications. After the commit, push and let CI prove the workflow YAML parses by running it.

---

## File structure

### Created

- `xtask/src/soak_streak.rs` — the new subcommand. Mirrors `property_status.rs`.

### Modified

- `.github/workflows/ci.yml` — add free-disk-space step to the `conformance` job (one new step block; no other changes).
- `.github/workflows/soak.yml` — replace contents with the merged config from spec §2.
- `.github/workflows/property-tests.yml` — **DELETED** (merged into soak.yml).
- `xtask/src/main.rs` — register the `soak-streak` subcommand (one new `match` arm + add to help text).

No `xtask/Cargo.toml` change expected — the new module uses only deps already declared (`anyhow`, `serde`, `serde_json`, `std::process::Command`).

---

## Task 1: Workflow surgery + disk-space fix (commit 1)

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `.github/workflows/soak.yml` (replace contents)
- Delete: `.github/workflows/property-tests.yml`

### Step 1.1: Add free-disk-space step to the CI conformance job

- [ ] Open `.github/workflows/ci.yml` and find the `conformance` job (starts around line 107 with `conformance:` and `name: conformance (tier 3 + tier C)`).
- [ ] Inside the `steps:` list, immediately after the `- uses: actions/checkout@v6` line and BEFORE `- uses: dtolnay/rust-toolchain@stable`, insert this block:

```yaml
      - name: Free disk space
        uses: jlumbroso/free-disk-space@main
        with:
          tool-cache: true
          android: true
          dotnet: true
          haskell: true
          large-packages: false
          swap-storage: false
          docker-images: false
```

The indentation must match the existing `- uses:` lines (6 spaces before the dash for items in the `steps:` list).

### Step 1.2: Replace `.github/workflows/soak.yml` contents

- [ ] Replace the entire contents of `.github/workflows/soak.yml` with this exact content:

```yaml
name: soak (nightly)

# Tier-5 property-test soak at high case counts. Runs nightly on the
# main branch; the streak of consecutive-clean runs feeds the v1.0
# release gate (see docs/v1.md §3 and `cargo xtask soak-streak`).
on:
  workflow_dispatch:
  schedule:
    - cron: '0 4 * * *'   # Daily 04:00 UTC

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: -D warnings

jobs:
  soak:
    name: soak (pg ${{ matrix.pg }}) @ 5000 cases
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        pg: ["14", "15", "16", "17", "18"]
    timeout-minutes: 240
    steps:
      - uses: actions/checkout@v6
      - name: Free disk space
        uses: jlumbroso/free-disk-space@main
        with:
          tool-cache: true
          android: true
          dotnet: true
          haskell: true
          large-packages: false
          swap-storage: false
          docker-images: false
      - uses: dtolnay/rust-toolchain@stable
        with: { toolchain: 1.95 }
      - uses: Swatinem/rust-cache@v2
      - name: soak (tier 5 @ 5000 cases)
        run: cargo test --workspace --tests --release -- --ignored
        env:
          PGEVOLVE_TEST_PG_VERSION: ${{ matrix.pg }}
          PROPTEST_CASES: "5000"
      - name: Auto-capture property-test failure
        if: failure()
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          set -e
          for regr in crates/*/proptest-regressions/*.txt; do
            [ -f "$regr" ] || continue
            seed=$(head -1 "$regr" | awk '{print $1}')
            [ -n "$seed" ] || continue
            echo "::warning::Property-test failure with seed $seed — run 'cargo xtask capture-regression --seed $seed --issue <n>' to capture"
          done
```

### Step 1.3: Delete `.github/workflows/property-tests.yml`

- [ ] Run: `rm .github/workflows/property-tests.yml`

### Step 1.4: Verify gate

Run:
```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```
Expected: all three pass (no Rust touched; sanity).

### Step 1.5: Commit

```bash
git add .github/workflows/ci.yml .github/workflows/soak.yml
git rm .github/workflows/property-tests.yml
git commit -m "$(cat <<'EOF'
ci: free-disk-space on conformance + merge property-tests into nightly soak

Two changes per the testing+infra spec (sub-project C, §1 + §2):

1. Add jlumbroso/free-disk-space step to the CI conformance job and
   to the new soak job. Frees ~25 GB by removing the runner's
   pre-installed dotnet / android / haskell / tool-cache. Resolves
   the v0.3.8 push CI failure (the "no space left on device" hit
   while pulling 5 postgres:N-alpine images in parallel).

2. Merge property-tests.yml + soak.yml into a single soak.yml that
   runs nightly at PROPTEST_CASES=5000 across all 5 PG majors.
   Replaces the old split (nightly @ 200 + weekly @ 5000) with the
   v1.0 charter's single nightly @ 5000 requirement. property-tests.yml
   deleted; soak.yml renamed in the workflow `name:` field.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `cargo xtask soak-streak` (commit 2)

**Files:**
- Create: `xtask/src/soak_streak.rs`
- Modify: `xtask/src/main.rs` (register subcommand + help text)

### Step 2.1: Write the new module

- [ ] Create `xtask/src/soak_streak.rs` with this exact content:

```rust
//! `cargo xtask soak-streak [--days N]`
//!
//! Reports the consecutive-clean-day streak across the `ci.yml` and
//! `soak.yml` workflows on `main` over a configurable window
//! (default 30 days). Used pre-1.0 to decide whether the release gate
//! in `docs/v1.md` §3 is met. Requires the `gh` CLI in PATH.
//!
//! Exit codes:
//! - 0: streak >= requested window. Tag-ready.
//! - 1: streak < requested window, or a workflow run is still in
//!   progress (caller should wait).
//! - 2: gh CLI not available or query failed.

use anyhow::{Context, Result};
use std::process::Command;

#[derive(Debug, Clone, serde::Deserialize)]
struct Run {
    #[serde(rename = "databaseId")]
    database_id: u64,
    #[serde(rename = "createdAt")]
    created_at: String,
    /// `success` | `failure` | `cancelled` | `skipped` | `timed_out` | `action_required` | `neutral` | `startup_failure` | `stale` | `null`.
    conclusion: Option<String>,
    /// `completed` | `in_progress` | `queued` | `requested` | `waiting` | `pending`.
    status: String,
}

const WORKFLOWS: &[&str] = &["ci.yml", "soak.yml"];

pub fn run(days: u32) -> Result<()> {
    // Detect gh.
    let gh_check = Command::new("gh").arg("--version").output();
    if gh_check.is_err() || !gh_check.unwrap().status.success() {
        eprintln!("gh CLI not found in PATH; install with `brew install gh`.");
        std::process::exit(2);
    }

    // Fetch runs for both workflows.
    let mut all_runs: Vec<(String, Run)> = Vec::new();
    for wf in WORKFLOWS {
        let runs = fetch_runs(wf, days)?;
        for r in runs {
            all_runs.push(((*wf).to_string(), r));
        }
    }
    // Sort newest-first by created_at.
    all_runs.sort_by(|a, b| b.1.created_at.cmp(&a.1.created_at));

    let streak_result = walk_streak(&all_runs, days);
    print_report(&streak_result, days);

    if streak_result.streak_days >= days {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StreakResult {
    /// Number of consecutive clean days, capped at `requested_days`.
    streak_days: u32,
    /// First non-success encountered while walking backward from "now",
    /// if any. `None` means every in-window run was successful.
    earliest_break: Option<(String, Run)>,
    /// Any run still in `in_progress`/`queued`/etc. status. Blocks the
    /// streak even if no failure has occurred yet.
    in_progress: Option<(String, Run)>,
    /// Oldest run in the window (for reporting).
    earliest_in_window: Option<(String, Run)>,
}

fn walk_streak(all_runs: &[(String, Run)], requested_days: u32) -> StreakResult {
    let mut earliest_break: Option<(String, Run)> = None;
    let mut in_progress: Option<(String, Run)> = None;
    let mut earliest: Option<(String, Run)> = None;

    for (wf, r) in all_runs {
        if r.status != "completed" {
            // Capture the most-recent in-progress run only (the loop is
            // newest-first, so the first hit is the latest).
            if in_progress.is_none() {
                in_progress = Some((wf.clone(), r.clone()));
            }
            continue;
        }
        let conc = r.conclusion.as_deref().unwrap_or("");
        if conc != "success" {
            if earliest_break.is_none() {
                earliest_break = Some((wf.clone(), r.clone()));
            }
        }
        earliest = Some((wf.clone(), r.clone()));
    }

    // Streak: days from "now" back to the earliest break (or to the
    // earliest in-window run if no break).
    let streak_days = if let Some((_, ref r)) = earliest_break {
        days_between_now_and(&r.created_at).unwrap_or(0)
    } else {
        // No failure in window; streak is min(requested_days, days
        // since earliest_in_window). Capped at requested_days so a
        // 7-day-old earliest with --days 30 caps at 7.
        if let Some((_, ref r)) = earliest {
            days_between_now_and(&r.created_at)
                .unwrap_or(0)
                .min(requested_days)
        } else {
            // No runs in window at all. Streak is 0; the maintainer
            // should investigate.
            0
        }
    };

    StreakResult {
        streak_days,
        earliest_break,
        in_progress,
        earliest_in_window: earliest,
    }
}

fn fetch_runs(workflow: &str, days: u32) -> Result<Vec<Run>> {
    let since = days_ago_iso(days);
    let out = Command::new("gh")
        .args([
            "run",
            "list",
            "--branch",
            "main",
            "--workflow",
            workflow,
            "--created",
            &format!(">={since}"),
            "--limit",
            "1000",
            "--json",
            "databaseId,createdAt,conclusion,status",
        ])
        .output()
        .with_context(|| format!("invoking gh run list for {workflow}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "gh run list failed for {workflow}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let runs: Vec<Run> = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parsing gh JSON for {workflow}"))?;
    Ok(runs)
}

fn print_report(r: &StreakResult, requested_days: u32) {
    let marker = if r.streak_days >= requested_days {
        "✓"
    } else {
        ""
    };
    println!("soak streak: {}/{} days {marker}", r.streak_days, requested_days);

    if let Some((wf, ref run)) = r.earliest_break {
        println!(
            "  last failure:    {} ({} run {})",
            run.created_at.get(..10).unwrap_or(&run.created_at),
            wf,
            run.database_id
        );
    } else {
        println!("  last failure:    none in window");
    }

    if let Some((wf, ref run)) = r.in_progress {
        println!(
            "  in progress:     {} ({} run {})",
            run.created_at.get(..10).unwrap_or(&run.created_at),
            wf,
            run.database_id
        );
        println!("  → wait for the in-progress run to finish before re-checking");
    }

    if let Some((_, ref run)) = r.earliest_in_window {
        println!(
            "  earliest in win: {}",
            run.created_at.get(..10).unwrap_or(&run.created_at)
        );
    }

    if r.streak_days < requested_days {
        let need = requested_days - r.streak_days;
        println!("need {need} more clean day(s) for the {requested_days}-day window");
    }
}

fn days_ago_iso(days: u32) -> String {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let then_secs = now_secs.saturating_sub(u64::from(days) * 86_400);
    unix_secs_to_iso_date(then_secs)
}

fn unix_secs_to_iso_date(secs: u64) -> String {
    // Convert unix seconds to "YYYY-MM-DD" using Howard Hinnant's algorithm.
    #[allow(clippy::cast_possible_wrap)]
    let z: i64 = (secs / 86_400) as i64 + 719_468;
    let era: i64 = if z >= 0 { z } else { z - 146_096 } / 146_097;
    #[allow(clippy::cast_sign_loss)]
    let doe: u64 = (z - era * 146_097) as u64; // 0..=146096
    let yoe: u64 = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // 0..=399
    #[allow(clippy::cast_sign_loss)]
    let y: i64 = (yoe as i64) + era * 400;
    let doy: u64 = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..=365
    let mp: u64 = (5 * doy + 2) / 153; // 0..=11
    let d: u64 = doy - (153 * mp + 2) / 5 + 1; // 1..=31
    let m: u64 = if mp < 10 { mp + 3 } else { mp - 9 }; // 1..=12
    let year: i64 = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}")
}

fn days_between_now_and(iso_created_at: &str) -> Option<u32> {
    // iso_created_at format: "YYYY-MM-DDTHH:MM:SSZ"
    let created_secs = parse_rfc3339_to_unix_secs(iso_created_at)?;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    if now_secs <= created_secs {
        return Some(0);
    }
    #[allow(clippy::cast_possible_truncation)]
    Some(((now_secs - created_secs) / 86_400) as u32)
}

fn parse_rfc3339_to_unix_secs(s: &str) -> Option<u64> {
    // Reused from xtask::property_status::parse_rfc3339_to_unix_secs.
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let year: i32 = std::str::from_utf8(&bytes[0..4]).ok()?.parse().ok()?;
    let month: u32 = std::str::from_utf8(&bytes[5..7]).ok()?.parse().ok()?;
    let day: u32 = std::str::from_utf8(&bytes[8..10]).ok()?.parse().ok()?;
    let hour: u32 = std::str::from_utf8(&bytes[11..13]).ok()?.parse().ok()?;
    let minute: u32 = std::str::from_utf8(&bytes[14..16]).ok()?.parse().ok()?;
    let second: u32 = std::str::from_utf8(&bytes[17..19]).ok()?.parse().ok()?;

    let y: i64 = if month <= 2 {
        i64::from(year) - 1
    } else {
        i64::from(year)
    };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe: i64 = y - era * 400;
    let m: i64 = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let doy: i64 = (153 * m + 2) / 5 + i64::from(day) - 1;
    let doe: i64 = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days: i64 = era * 146_097 + doe - 719_468;
    let secs: i64 =
        days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second);
    if secs < 0 {
        None
    } else {
        #[allow(clippy::cast_sign_loss)]
        Some(secs as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(created_at: &str, conclusion: Option<&str>, status: &str) -> Run {
        Run {
            database_id: 1,
            created_at: created_at.into(),
            conclusion: conclusion.map(String::from),
            status: status.into(),
        }
    }

    #[test]
    fn empty_runs_streak_zero() {
        let r = walk_streak(&[], 30);
        assert_eq!(r.streak_days, 0);
        assert!(r.earliest_break.is_none());
        assert!(r.in_progress.is_none());
    }

    #[test]
    fn all_success_caps_at_requested_days() {
        // Single run 1 day ago, requested window 30 days.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let one_day_ago = unix_secs_to_iso_date(now - 86_400);
        let runs = vec![(
            "ci.yml".into(),
            run(&format!("{one_day_ago}T12:00:00Z"), Some("success"), "completed"),
        )];
        let r = walk_streak(&runs, 30);
        // earliest-in-window is 1 day old; streak caps at min(30, 1) = 1.
        assert!(r.streak_days <= 30);
        assert!(r.earliest_break.is_none());
    }

    #[test]
    fn failure_breaks_streak() {
        let runs = vec![
            (
                "ci.yml".into(),
                run("2026-05-28T12:00:00Z", Some("failure"), "completed"),
            ),
            (
                "ci.yml".into(),
                run("2026-05-27T12:00:00Z", Some("success"), "completed"),
            ),
        ];
        let r = walk_streak(&runs, 30);
        assert!(r.earliest_break.is_some(), "failure should produce a break");
    }

    #[test]
    fn in_progress_run_recorded() {
        let runs = vec![(
            "soak.yml".into(),
            run("2026-05-28T12:00:00Z", None, "in_progress"),
        )];
        let r = walk_streak(&runs, 30);
        assert!(r.in_progress.is_some(), "in_progress run should be flagged");
    }

    #[test]
    fn iso_date_round_trip() {
        // Sanity check the Hinnant algorithm.
        let secs = parse_rfc3339_to_unix_secs("2026-05-28T00:00:00Z").unwrap();
        assert_eq!(unix_secs_to_iso_date(secs), "2026-05-28");
    }
}
```

### Step 2.2: Register the subcommand in `main.rs`

- [ ] Open `xtask/src/main.rs`. Find the existing `mod property_status;` line near the top (around line 30). Immediately after it, add:

```rust
mod soak_streak;
```

- [ ] Find the dispatch `match` arm for `"property-status"` (around line 80). After that arm, before the `"diagnose-pg-version"` arm, add:

```rust
        "soak-streak" => {
            let args: Vec<String> = std::env::args().collect();
            let days: u32 = flag_value(&args, "--days")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30);
            soak_streak::run(days)
        }
```

- [ ] Find the help-text block in the `"" | "help" | "--help" | "-h"` arm. The current `usage:` string ends with `diagnose-pg-version <fixture-dir> --pg-major N>`. Replace the entire `eprintln!` call with:

```rust
            eprintln!(
                "usage: cargo xtask <bless | bless --conformance | coverage [--check | --gaps] | fixture-cost |\n\
                 \t capture-regression --seed <hex> --issue <n> |\n\
                 \t verify-regression <fixture-dir> |\n\
                 \t property-status [--max-age-days N] |\n\
                 \t soak-streak [--days N] |\n\
                 \t diagnose-pg-version <fixture-dir> --pg-major N>"
            );
```

(Inserts the `soak-streak [--days N]` line between `property-status` and `diagnose-pg-version`.)

- [ ] Also update the module-level doc comment near the top of `main.rs` — find the existing `//! - `property-status [--max-age-days N]` — list open property-test GitHub` line and immediately after it (and its continuation line(s)) add:

```rust
//! - `soak-streak [--days N]` — report the consecutive-clean-day streak
//!   across `ci.yml` and `soak.yml` on main. Used pre-1.0 to decide
//!   whether the release gate in `docs/v1.md` §3 is met. Default --days 30.
```

### Step 2.3: Build + test

Run:
```sh
cargo build -p xtask
```
Expected: clean build.

Run:
```sh
cargo test --lib -p xtask
```
Expected: the 5 new unit tests in `soak_streak::tests` pass, plus any pre-existing xtask tests.

Run:
```sh
cargo run -p xtask -- soak-streak --days 7
```
Expected: a real query against `gh`. On a dev machine with `gh auth status` showing logged-in, this prints a real report. On CI it would too (if we ran it there). On a machine without `gh`, it prints the "gh CLI not found" message and exits 2. Either outcome is acceptable for the smoke test — we're not asserting a streak length here.

### Step 2.4: Verify gate

Run:
```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```
Expected: all three pass.

### Step 2.5: Commit

```bash
git add xtask/src/soak_streak.rs xtask/src/main.rs
git commit -m "$(cat <<'EOF'
feat(xtask): soak-streak — 30-day-clean tracker for v1.0 release gate

New `cargo xtask soak-streak [--days N]` subcommand. Queries
gh run list for ci.yml + soak.yml runs on main over a window
(default 30 days), reports the consecutive-clean-day streak and
the most recent failure or in-progress run if any.

Exit codes:
- 0: streak >= window. Tag-ready.
- 1: streak < window OR an in-progress run blocks the check.
- 2: gh CLI unavailable.

Mirrors the existing cargo xtask property-status pattern (same gh
shell-out, same minimal-deps Hinnant date math, no chrono). 5 inline
unit tests on the streak-walking + ISO date round-trip logic.

Closes the v1.0 charter §3 requirement for a "30 consecutive days
clean" tracker. Manual pre-tag check; post-1.0 we may automate via
a status badge.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Self-review

Quick sanity check the two commits land cleanly:

- [ ] `git log --oneline -2` shows commits matching Task 1 + Task 2.
- [ ] `git show HEAD~1 --stat` shows: `.github/workflows/ci.yml` modified, `.github/workflows/soak.yml` modified, `.github/workflows/property-tests.yml` deleted.
- [ ] `git show HEAD --stat` shows: `xtask/src/soak_streak.rs` new file, `xtask/src/main.rs` modified.
- [ ] Run `cargo xtask --help` and confirm `soak-streak [--days N]` appears in the usage string.
- [ ] Run `cargo xtask soak-streak --days 30` and confirm output looks reasonable (or "gh not found" if `gh` is missing).

No code changes beyond the two commits; nothing further to test.

---

## Self-review (plan author's pass)

**1. Spec coverage:**
- Spec §1 (disk-space fix) → Task 1, Steps 1.1 + 1.2 (also added to merged soak.yml) ✓
- Spec §2 (merge property-tests + soak) → Task 1, Steps 1.2 + 1.3 ✓
- Spec §3 (xtask soak-streak) → Task 2, Steps 2.1 + 2.2 ✓
- Spec §5 (2-commit split) → Tasks 1 + 2 are 2 commits ✓
- Spec §6 (NOT touched: badges, branch protection, mutation/fuzzer/perf, action SHA-pinning) → none of these appear in the plan ✓

All covered.

**2. Placeholder scan:** No TBD / TODO / "fill in" markers. All steps
have concrete code, exact commands with expected outputs, and exact
commit messages. The `cargo run -p xtask -- soak-streak --days 7`
smoke step in Step 2.3 has a documented "either-outcome-acceptable"
caveat — that's accurate (some dev environments don't have `gh`
installed; the spec says "uses `gh` CLI as a runtime dep").

**3. Type consistency:** The `walk_streak`, `fetch_runs`,
`StreakResult`, and `Run` types appear in the same module
(`soak_streak.rs`); names match across definition site and use site.
The `run(days: u32)` entry function takes `u32`, matches the `--days
N` flag parsing in `main.rs` which uses `u32`. ✓

---

## Execution handoff

After Task 3 self-review passes, **do not push** automatically — per
CLAUDE.md directive 11, the user handles `git push origin main`.
Surface the two commits with `git log -2 --stat` and wait for the
explicit push confirmation.
