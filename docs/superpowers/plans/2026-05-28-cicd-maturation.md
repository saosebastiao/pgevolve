# CI/CD Maturation Implementation Plan (sub-project D of v1.0 path)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Three independent commits maturing the release pipeline: (1) branch protection on `main` via a script, (2) two new README badges (soak + MSRV), (3) `scripts/release.sh` runbook walker + RELEASING.md update.

**Architecture:** All shell + Markdown. No Rust code. The branch-protection and release scripts use `gh` CLI + `jq`. The actual branch-protection API call requires a maintainer admin token and isn't auto-run by this plan; the script is committed for reproducibility and the rule is applied manually after the script lands.

**Tech Stack:** Bash, `gh` CLI, `jq`, Markdown.

**Spec:** [`../specs/2026-05-28-cicd-maturation-design.md`](../specs/2026-05-28-cicd-maturation-design.md)

---

## Pre-flight

1. Confirm `main` is green: `git log --oneline -1`, `gh run list --branch main --limit 1` → ✅.
2. Read the spec end-to-end. Sections §1–§4 are the work; §5 is out-of-scope; §6 specifies the 3-commit split.
3. Verify local tooling: `gh --version` returns ≥ 2.0, `jq --version` returns ≥ 1.6, `bash --version` returns ≥ 4.0.

## Per-task verify gate (run before each commit)

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

Plus, for the script changes:
```sh
bash -n scripts/<file>.sh   # parse-only syntax check on each new/modified script
shellcheck scripts/<file>.sh # if shellcheck is installed (optional but recommended)
```

(No `cargo test` — this plan touches no Rust.)

---

## File structure

### Created
- `scripts/setup-branch-protection.sh` — Task 1
- `scripts/release.sh` — Task 3

### Modified
- `README.md` — Task 2 (2 badge lines inserted)
- `docs/RELEASING.md` — Task 3 (CLAUDE.md §11 step inserted, scripts/release.sh cross-reference added, yank step added)

### New top-level directory
- `scripts/` — new directory. No existing `scripts/` in the repo today; this plan establishes it. The directory has no `.gitignore` requirements; the two `.sh` files are committed as executable.

---

## Task 1: Branch-protection setup script (commit 1)

**Files:**
- Create: `scripts/setup-branch-protection.sh`

### Step 1.1: Create the script

- [ ] Create `scripts/setup-branch-protection.sh` with this exact content:

```bash
#!/usr/bin/env bash
# scripts/setup-branch-protection.sh — apply main's branch-protection
# rule per docs/superpowers/specs/2026-05-28-cicd-maturation-design.md §1.
#
# Requires: gh CLI logged in as a repo admin.
# Idempotent: safe to re-run when the required-check set changes.
set -euo pipefail

REPO="${PGEVOLVE_REPO:-saosebastiao/pgevolve}"

CHECKS_JSON='[
  "rustfmt",
  "clippy",
  "cargo deny check",
  "cargo doc",
  "test (unit + tier-2)",
  "Property-test issue compliance",
  "CHANGELOG version sync",
  "conformance (tier 3 + tier C) (14)",
  "conformance (tier 3 + tier C) (15)",
  "conformance (tier 3 + tier C) (16)",
  "conformance (tier 3 + tier C) (17)",
  "conformance (tier 3 + tier C) (18)"
]'

BODY=$(jq -n --argjson contexts "$CHECKS_JSON" '{
  required_status_checks: {
    strict: true,
    contexts: $contexts
  },
  enforce_admins: false,
  required_pull_request_reviews: null,
  restrictions: null,
  allow_force_pushes: false,
  allow_deletions: false
}')

echo "Applying branch protection to $REPO/main..."
gh api -X PUT "repos/$REPO/branches/main/protection" --input - <<< "$BODY"
echo "Done. Verify: gh api repos/$REPO/branches/main/protection | jq ."
```

### Step 1.2: Make the script executable

- [ ] Run:
```sh
chmod +x scripts/setup-branch-protection.sh
```

### Step 1.3: Syntax-check the script

- [ ] Run:
```sh
bash -n scripts/setup-branch-protection.sh
```
Expected: no output, exit code 0.

(Optional, if `shellcheck` is installed:)
```sh
shellcheck scripts/setup-branch-protection.sh
```
Expected: no output, exit code 0.

### Step 1.4: Verify gate

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```
Expected: all three pass (no Rust touched; sanity).

### Step 1.5: Commit

```bash
git add scripts/setup-branch-protection.sh
git commit -m "$(cat <<'EOF'
ci: scripts/setup-branch-protection.sh for main

