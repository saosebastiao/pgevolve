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
