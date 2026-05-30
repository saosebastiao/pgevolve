# Development Process Docs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `.github/CONTRIBUTING.md` so external contributors have a clear entry-point to pgevolve's discuss-first workflow.

**Architecture:** A single hand-written markdown file under `.github/`, ~150 lines, eight sections (Welcome, Before you start, Quick start, Verify gate, How decisions get made, Filing issues, License, Code of conduct). Pure documentation: no code paths change, the existing verify gate (fmt + clippy + tests + cargo doc + cargo deny) still passes trivially. CODE_OF_CONDUCT.md was originally in scope but was deferred during plan-writing because no working enforcement-contact channel exists yet — see the amended spec §3.

**Tech Stack:** Markdown only. No build/test changes.

**Source spec:** [`docs/superpowers/specs/2026-05-28-process-docs-design.md`](../specs/2026-05-28-process-docs-design.md) (amended 2026-05-29 to drop CoC from scope).

**Standing project rules (from `CLAUDE.md`):**
- Workspace lints are strict (`clippy::pedantic`, `clippy::nursery`, `-D warnings`). Never `--no-verify`.
- Commits go directly to `main`. Each commit is a coherent, testable unit.
- Every commit ends with the Co-Authored-By trailer.
- **CLAUDE.md §11: never `cargo publish` until CI is green.** Not applicable to this plan (no release), but always in force.

---

## File structure

**Created (1 file):**
- `.github/CONTRIBUTING.md` — 8-section onboarding doc; ~150 lines of markdown.