Idempotent script that applies the GitHub branch-protection rule per
docs/superpowers/specs/2026-05-28-cicd-maturation-design.md §1:
- 12 required status checks (fmt, clippy, deny, doc, test,
  property-status, changelog, 5-PG conformance matrix)
- strict mode (branches must be up-to-date)
- no force pushes (everyone, including admins)
- no deletions
- admin-exempt for PR-review-required (none required today; future
  contributor PRs would gate on the status checks)

The script is tooling + documentation; running it requires the
maintainer's admin token. Apply once now:
  scripts/setup-branch-protection.sh
Re-run only when the required-check name set changes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Step 1.6: Apply the rule (one-time, requires admin auth)

After the commit lands and is pushed, the maintainer runs:
```sh
./scripts/setup-branch-protection.sh
```
Expected output (truncated):
```
Applying branch protection to saosebastiao/pgevolve/main...
{
  "url": "https://api.github.com/repos/saosebastiao/pgevolve/branches/main/protection",
  ...
}
Done. Verify: gh api repos/saosebastiao/pgevolve/branches/main/protection | jq .
```

This step is operational, not part of the commit. The script is the deliverable; the rule taking effect on GitHub is a one-time maintainer action after the commit pushes.

---

## Task 2: README badges (commit 2)

**Files:**
- Modify: `README.md`

### Step 2.1: Add the soak badge

- [ ] Open `README.md`. The existing badge block sits at lines 5-8:

```markdown
[![crates.io](https://img.shields.io/crates/v/pgevolve.svg)](https://crates.io/crates/pgevolve)
[![docs.rs](https://img.shields.io/docsrs/pgevolve-core)](https://docs.rs/pgevolve-core)
[![CI](https://github.com/saosebastiao/pgevolve/actions/workflows/ci.yml/badge.svg)](https://github.com/saosebastiao/pgevolve/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
```

Insert a new badge line immediately AFTER the `[![CI]` line and BEFORE the `[![License]` line:

```markdown
[![Soak](https://github.com/saosebastiao/pgevolve/actions/workflows/soak.yml/badge.svg)](https://github.com/saosebastiao/pgevolve/actions/workflows/soak.yml)
```

### Step 2.2: Add the MSRV badge

- [ ] Append a new badge line AFTER the `[![License]` line:

```markdown
[![MSRV: 1.95+](https://img.shields.io/badge/MSRV-1.95+-blue.svg)](#install)
```

Final 6-badge block:

```markdown
[![crates.io](https://img.shields.io/crates/v/pgevolve.svg)](https://crates.io/crates/pgevolve)
[![docs.rs](https://img.shields.io/docsrs/pgevolve-core)](https://docs.rs/pgevolve-core)
[![CI](https://github.com/saosebastiao/pgevolve/actions/workflows/ci.yml/badge.svg)](https://github.com/saosebastiao/pgevolve/actions/workflows/ci.yml)
[![Soak](https://github.com/saosebastiao/pgevolve/actions/workflows/soak.yml/badge.svg)](https://github.com/saosebastiao/pgevolve/actions/workflows/soak.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![MSRV: 1.95+](https://img.shields.io/badge/MSRV-1.95+-blue.svg)](#install)
```

### Step 2.3: Verify gate

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```
Expected: all three pass.

### Step 2.4: Commit

```bash
git add README.md
git commit -m "$(cat <<'EOF'
docs(README): add Soak + MSRV badges

Soak badge surfaces the v1.0-gate signal (per docs/v1.md §3 and
cargo xtask soak-streak). MSRV badge pins to the workspace
rust-version = 1.95.

Final block: crates.io | docs.rs | CI | Soak | License | MSRV.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `scripts/release.sh` + RELEASING.md update (commit 3)

**Files:**
- Create: `scripts/release.sh`
- Modify: `docs/RELEASING.md`

### Step 3.1: Create the release script

- [ ] Create `scripts/release.sh` with this exact content:

