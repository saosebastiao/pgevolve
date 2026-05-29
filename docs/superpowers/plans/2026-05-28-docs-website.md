# Docs Website Implementation Plan (sub-project E of v1.0 path)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two commits that ship the mdBook-built docs site to `https://saosebastiao.github.io/pgevolve/`: (1) site source files (book.toml, SUMMARY.md, landing page, symlink, two index pages), (2) CI workflow + README link.

**Architecture:** Pure markdown + one TOML + one YAML + one symlink. No Rust changes. `mdbook` builds locally with `cargo install mdbook --version "^0.4" --locked && mdbook build`.

**Tech Stack:** mdBook (Rust binary), GitHub Pages, GitHub Actions (`actions/upload-pages-artifact@v3` + `actions/deploy-pages@v4`).

**Spec:** [`../specs/2026-05-28-docs-website-design.md`](../specs/2026-05-28-docs-website-design.md)

---

## Pre-flight

1. Confirm `main` is green: `git log --oneline -1`, `gh run list --branch main --limit 1` → ✅.
2. Read the spec end-to-end. §1-§8 are the work; §9 is out-of-scope; §10 specifies the 2-commit split.
3. Install mdBook locally if not present: `cargo install mdbook --version "^0.4" --locked` (~2 min build). Verifies the same toolchain CI uses.

## Per-task verify gate (run before each commit)

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

For Task 1 also:
```sh
mdbook build           # build the book locally to confirm SUMMARY.md is valid
mdbook test            # parse-check every markdown file (mdBook treats this as a no-op for non-Rust code blocks)
```

No `cargo test` — docs only.

---

## File structure

### Created (commit 1)
- `book.toml` (repo root)
- `docs/SUMMARY.md`
- `docs/README.md` (site landing page; new file)
- `docs/CHANGELOG.md` (symlink → `../CHANGELOG.md`)
- `docs/superpowers/specs/README.md` (index of 29 spec files)
- `docs/superpowers/plans/README.md` (index of 31 plan files)

### Created (commit 2)
- `.github/workflows/docs.yml`

### Modified (commit 2)
- `README.md` (Documentation link)

### `.gitignore`

mdBook builds to `book/` (configurable via `[build].build-dir`; we leave the default). The repo's `.gitignore` does not currently exclude `book/`. Add a line to `.gitignore` in commit 1 so local `mdbook build` doesn't dirty the working tree:

```
# mdBook build output
/book/
```

If `.gitignore` doesn't exist (unlikely — check `ls -a` first), create it with just that line. If it exists, append.

---

## Task 1: Site source files (commit 1)

**Files:**
- Create: `book.toml`
- Create: `docs/SUMMARY.md`
- Create: `docs/README.md`
- Create: `docs/CHANGELOG.md` (symlink)
- Create: `docs/superpowers/specs/README.md`
- Create: `docs/superpowers/plans/README.md`
- Modify: `.gitignore` (add `/book/`)

### Step 1.1: Create `book.toml`

- [ ] Create `book.toml` at the repo root with exactly this content:

```toml
[book]
title = "pgevolve"
description = "Postgres declarative schema management"
authors = ["Daniel Toone"]
language = "en"
multilingual = false
src = "docs"

[output.html]
git-repository-url = "https://github.com/saosebastiao/pgevolve"
git-repository-icon = "fa-github"
edit-url-template = "https://github.com/saosebastiao/pgevolve/edit/main/{path}"
default-theme = "ayu"
preferred-dark-theme = "ayu"
no-section-label = true

[output.html.search]
limit-results = 30
use-boolean-and = true
```

### Step 1.2: Create `docs/SUMMARY.md`

- [ ] Create `docs/SUMMARY.md` with exactly this content:

```markdown
# Summary

[Introduction](./README.md)

# Getting Started
- [Install](./user/installation.md)
- [Quick start](./user/getting-started.md)
- [Cookbook](./user/cookbook.md)
- [Troubleshooting](./user/troubleshooting.md)

# User Guide
- [Commands](./user/commands.md)
- [Configuration](./user/configuration.md)
- [Plan format](./user/plan-format.md)

# Reference
- [Object kinds](./spec/objects.md)
- [Column types](./spec/column-types.md)
- [Constraints](./spec/constraints.md)
- [Indexes](./spec/indexes.md)
- [Grants & ownership](./spec/grants.md)
- [RLS policies](./spec/policies.md)
- [Reloptions](./spec/reloptions.md)
- [Publications](./spec/publications.md)
- [Subscriptions](./spec/subscriptions.md)
- [Statistics](./spec/statistics.md)
- [Collations](./spec/collations.md)
- [Cluster surface (roles)](./spec/cluster.md)
- [CLI surface](./spec/cli.md)
- [Lint & layout](./spec/lint-and-layout.md)
- [Pipeline](./spec/pipeline.md)
- [Testing tiers](./spec/testing.md)

# Architecture
- [Overview](./system/architecture.md)
- [IR](./system/ir.md)
- [Planner](./system/planner.md)
- [Executor](./system/executor.md)

# Project
- [v1.0 charter](./v1.md)
- [Constitution](./CONSTITUTION.md)
- [Releasing](./RELEASING.md)
- [Changelog](./CHANGELOG.md)
- [Roadmap](./spec/roadmap.md)

# Contributors
- [Specs](./superpowers/specs/README.md)
- [Plans](./superpowers/plans/README.md)
```

