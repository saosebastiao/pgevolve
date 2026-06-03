# Community Surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the community-facing files under `.github/` (3 issue templates + a config.yml + a PR template) and lightly polish `SECURITY.md` and `CONTRIBUTING.md` to close out the v1.0 path's sub-project G.

**Architecture:** Pure markdown + a small GitHub-specific YAML config. No build or test changes. GitHub auto-detects all the added files (renders templates in the issue/PR UX, surfaces the security contact, hides blank-issue creation). One commit, 5 new files added, 2 existing files edited.

**Tech Stack:** Markdown only.

**Source spec:** [`docs/superpowers/specs/2026-06-02-community-surface-design.md`](../specs/2026-06-02-community-surface-design.md).

**Standing project rules** (from `CLAUDE.md`):
- Workspace lints are strict (`clippy::pedantic`, `clippy::nursery`, `-D warnings`). Pure docs changes don't touch Rust, but `cargo fmt --check` / `cargo doc -D warnings` still run as a sanity gate.
- Commits go directly to `main`. Each commit is a coherent unit.
- Every commit ends with the Co-Authored-By trailer.
- **CLAUDE.md §11: never `cargo publish` until CI is green.** Not applicable to this plan (pure docs, no release), but always in force.

---

## File structure

**Created (5 files):**
- `.github/ISSUE_TEMPLATE/bug.md` — bug report template (~40 lines).
- `.github/ISSUE_TEMPLATE/feature.md` — feature request template (~25 lines).
- `.github/ISSUE_TEMPLATE/question.md` — question template (~20 lines).
- `.github/ISSUE_TEMPLATE/config.yml` — disables blank issues, surfaces security channel (~10 lines).
- `.github/PULL_REQUEST_TEMPLATE.md` — PR template with verify-gate checklist (~30 lines).

**Modified (2 files):**
- `.github/SECURITY.md` — refresh the Supported Versions table at the bottom (~5 line diff).
- `.github/CONTRIBUTING.md` — insert a new section "What to expect after you file" between §6 (Filing issues) and §7 (License). Adds ~15 lines.

**Modified (1 file, plans index):**
- `docs/superpowers/plans/README.md` — append a row for this plan (per the sub-project E convention).

**Deleted:** none.

`.github/CONTRIBUTING.md`, `.github/CODEOWNERS`, `.github/SECURITY.md`, and `.github/workflows/` already exist; the new files slot in alongside without changing the directory's existing structure.

---

## Task 1: Add the community surface files

**Files:**
- Create: `.github/ISSUE_TEMPLATE/bug.md`
- Create: `.github/ISSUE_TEMPLATE/feature.md`
- Create: `.github/ISSUE_TEMPLATE/question.md`
- Create: `.github/ISSUE_TEMPLATE/config.yml`
- Create: `.github/PULL_REQUEST_TEMPLATE.md`
- Modify: `.github/SECURITY.md` (Supported Versions table only)
- Modify: `.github/CONTRIBUTING.md` (insert new section)
- Modify: `docs/superpowers/plans/README.md` (append index row)

- [ ] **Step 1: Confirm none of the new files already exist**

Run:
```sh
ls .github/ISSUE_TEMPLATE 2>&1; ls .github/PULL_REQUEST_TEMPLATE.md 2>&1
```

Expected: both produce `No such file or directory`. If any of the new paths already exist, STOP — the plan assumes greenfield creation and would silently overwrite.

