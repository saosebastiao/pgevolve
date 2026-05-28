---
status: design
target: v1.0
sub_project: A (v1.0 charter)
---

# pgevolve v1.0 charter — design

This is sub-project **A** in the v1.0 path decomposition. Its sole
deliverable is a single short charter document
(`docs/v1.md`) that the rest of the v1.0 sub-projects (B object-coverage
roadmap, C testing maturation, D CI/CD maturation, E docs website, F
process docs, G community surface) reference and conform to.

The charter is **living-until-1.0**. At the 1.0 cut its sections are
either retired (the gate-tracking ones) or partially merged into
[`docs/CONSTITUTION.md`](../../CONSTITUTION.md) (the stability
commitments + cadence rules).

---

## §1. What v1.0 means

A release of pgevolve where the project commits, in writing, to:

- a defined surface that won't break in the v1.x line,
- a defined quality bar that every release meets,
- a defined Postgres-version support window that rolls with the upstream
  EOL schedule.

The 1.0 cut happens when **both** of the following are true:

1. **Feature checklist complete (§4)** — every `📋 Planned` row currently
   in [`docs/spec/roadmap.md`](../../spec/roadmap.md) plus the explicit
   v1.0-blockers listed in §4 below are shipped.
2. **Quality gates green (§3)** — the existing per-push CI gate plus a
   new nightly proptest soak have been running clean for at least
   **30 consecutive days**.

The maintainer cuts the 1.0 release manually. No formal RC cycle (per
the "ship-when-ready" cadence in §5); the 30-day clean-CI window
functions as the de-facto RC.

---

## §2. Stability commitments

### Stable in v1.x — no breaking changes without a major bump

- `pgevolve` CLI: command names, flags, exit codes, argument shapes.
- `pgevolve.toml` config schema.
- On-disk plan format: `plan.sql` directive headers, `intent.toml`
  schema, `manifest.toml` schema, the BLAKE3 plan-id derivation.
- The set of supported PG majors **must include** the Postgres
  community-supported versions at the time of each minor release
  (see §6).

### Explicitly unstable in v1.x — can break without a major bump

- The `pgevolve-core` library API. Library consumers depend on path /
  git deps or pin to exact patch versions. The crate's docs.rs page
  leads with this notice.
- Lint rule IDs and severities — new rules may land, old rules may be
  removed, severities may change up or down.
- `StepKind` names in `plan.sql` — backward-compatible additions only;
  renames bump major.
- Catalog snapshot JSON schema.
- Internal modules (anything `pub(crate)`).

### Deprecation policy for the stable surface

Any breaking change starts with a **one-minor-cycle deprecation**: the
old form keeps working with a stderr warning in version N, becomes a
hard error in N+1. CHANGELOG notes the deprecation in N and the removal
in N+1.

---

## §3. Quality gates (gate #2 for the 1.0 cut)

All five must hold on `main` for **30 consecutive days** before the 1.0
tag:

| Gate | What it checks | Where it runs |
|---|---|---|
| **Per-push CI** | `cargo fmt --check`, `clippy -D warnings`, `cargo doc -D warnings`, `cargo deny check`, lib tests, full conformance suite on PG 14/15/16/17/18 | `.github/workflows/ci.yml` on every push |
| **Property-test soak (nightly)** | The full Tier-5 property-test set at `PROPTEST_CASES=5000` per PG major | `.github/workflows/soak.yml` cron (already exists; bump cases to 5000 and add a 1.0-readiness check that flips green/red based on the most recent 30 days of soak runs) |
| **Cargo deny advisories** | Zero open advisories for ≥ 7 days before the tag | per-push + nightly |
| **GH Actions disk-space** | The PG-matrix conformance job survives without `no space left on device` (today's flake) | per-push; add runner-cleanup step OR move to larger runner |
| **Cargo doc clean** | No broken intra-doc links across the workspace | per-push (already enforced post-v0.3.8) |

**Out of scope for the 1.0 gate** (deferred to post-1.0): mutation
testing (cargo-mutants), fuzzer (cargo-fuzz), code-coverage badges,
perf benchmarks. Each gets its own follow-up sub-project if/when
desired.

### Operational definition of "30 consecutive days clean"

- The most recent 30 days of pushes to `main` had **all** required CI
  jobs end in `success` on first attempt (reruns count against the
  streak — the v0.3.8 disk-space flake would have reset it).
- Soak cron ran every night during the window with `success`.
- The streak is tracked manually pre-1.0 (a one-liner check the
  maintainer runs before tagging). Post-1.0 we may automate.

---

## §4. Feature completeness (gate #1 for the 1.0 cut)

Every `📋 Planned` row in [`docs/spec/roadmap.md`](../../spec/roadmap.md)
must be `✅ Implemented` and shipped. As of today (2026-05-28) the
charter v1.0 blockers are:

| Source | Items |
|---|---|
| v0.4.0 | EVENT TRIGGER, per-partition TABLESPACE, TABLE … USING access method |
| v0.4.1 | AGGREGATE, PG 18 virtual generated columns |
| v0.4.2 | TABLESPACE (cluster object), PL-language wiring → non-SQL FUNCTION bodies |
| v0.4.3 | TEXT SEARCH family |
| v0.5.0 | FDW family (FDW, SERVER, USER MAPPING, FOREIGN TABLE, IMPORT FOREIGN SCHEMA) |
| v0.5.1 | OPERATOR / OPERATOR CLASS / OPERATOR FAMILY |
| v0.5.2 | CAST |
| **v0.5.3 (new)** | **Recursive views (`WITH RECURSIVE`)** — promoted from "🔮 Future" to a v1.0 blocker. Requires cycle-aware dep-graph handling; sub-project B should specify the work. |

That's **12 sub-spec slots** across 8 minor releases. At the current
solo-maintainer cadence (~1 sub-spec per session), this is months of
work — but every row is independent of every other (modulo declared dep
edges in the roadmap). The order is fixed; the timing slips.

**Sub-project B (object-coverage roadmap) revisits this table** — its
job will be to confirm/refine the list, slot dep edges, and assess
whether anything else currently `🔮 Future` should also move into the
v1.0 checklist.

---

## §5. Cadence + versioning

Post-1.0, the rhythm stays the same as today: **ship-when-ready**, no
time-boxed schedule. Each release is a minor (new feature) or patch
(bugfix). pgevolve commits to `main` only — no release branches — so
every release tag points at a commit on `main` that passed the
per-push CI gate.

### Versioning rules

- **Major bump (v1 → v2)**: only when a breaking change to the stable
  surface (§2) is unavoidable. Likely never needed in 2026; pgevolve
  will plan to ship v1.x for the indefinite future.
- **Minor bump (v1.N → v1.N+1)**: new object kind, new lint rule, new
  CLI subcommand, new config key, new PG major support.
- **Patch bump (v1.N.M → v1.N.M+1)**: bug fixes only. The v0.3.8 →
  v0.3.9 emergency-fix flow this session is the patch template.

### Release ceremony

Codified in [`CLAUDE.md`](../../../CLAUDE.md) directive 11:
push `main`, sign + push tag, **wait for full 5-PG-major CI green**,
then `cargo publish` and `cargo yank` the prior version if it had a
shipped bug. This rule is binding for every release, v0.x and v1.x.

---

## §6. Postgres-version support commitment

v1.0 supports **every Postgres major version supported by upstream at
the time of the v1.0 release**. The current set is PG 14–18. The
version window rolls automatically:

- When a PG major reaches upstream EOL, the next minor release of
  pgevolve drops support and removes its conformance fixtures. Each
  removal lands in its own commit tagged `chore(eol): drop PG X`. The
  CHANGELOG explicitly calls out the drop.
- When a PG major is released (e.g., PG 19 in late 2026), it's added
  in the next minor release of pgevolve after the corresponding
  catalog-query work lands. The v0.3.6 release ("PG 18 catalog
  support") is the precedent.
- Per-version code paths are marked `// PG X+ only` at the call site
  so the EOL-drop is a mechanical grep + delete.

**No LTS branch.** Bug fixes target current `main` only. Anyone needing
a fix on an older minor cuts their own patch.

---

## §7. Explicitly post-1.0 (parking lot)

These are tracked but won't gate v1.0:

| Item | Why post-1.0 |
|---|---|
| Library API stability for `pgevolve-core` | Tied to a clear set of external consumers materializing first. |
| `pgevolve-core` re-export polish / typed errors at the public boundary | Same. |
| Partition pruning at plan time | Optimization, not correctness. |
| `SECURITY LABEL` integration | Used primarily by SE-Linux; low demand. |
| Security-barrier / leakproof per-function flag review | Lands alongside finer-grained policy review. |
| `RULE`, `BASE TYPE`, `INHERITS`, `DETACH PARTITION CONCURRENTLY` | Already ⛔ Not planned — kept here as "stays out". |
| Mutation testing, fuzzer, perf benchmarks | Quality investments deferred per §3. |
| Distribution surface beyond `cargo install` (Homebrew, Docker, GH Releases binaries) | Picked up once user-facing demand justifies. |

(Note: **recursive views moved OUT of the parking lot per the user's
decision** — see §4.)

The charter is **amendable**: items can move from post-1.0 into the
active checklist (§4) with a CHANGELOG note. Items can also move *out*
of the checklist if the maintainer judges them not v1.0-blocking, with
a CHANGELOG note.

---

## What this design produces

A single new file at `docs/v1.md`. No code changes; pure documentation.
The accompanying writing-plans output will be small (a few tasks: write
the file, add references from `README.md` + `docs/spec/roadmap.md` +
`docs/CONSTITUTION.md`, commit).

Once `docs/v1.md` lands, sub-projects B–G can each be brainstormed
independently in their own design + plan + implement cycles, conforming
to this charter.

## What's deliberately NOT in this design

- The actual deferred quality investments (mutation tests, fuzzer,
  perf benchmarks). Each gets its own brainstorm if pursued.
- The library API stabilization work for `pgevolve-core`. Same.
- Roadmap slotting for the v1.0 feature checklist beyond noting that
  recursive views land in v0.5.3. Sub-project B owns it.
- The docs website choice (mdBook vs Docusaurus vs Starlight). Sub-
  project E owns it.
- CI/CD automation (release-from-CI? signed-publish?). Sub-project D
  owns it.