### Step 1.3: Create `docs/README.md` (site landing page)

- [ ] Create `docs/README.md` with exactly this content:

```markdown
# pgevolve

Postgres-specific declarative schema management.

`pgevolve` treats a directory of `CREATE`-style SQL files as the
source of truth for one or more Postgres schemas, introspects a live
database to derive its current state, and computes ordered,
dependency-aware migration plans that bring the database to the
desired state. It refuses to lose data unless explicitly authorized
in a per-plan intent file.

Current release: **v0.3.9** (Postgres 14–18). See the
[Changelog](./CHANGELOG.md) for per-release detail.

## What to read

- **New?** Start with [Install](./user/installation.md) and
  [Quick start](./user/getting-started.md).
- **Day-to-day user?** [Commands](./user/commands.md),
  [Configuration](./user/configuration.md),
  [Cookbook](./user/cookbook.md), and
  [Troubleshooting](./user/troubleshooting.md).
- **Asking "does pgevolve support X?"** — the
  [Reference](./spec/objects.md) section is the living capability
  catalogue. Every claimed feature has a fixture; status legend at
  the top of each spec file.
- **Curious how it works?** [Architecture](./system/architecture.md)
  explains the parser → IR → diff → planner → executor pipeline.
- **Considering contributing?** Read the
  [Constitution](./CONSTITUTION.md), the [v1.0 charter](./v1.md),
  and any recent [Specs](./superpowers/specs/README.md) +
  [Plans](./superpowers/plans/README.md) to see how features get
  designed and shipped.

## Project links

- Source: [github.com/saosebastiao/pgevolve](https://github.com/saosebastiao/pgevolve)
- Library API: [docs.rs/pgevolve-core](https://docs.rs/pgevolve-core)
- Issues: [GitHub Issues](https://github.com/saosebastiao/pgevolve/issues)

## License

Dual-licensed under MIT or Apache-2.0.
```

### Step 1.4: Create the `docs/CHANGELOG.md` symlink

- [ ] Run:
```sh
ln -s ../CHANGELOG.md docs/CHANGELOG.md
```

Verify:
```sh
ls -l docs/CHANGELOG.md
```
Expected output (last line):
```
... docs/CHANGELOG.md -> ../CHANGELOG.md
```

### Step 1.5: Create `docs/superpowers/specs/README.md`

- [ ] Create `docs/superpowers/specs/README.md` with exactly this content:

```markdown
# Specs

Design documents — one per sub-spec / feature. Each spec captures the
brainstorming session that produced it, the decisions made, and the
final design before implementation. The matching plan in
[`../plans/`](../plans/README.md) decomposes the design into tasks.

| Date | Spec |
|---|---|
| 2026-05-09 | [pgevolve (initial design)](./2026-05-09-pgevolve-design.md) |
| 2026-05-11 | [Conformance test suite](./2026-05-11-conformance-test-suite-design.md) |
| 2026-05-11 | [Views and materialized views](./2026-05-11-views-and-materialized-views-design.md) |
| 2026-05-15 | [Test strategy v2](./2026-05-15-test-strategy-v2-design.md) |
| 2026-05-15 | [v0.2 architecture review](./2026-05-15-v0.2-architecture-review-design.md) |
| 2026-05-18 | [Functions / procedures](./2026-05-18-functions-procedures-design.md) |
| 2026-05-18 | [User types](./2026-05-18-types-design.md) |
| 2026-05-19 | [Canon consolidation](./2026-05-19-canon-consolidation-design.md) |
| 2026-05-19 | [Diff-derive macro](./2026-05-19-diff-derive-macro-design.md) |
| 2026-05-19 | [In-process apply runner](./2026-05-19-in-process-apply-runner-design.md) |
| 2026-05-20 | [Extensions](./2026-05-20-extensions-design.md) |
| 2026-05-20 | [Rewrite split](./2026-05-20-rewrite-split-design.md) |
| 2026-05-20 | [Triggers](./2026-05-20-triggers-design.md) |
| 2026-05-21 | [Cluster roles](./2026-05-21-cluster-roles-design.md) |
| 2026-05-21 | [Column physical attributes](./2026-05-21-column-physical-attributes-design.md) |
| 2026-05-21 | [Partitioning](./2026-05-21-partitioning-design.md) |
| 2026-05-22 | [Grants and ownership](./2026-05-22-grants-and-ownership-design.md) |
| 2026-05-22 | [RLS policies](./2026-05-22-rls-policies-design.md) |
| 2026-05-22 | [Table reloptions](./2026-05-22-table-reloptions-design.md) |
| 2026-05-26 | [PG 18 + object roadmap](./2026-05-26-pg18-and-object-roadmap-design.md) |
| 2026-05-26 | [Publications](./2026-05-26-publications-design.md) |
| 2026-05-26 | [Subscriptions](./2026-05-26-subscriptions-design.md) |
| 2026-05-27 | [Statistics + view check option](./2026-05-27-statistics-and-check-option-design.md) |
| 2026-05-28 | [Collation + range type](./2026-05-28-collation-and-range-type-design.md) |
| 2026-05-28 | [v1.0 charter](./2026-05-28-v1-charter-design.md) |
| 2026-05-28 | [Roadmap revision](./2026-05-28-roadmap-revision-design.md) |
| 2026-05-28 | [Testing + infra maturation](./2026-05-28-testing-infra-maturation-design.md) |
| 2026-05-28 | [CI/CD maturation](./2026-05-28-cicd-maturation-design.md) |
| 2026-05-28 | [Docs website](./2026-05-28-docs-website-design.md) |
```

### Step 1.6: Create `docs/superpowers/plans/README.md`

- [ ] Create `docs/superpowers/plans/README.md` with exactly this content:

```markdown
# Plans

Implementation plans — one per spec. Each plan decomposes its
matching [spec](../specs/README.md) into bite-sized tasks with
verbatim code, exact commands, and TDD-shaped checkpoints. Most
plans are executed via the subagent-driven-development skill: a
fresh subagent per task with controller review between.

Skeleton stubs for upcoming roadmap items live under
[`_skeleton/`](./_skeleton/); they get promoted to dated plans
when brainstorming begins.

| Date | Plan |
|---|---|
| 2026-05-09 | [pgevolve v0.1](./2026-05-09-pgevolve-v0.1.md) |
| 2026-05-11 | [Conformance test suite](./2026-05-11-conformance-test-suite.md) |
| 2026-05-11 | [Views and materialized views](./2026-05-11-views-and-materialized-views.md) |
| 2026-05-15 | [Test strategy v2](./2026-05-15-test-strategy-v2.md) |
| 2026-05-15 | [v0.2 architecture readiness](./2026-05-15-v0.2-architecture-readiness.md) |
| 2026-05-18 | [Functions / procedures](./2026-05-18-functions-procedures.md) |
| 2026-05-18 | [T13 shadow-validate views](./2026-05-18-t13-shadow-validate-views.md) |
| 2026-05-18 | [User types](./2026-05-18-types.md) |
| 2026-05-19 | [Canon consolidation](./2026-05-19-canon-consolidation.md) |
| 2026-05-19 | [Diff-derive macro](./2026-05-19-diff-derive-macro.md) |
| 2026-05-19 | [In-process apply runner](./2026-05-19-in-process-apply-runner.md) |
| 2026-05-20 | [Extensions](./2026-05-20-extensions.md) |
| 2026-05-20 | [Rewrite split](./2026-05-20-rewrite-split.md) |
| 2026-05-20 | [Triggers](./2026-05-20-triggers.md) |
| 2026-05-21 | [Cluster roles](./2026-05-21-cluster-roles.md) |
| 2026-05-21 | [Column physical attributes](./2026-05-21-column-physical-attributes.md) |
| 2026-05-21 | [Constitution cleanup](./2026-05-21-constitution-cleanup.md) |
| 2026-05-21 | [Partitioning](./2026-05-21-partitioning.md) |
| 2026-05-22 | [Grants and ownership](./2026-05-22-grants-and-ownership.md) |
| 2026-05-22 | [RLS policies](./2026-05-22-rls-policies.md) |
| 2026-05-22 | [Table reloptions](./2026-05-22-table-reloptions.md) |
| 2026-05-26 | [PG 18 + object roadmap](./2026-05-26-pg18-and-object-roadmap.md) |
| 2026-05-26 | [Postgres 18 support](./2026-05-26-postgres-18-support.md) |
| 2026-05-26 | [Publications](./2026-05-26-publications.md) |
| 2026-05-26 | [Subscriptions](./2026-05-26-subscriptions.md) |
| 2026-05-27 | [Statistics + view check option](./2026-05-27-statistics-and-check-option.md) |
| 2026-05-28 | [Collation + range type](./2026-05-28-collation-and-range-type.md) |
| 2026-05-28 | [v1.0 charter](./2026-05-28-v1-charter.md) |
| 2026-05-28 | [Roadmap revision](./2026-05-28-roadmap-revision.md) |
| 2026-05-28 | [Testing + infra maturation](./2026-05-28-testing-infra-maturation.md) |
| 2026-05-28 | [CI/CD maturation](./2026-05-28-cicd-maturation.md) |
| 2026-05-28 | [Docs website](./2026-05-28-docs-website.md) |
```

