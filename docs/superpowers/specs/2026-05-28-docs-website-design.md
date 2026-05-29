---
status: design
target: v1.0
sub_project: E (docs website)
---

# Docs website (GitHub Pages, mdBook) — design

Sub-project **E** of the v1.0 path. Publishes the existing `docs/`
tree as a browsable site at `https://saosebastiao.github.io/pgevolve/`
using mdBook, the Rust ecosystem's standard documentation toolchain.

Transparency-first: user, system, spec, AND superpowers (specs +
plans) all render. The site complements docs.rs (which handles
library API) by surfacing the operational + capability + process
docs that aren't in rustdoc.

---

## §1. Tool choice — mdBook

mdBook is the natural fit:

- Used by the Rust Book, Cargo Book, Rustonomicon, rustc-dev-guide,
  rust-bindgen book, and many other tier-1 Rust projects.
- Single static binary (`cargo install mdbook`); no Node/Python.
- Builds from a SUMMARY.md table of contents + `.md` source tree.
- Built-in search, dark mode, GitHub edit-this-page link.
- Zero JS dependencies in the generated site (search is a
  pre-built Elasticlunr index).

Trade-off accepted: limited customization (no MDX/components, no
versioned docs). Versioning revisited at v2.0 horizon; until then a
single "latest = main" site is fine.

---

## §2. File layout

### Files added (3 + 2 = 5 new files)

- `book.toml` (repo root) — mdBook config.
- `docs/SUMMARY.md` — site nav (hand-curated, the only mdBook
  artifact inside `docs/`).
- `.github/workflows/docs.yml` — build + deploy on every push to main.
- `docs/superpowers/specs/README.md` — new index page listing every
  spec in chronological order with a one-line summary each.
- `docs/superpowers/plans/README.md` — same for plans.

### Files modified

- `README.md` — one-line site-URL pointer after the current-release
  paragraph.

### Files NOT touched

- No `.md` content moves. Every existing user / system / spec /
  superpowers file stays where it is; the site renders them in place.

### CHANGELOG (repo root) — symlink

`CHANGELOG.md` lives at the repo root. mdBook's `src` is `docs/` so
it can't reach repo-root files directly. The fix: a symlink
`docs/CHANGELOG.md` → `../CHANGELOG.md`. mdBook follows symlinks
silently. Works on every filesystem GitHub Actions uses (Linux
runner, ext4). The repo root stays the canonical home for
`CHANGELOG.md`; the symlink is a build-time alias only.

Same trick would work for `CONSTITUTION.md`, `RELEASING.md`, `v1.md`
if those were at the repo root — but they're already inside `docs/`
so no symlink needed.

---

## §3. `book.toml`

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

`ayu` theme is mdBook's default dark theme used by many Rust books
(Cargo Book uses it). User can flip to `light` / `rust` / `coal` /
`navy` via the in-page theme toggle.

---

## §4. `docs/SUMMARY.md` (site nav)

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

The `Introduction` link maps to `docs/README.md`. That file doesn't
exist today — the project's README is at the repo root. **Will need
a small `docs/README.md` that serves as the site landing page.**
Content: a couple paragraphs introducing pgevolve at site-visit
level (versus the repo-root README which is GitHub-rendering level).
Specified in the implementation plan; trivially extractable from the
repo-root README's first ~30 lines.

---

## §5. New index pages

### `docs/superpowers/specs/README.md`

A single-page index of every design doc. Auto-generation is
overkill; hand-maintained list (one row per spec):

```markdown
# Specs

Design documents — one per sub-spec/feature. Each spec captures the
brainstorming session that produced it, the decisions made, and the
final design before implementation. The matching plan in
[`../plans/`](../plans/README.md) decomposes the design into tasks.

| Date | Spec | Topic |
|---|---|---|
| 2026-05-28 | [v1.0 charter](./2026-05-28-v1-charter-design.md) | Defines what pgevolve v1.0 commits to. |
| 2026-05-28 | [Roadmap revision](./2026-05-28-roadmap-revision-design.md) | Aligns roadmap.md with the v1.0 charter checklist. |
| ... | ... | ... |
```

The full list is built mechanically by walking `docs/superpowers/specs/`
chronologically. The plan task spells out the script (or just `ls`-and-grep).

### `docs/superpowers/plans/README.md`

Same shape for the plan files.

---

## §6. `.github/workflows/docs.yml`

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

Two jobs: build the book, deploy the artifact. `concurrency: pages`
prevents two simultaneous deploys from racing.

---

## §7. One-time admin actions (not commits)

After the workflow lands and the first push completes:

1. **GitHub Settings → Pages → Source = "GitHub Actions"** (not
   "Deploy from a branch"). This authorizes the
   `actions/deploy-pages@v4` action.
2. Verify site at `https://saosebastiao.github.io/pgevolve/`.
3. (Optional) Add a soak-badge-equivalent for the docs build
   to the README later, if desired.

These steps are operational; the spec + plan deliver the code, not
the admin actions.

---

## §8. README link

Add immediately after the existing `Current release:` paragraph in
`README.md`:

```markdown
**Documentation:** <https://saosebastiao.github.io/pgevolve/>
```

Could also add a "Docs" badge — deferred; the inline link is
sufficient. The link makes the site discoverable from the GitHub
landing page.

---

## §9. Out of scope (deferred)

- Custom domain (`pgevolve.dev` etc.) — user explicitly declined.
- Versioned docs (per-release subsites) — no need pre-v1.0.
- Custom theme / CSS — mdBook default `ayu` is sufficient.
- Algolia / external search — built-in mdBook search is enough.
- Doc test (`mdbook test`) in CI — could add later if Rust snippets
  appear in the markdown; today they're all SQL/TOML/JSON which
  mdBook can't test.
- Markdown link-check in CI — useful future polish; not v1.0-blocking.
- API docs surfaced inside the book — `docs.rs` handles it.
- Versioning via gh-pages branch (the older deployment model) — using
  the modern `actions/deploy-pages@v4` artifact-based deploy
  instead.

---

## §10. What this design produces

Two commits, in order:

**Commit 1** — Site source files:
- `book.toml`
- `docs/SUMMARY.md`
- `docs/README.md` (site landing page)
- `docs/CHANGELOG.md` (symlink to `../CHANGELOG.md`)
- `docs/superpowers/specs/README.md`
- `docs/superpowers/plans/README.md`

**Commit 2** — CI workflow + README link:
- `.github/workflows/docs.yml`
- `README.md` (Documentation link)

Split so commit 1 establishes the buildable book (testable locally
with `mdbook serve`) before commit 2 wires CI to publish it.