```bash
#!/usr/bin/env bash
# scripts/release.sh — interactive release runbook walker.
# Encodes CLAUDE.md §11: never `cargo publish` until CI is green on
# both the push commit and the tag push.
#
# Usage: scripts/release.sh X.Y.Z
set -euo pipefail

VERSION="${1:?usage: scripts/release.sh X.Y.Z}"

confirm() {
  local prompt="$1"
  read -r -p "$prompt [y/N] " resp
  [[ "$resp" == "y" || "$resp" == "Y" ]] || { echo "aborted."; exit 1; }
}

echo "=== Pre-flight verify gate (local) ==="
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo deny check
echo "Pre-flight passed."

echo
echo "=== Bump versions ==="
echo "Edit Cargo.toml + crates/pgevolve-core-macros/Cargo.toml + CHANGELOG.md."
echo "Set workspace.package.version = \"$VERSION\""
echo "Set crates/pgevolve-core-macros/Cargo.toml [package].version = \"$VERSION\""
echo "Date-stamp the CHANGELOG entry: ## [$VERSION] — $(date -u +%Y-%m-%d)"
${EDITOR:-vi} Cargo.toml crates/pgevolve-core-macros/Cargo.toml CHANGELOG.md
cargo build --workspace   # refresh Cargo.lock
confirm "Versions + CHANGELOG ready?"

echo
echo "=== Release commit ==="
git add Cargo.toml Cargo.lock CHANGELOG.md crates/pgevolve-core-macros/Cargo.toml
git commit -m "release: v$VERSION"
git log --oneline -1

echo
echo "=== Push main + wait for CI ==="
confirm "Push main?"
git push origin main
COMMIT=$(git rev-parse HEAD)
sleep 5
RUN_ID=$(gh run list --branch main --commit "$COMMIT" --limit 1 --json databaseId --jq '.[0].databaseId')
if [ -z "$RUN_ID" ] || [ "$RUN_ID" = "null" ]; then
  echo "Waiting briefly for CI to register the push..."
  sleep 15
  RUN_ID=$(gh run list --branch main --commit "$COMMIT" --limit 1 --json databaseId --jq '.[0].databaseId')
fi
echo "Watching CI run $RUN_ID (Ctrl-C halts; tag + publish not run yet)"
gh run watch "$RUN_ID" --exit-status

echo
echo "=== Sign + push tag ==="
git tag -s "v$VERSION" -m "pgevolve v$VERSION"
git verify-tag "v$VERSION"
confirm "Push tag?"
git push origin "v$VERSION"

echo
echo "=== Publish ==="
confirm "Publish to crates.io?"
cargo publish -p pgevolve-core
echo "Waiting 30s for crates.io index sync..."
sleep 30
cargo publish -p pgevolve

echo
echo "v$VERSION released."
echo "If a prior version had a bug, yank it now:"
echo "  cargo yank --version <prior> pgevolve-core"
echo "  cargo yank --version <prior> pgevolve"
```

### Step 3.2: Make the script executable

- [ ] Run:
```sh
chmod +x scripts/release.sh
```

### Step 3.3: Syntax-check the script

- [ ] Run:
```sh
bash -n scripts/release.sh
```
Expected: no output, exit code 0.

(Optional, if `shellcheck` is installed:)
```sh
shellcheck scripts/release.sh
```
Expected: no errors. May surface info-level suggestions; address only if they're clearly bugs.

### Step 3.4: Rewrite `docs/RELEASING.md`

- [ ] Replace the entire contents of `docs/RELEASING.md` with the version below. The intent is to preserve the prose runbook (still useful as reference) while making it clear that `scripts/release.sh` is the canonical executable form, and inserting the CLAUDE.md §11 wait-for-CI-green step explicitly.

```markdown
# Releasing pgevolve

This runbook applies the [Constitution §9](./CONSTITUTION.md#9-cicd-and-release-discipline)
release discipline + the [CLAUDE.md §11](../CLAUDE.md) "never `cargo
publish` until CI is green" rule (added 2026-05-28 after the v0.3.8
disaster).

## Canonical executable form

For a normal release, run:

```sh
scripts/release.sh X.Y.Z
```

The script walks every step below, gates on the pre-flight verify,
waits for CI green on both the push and the tag, and prompts before
each irreversible action (publish, push, tag). If you bail at any
prompt, nothing irrecoverable has happened.

The prose runbook below documents what the script is doing, for
when something goes wrong and you need to step through manually.

---

## Pre-flight

Before starting the release commit, the following must be true:

- [ ] All open PRs targeting the release have merged.
- [ ] `cargo test --workspace --all-targets` is green locally.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is green.
- [ ] `cargo fmt --all -- --check` is clean.
- [ ] `cargo doc --workspace --no-deps` builds with zero warnings (run with `RUSTDOCFLAGS=-D warnings`).
- [ ] `cargo deny check` passes. Install if missing: `cargo install cargo-deny`.
- [ ] The `[Unreleased]` section of `CHANGELOG.md` accurately describes everything in the release.
- [ ] The previous release tag's signature verifies (`git verify-tag <prev-tag>`) — proves your signing key is configured.

## Release commit

1. **Update versions** — three locations must agree:
   - `Cargo.toml` → `[workspace.package].version`
   - `crates/pgevolve-core-macros/Cargo.toml` → `[package].version` (not workspace-inherited because it's a proc-macro crate with its own version literal)
   - The next CHANGELOG section header

2. **Date-stamp the CHANGELOG entry**:
   ```markdown
   ## [X.Y.Z] — YYYY-MM-DD
   ```
   The CI `changelog` job (in `.github/workflows/ci.yml`) verifies that the `Cargo.toml` version has a matching `## [X.Y.Z] — DATE` line.

