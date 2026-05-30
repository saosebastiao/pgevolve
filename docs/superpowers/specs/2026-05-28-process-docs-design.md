---
status: design
target: v1.0
sub_project: F (development process docs)
amended: 2026-05-29 — CoC deferred to a follow-up; scope reduced to a single file. See §1, §3.
---

# Development process docs — design

Sub-project **F** of the v1.0 path. One new file in `.github/` that
makes pgevolve's contribution model legible to outsiders: a
medium-depth CONTRIBUTING.md with a discuss-first tone.

A Code of Conduct was originally part of this sub-project but was
deferred during plan-writing (2026-05-29) — see §3 — because the
project does not yet have a working CoC-enforcement contact channel,
and a CoC with a broken contact is worse than no CoC.

These complement the existing process knowledge already in
[`docs/CONSTITUTION.md`](../../CONSTITUTION.md),
[`docs/v1.md`](../../v1.md),
[`docs/RELEASING.md`](../../RELEASING.md),
[`docs/superpowers/specs/`](../specs/),
and [`docs/superpowers/plans/`](../plans/) — they don't duplicate
that content, they point at it from a contributor-entry-point view.

GOVERNANCE.md is **explicitly skipped** as premature for a
solo-maintained pre-v1.0 project. Issue/PR templates are
**explicitly out of scope** here and belong to sub-project G.

---

## §1. Files added

| Path | Purpose | Source |
|---|---|---|
| `.github/CONTRIBUTING.md` | Onboarding doc for external contributors. Sets the discuss-first expectation, points at canonical docs, sketches the brainstorm → spec → plan → implement workflow. | Hand-written; ~150 lines. |

Placement in `.github/` matches the existing convention:
`.github/SECURITY.md`, `.github/CODEOWNERS` are already there. GitHub
auto-detects CONTRIBUTING.md and surfaces it when contributors open
issues/PRs.

**CODE_OF_CONDUCT.md is not in this sub-project** (see §3 / §4) — it
will land in a follow-up after a working enforcement contact channel
is set up. Until then, CONTRIBUTING.md's §8 documents the interim
expectation honestly and points reports at GitHub Issues.

## §2. `.github/CONTRIBUTING.md` outline

Sections:

1. **Welcome** — short, sets discuss-first tone.
2. **Before you start** — discuss-first ask, link to issue tracker,
   reference the brainstorm → spec → plan workflow with concrete
   pointers into `docs/superpowers/specs/` for worked examples.
3. **Quick start** — clone, build, test commands. References Rust
   1.95+ requirement and Docker dependency for conformance suite.
4. **The verify gate** — the same checklist that the release runbook
   uses (fmt + clippy + test + cargo doc -D warnings + cargo deny).
5. **How decisions get made** — links to:
   - `docs/CONSTITUTION.md` (binding principles)
   - `docs/v1.md` (v1.0 commitments)
   - `docs/superpowers/specs/` (per-feature ADRs)
   - `docs/superpowers/plans/` (implementation plans)
6. **Filing issues** — bug, feature, question; security goes to
   SECURITY.md.
7. **License** — dual MIT/Apache; deps must be permissive (no
   copyleft, per Constitution).
8. **Code of conduct** — link to CODE_OF_CONDUCT.md.

Full file content lives in the implementation plan; not duplicated
in the spec to avoid drift between the two.

Voice / tone:
- First person plural ("we") for the project's collective workflow.
- Second person ("you") when addressing the contributor.
- Imperative but friendly: "please open an issue first", not "you
  must open an issue first".

## §3. CODE_OF_CONDUCT.md — deferred (was originally in this sub-project)

**Status: deferred to a follow-up sub-project, not in F's deliverable.**

Original intent: ship Contributor Covenant v2.1 verbatim with the
Enforcement section's contact-info line filled in.

What changed (2026-05-29, during plan-writing): the contact-info
question surfaced a real constraint — the address shipped in a CoC's
Enforcement section must actually receive email. Available options
were judged worse than deferral:

- `daniel.toone@gmail.com` — works, but permanently bakes a personal
  address into git history; rotation is impossible once shipped.
- `*@users.noreply.github.com` — *does not receive incoming email*;
  using one would silently drop reports.
- Custom domain alias (e.g., `conduct@pgevolve.dev`) — requires
  domain registration + forwarding setup the maintainer hasn't done.

Decision: defer the CoC file entirely until a real receiving channel
exists. CONTRIBUTING.md's §8 covers the interim posture honestly:
"a formal CoC will land once the project has a private reporting
channel; until then, concerns can be raised via GitHub Issues."

Tracking: a follow-up issue is filed once CONTRIBUTING.md ships
(manual step in the implementation plan's post-task notes).

Why still Contributor Covenant when we do ship it: industry-standard
for OSS projects, recognized by GitHub Community Standards, low
controversy. That choice survives the deferral.

## §4. Out of scope (deferred)

- **GOVERNANCE.md** — premature for solo-maintained project. Add
  when a second maintainer or formal foundation appears.
- **ADR doc format change** — `docs/superpowers/specs/` already
  serves the ADR role; CONTRIBUTING.md documents that. Don't
  duplicate as a separate `docs/adr/` tree.
- **Issue templates, PR template, triage SLOs** — sub-project G
  (community surface).
- **DCO (Developer Certificate of Origin) sign-off** — would add a
  CI step + commit-message convention. For solo-with-occasional-
  contributors, the license note in CONTRIBUTING.md is sufficient.
  Revisit if/when CLA-grade compliance becomes a requirement.
- **Maintainer email change** — out of scope here; if the maintainer
  wants a noreply contact address before the CoC ships, swap inline
  when filling the file. Otherwise default to the existing public
  email.

- **Cross-references from existing docs** — no separate commit
  backlinks CONSTITUTION/RELEASING/etc. to the new files. Folded
  in incidentally on next edit of each.
- **docs-site nav surfacing** — the mdBook nav
  (`docs/SUMMARY.md`, from sub-project E) does NOT currently surface
  `.github/CONTRIBUTING.md` / `.github/CODE_OF_CONDUCT.md`. Those
  are GitHub-surface files. If desired later, it's a one-line
  SUMMARY.md tweak; not F's deliverable.

## §5. What this design produces

One commit, **one file** added under `.github/`
(`.github/CONTRIBUTING.md`). Pure markdown. No verify-gate Rust
implications; the standard `cargo fmt --check`, `clippy`, `cargo doc`
gates still pass trivially.

Post-task manual step (out of commit): file a GitHub issue titled
"Add CODE_OF_CONDUCT.md once contact channel exists" linking back to
§3 of this spec.
