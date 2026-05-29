---
status: design
target: v1.0
sub_project: C (testing + infra maturation)
---

# Testing + infra maturation — design

Sub-project **C** of the v1.0 path. Three independent changes that
collectively close the gap between today's CI/test infrastructure and
the v1.0 charter §3 quality gate. The charter `docs/v1.md` §3 is the
contract; this spec is the implementation plan source.

---

## §1. Disk-space fix (CI conformance job)

**Problem.** The v0.3.8 push CI failed with `no space left on device`
while the GitHub-hosted ubuntu-latest runner pulled multiple
`postgres:N-alpine` images for the conformance matrix. The runner
ships with ~14 GB free; pulling 5 Postgres images + Rust build
artifacts compounds past that limit. This blocked the v0.3.8 release
and forced the v0.3.9 patch + yank.

**Fix.** Add a "Free disk space" step as the first step of the
`conformance` job in `.github/workflows/ci.yml`. Same step also added
to the merged soak workflow (§2) since it pulls the same images.

Step uses [`jlumbroso/free-disk-space@main`](https://github.com/jlumbroso/free-disk-space):

```yaml
- name: Free disk space
  uses: jlumbroso/free-disk-space@main
  with:
    tool-cache: true        # frees ~10 GB
    android: true           # frees ~9 GB
    dotnet: true            # frees ~2 GB
    haskell: true           # frees ~5 GB
    large-packages: false   # keep system libs (test deps may need them)
    swap-storage: false     # leave swap alone
    docker-images: false    # KEEP — we want Docker
```

Frees ~25 GB. Adds ~30s per affected job. Applies to:
- `conformance` job in `ci.yml` (5 PG majors)
- `soak` job in the merged `soak.yml` (5 PG majors)
- NOT to other jobs (fmt, clippy, test, deny, doc, property-status —
  none pull Docker images)

The action is pinned to `@main` rather than a tagged version because
the upstream tag cadence is irregular; both `actions/checkout@v6` and
`Swatinem/rust-cache@v2` (already in our workflows) follow the same
pattern. If supply-chain pinning becomes a charter requirement
(sub-project D's call), revisit then.

---

## §2. Merge property-tests + soak workflows

**Problem.** Today we have two parallel proptest workflows:
- `property-tests.yml` — nightly cron, 5 PG majors at `PROPTEST_CASES=200`
- `soak.yml` — weekly Sunday cron, 5 PG majors at `PROPTEST_CASES=5000`

The charter `docs/v1.md` §3 specifies a single nightly soak at 5000
cases. The two-workflow setup doesn't match the charter, and the
nightly-at-200 run isn't doing enough work to count as a soak streak.

**Fix.** Merge into one workflow. The renamed file is `soak.yml`
(it's doing soak duty now); `property-tests.yml` is deleted.

New `.github/workflows/soak.yml`:

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

Key changes from today's setup:
- `PROPTEST_CASES: "5000"` (was 200 in property-tests.yml, 5000 in soak.yml).
- Cron daily (was weekly in soak.yml, daily in property-tests.yml).
- `--release` flag on the cargo test invocation (from soak.yml; faster runtime offsets the 5000-case bump).
- `-- --ignored` flag (from property-tests.yml; required to run the `#[ignore]`'d Docker-bound proptests).
- Free-disk-space step added per §1.
- Auto-capture step preserved from property-tests.yml.
- Renamed `name: soak (nightly)` for clarity.

`property-tests.yml` is deleted in the same commit.

---

## §3. `cargo xtask soak-streak` — 30-day clean tracker

**Problem.** The v1.0 charter §3 mandates "30 consecutive days clean"
before the 1.0 tag. There is no tooling today to compute that streak.
Doing it by hand (`gh run list --workflow=ci.yml`, eyeball N pages of
output) is error-prone.

**Fix.** New xtask subcommand `cargo xtask soak-streak`. Queries
GitHub Actions for `ci.yml` + `soak.yml` runs on `main` over a
configurable window (default 30 days), reports the streak and the
last failure if any. Mirrors the existing `cargo xtask property-status`
shape.

### CLI

```
USAGE:
    cargo xtask soak-streak [--days N]

OPTIONS:
    --days N           Window size in days. Default 30.

EXIT CODES:
    0   Streak >= N days. Tag-ready.
    1   Streak < N days OR a workflow run is still in_progress.
    2   gh CLI not available, or query failed.
```

### Output

```
$ cargo xtask soak-streak
soak streak: 28/30 days
  - last ci.yml failure:   2026-05-15 (run 26500001234)
  - last soak.yml failure: none in window
  - earliest in-window run: 2026-04-28
need 2 more clean days for the 1.0 cut

$ cargo xtask soak-streak --days 7
soak streak: 7/7 days ✓
  - last ci.yml failure:   none in window
  - last soak.yml failure: none in window
```

### Implementation

New file `xtask/src/soak_streak.rs`. The xtask binary's main dispatch
already has a `match` over subcommand names — add `"soak-streak"` arm.

The implementation calls `gh run list` twice (once per workflow),
parses the JSON, and computes the streak. Use `std::process::Command`
matching how `xtask::property_status` shells out today.

Query shape per workflow:
```sh
gh run list \
    --branch main \
    --workflow <ci.yml | soak.yml> \
    --created '>=YYYY-MM-DD' \
    --limit 1000 \
    --json conclusion,createdAt,name,databaseId,status
```

Reduce to: `Vec<(datetime, conclusion)>`. Sort by datetime. Walk from
today backwards; stop at the first non-`success` entry (or end of
window). The streak is the number of days between that entry's
`createdAt` and today.

In-progress runs (`status: "in_progress"`) count as "blocking" — the
streak ends at whichever side of an in-progress run is earlier; the
tool exits with code 1 and tells the user to wait for the run.

No new deps. Uses `serde_json` (already in xtask via the workspace),
`std::process::Command`, and `time` crate (already in workspace).

### Tests

Inline `#[cfg(test)] mod tests` for the streak-walking logic with
hand-built `Vec<(DateTime, &str)>` fixtures. No GitHub calls in tests.

---

## §4. Out of scope (deferred)

Charter §3 deferred to post-1.0:
- Mutation testing (cargo-mutants)
- Fuzzer (cargo-fuzz)
- Code coverage badges
- Perf benchmarks

This sub-project does NOT touch any of those. Each gets its own
brainstorm later if pursued.

Also out of scope (would belong to sub-project D):
- Branch protection rules
- Required-status-check enforcement
- Release automation (CI cuts tags?)
- Status badges in README

---

## §5. What this design produces

Three changes, in two commits:

**Commit 1** — workflow surgery + disk-space fix (`.github/workflows/`):
- Edit `.github/workflows/ci.yml` to add the Free-disk-space step to the conformance job.
- Replace `.github/workflows/property-tests.yml` with the new `soak.yml` content per §2.
- Delete the old `.github/workflows/soak.yml` (weekly variant).
- Net: 2 files modified + 1 deleted + 1 renamed.

**Commit 2** — `cargo xtask soak-streak`:
- Create `xtask/src/soak_streak.rs`.
- Modify `xtask/src/main.rs` to register the subcommand.
- Modify `xtask/Cargo.toml` only if a new dep is needed (none expected).
- Run `cargo build -p xtask` to confirm.
- Inline tests in `soak_streak.rs`.

Split for safe-to-revert reasons: if the workflow changes break a CI
run, commit 1 can be reverted without losing the xtask work.

---

## §6. What this design does NOT do

- Add a soak-streak status badge to README (deferred to post-1.0 per
  the charter; the xtask command is enough for the manual pre-tag
  check).
- Touch the per-push CI matrix (the conformance job already runs all
  5 PG majors; only the disk-space step is added).
- Change the auto-capture step's behavior beyond preserving it in the
  merged workflow.
- Pin `jlumbroso/free-disk-space` to a SHA (revisited if supply-chain
  pinning becomes a project-wide policy in sub-project D).