### Step 1.7: Update `.gitignore`

- [ ] Read `.gitignore` first. If it exists, append a blank line and:

```
# mdBook build output
/book/
```

If `.gitignore` does not exist, create it with that content.

### Step 1.8: Build the book locally

- [ ] Run:
```sh
mdbook build
```

Expected: a `book/` directory appears at the repo root. Open
`book/index.html` in a browser to spot-check. The site should render
the landing page (`docs/README.md`'s content) with a left-side nav
matching `SUMMARY.md`'s structure.

Common failures and fixes:
- "SUMMARY.md is missing a chapter file" → a link in SUMMARY.md
  points at a path that doesn't exist; check the path against `ls
  docs/<path>`.
- "Symbol not found" on the CHANGELOG link → the symlink isn't
  resolving; verify `ls -l docs/CHANGELOG.md` shows the arrow.
- "Could not find configuration file" → `book.toml` isn't at the
  repo root; verify `ls book.toml`.

### Step 1.9: Verify gate

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```
Expected: all three pass (no Rust touched; sanity).

### Step 1.10: Commit

```bash
git add book.toml docs/SUMMARY.md docs/README.md docs/CHANGELOG.md docs/superpowers/specs/README.md docs/superpowers/plans/README.md .gitignore
git commit -m "$(cat <<'EOF'
docs(site): mdBook source files for the docs website

Adds the static-site scaffolding for sub-project E of the v1.0 path:

- book.toml at the repo root (src=docs, default ayu theme, GitHub
  edit-this-page links).
- docs/SUMMARY.md: curated nav with 7 sections (Getting Started,
  User Guide, Reference, Architecture, Project, Contributors,
  Introduction).
- docs/README.md: site landing page (separate from repo-root README,
  which stays the GitHub-rendering version).
- docs/CHANGELOG.md → symlink to repo-root CHANGELOG.md (mdBook's
  src=docs can't reach repo-root files directly; symlink is the
  cleanest bridge).
- docs/superpowers/specs/README.md + plans/README.md: chronological
  indexes of 29 specs + 31 plans for the Contributors section.
- .gitignore: exclude /book/ build output.

Local mdbook build validates the structure. CI deployment lands in
the next commit (.github/workflows/docs.yml).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: CI workflow + README link (commit 2)

**Files:**
- Create: `.github/workflows/docs.yml`
- Modify: `README.md` (Documentation link)

### Step 2.1: Create the workflow

- [ ] Create `.github/workflows/docs.yml` with exactly this content:

```yaml
name: docs

on:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read
  pages: write
  id-token: write

concurrency:
  group: pages
  cancel-in-progress: false

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - name: Install mdBook
        run: cargo install mdbook --version "^0.4" --locked
      - name: Build book
        run: mdbook build
      - uses: actions/upload-pages-artifact@v3
        with: { path: book }

  deploy:
    needs: build
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.deploy.outputs.page_url }}
    steps:
      - id: deploy
        uses: actions/deploy-pages@v4
```

### Step 2.2: Add Documentation link to README

- [ ] Open `README.md`. Find the line `Current release: **v0.3.9** (Postgres 14–18). See` (around line 16). The next line says `[`CHANGELOG.md`](./CHANGELOG.md) for per-release detail.`

After that paragraph (and before the blank line that precedes `## Install`), insert:

```markdown

**Documentation:** <https://saosebastiao.github.io/pgevolve/>
```

(Leading blank line so it becomes its own paragraph.)

