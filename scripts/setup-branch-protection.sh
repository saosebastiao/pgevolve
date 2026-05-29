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