`.github/CONTRIBUTING.md`, `.github/SECURITY.md`, and `.github/CODEOWNERS` should already exist; those aren't being created, they're being edited (or left alone in CODEOWNERS' case).

- [ ] **Step 2: Create `.github/ISSUE_TEMPLATE/` directory**

The directory doesn't exist yet (Step 1 confirmed). Create it as a side-effect of writing the first file in it; you do not need a separate `mkdir` step — `git add` on a path with parent dirs that don't exist will fail, but the Write tool / your editor / `mkdir -p` patterns handle the creation transparently. If your tooling requires the parent dir up-front:

```sh
mkdir -p .github/ISSUE_TEMPLATE
```

- [ ] **Step 3: Create `.github/ISSUE_TEMPLATE/bug.md`** with this exact content

```markdown
---
name: Bug report
about: Something broke; help us reproduce it
title: ""
labels: bug
assignees: ""
---

## What happened

<!-- Short description of the bug. -->

## Steps to reproduce

1.
2.
3.

A minimal schema input that triggers the bug is the single most useful
thing you can attach.

## Expected vs actual

**Expected:**

**Actual:**

## Environment

- pgevolve version (e.g., 0.3.9):
- Postgres major version (14 / 15 / 16 / 17 / 18):
- Operating system + version:
- Install method (cargo install / brew / source build / other):

## Additional context

<!-- Logs, error output, related PRs/issues, anything else. -->
```

- [ ] **Step 4: Create `.github/ISSUE_TEMPLATE/feature.md`** with this exact content

```markdown
---
name: Feature request
about: Suggest a feature or enhancement
title: ""
labels: enhancement
assignees: ""
---

## Use case

<!-- What are you trying to do? -->

## Current workaround

<!-- What do you do today, if anything? Leave blank if no workaround
exists. -->

## Proposed change

<!-- High-level shape of the change. Describe the use case, not the
implementation — a pre-baked implementation sketch is harder to
redirect. -->

## Alternatives considered

<!-- Other approaches you thought about and why you didn't pick them. -->
```

- [ ] **Step 5: Create `.github/ISSUE_TEMPLATE/question.md`** with this exact content

```markdown
---
name: Question
about: Ask about pgevolve usage, behavior, or design
title: ""
labels: question
assignees: ""
---

> Before filing, check [`docs/CONSTITUTION.md`](../../docs/CONSTITUTION.md),
> [`docs/v1.md`](../../docs/v1.md), and
> [`docs/superpowers/specs/`](../../docs/superpowers/specs/) — questions
> already answered there may close fast with a pointer.

## What you're trying to do

<!-- Context: the goal, not just the question. -->

## What you've already tried or read

<!-- Pointers to docs you've already checked save round-trips. -->
```

- [ ] **Step 6: Create `.github/ISSUE_TEMPLATE/config.yml`** with this exact content

```yaml
blank_issues_enabled: false
contact_links:
  - name: Security vulnerability
    url: https://github.com/saosebastiao/pgevolve/security/advisories/new
    about: |
      Do not file public issues for security vulnerabilities. Use
      GitHub's private security advisory form. See SECURITY.md for the
      full disclosure policy.
```

- [ ] **Step 7: Create `.github/PULL_REQUEST_TEMPLATE.md`** with this exact content

```markdown
## Summary

<!-- 1-3 sentences: what changes, and why. -->

## Related issue

<!-- Most PRs should link an issue. Use `Closes #N` or `Fixes #N` so
the issue auto-closes on merge. -->

Closes #

## Verify gate

Run locally before pushing:

- [ ] `cargo fmt --all -- --check` is clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean
- [ ] `cargo test --workspace --all-targets` passes
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` is clean
- [ ] `cargo deny check` passes
- [ ] `CHANGELOG.md` `[Unreleased]` section updated for any user-visible change

## Notes for the reviewer

<!-- Optional: things worth calling out (deliberate trade-offs, areas
that need extra attention, follow-ups deferred to a separate PR). -->
```

- [ ] **Step 8: Edit `.github/SECURITY.md`** to refresh the Supported Versions table

The file currently has this block (look for the `## Supported versions` heading):

```markdown
## Supported versions

| Version | Supported          |
|---------|--------------------|
| 0.2.x   | ✅                 |
| 0.1.x   | ❌ (please upgrade)|
```

Replace **only** the table (keep the heading) with:

```markdown
| Version | Supported                              |
|---------|----------------------------------------|
| 0.3.x   | ✅ Active                              |
| 0.2.x   | ⚠️  Security fixes only (until v1.0)   |
| 0.1.x   | ❌ Unsupported (please upgrade)        |
```

Do not touch any other part of `SECURITY.md` — the disclosure flow, SLAs, scope, and Constitution cross-reference all stay as-is.

- [ ] **Step 9: Edit `.github/CONTRIBUTING.md`** to insert the "What to expect after you file" section

Find the end of the "Filing issues" section. The exact text to look for, in order:

```
- **Security concerns** — **do not** file in the public tracker. See
  [`.github/SECURITY.md`](./SECURITY.md) for the private disclosure
  channel.

---

## License
```

Replace that block with this exact text (a new section + horizontal rule is inserted between the closing `---` of "Filing issues" and the `## License` heading):

```
- **Security concerns** — **do not** file in the public tracker. See
  [`.github/SECURITY.md`](./SECURITY.md) for the private disclosure
  channel.

---

## What to expect after you file

pgevolve is solo-maintained today. Issues typically get a first
response within a week or so, but that's an expectation, not an
SLA — vacation, day-job demands, and the occasional incident arc
can stretch it. We don't auto-close stale issues; if something goes
quiet you didn't do anything wrong.

Security reports get the SLA promised in
[`SECURITY.md`](./SECURITY.md): 7-day acknowledgment, 30-day fix
for medium-severity, faster for critical.

---

## License
```

Do not touch any other part of CONTRIBUTING.md. The new section sits between §6 (Filing issues) and §7 (License), making it the new §7 and renumbering nothing because the sections aren't numbered in the file body.

- [ ] **Step 10: Append a row to `docs/superpowers/plans/README.md`**

The chronological table currently ends at `| 2026-05-29 | [Process docs](./2026-05-29-process-docs.md) |`. Add immediately after that row:

```markdown
| 2026-06-02 | [Community surface](./2026-06-02-community-surface.md) |
```

No other content in `README.md` changes.

- [ ] **Step 11: Run the verify gate**

Run, in order:
```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

Expected: all three exit 0. (No Rust files changed; this is a sanity check that the working tree is clean.)

`cargo test --workspace` and `cargo deny check` are not strictly required for a markdown-only commit, but if any of the above fails it indicates the working tree is dirty from something else — stop and investigate.

- [ ] **Step 12: Confirm the staged change is exactly the 5 new files + 3 edits**

Run:
```sh
git status --porcelain
```

Expected output (exact, in this order):
```
 M .github/CONTRIBUTING.md
 M .github/SECURITY.md
 M docs/superpowers/plans/README.md
?? .github/ISSUE_TEMPLATE/
?? .github/PULL_REQUEST_TEMPLATE.md
```

(`?? .github/ISSUE_TEMPLATE/` covers all 4 new files in that untracked directory.)

If anything else appears, stop and resolve it before committing — the plan's commit must be a single coherent unit.

- [ ] **Step 13: Stage and commit**

```sh
git add .github/ISSUE_TEMPLATE/ .github/PULL_REQUEST_TEMPLATE.md .github/SECURITY.md .github/CONTRIBUTING.md docs/superpowers/plans/README.md
git commit -m "$(cat <<'EOF'
docs(community): add issue/PR templates + refresh SECURITY.md table

Implements sub-project G of the v1.0 path. Five new files under
.github/ (bug, feature, question issue templates + config.yml + PR
template), supported-versions table refresh in SECURITY.md, and a
short "what to expect after you file" section in CONTRIBUTING.md.

Tone matches the rest of the v1.0 docs: solo-maintained honesty, no
over-promised SLAs.

Closes the v1.0 path scaffolding — sub-projects A through G all
complete. Remaining v1.0 work is feature-coverage and the 30-day
soak streak (per docs/v1.md §3, §4).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Expected: commit succeeds.

- [ ] **Step 14: Verify the commit landed cleanly**

Run:
```sh
git show --stat HEAD
```

Expected: 8 files changed:
- `.github/CONTRIBUTING.md` — ~15 insertions
- `.github/SECURITY.md` — ~5 line diff (mostly the table replacement)
- `.github/ISSUE_TEMPLATE/bug.md` — ~40 lines added
- `.github/ISSUE_TEMPLATE/config.yml` — ~10 lines added
- `.github/ISSUE_TEMPLATE/feature.md` — ~25 lines added
- `.github/ISSUE_TEMPLATE/question.md` — ~20 lines added
- `.github/PULL_REQUEST_TEMPLATE.md` — ~30 lines added
- `docs/superpowers/plans/README.md` — 1 line added

If the file count or insertion count is materially off (e.g., 7 files instead of 8, or no insertions in CONTRIBUTING.md), stop and inspect.

---

## Self-review (done by plan author, 2026-06-02)

**1. Spec coverage:**
- Spec §1 (file table — 5 added, 2 edited): covered by Task 1 Steps 3-9. Plus the plans-index update from sub-project E's convention (added in Step 10).
- Spec §2 (issue template structure: bug, feature, question, config.yml): covered by Task 1 Steps 3-6 with verbatim content.
- Spec §3 (PR template: minimal, verify-gate checklist, no breaking-changes flag, no screenshots): covered by Task 1 Step 7 with verbatim content.
- Spec §4 (SECURITY.md polish: Supported Versions table refresh only): covered by Task 1 Step 8.
- Spec §5 (CONTRIBUTING.md "what to expect after you file" between §6 and §7): covered by Task 1 Step 9 with verbatim placement.
- Spec §6 (locked decisions): no separate task needed; decisions are reflected in the content of the embedded files.
- Spec §7 (out of scope): nothing in the plan touches deferred items (no Discussions config, no TRIAGE.md, no labels, no Dependabot config, no CoC, no FUNDING.yml). Verified.
- Spec §8 (closes v1.0 path): the commit message in Step 13 calls this out; no additional task needed.

**2. Placeholder scan:** No TBD / TODO / "fill in" / "similar to" in the plan. All file content is verbatim. All commands are runnable as written. The HTML comments inside the templates (`<!-- ... -->`) are intentional prompts shown to issue/PR authors in the GitHub textarea — they're not plan placeholders.

**3. Type consistency:** Markdown + YAML only — no types to track. Internal links verified:
- `.github/ISSUE_TEMPLATE/question.md` → `../../docs/CONSTITUTION.md`, `../../docs/v1.md`, `../../docs/superpowers/specs/` — all resolve from `.github/ISSUE_TEMPLATE/` going up two levels to repo root. All target paths exist today.
- `.github/CONTRIBUTING.md` "what to expect" section → `./SECURITY.md` — resolves within `.github/`. Exists.
- `.github/ISSUE_TEMPLATE/config.yml` → security advisory URL — same one already used in `SECURITY.md`. Verified.
- `.github/PULL_REQUEST_TEMPLATE.md` verify-gate commands — same list as `CONTRIBUTING.md` §4 (verified during plan-writing).
