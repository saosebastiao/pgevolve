---
status: design
target: v1.0
sub_project: G (community surface)
---

# Community surface — design

Sub-project **G** of the v1.0 path. The last of the seven. Five new
files plus two edited ones, all under `.github/`, that close the
project's external contributor surface: issue templates, a PR
template, a small `SECURITY.md` refresh, and an "after filing, what to
expect" paragraph in `CONTRIBUTING.md`.

Tone matches the rest of the v1.0 docs: solo-maintained pre-v1.0
honesty, no over-promised SLAs, no automation theater. Heavy on
information density, light on ceremony.

---

## §1. What this design produces

One commit, five files added under `.github/`, two existing files
edited. Pure markdown + a small YAML config. No verify-gate Rust
implications.

| Path | Action | Approx size |
|---|---|---|
| `.github/ISSUE_TEMPLATE/bug.md` | add | ~40 lines |
| `.github/ISSUE_TEMPLATE/feature.md` | add | ~25 lines |
| `.github/ISSUE_TEMPLATE/question.md` | add | ~15 lines |
| `.github/ISSUE_TEMPLATE/config.yml` | add | ~10 lines |
| `.github/PULL_REQUEST_TEMPLATE.md` | add | ~30 lines |
| `.github/SECURITY.md` | edit | ~5 line diff |
| `.github/CONTRIBUTING.md` | edit | ~10 line addition |

After this lands, the v1.0 path's seven sub-projects are all
complete; remaining v1.0-blocker work is feature-coverage and the
30-day soak streak.

---

## §2. Issue templates

Classic-markdown format (`.md` with frontmatter), not YAML forms. The
trade-off was discussed at brainstorming time; markdown's faster to
maintain and adequately structured for current contributor volume.
YAML forms can be a follow-up if external PR volume grows.

### `.github/ISSUE_TEMPLATE/bug.md`

Frontmatter:

```yaml
---
name: Bug report
about: Something broke; help us reproduce it
title: ""
labels: bug
assignees: ""
---
```

Body sections (each appears as an `## H2` in the prefilled textarea):

1. **What happened** — short description.
2. **Steps to reproduce** — numbered list, ideally a minimal schema
   snippet that triggers the bug.
3. **Expected vs actual** — what you thought would happen vs what
   did.
4. **Environment** — bullet list prompting `pgevolve` version,
   Postgres major version, OS, install method (cargo / brew / source).
5. **Additional context** — logs, screenshots, related PRs, anything
   else.

