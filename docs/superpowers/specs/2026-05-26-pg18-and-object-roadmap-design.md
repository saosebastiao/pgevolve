---
date: 2026-05-26
status: drafting
sub_specs:
  - postgres-18-support
  - object-kind-roadmap
---

# PG 18 support and remaining-object-types roadmap

Two related additions to the pgevolve spec surface:

1. Add **Postgres 18** to the actively-supported version matrix.
2. Publish a **roadmap** that orders every remaining 🔮 Future / 📋 Planned
   object kind into target releases, backed by skeleton plan stubs so each
   future brainstorm starts from a known shape.

These are spec/roadmap changes plus one implementation plan for the PG 18
catalog read path. No IR shapes are added here.

## Motivation

- **PG 18** went GA in September 2025; the constitution commits us to supporting
  every actively-maintained Postgres version, and the conformance matrix
  currently stops at PG 17. Adding PG 18 to the matrix removes a standing
  drift between policy and reality.
- **The remaining 🔮 Future rows in `docs/spec/objects.md`** have no ordering,
  no version targets, and no shared structure for the eventual plan files. A
  contributor (human or Claude) picking up the next sub-spec has to derive
  the dependency order from scratch and invent a plan layout each time. A
  single roadmap doc + uniform skeleton template eliminates that overhead.

Both changes are pure additions: no existing spec entries are removed or
re-scoped.

---

## Section 1 — Postgres 18 support

### Doc changes (mechanical)

| File | Change |
|---|---|
| `docs/CONSTITUTION.md` §6 | Active list becomes `14, 15, 16, 17, 18`. PG 14 EOL note (November 2026) is preserved verbatim. |
| `docs/user/installation.md` | "Postgres 14–17" → "Postgres 14–18". |
| `docs/user/configuration.md` | `min_pg_version` default stays at `14`; max documented value is now `18`. |
| `docs/spec/README.md` | Naming-conventions paragraph gains a one-line "v0.3.6 = PG 18 support" entry. |
| `docs/spec/objects.md` | New short "PG 18-only features" callout listing virtual generated columns and `NOT NULL NOT VALID` as 🔮 Future. These are *not* part of the v0.3.6 work — they get their own skeleton plans in the roadmap. |

### Phase plan

A new plan at `docs/superpowers/plans/2026-05-26-postgres-18-support.md`
covers the **catalog-read + conformance** scope only. It does *not* add
new IR features.

Scope:

- Add `PgVersion::Pg18` variant in
  `crates/pgevolve-core/src/catalog/version.rs`, with detection mapping
  `180_000 → Pg18`.
- Add `crates/pgevolve-core/src/catalog/queries/pg18.rs`. Initial content
  re-exports from `shared` (no known divergences against v0.3 IR — confirm
  by re-running each `CatalogQuery::*` under PG 18 in tier-2 round-trip
  tests).
- Extend the `match (version, query)` arms in
  `crates/pgevolve-core/src/catalog/queries/mod.rs` to dispatch `Pg18`.
- Add PG 18 to the conformance matrix:
  - `pgevolve-testkit::ephemeral_pg::default_pg_version` gains a `Pg18`
    case.
  - CI `pg-matrix` job (see `docs/superpowers/plans/phase-11-testkit.md`)
    runs property + Tier-3 + Tier-4 against `pg:18` in addition to 14–17.
- Bump `[managed].min_pg_version` validation upper bound to 18 in the CLI
  config layer.

Out of scope (deferred to their own skeleton plans, see Section 2):

- Virtual generated columns (`GENERATED ALWAYS AS (...) VIRTUAL`).
- `NOT NULL NOT VALID` constraint variant.
- Any other PG 18 SQL-surface additions surfaced during the catalog
  re-run.

Target release: **v0.3.6**, immediately after the in-flight subscriptions
work ships.

---

## Section 2 — Remaining-object-types roadmap

### New doc: `docs/spec/roadmap.md`