3. **Rebuild so `Cargo.lock` picks up the new version**:
   ```sh
   cargo build --workspace
   ```

4. **Commit**:
   ```sh
   git add Cargo.toml Cargo.lock CHANGELOG.md crates/pgevolve-core-macros/Cargo.toml
   git commit -m "release: vX.Y.Z"
   ```

## Push main + WAIT FOR CI GREEN

Push the release commit:
```sh
git push origin main
```

Then **wait for the per-push CI run to finish green across all 5 PG
majors**. Per CLAUDE.md §11, this is non-negotiable — today's v0.3.8
disaster happened because the maintainer published immediately after
the tag push while CI was still running. CI then failed on PG 15 + 16
(broken ICU SQL); the buggy v0.3.8 was already on crates.io.

```sh
COMMIT=$(git rev-parse HEAD)
RUN_ID=$(gh run list --branch main --commit "$COMMIT" --limit 1 --json databaseId --jq '.[0].databaseId')
gh run watch "$RUN_ID" --exit-status
```

`--exit-status` makes the command return non-zero if any job fails;
do not proceed past this step until it returns 0.

## Tag

**Tags must be signed.** Per Constitution §9, an unsigned release tag is not a valid release.

```sh
git tag -s vX.Y.Z -m "pgevolve vX.Y.Z

<short release summary, 2-3 lines>"
```

Verify locally:
```sh
git verify-tag vX.Y.Z
```

If `git tag -s` complains, your signing key isn't configured. See:
- `git config user.signingkey <KEY-ID>`
- `git config gpg.format ssh` (for SSH signing — recommended in 2026)
- `git config gpg.ssh.allowedSignersFile <path>` (for verification)

The first time you sign, also enable signing-by-default for safety:
```sh
git config commit.gpgsign true
git config tag.gpgsign true
```

## Push tag

```sh
git push origin vX.Y.Z
```

The tag push does not trigger CI by itself in the current workflow
setup — CI already ran on the underlying commit when you pushed
main above. If for some reason the tag is on a different commit
than main's HEAD, wait for CI green on that commit too before
publishing.

## Publish to crates.io

When the release is ready for crates.io, publish in dependency order:

```sh
cargo publish -p pgevolve-core-macros
# Wait ~30 seconds for the index to sync, then:
cargo publish -p pgevolve-core
# Wait ~30 seconds again, then:
cargo publish -p pgevolve
```

`pgevolve-core-macros` is a proc-macro crate that's only published so
`pgevolve-core` resolves on crates.io — it's not a stable public API.
Bumping its version follows the same workspace-bump cadence; lock it
in lockstep with `pgevolve-core` to avoid version-skew surprises.

`pgevolve-conformance`, `pgevolve-testkit`, and `xtask` are all
`publish = false` and stay local.

For pre-publish sanity:
```sh
cargo publish --dry-run -p pgevolve-core-macros
cargo publish --dry-run -p pgevolve-core
cargo publish --dry-run -p pgevolve
```

## Yank a prior version (if shipping a fix)

If the version you just published replaces a buggy prior version
(v0.3.9 replacing the broken v0.3.8 is the canonical example), yank
the prior so cargo prefers the new version:

```sh
cargo yank --version <prior> pgevolve-core
cargo yank --version <prior> pgevolve
```

The CHANGELOG entry for the fix release should call out the yank
explicitly in a `### Yanked` section. Yanking does not remove the
prior version from crates.io — cargo can still resolve it if pinned —
but it stops new installs from picking it up.

## Post-release