Tone: imperative + friendly, matching `CONTRIBUTING.md`. Reproducer
is the most-emphasized field (per CONTRIBUTING §6: "A minimal
reproducer is the single most useful thing you can attach").

### `.github/ISSUE_TEMPLATE/feature.md`

Frontmatter:

```yaml
---
name: Feature request
about: Suggest a feature or enhancement
title: ""
labels: enhancement
assignees: ""
---
```

Body sections:

1. **Use case** — what you're trying to do.
2. **Current workaround** — what you do today (if anything).
3. **Proposed change** — high-level shape; not a full design (per
   CONTRIBUTING §6: "describe the use case, not the implementation").
4. **Alternatives considered** — other approaches you thought about.

### `.github/ISSUE_TEMPLATE/question.md`

Frontmatter:

```yaml
---
name: Question
about: Ask about pgevolve usage, behavior, or design
title: ""
labels: question
assignees: ""
---
```

Body sections (kept minimal):

1. **What you're trying to do** — context.
2. **What you've already tried / read** — pointers to docs you've
   already checked help avoid "did you read this" answers.

Plus a one-line note at the top of the prefilled body:

> Before filing: check [`docs/CONSTITUTION.md`](../../docs/CONSTITUTION.md),
> [`docs/v1.md`](../../docs/v1.md), and
> [`docs/superpowers/specs/`](../../docs/superpowers/specs/). Questions
> already answered there may close fast with a pointer.

### `.github/ISSUE_TEMPLATE/config.yml`

```yaml
blank_issues_enabled: false
contact_links:
  - name: Security vulnerability
    url: https://github.com/saosebastiao/pgevolve/security/advisories/new
    about: Do not file public issues for security vulnerabilities; use GitHub's private advisory form (see SECURITY.md).
```

Disabling blank issues forces choice of a template, which improves
triage signal. The security contact link surfaces the disclosure
channel at issue-filing time, not just in `SECURITY.md`.

**Why no GitHub Discussions link:** discussion volume is zero today;
adding a venue we don't actively staff is worse than not adding it.
Revisit if/when an actual question backlog appears.

---

## §3. Pull request template

### `.github/PULL_REQUEST_TEMPLATE.md`

Minimal template, ~30 lines. Sections:

1. **Summary** — what changes and why, 1-3 sentences.
2. **Related issue** — `Closes #N` / `Fixes #N` prompt. Most PRs
   should be tied to an existing issue; this nudges that pattern.
3. **Verify gate** — checklist:
   - [ ] `cargo fmt --all -- --check` is clean
   - [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean
   - [ ] `cargo test --workspace --all-targets` passes
   - [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` is clean
   - [ ] `cargo deny check` passes
   - [ ] CHANGELOG `[Unreleased]` section updated for any user-visible change
4. **Notes for the reviewer** — optional free-form, e.g., "deliberately
   leaving X for a follow-up", "this touches the planner; pay attention
   to ordering".

No "breaking changes" flag — the v1.0 charter's stability rules in
§2 already define what counts as breaking; PR authors don't need a
checkbox to re-litigate that.

No "screenshots" section — pgevolve is a CLI with no GUI; not
relevant.

---

## §4. SECURITY.md polish

Single change: the Supported Versions table at the bottom is stale.
Current state:

```markdown
| Version | Supported          |
|---------|--------------------|
| 0.2.x   | ✅                 |
| 0.1.x   | ❌ (please upgrade)|
```

New state:

```markdown
| Version | Supported                              |
|---------|----------------------------------------|
| 0.3.x   | ✅ Active                              |
| 0.2.x   | ⚠️  Security fixes only (until v1.0)   |
| 0.1.x   | ❌ Unsupported (please upgrade)        |
```

Rationale:

- 0.3.x is the current active series (latest is 0.3.9 per CHANGELOG).
- 0.2.x is recent enough that security-fix backports are reasonable
  until v1.0 stabilizes the surface; after v1.0 cuts, 0.2.x can drop
  to ❌.
- 0.1.x has been ❌ since the last revision; no change.

No other SECURITY.md changes. The 7-day-ack / 30-day-mitigation SLAs,
the scope sections, the no-blame disclosure posture, and the
constitution cross-ref all stay as-is — they're already mature.

---

## §5. CONTRIBUTING.md: "what to expect after filing"

Insert a new section between `§6. Filing issues` and `§7. License`
in `.github/CONTRIBUTING.md`. Title: **"What to expect after you
file"**. Body (~3-4 sentences, kept honest):

> pgevolve is solo-maintained today. Issues typically get a first
> response within a week or so, but that's an expectation, not an
> SLA — vacation, day-job demands, and the occasional incident
> arc can stretch it. We don't auto-close stale issues; if something
> goes quiet you didn't do anything wrong.
>
> Security reports get the SLA promised in
> [`SECURITY.md`](./SECURITY.md): 7-day acknowledgment, 30-day
> fix for medium-severity, faster for critical.

The "what to expect" framing avoids promising specific times for
non-security issues while still setting reasonable expectations. The
contrast with the SECURITY.md SLA makes the asymmetry intentional.

No labels-explained section, no triage workflow document, no
issue-priority taxonomy. The project is small enough that those
add ceremony without adding clarity. Revisit if/when scale demands.

---

## §6. Decisions made + rationale (locked at brainstorming)

| Decision | Choice | Why |
|---|---|---|
| Triage SLO | informal "best-effort" | Solo-maintained pre-v1.0; over-promising creates worse contributor experience than under-promising. |
| Template format | classic markdown | Lower maintenance cost, no YAML form schema to learn, adequate at current contributor volume. |
| Issue template kinds | bug + feature + question + `config.yml` | Three distinct triage modes; `config.yml` disables blank issues + surfaces security channel. |
| PR template depth | minimal | Verify-gate checklist + summary + linked issue covers what reviewers (and future-maintainers reading PR history) actually need. |
| SECURITY.md polish | supported-versions table only | The rest is already mature; don't churn what isn't broken. |
| Triage doc location | inline in `CONTRIBUTING.md` | Avoids a new top-level doc; readers already there. |

---

## §7. Out of scope (explicitly deferred)

- **GitHub Discussions** — no discussion volume justifies a second
  venue today. Easy to enable later if a question backlog appears.
- **A standalone `TRIAGE.md`** — the CONTRIBUTING.md paragraph covers
  the same info without a new doc.
- **Label taxonomy + triage workflow automation** — Constitution
  principle of "small project, keep it small" applies. Manual labels
  stay.
- **`CODEOWNERS` changes** — file already exists at
  `.github/CODEOWNERS` and is fine. Untouched.
- **Code of Conduct** — deferred via sub-project F's spec §3 + the
  follow-up GitHub issue documenting it. Out of G.
- **Bot configuration** — Dependabot / Renovate / stale-bot are all
  potentially useful but each is its own decision and its own
  potential noise source. Follow-up if/when needed.
- **GitHub-Pages-deploy-from-fork friction** — irrelevant; the docs
  workflow runs from main only (sub-project E).
- **Sponsorship / FUNDING.yml** — out; no funding setup in place.

---

## §8. Closes the v1.0 path

After this lands, the seven v1.0 path sub-projects are all complete:

- A — v1.0 charter (`docs/v1.md`)
- B — roadmap revision (`docs/spec/roadmap.md`)
- C — testing + infra maturation (soak workflow, xtask soak-streak, free-disk-space)
- D — CI/CD maturation (branch-protection script, `scripts/release.sh`, RELEASING.md, README badges)
- E — docs website (mdBook at https://saosebastiao.github.io/pgevolve/)
- F — process docs (`.github/CONTRIBUTING.md`; CoC deferred)
- G — community surface (this sub-project)

What's left between here and v1.0:

1. **Feature checklist** (per `docs/v1.md` §4) — every `📋 Planned` row
   on `docs/spec/roadmap.md` plus any explicit v1.0-blockers.
2. **30-day soak streak** (per `docs/v1.md` §3) — the nightly soak
   needs to run clean for 30 consecutive days. Round-3 soak fixes have
   cleared the high-frequency failure modes; remaining open soak
   issues (#14, #17, #19, #20, plus any future rounds) need to fix
   before the streak counter can start ticking.

Both are tracked in their respective canonical documents; not G's job
to enumerate them again.