Single canonical roadmap table ordering every remaining 🔮 Future and
📋 Planned row from `docs/spec/objects.md` into target versions. Format:

```
| Target | Object / sub-feature | Skeleton plan | Notes |
|---|---|---|---|
| v0.3.6 | PG 18 support | plans/2026-05-26-postgres-18-support.md | … |
| v0.3.7 | STATISTICS | plans/_skeleton/statistics.md | promoted from 📋 v0.3 |
| …
```

Each row links forward to a skeleton plan stub (Section 3) and backward
to the corresponding `objects.md` row. `objects.md` status markers are
updated from 🔮 to 📋 with the version target where the roadmap commits
to one.

### Version map

| Version | Objects |
|---|---|
| v0.3.6 | PG 18 support |
| v0.3.7 | `STATISTICS`; `VIEW ... WITH CHECK OPTION` |
| v0.3.8 | `CREATE COLLATION`; `RANGE TYPE` |
| v0.4.0 | `EVENT TRIGGER`; per-partition `TABLESPACE`; `TABLE … USING <access method>` |
| v0.4.1 | `AGGREGATE` (SQL / plpgsql state functions only); PG 18 virtual generated columns |
| v0.4.2 | `TABLESPACE` (cluster object); PL-language wiring → non-SQL/plpgsql `FUNCTION` bodies |
| v0.4.3 | `TEXT SEARCH` family (configuration, dictionary, parser, template) |
| v0.5.0 | FDW family (`FOREIGN DATA WRAPPER`, `SERVER`, `USER MAPPING`, `FOREIGN TABLE`, `IMPORT FOREIGN SCHEMA`) |
| v0.5.1 | `OPERATOR` / `OPERATOR CLASS` / `OPERATOR FAMILY` |
| v0.5.2 | `CAST` |
| Future | Recursive views; partition pruning at plan time |
| ⛔ | `RULE`; `SECURITY LABEL`; `BASE TYPE`; `INHERITS`; `DETACH PARTITION CONCURRENTLY`; cluster-wide settings; backups; data |

### Ordering rationale

Two principles, applied in order:

1. **Postgres dependency order.** An object can't ship before its
   prerequisites. Examples:
   - `CREATE COLLATION` must precede `TEXT SEARCH` (text-search
     configurations carry collation references).
   - PL-language wiring must precede non-SQL `FUNCTION` bodies.
   - FDW `SERVER` / `USER MAPPING` must precede `FOREIGN TABLE`.
2. **User impact / demand.** Within a dep-respecting slot, lands the
   objects that unblock the most real applications first. `STATISTICS`,
   `EVENT TRIGGER`, `RANGE TYPE`, `VIEW ... WITH CHECK OPTION`, and
   `CREATE COLLATION` rank high here. `OPERATOR FAMILY` and `CAST` rank
   low.

### Notable scope notes

- **`AGGREGATE` in v0.4.1** is intentionally constrained: only aggregates
  whose state function is SQL or plpgsql can be managed. Aggregates whose
  state function is in a PL language pgevolve does not yet read are
  rejected at IR-build time with a structured error. PL-language support
  arrives in v0.4.2, at which point the constraint is relaxed.
- **PG 18 virtual generated columns** in v0.4.1 is the IR follow-up to
  the v0.3.6 catalog-read work. Stored generated columns are already
  supported; virtual columns add a new `GeneratedKind` variant.
- **`TABLESPACE` (cluster object) in v0.4.2** manages tablespace
  *creation* via the `pgevolve cluster …` surface, paired with the
  per-partition tablespace work landed in v0.4.0. It does not manage
  filesystem layout. **This reverses the current `objects.md` stance**
  ("cluster-level admin objects outside the schema-management remit").
  Rationale: the `pgevolve cluster …` surface already manages roles —
  another cluster-level admin object — so the precedent is set.
  Filesystem layout (directory creation, mount points) stays out of
  scope; only the SQL `CREATE TABLESPACE` step is managed.