- Push the tag (already done above; this is the reminder bullet).
- Verify the badge updates: README's `[![crates.io]` and `[![Soak]`
  badges refresh within a few minutes.
- If this release closes a v1.0-checklist row (per
  [`v1.md`](./v1.md) §4), flip the row's status in
  [`spec/objects.md`](./spec/objects.md) and remove it from
  [`spec/roadmap.md`](./spec/roadmap.md)'s Active matrix in a
  follow-up docs commit.
```

### Step 3.5: Verify gate

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
bash -n scripts/release.sh
```
Expected: all four pass.

### Step 3.6: Commit

```bash
git add scripts/release.sh docs/RELEASING.md
git commit -m "$(cat <<'EOF'
ci+docs: scripts/release.sh runbook walker + RELEASING.md rewrite

scripts/release.sh walks the release runbook step-by-step, gating on
the local verify gate, then waiting for CI green on the push commit
BEFORE allowing the tag-and-publish steps. Encodes CLAUDE.md §11 (the
"wait for CI green before cargo publish" rule that today's v0.3.8
disaster taught us). Each irreversible step is gated by a `confirm`
prompt; the maintainer can bail with Ctrl-C at any point and nothing
irrecoverable has happened.

RELEASING.md rewritten to:
- Point at scripts/release.sh as the canonical executable form
- Keep the prose runbook for when something goes wrong and a manual
  walk-through is needed
- Insert the "wait for CI green" step explicitly (the gap that caused
  v0.3.8)
- Add the post-release yank step with v0.3.8 → v0.3.9 as worked
  example

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Self-review

Quick sanity check the three commits land cleanly:

- [ ] `git log --oneline -3` shows three new commits matching Tasks 1, 2, 3.
- [ ] `git show HEAD~2 --stat` shows: `scripts/setup-branch-protection.sh` new file (executable).
- [ ] `git show HEAD~1 --stat` shows: `README.md` modified (2 line insertions).
- [ ] `git show HEAD --stat` shows: `scripts/release.sh` new file (executable), `docs/RELEASING.md` modified.
- [ ] Run `ls -l scripts/` and confirm both scripts have the executable bit set.
- [ ] Re-read the new `docs/RELEASING.md`:
  - Confirm the canonical-executable-form section appears at the top.
  - Confirm the "Push main + WAIT FOR CI GREEN" section exists with the `gh run watch --exit-status` snippet.
  - Confirm the yank section exists.
- [ ] Re-read the new badges block in `README.md`:
  - Confirm 6 badges in the order: crates.io, docs.rs, CI, Soak, License, MSRV.
- [ ] Open `scripts/release.sh` and confirm the `confirm()` function gates every irreversible step.

No code changes; nothing further to test beyond the verify gate (already run per task) and this manual audit.

---

## Self-review (plan author's pass)

**1. Spec coverage:**
- Spec §1 (branch protection script + JSON body) → Task 1 ✓
- Spec §2 (soak badge + MSRV badge) → Task 2 ✓
- Spec §3 (scripts/release.sh) → Task 3, Step 3.1 ✓
- Spec §4 (RELEASING.md rewrite: cross-ref the script, insert CI-green-wait step, add yank step) → Task 3, Step 3.4 ✓
- Spec §6 (3 independent commits) → Tasks 1, 2, 3 are 3 commits ✓
- Spec §5 (out of scope: issue templates, mutation tests, etc.) → none touched ✓

All covered.

**2. Placeholder scan:** No TBD / TODO / "fill in" markers. All
scripts have exact content; all commands have expected outputs; all
commit messages are concrete. The `shellcheck` runs are marked
optional (some dev environments don't have it; the plan doesn't fail
on its absence).

**3. Type consistency:** N/A — no Rust. The 12 required-status-check
names in Task 1 must exactly match the `name:` fields of the
corresponding jobs in `.github/workflows/ci.yml` (and `soak.yml`
post-sub-project-C). The names listed match what `gh run list --json
jobs` returns today; verified during the brainstorm explore phase.

---

## Execution handoff

After Task 4 self-review passes, **do not push** automatically — per
CLAUDE.md directive 11, the user handles `git push origin main`.
Surface the three commits with `git log -3 --stat` and wait for the
explicit push confirmation.

After the push lands and CI confirms the new soak badge is rendering,
the maintainer manually runs:
```sh
./scripts/setup-branch-protection.sh
```
to apply the branch-protection rule. That step is operational (needs
admin auth), not part of any commit.
