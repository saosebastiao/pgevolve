---
status: design
target: v1.0
sub_project: D (CI/CD maturation)
---

# CI/CD maturation — design

Sub-project **D** of the v1.0 path. Three changes that mature the
release pipeline without changing the established direct-to-main +
manual-publish workflow: branch protection on `main`, two new README
badges, and a `scripts/release.sh` runbook walker that operationalises
the CLAUDE.md §11 "wait for CI green before publishing" rule.

The user chose **manual release** (no publish-on-tag automation) in
the brainstorm: the runbook walker is the chosen safety net.

---

## §1. Branch protection on `main`

**Problem.** Today `main` has no GitHub branch protection. Direct
push of arbitrary commits is allowed by anyone with write access, and
nothing prevents force-push or branch deletion. This is fine for a
solo maintainer who's careful — but it provides no safety net, and
signals "no quality bar" to first-time contributors.

**Fix.** Enable a narrow branch-protection rule via the GitHub API:

| Setting | Value |
|---|---|
| Required status checks | strict mode (branches must be up to date with main) |
| Required check names | `rustfmt`, `clippy`, `cargo deny check`, `cargo doc`, `test (unit + tier-2)`, `Property-test issue compliance`, `CHANGELOG version sync`, `conformance (tier 3 + tier C) (14)`, `…(15)`, `…(16)`, `…(17)`, `…(18)` |
| Allow force pushes | **no** (everyone, including admins) |
| Allow deletions | **no** |
| Required pull-request reviews | not enabled |
| Enforce for administrators | **no** — direct push for the maintainer still works |

This combination:
- Blocks future contributor PRs that don't pass CI.
- Prevents force-push to main (the most common foot-gun).
- Prevents accidental branch deletion.
- Keeps the maintainer's direct-push flow working (admin-exempt).
- Doesn't require pull requests for the maintainer's own work
  (matches CLAUDE.md §9 "commits go directly to main for this
  project").

Configured via `scripts/setup-branch-protection.sh`. The script is
idempotent: re-running it overwrites the rule with the same JSON
body. Committed for reproducibility; the actual API call requires an
admin token, so it's tooling + documentation, not auto-run.

### Script

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

---

## §2. README badges

Add two badges to the existing 4-badge block in `README.md`:

1. **Soak workflow status** — the v1.0 release-gate signal per
   charter §3. Direct link to the soak.yml workflow's latest run:
   ```markdown
   [![Soak](https://github.com/saosebastiao/pgevolve/actions/workflows/soak.yml/badge.svg)](https://github.com/saosebastiao/pgevolve/actions/workflows/soak.yml)
   ```

2. **MSRV badge** — pinned to 1.95, matching `Cargo.toml`
   `rust-version` and CLAUDE.md "requires Rust 1.95+":
   ```markdown
   [![MSRV: 1.95+](https://img.shields.io/badge/MSRV-1.95+-blue.svg)](#install)
   ```
   The badge link anchors to the README's own Install section
   (which states the MSRV); no external URL dependency.

Final badge order in README (left-to-right):

1. crates.io
2. docs.rs
3. CI
4. Soak (new)
5. License
6. MSRV (new)

Insert positions: soak goes right after CI (both are workflow
badges); MSRV goes at the end (it's a static informational badge,
not a status badge).

---

## §3. `scripts/release.sh` — runbook walker

**Problem.** RELEASING.md is a prose runbook. Today's v0.3.8 disaster
happened because the maintainer published immediately after pushing
the tag, before CI confirmed green — RELEASING.md predated CLAUDE.md
§11 and didn't enforce the wait. A script makes the steps mandatory
and ordered.

**Fix.** New `scripts/release.sh` that walks the release runbook
step-by-step with `confirm` prompts between gates. The maintainer
runs it with the new version number; the script handles the
mechanical steps and forces the wait-for-CI-green checkpoint.

### Script

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
RUN_ID=$(gh run list --branch main --commit "$COMMIT" --limit 1 --json databaseId --jq '.[0].databaseId')
if [ -z "$RUN_ID" ] || [ "$RUN_ID" = "null" ]; then
  echo "Waiting briefly for CI to register the push..."
  sleep 15
  RUN_ID=$(gh run list --branch main --commit "$COMMIT" --limit 1 --json databaseId --jq '.[0].databaseId')
fi
echo "Watching CI run $RUN_ID (Ctrl-C if you need to bail; tag + publish not run yet)"
gh run watch "$RUN_ID" --exit-status

echo
echo "=== Sign + push tag ==="
git tag -s "v$VERSION" -m "pgevolve v$VERSION"
git verify-tag "v$VERSION"
confirm "Push tag?"
git push origin "v$VERSION"

echo "Waiting for tag-push CI run to register..."
sleep 15
TAG_RUN_ID=$(gh run list --branch main --commit "$COMMIT" --workflow ci.yml --limit 5 --json databaseId,event --jq 'map(select(.event=="push")) | .[0].databaseId')
echo "Watching tag-push CI run $TAG_RUN_ID"
gh run watch "$TAG_RUN_ID" --exit-status

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

### Failure modes the script handles

- Local verify-gate fails → exits before any commit.
- CHANGELOG/version edit incorrect → maintainer answers `n` at the
  confirmation; script exits without committing.
- Main-push CI fails → `gh run watch --exit-status` exits non-zero;
  script halts before tagging. Maintainer can either fix-forward
  with a new commit and re-run, or `git reset --hard HEAD~1` to undo
  the release commit.
- Tag-push CI fails → same: `--exit-status` halts before publish.
  Maintainer can `git tag -d v$VERSION && git push --delete origin
  v$VERSION` to undo if the tag needs to move, OR roll forward to a
  new patch.
- Publish fails (e.g. token expired) → cargo's own error message;
  maintainer re-runs the publish step manually.

---

## §4. RELEASING.md update

Rewrite RELEASING.md to:
- Point at `scripts/release.sh` as the canonical executable form.
- Keep the prose runbook for "what's happening behind the script"
  reference.
- Insert the CLAUDE.md §11 "wait for CI green" step explicitly in the
  prose runbook (currently absent — that gap caused v0.3.8).
- Add a post-release "yank the prior version if it had a bug" step,
  using v0.3.8 → v0.3.9 as the worked example.

The full rewritten RELEASING.md is included in the implementation
plan's task list; the spec just states the shape.

---

## §5. Out of scope (deferred)

- Issue templates, PR template, SECURITY.md polish → **sub-project G**
  (community surface).
- Mutation testing, fuzzer, perf benchmarks, coverage badges →
  charter §3 says post-1.0.
- Automated publish-on-tag → user explicitly chose manual.
- Required PR reviews → would block the established direct-to-main
  flow (CLAUDE.md §9).
- Dependabot / Renovate config → could add, but `cargo deny` +
  manual `cargo update` is the established cadence; defer to a future
  brainstorm if/when dep churn becomes burdensome.
- README badges beyond the 2 added here → coverage / mutation / etc.
  are post-1.0 per charter §3.

---

## §6. What this design produces

Three commits (in any order; independent):

**Commit 1** — `scripts/setup-branch-protection.sh` + the actual
branch-protection API call. The script is committed; running it
requires the maintainer's admin token (one-off action). The commit
message documents that the rule was applied at commit time.

**Commit 2** — `README.md` badge additions (2 lines added).

**Commit 3** — `scripts/release.sh` + RELEASING.md rewrite.

Could collapse to fewer commits at the maintainer's discretion; each
is small enough to stand alone.