- **FDW family in v0.5.0** owns an entire minor because it introduces
  secrets handling for connection strings (mirroring the work already
  done for subscriptions in v0.3.5) plus four interlocking object kinds.

### `objects.md` updates

After this design lands, every row in `objects.md` that the roadmap
commits to a version is updated to:

- Status changes from 🔮 Future to 📋 Planned.
- Notes column gains "Target: v0.x.y. See `roadmap.md`".

Rows that remain 🔮 (recursive views, partition pruning) keep that
status; rows marked ⛔ Not planned are unchanged.

---

## Section 3 — Skeleton plan stubs

### Location and naming

`docs/superpowers/plans/_skeleton/<topic>.md`

The `_skeleton/` prefix is the signal that brainstorming has not yet
happened for this object. When brainstorming kicks off, the skeleton is
*promoted* — moved to `docs/superpowers/plans/<YYYY-MM-DD>-<topic>.md`
with the date set to the brainstorm date, and its status field flipped
from `skeleton` to `brainstorming`.

This is symmetric with the existing `docs/superpowers/plans/` layout:
real, dated plans live at the top level; in-progress stubs live in
`_skeleton/`.

### Skeleton template

```markdown
---
status: skeleton
target_version: v0.x.y
sub_spec: <slug>
---

# <Object> — implementation plan (skeleton)

## Problem
One paragraph: what does pgevolve need to do that it doesn't today?

## Scope
- In: …
- Out: …

## IR sketch
TBD — define when brainstorm runs.

## Catalog reader notes
TBD — list pg_catalog tables involved + known PG-version divergences.

## Conformance fixtures
TBD — what tier-C fixtures will exist when this lands.

## Open questions
- …

## Dependencies on other roadmap items
- …
```

### Stubs created by this design

One file per row in the version map above, *except* PG 18 support
(which has a real plan, not a skeleton). Initial set:

- `_skeleton/statistics.md` (v0.3.7)
- `_skeleton/view-with-check-option.md` (v0.3.7)
- `_skeleton/create-collation.md` (v0.3.8)
- `_skeleton/range-type.md` (v0.3.8)
- `_skeleton/event-trigger.md` (v0.4.0)
- `_skeleton/per-partition-tablespace.md` (v0.4.0)
- `_skeleton/table-access-method.md` (v0.4.0)
- `_skeleton/aggregate.md` (v0.4.1)
- `_skeleton/virtual-generated-columns.md` (v0.4.1)
- `_skeleton/cluster-tablespace.md` (v0.4.2)
- `_skeleton/pl-language-wiring.md` (v0.4.2)
- `_skeleton/text-search.md` (v0.4.3)
- `_skeleton/fdw-family.md` (v0.5.0)
- `_skeleton/operator-family.md` (v0.5.1)
- `_skeleton/cast.md` (v0.5.2)

Each stub gets a one-paragraph problem statement and a populated scope /
dependencies section. Everything else is `TBD`.

---

## Out of scope for this design

- Writing the actual brainstorm or full plan for any roadmap item beyond
  PG 18 — each gets its own brainstorm when its slot comes up.
- Schedule commitments. The roadmap is an *ordering*, not a delivery
  calendar. Version numbers may slip; the order does not.
- IR shape decisions for any future object. Skeletons leave IR `TBD`.

## Verification

- `docs/spec/roadmap.md` exists and renders cleanly.
- Every 🔮 / 📋 row in `docs/spec/objects.md` either appears in the
  roadmap table or is in the ⛔ list with a one-line reason.
- Constitution §6 and `docs/user/*.md` agree on the version range
  `14–18`.
- The PG 18 plan (`plans/2026-05-26-postgres-18-support.md`) compiles
  to actionable steps that a single PR can execute (Section 1 scope).
- All skeleton stubs exist under `docs/superpowers/plans/_skeleton/`
  and follow the template.