### Step 2.3: Verify gate

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```
Expected: all three pass.

### Step 2.4: Commit

```bash
git add .github/workflows/docs.yml README.md
git commit -m "$(cat <<'EOF'
ci+docs: deploy mdBook site to GitHub Pages + add README link

.github/workflows/docs.yml: builds the book on every push to main
and on workflow_dispatch, then deploys via actions/deploy-pages@v4.
concurrency: pages prevents parallel deploys from racing.

README.md: adds a one-line Documentation link to the new site.

One-time follow-up: in GitHub repo Settings → Pages → set Source =
"GitHub Actions" (not "Deploy from a branch"). Without that, the
deploy-pages action will fail. After flipping the setting, the next
push to main publishes the site at:
  https://saosebastiao.github.io/pgevolve/

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Self-review

Quick sanity check the two commits hang together:

- [ ] `git log --oneline -2` shows two new commits matching Tasks 1 + 2.
- [ ] `git show HEAD~1 --stat` shows: `book.toml`, `docs/SUMMARY.md`, `docs/README.md`, `docs/CHANGELOG.md`, `docs/superpowers/specs/README.md`, `docs/superpowers/plans/README.md`, `.gitignore` (7 files; CHANGELOG.md shows as a symlink — git reports it as a regular file in the stat but `ls -l` confirms the arrow).
- [ ] `git show HEAD --stat` shows: `.github/workflows/docs.yml` new, `README.md` modified.
- [ ] Run `mdbook build` one more time to confirm a clean build.
- [ ] Open `book/index.html` in a browser; navigate:
  - Top of nav shows "Introduction" linking to the landing page.
  - "Getting Started" section has 4 entries (Install, Quick start, Cookbook, Troubleshooting).
  - "Reference" has all 16 spec entries.
  - "Contributors" → "Specs" page lists 29 entries.
  - "Contributors" → "Plans" page lists 31 entries.
  - "Project" → "Changelog" loads the CHANGELOG.md content (proves the symlink works).
- [ ] Confirm `.gitignore` includes `/book/`; run `git status` and verify `book/` is NOT in the listed untracked files.

No code changes; nothing further to test.

---

## Task 4: One-time admin action (NOT a commit)

After both commits push and the docs workflow run completes for the
first time, the maintainer manually does:

1. **GitHub repo Settings → Pages → Source = "GitHub Actions"** (not
   "Deploy from a branch").
2. Trigger a workflow run if the first push fired before the setting
   flipped: `gh workflow run docs.yml --ref main`.
3. Visit `https://saosebastiao.github.io/pgevolve/`. Should render the
   landing page within ~1 min of the deploy job completing.

This is operational; the spec + plan deliver the code, the maintainer
does the one-time settings flip.

---

## Self-review (plan author's pass)

**1. Spec coverage:**
- Spec §1 (mdBook tool choice) → implicit; `book.toml` + `cargo install mdbook` use mdBook ✓
- Spec §2 (file layout incl. CHANGELOG symlink) → Task 1, Steps 1.1-1.6 ✓
- Spec §3 (book.toml content) → Task 1, Step 1.1 ✓
- Spec §4 (SUMMARY.md content) → Task 1, Step 1.2 ✓
- Spec §5 (new index pages) → Task 1, Steps 1.5 + 1.6 ✓
- Spec §6 (docs.yml workflow) → Task 2, Step 2.1 ✓
- Spec §7 (one-time admin action) → Task 4 ✓
- Spec §8 (README link) → Task 2, Step 2.2 ✓
- Spec §9 (out of scope) → none touched in plan ✓
- Spec §10 (2-commit split) → Tasks 1 + 2 are 2 commits ✓

All covered.

**2. Placeholder scan:** No TBD / TODO / "fill in" markers. The
`docs/superpowers/specs/README.md` and `plans/README.md` tables
enumerate every file currently in those directories — the lists are
verbatim from `ls`-output captured during the brainstorm explore
phase. Will be one-line stale by the time the next sub-project
ships (this very plan's brainstorm-spec-plan trio would add 3
rows); the maintainer can either pre-emptively add them now or
accept a one-commit-stale state until they ship.

**3. Type consistency:** N/A — no Rust. Markdown link paths in
SUMMARY.md, the index pages, and `docs/README.md` all use relative
paths from `docs/` as the root, matching mdBook's `src = "docs"`
config.

---

## Execution handoff

After Task 3 self-review passes, **do not push** automatically — per
CLAUDE.md directive 11, the user handles `git push origin main`.
Surface the two commits with `git log -2 --stat` and wait for the
explicit push confirmation. Mention Task 4 (the Pages settings flip)
in the handoff message so the user knows it's needed.