**Modified (1 file):**
- `docs/superpowers/plans/README.md` — append one row for this plan to the chronological index (per sub-project E's convention).

**Deleted:** none.

`.github/SECURITY.md` and `.github/CODEOWNERS` already exist and establish the placement convention.

---

## Task 1: Add `.github/CONTRIBUTING.md`

**Files:**
- Create: `.github/CONTRIBUTING.md`

- [ ] **Step 1: Verify the target file does not already exist**

Run: `ls .github/CONTRIBUTING.md 2>&1`

Expected: `ls: .github/CONTRIBUTING.md: No such file or directory`

If the file *does* exist, STOP and surface this — the plan assumes a greenfield create and would silently overwrite otherwise.

- [ ] **Step 2: Create `.github/CONTRIBUTING.md`** with the following exact content

````markdown
# Contributing to pgevolve

Thanks for thinking about contributing! pgevolve is a Postgres
declarative schema management tool written in Rust. This document
explains how to get involved.

---

## Welcome

pgevolve is a small, opinionated project. We care about correctness,
type-system rigor, and a tight feedback loop with users. Contributions
of all sizes are welcome — typo fixes, bug reports, feature ideas, and
code — as long as they fit the project's direction.

Before sinking time into a non-trivial change, please read **Before
you start** below: we prefer a short discussion up front to a polished
pull request we then have to ask you to redo.

---

## Before you start

**Open an issue first** for anything beyond a typo or a one-line bug
fix. A two-sentence comment ("I'm thinking of adding X because Y —
does that fit?") saves everyone time. We'd rather catch a scope or
design concern in an issue than in a pull request that has to be
rewritten.

For larger changes, pgevolve uses a lightweight workflow that we
recommend you follow:

1. **Brainstorm** — agree on the shape of the problem in an issue.
2. **Spec** — write the design as a short document under
   [`docs/superpowers/specs/`](../docs/superpowers/specs/) using the
   existing files there as templates. The spec is the contract.
3. **Plan** — decompose the spec into bite-sized tasks under
   [`docs/superpowers/plans/`](../docs/superpowers/plans/).
4. **Implement** — work the plan task-by-task, committing in small
   coherent units.

Steps 2 and 3 sound heavy but they're short documents — usually one
page each. They exist so the design discussion lives in version control
instead of pull-request comment threads. Look at any recent spec/plan
pair for the expected size and tone.

---

## Quick start

You'll need:

- **Rust** ≥ 1.95 (set as `rust-version` in `Cargo.toml`).
- **Docker** — the conformance suite spins up ephemeral Postgres 14, 15,
  16, and 17 containers via testkit fixtures.
- **`cargo-deny`** for the license/advisory audit. Install once with:

  ```sh
  cargo install cargo-deny
  ```

Then:

```sh
git clone https://github.com/saosebastiao/pgevolve.git
cd pgevolve
cargo build --workspace
cargo test --workspace
```

The first `cargo test` pulls Postgres container images and will take
a few minutes. Subsequent runs reuse the images.

---

## The verify gate

Before opening a pull request, run the same gates CI runs. They must
all pass:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
cargo deny check
```

These are non-negotiable: workspace lints are configured to the
pedantic + nursery set with `-D warnings`, and the doc build rejects
broken intra-doc links. If a clippy lint is genuinely wrong for a
specific call site, prefer fixing the call site over `#[allow(...)]`;
when an allow is the right call, justify it in a short comment.

For changes that touch flaky territory (property tests, integration
tests, anything timing-sensitive), run the relevant suite at least
ten times before declaring it green. Non-determinism is a bug, not a
feature.

---

## How decisions get made

The authoritative documents, in order of precedence:

- [**`docs/CONSTITUTION.md`**](../docs/CONSTITUTION.md) — binding
  principles: licensing, dependency policy, type-system rigor,
  Postgres support goals, conventions, security posture. Read this
  first; it overrides everything else in this list.
- [**`docs/v1.md`**](../docs/v1.md) — what pgevolve commits to at v1.0
  (stability surface, quality gates, feature checklist).
- [**`docs/spec/`**](../docs/spec/) — the living capability catalogue:
  which Postgres objects, column types, constraints, etc. pgevolve
  supports today.
- [**`docs/superpowers/specs/`**](../docs/superpowers/specs/) — per-
  feature design documents. Each one captures the reasoning behind a
  specific change. Treat these as ADRs.
- [**`docs/superpowers/plans/`**](../docs/superpowers/plans/) —
  implementation plans matched to specs.

When in doubt, the Constitution wins. If you find a tension between
documents, that's a bug worth opening an issue about.

---

## Filing issues

A few notes on the issue tracker:

- **Bug reports** — please include the pgevolve version, Postgres
  major version, the schema input that triggered the bug, and the
  observed-vs-expected output. A minimal reproducer is the single
  most useful thing you can attach.
- **Feature requests** — describe the use case, not the
  implementation. "I want pgevolve to manage X object kind for
  reason Y" lets us evaluate the request against the v1.0 charter and
  roadmap; a pre-baked implementation sketch is harder to redirect.
- **Questions** — fine to file as issues. They help us see what's
  unclear in the docs.
- **Security concerns** — **do not** file in the public tracker. See
  [`.github/SECURITY.md`](./SECURITY.md) for the private disclosure
  channel.

---

## License

pgevolve is dual-licensed under **MIT OR Apache-2.0**. By submitting a
contribution, you agree it can be released under both licenses.

Dependencies must be permissively licensed (MIT, Apache-2.0, BSD,
ISC, Unicode, Zlib, MPL-2.0 where unavoidable). Copyleft licenses
(GPL, AGPL, LGPL) and proprietary licenses are not allowed and will
be rejected by `cargo deny check`. If a crate you want to add isn't
in the existing `deny.toml` allow-list, please raise it in your
issue before opening the PR.

Adding a dependency at all is a decision worth a sentence of
justification — pgevolve treats dependencies as a liability and
defaults to writing things ourselves rather than pulling in a crate.
Constitution §2 has the full reasoning.

---

## Code of conduct

We expect contributors to treat each other with respect. Disagreements
about technical direction are fine and useful; disagreements that
become personal are not.

A formal Code of Conduct will be added once the project has a
private reporting channel set up. Until then, if you have concerns
about contributor behavior, please raise them via GitHub Issues —
either in the relevant thread if it's a public matter, or by opening
a new issue if a separate report is warranted.

Thanks again for reading this. We're glad you're here.
````

- [ ] **Step 2b: Add a row for this plan to `docs/superpowers/plans/README.md`**

Open `docs/superpowers/plans/README.md` and append a new row to the chronological table — the table currently ends at `| 2026-05-28 | Docs website | ...`. Add immediately after that row:

```markdown
| 2026-05-29 | [Process docs](./2026-05-29-process-docs.md) |
```

No other content in `README.md` changes. The header text, intro paragraph, and `_skeleton/` reference stay as-is.

Sub-project E's spec specified that this index is hand-maintained chronologically; new plans append in date order.

- [ ] **Step 3: Run the verify gate**

Run, in order:
```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

Expected: all three exit 0 with no diff / no warnings. (No Rust files changed, so this is a sanity check that the working tree is clean.)

`cargo test --workspace` is not strictly required for a markdown-only commit, but if any of the above fails it indicates the working tree is dirty from something else — stop and investigate before committing.

`cargo deny check` is also not strictly required for a markdown-only commit (no dependency manifest changes), but running it is cheap and confirms `deny.toml` is still happy:
```sh
cargo deny check
```
Expected: exit 0.

- [ ] **Step 4: Confirm the staged change is exactly one new file**

Run:
```sh
git status --porcelain
```

Expected output (exact):
```
 M docs/superpowers/plans/README.md
?? .github/CONTRIBUTING.md
```

If anything else appears in the status output, stop and resolve it before committing — the plan's commit must be a single coherent unit.

- [ ] **Step 5: Stage and commit**

```sh
git add .github/CONTRIBUTING.md docs/superpowers/plans/README.md
git commit -m "$(cat <<'EOF'
docs(contributing): add CONTRIBUTING.md

Onboarding doc for external contributors. Eight sections covering
the discuss-first workflow, quick-start, the verify gate, decision-
making pointers, issue-filing guidance, licensing, and the interim
code-of-conduct posture.

CODE_OF_CONDUCT.md is intentionally deferred until the project has a
working enforcement-contact channel; the spec at
docs/superpowers/specs/2026-05-28-process-docs-design.md §3 documents
the reasoning.

Implements sub-project F of the v1.0 path (docs/v1.md).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Expected: commit succeeds.

- [ ] **Step 6: Verify the commit landed cleanly**

Run:
```sh
git show --stat HEAD
```

Expected: two files changed — `.github/CONTRIBUTING.md` (~150 insertions) and `docs/superpowers/plans/README.md` (1 insertion).

- [ ] **Step 7: (Out-of-band, for the maintainer — not a code task)**

After the commit lands, file a tracking issue so the deferred CoC
work isn't lost:

```sh
gh issue create \
  --title "Add CODE_OF_CONDUCT.md once a contact channel exists" \
  --body "$(cat <<'EOF'
Sub-project F (development process docs) originally bundled
\`.github/CODE_OF_CONDUCT.md\` alongside CONTRIBUTING.md, but the CoC
file was deferred during plan-writing on 2026-05-29 because no
working enforcement-contact channel exists yet.

See [\`docs/superpowers/specs/2026-05-28-process-docs-design.md\` §3](../blob/main/docs/superpowers/specs/2026-05-28-process-docs-design.md) for the reasoning. Constraints in summary:

- \`daniel.toone@gmail.com\` works but permanently bakes a personal
  address into git history; rotation impossible after shipping.
- \`*@users.noreply.github.com\` does not receive incoming email —
  reports would silently drop.
- A custom-domain alias (e.g., \`conduct@pgevolve.dev\`) needs a
  domain + forwarding setup that hasn't been done.

When a working channel exists, ship Contributor Covenant v2.1
verbatim at \`.github/CODE_OF_CONDUCT.md\` with the Enforcement
section's contact-info line pointing at that channel, and update
CONTRIBUTING.md §8 to link to the new file.
EOF
)"
```

This is not a code change and does not need a commit. If `gh` is not
authenticated, file the issue manually via the GitHub web UI with
equivalent text.

---

## Self-review (done by plan author, 2026-05-29)

**1. Spec coverage:**
- Spec §1 (one file under `.github/`): covered by Task 1 Step 2.
- Spec §2 (CONTRIBUTING.md 8-section outline): covered by the
  verbatim file content in Task 1 Step 2. All 8 sections present in
  the same order as the spec.
- Spec §3 (CoC deferral): covered by Task 1 Step 2's §8 in the file
  body + Task 1 Step 7 (tracking-issue creation).
- Spec §4 (out-of-scope items): nothing in the plan touches them.
- Spec §5 (one commit, one file, gates still pass): covered by Task 1
  Steps 3–6.

**2. Placeholder scan:** No TBD / TODO / "fill in" / "similar to" in
the plan. All file content is verbatim. All commands are runnable
exactly as written.

**3. Type consistency:** Markdown only — no types to track. Internal
links in the embedded file (`../docs/CONSTITUTION.md`,
`../docs/v1.md`, `./SECURITY.md`, etc.) are all paths relative to
`.github/CONTRIBUTING.md`'s location and resolve to files that exist
today (verified during plan-writing).
