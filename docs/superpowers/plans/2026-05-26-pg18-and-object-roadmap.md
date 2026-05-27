# PG 18 + Object-Kinds Roadmap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the spec/docs changes for PG 18 support and publish a roadmap + skeleton-plan stubs for every remaining 🔮 Future / 📋 Planned object kind in `docs/spec/objects.md`.

**Architecture:** Pure documentation + new plan files. No Rust code changes. The PG 18 *catalog-read* work is a separate, code-level plan (`2026-05-26-postgres-18-support.md`) that this plan creates but does not execute.

**Tech Stack:** Markdown only. Verified with `grep`, `ls`, and visual review.

---

## File Structure

| Path | Action | Responsibility |
|---|---|---|
| `docs/CONSTITUTION.md` | Modify | §6 active-version list → `14, 15, 16, 17, 18` |
| `docs/user/installation.md` | Modify | "Postgres 14–18" in supported-versions blurb |
| `docs/user/configuration.md` | Modify | `min_pg_version` max documented to 18 |
| `docs/spec/README.md` | Modify | Naming-conventions paragraph gains v0.3.6 entry |
| `docs/spec/objects.md` | Modify | Status flips per roadmap; new PG 18 callout |
| `docs/spec/roadmap.md` | Create | Canonical roadmap table |
| `docs/superpowers/plans/2026-05-26-postgres-18-support.md` | Create | Code-level plan for v0.3.6 PG 18 catalog work |
| `docs/superpowers/plans/_skeleton/*.md` (15 files) | Create | Stub plans for each remaining object kind |

---

## Task 1: Update CONSTITUTION.md §6 active-version list

**Files:**
- Modify: `docs/CONSTITUTION.md` (the paragraph beginning "We support every Postgres version…")

- [ ] **Step 1: Read the current §6 paragraph**

Run: `sed -n '49,54p' docs/CONSTITUTION.md`
Expected: text mentioning "14, 15, 16, and 17"

- [ ] **Step 2: Replace the supported-versions sentence**

Edit `docs/CONSTITUTION.md` §6.

Old:
```
We support every Postgres version that the Postgres community actively maintains. The currently supported versions are **14, 15, 16, and 17**. The conformance suite runs against all four.
```

New:
```
We support every Postgres version that the Postgres community actively maintains. The currently supported versions are **14, 15, 16, 17, and 18**. The conformance suite runs against all five.
```

The EOL paragraph (`Postgres 14 reaches EOL in November 2026…`) is unchanged.

- [ ] **Step 3: Verify the change**

Run: `grep -n "14, 15, 16, 17, and 18" docs/CONSTITUTION.md`
Expected: one match on §6 line ~51.

Run: `grep -n "14, 15, 16, and 17" docs/CONSTITUTION.md`
Expected: no matches.

- [ ] **Step 4: Commit**

```bash
git add docs/CONSTITUTION.md
git commit -m "$(cat <<'EOF'
docs(constitution): add PG 18 to the active version matrix

Postgres 18 went GA in September 2025; the constitution should reflect
that pgevolve commits to supporting it. PG 14 EOL note is unchanged.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Update user/installation.md supported-versions blurb

**Files:**
- Modify: `docs/user/installation.md` (line 11 area — the "Postgres 14–17" bullet)

- [ ] **Step 1: Read the current line**

Run: `grep -n "Postgres 14" docs/user/installation.md`
Expected: one match referring to "Postgres 14–17".

- [ ] **Step 2: Replace `14–17` with `14–18`**

Edit the bullet so it reads "Postgres 14–18" (preserve surrounding prose verbatim — only the version range changes).

- [ ] **Step 3: Verify**

Run: `grep -n "Postgres 14" docs/user/installation.md`
Expected: one match reading "Postgres 14–18".

Run: `grep -n "Postgres 14–17" docs/user/installation.md`
Expected: no matches.

- [ ] **Step 4: Commit**

```bash
git add docs/user/installation.md
git commit -m "$(cat <<'EOF'
docs(installation): bump supported range to Postgres 14-18

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Update user/configuration.md min_pg_version row

**Files:**
- Modify: `docs/user/configuration.md` (line 45 — the `min_pg_version` table row)

- [ ] **Step 1: Read the row**

Run: `grep -n "min_pg_version" docs/user/configuration.md`
Expected: one row reading something like `| min_pg_version | 14 | Minimum PG major version…`.

- [ ] **Step 2: Update the row's description to acknowledge PG 18**

Edit the row so the description ends with `…targets. Accepted values: 14, 15, 16, 17, 18. Gates PG-version-specific source features (e.g., publication row filters need PG 15+).`

The default value stays `14`.

- [ ] **Step 3: Verify**

Run: `grep -n "14, 15, 16, 17, 18" docs/user/configuration.md`
Expected: one match in the `min_pg_version` row.

- [ ] **Step 4: Commit**

```bash
git add docs/user/configuration.md
git commit -m "$(cat <<'EOF'
docs(configuration): document min_pg_version range up to 18

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Update spec/README.md naming-conventions paragraph

**Files:**
- Modify: `docs/spec/README.md` (the "Naming conventions" section, lines 44–53)

- [ ] **Step 1: Read the current bullet list**

Run: `sed -n '44,54p' docs/spec/README.md`
Expected: bullets for v0.1, v0.2, v0.3, Future.

- [ ] **Step 2: Append a v0.3.6+ bullet**

After the existing v0.3 bullet, insert:

```markdown
- **"v0.3.6+"** continues v0.3 with PG 18 support (v0.3.6),
  `STATISTICS` + `WITH CHECK OPTION` (v0.3.7), `CREATE COLLATION`
  + `RANGE TYPE` (v0.3.8). See [`roadmap.md`](./roadmap.md) for the
  full per-version plan.
```

- [ ] **Step 3: Verify**

Run: `grep -n "v0.3.6" docs/spec/README.md`
Expected: one match in the naming-conventions section.

- [ ] **Step 4: Commit**

```bash
git add docs/spec/README.md
git commit -m "$(cat <<'EOF'
docs(spec): note v0.3.6+ roadmap in naming conventions

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Create `docs/spec/roadmap.md`

**Files:**
- Create: `docs/spec/roadmap.md`

- [ ] **Step 1: Confirm parent directory exists**

Run: `ls docs/spec/`
Expected: existing files (`README.md`, `objects.md`, etc.); no `roadmap.md` yet.

- [ ] **Step 2: Write the file**

Create `docs/spec/roadmap.md` with this exact content:

````markdown
# pgevolve roadmap

This document orders every remaining 🔮 Future / 📋 Planned object kind
in [`objects.md`](./objects.md) into target releases. The ordering
principle is **Postgres dependency order × user impact**: prerequisite
objects ship first; within a dep-respecting slot, the objects that
unblock the most real applications go earlier.

Version numbers may slip; the **order** does not. Each row links to a
plan stub under [`../superpowers/plans/_skeleton/`](../superpowers/plans/_skeleton/);
the stub is promoted to a dated plan when brainstorming begins.

## Active matrix

| Target | Object / sub-feature | Plan | Notes |
|---|---|---|---|
| v0.3.5 | `SUBSCRIPTION` | [`2026-05-26-subscriptions.md`](../superpowers/plans/2026-05-26-subscriptions.md) | In flight |
| v0.3.6 | PG 18 catalog support | [`2026-05-26-postgres-18-support.md`](../superpowers/plans/2026-05-26-postgres-18-support.md) | Catalog read + conformance only; new IR features deferred |
| v0.3.7 | `STATISTICS` | [`_skeleton/statistics.md`](../superpowers/plans/_skeleton/statistics.md) | Promoted from 📋 v0.3 |
| v0.3.7 | `VIEW ... WITH CHECK OPTION` | [`_skeleton/view-with-check-option.md`](../superpowers/plans/_skeleton/view-with-check-option.md) | Trivial extension of `View` IR |
| v0.3.8 | `CREATE COLLATION` | [`_skeleton/create-collation.md`](../superpowers/plans/_skeleton/create-collation.md) | Unblocks text-search |
| v0.3.8 | `RANGE TYPE` | [`_skeleton/range-type.md`](../superpowers/plans/_skeleton/range-type.md) | Adds a `UserType` variant |
| v0.4.0 | `EVENT TRIGGER` | [`_skeleton/event-trigger.md`](../superpowers/plans/_skeleton/event-trigger.md) | Independent surface |
| v0.4.0 | Per-partition `TABLESPACE` | [`_skeleton/per-partition-tablespace.md`](../superpowers/plans/_skeleton/per-partition-tablespace.md) | `tablespace` override on partition children |
| v0.4.0 | `TABLE ... USING <access method>` | [`_skeleton/table-access-method.md`](../superpowers/plans/_skeleton/table-access-method.md) | New `access_method` field on `Table` |
| v0.4.1 | `AGGREGATE` (SQL/plpgsql state) | [`_skeleton/aggregate.md`](../superpowers/plans/_skeleton/aggregate.md) | Constrained: rejects non-readable state-function languages |
| v0.4.1 | PG 18 virtual generated columns | [`_skeleton/virtual-generated-columns.md`](../superpowers/plans/_skeleton/virtual-generated-columns.md) | New `GeneratedKind` variant |
| v0.4.2 | `TABLESPACE` (cluster object) | [`_skeleton/cluster-tablespace.md`](../superpowers/plans/_skeleton/cluster-tablespace.md) | Reverses the "out of scope" stance in `objects.md`; see design doc |
| v0.4.2 | PL-language wiring → non-SQL `FUNCTION` bodies | [`_skeleton/pl-language-wiring.md`](../superpowers/plans/_skeleton/pl-language-wiring.md) | Enables PL/Python, PL/Perl, etc. |
| v0.4.3 | `TEXT SEARCH` family | [`_skeleton/text-search.md`](../superpowers/plans/_skeleton/text-search.md) | Configuration / dictionary / parser / template |
| v0.5.0 | FDW family | [`_skeleton/fdw-family.md`](../superpowers/plans/_skeleton/fdw-family.md) | `FDW`, `SERVER`, `USER MAPPING`, `FOREIGN TABLE`, `IMPORT FOREIGN SCHEMA`; includes secrets handling |
| v0.5.1 | `OPERATOR` / `OPERATOR CLASS` / `OPERATOR FAMILY` | [`_skeleton/operator-family.md`](../superpowers/plans/_skeleton/operator-family.md) | Heavy admin surface |
| v0.5.2 | `CAST` | [`_skeleton/cast.md`](../superpowers/plans/_skeleton/cast.md) | Depends on custom types + functions |

## Future (no version commitment)

| Object / feature | Why deferred |
|---|---|
| Recursive views (`WITH RECURSIVE`) | Requires cycle-aware dep-graph handling |
| Partition pruning at plan time | Optimization, not correctness |
| `SECURITY LABEL` integration | Used primarily by SE-Linux; low demand |
| Security-barrier / leakproof per-function flag review | Lands alongside finer-grained policy review |

## Explicitly out of scope

These remain ⛔ Not planned (rationale lives in `objects.md`):

- `RULE` — superseded by triggers
- `BASE TYPE` — requires C-language functions
- `INHERITS` — superseded by declarative partitioning
- `DETACH PARTITION CONCURRENTLY` — minimal benefit, high apply-time complexity
- `DATABASE` itself, `TABLESPACE` filesystem layout, cluster-wide settings, backups, data

## Ordering rationale

Two principles, applied in order:

1. **Postgres dependency order.** `CREATE COLLATION` precedes `TEXT
   SEARCH`. PL-language wiring precedes non-SQL/plpgsql `FUNCTION`
   bodies. FDW `SERVER` / `USER MAPPING` precede `FOREIGN TABLE`.
2. **User impact / demand.** Within a dep-respecting slot, the objects
   that unblock the most real applications go earlier. `STATISTICS`,
   `EVENT TRIGGER`, `RANGE TYPE`, `VIEW ... WITH CHECK OPTION`, and
   `CREATE COLLATION` rank high. `OPERATOR FAMILY` and `CAST` rank low.

## How to use this document

- **Adding a new object kind:** insert a row in the active matrix at the
  appropriate version, link to a `_skeleton/` stub, and update
  `objects.md` to flip the status from 🔮 to 📋.
- **Starting brainstorming on an object:** promote the `_skeleton/<topic>.md`
  file to `<YYYY-MM-DD>-<topic>.md` at the top of `docs/superpowers/plans/`,
  flip `status: skeleton` → `status: brainstorming`, and update the
  roadmap row's plan link.
- **Slipping a version:** edit only the `Target` column; the order does
  not change.
````

- [ ] **Step 3: Verify**

Run: `ls -l docs/spec/roadmap.md`
Expected: file exists, non-zero size.

Run: `grep -c "_skeleton/" docs/spec/roadmap.md`
Expected: 15 (one per skeleton link).

- [ ] **Step 4: Commit**

```bash
git add docs/spec/roadmap.md
git commit -m "$(cat <<'EOF'
docs(spec): add roadmap.md ordering remaining object kinds

Single canonical table mapping every 🔮 Future / 📋 Planned object kind
in objects.md to a target release, ordered by PG dependency × user
impact. Each row links to a skeleton plan stub.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Update objects.md status flips + PG 18 callout

**Files:**
- Modify: `docs/spec/objects.md`

- [ ] **Step 1: Flip statuses for items now committed to a version**

For each of the following rows in `docs/spec/objects.md`, change `🔮 Future` to `📋 Planned, vX.Y.Z` and append `See [\`roadmap.md\`](./roadmap.md).` to the notes:

| Row anchor (search string) | New status | Version |
|---|---|---|
| `CREATE VIEW ... WITH CHECK OPTION` | 📋 Planned, v0.3.7 | v0.3.7 |
| `EVENT TRIGGER` | 📋 Planned, v0.4.0 | v0.4.0 |
| `AGGREGATE` | 📋 Planned, v0.4.1 | v0.4.1 |
| `RANGE TYPE` | 📋 Planned, v0.3.8 | v0.3.8 |
| `FUNCTION` (other PL languages | 📋 Planned, v0.4.2 | v0.4.2 |
| `FOREIGN DATA WRAPPER` | 📋 Planned, v0.5.0 | v0.5.0 |
| `FOREIGN TABLE` | 📋 Planned, v0.5.0 | v0.5.0 |
| `TABLESPACE` (the row in "Storage and physical layout", *not* per-partition) | 📋 Planned, v0.4.2 | v0.4.2 |
| `TABLE ... USING <access method>` | 📋 Planned, v0.4.0 | v0.4.0 |
| `OPERATOR` / `OPERATOR CLASS` / `OPERATOR FAMILY` | 📋 Planned, v0.5.1 | v0.5.1 |
| `CAST` | 📋 Planned, v0.5.2 | v0.5.2 |
| `CREATE COLLATION` (the second half of the COLLATION row's notes) | 📋 Planned, v0.3.8 (CREATE COLLATION half only) | v0.3.8 |
| `TEXT SEARCH CONFIGURATION` | 📋 Planned, v0.4.3 | v0.4.3 |
| `SERVER` (FDW server) | 📋 Planned, v0.5.0 | v0.5.0 |
| `USER MAPPING` | 📋 Planned, v0.5.0 | v0.5.0 |
| `Per-partition TABLESPACE` (in the "Partitioning / Out of scope" notes block) | 📋 Planned, v0.4.0 | v0.4.0 |

Rows that stay 🔮 Future (no version commitment): Recursive views, Partition pruning at plan time, Security barriers / leakproof flags.

Rows that stay ⛔ Not planned are unchanged.

The `SUBSCRIPTION` row, currently 🔮 Future, becomes `✅ Implemented, v0.3.5` *only if the in-flight subscription work has merged by the time this task runs*. Otherwise leave it 🔮 and call it out in the commit message.

- [ ] **Step 2: Add PG 18 callout subsection**

Append a new H2 section to `docs/spec/objects.md`, after the "What `pgevolve` deliberately does not manage" section:

```markdown
## PG 18-only features

These features ship only on Postgres 18+. They are *not* part of the
v0.3.6 PG 18 catalog-support work; each gets its own roadmap entry.

| Feature | Status | Notes |
|---|---|---|
| Virtual generated columns (`GENERATED ALWAYS AS (...) VIRTUAL`) | 📋 Planned, v0.4.1 | New `GeneratedKind::Virtual` variant alongside the existing stored generated columns. Requires `[managed].min_pg_version >= 18`. |
| `NOT NULL NOT VALID` constraint variant | 🔮 Future | Allows declaring a NOT NULL constraint without validating existing rows. Useful for large-table migrations. |
```

- [ ] **Step 3: Verify**

Run: `grep -c "📋 Planned" docs/spec/objects.md`
Expected: at least 14 matches (one per committed row above; STATISTICS already had 📋 Planned).

Run: `grep -n "PG 18-only features" docs/spec/objects.md`
Expected: one match for the new H2.

Run: `grep -c "roadmap.md" docs/spec/objects.md`
Expected: at least 14 matches (links from each updated row).

- [ ] **Step 4: Commit**

```bash
git add docs/spec/objects.md
git commit -m "$(cat <<'EOF'
docs(spec): flip 🔮 statuses to 📋 per roadmap; add PG 18 features callout

Every object kind that roadmap.md commits to a target version now reads
📋 Planned with a backlink. New "PG 18-only features" section covers
virtual generated columns + NOT NULL NOT VALID.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Create the PG 18 support implementation plan

**Files:**
- Create: `docs/superpowers/plans/2026-05-26-postgres-18-support.md`

This is the code-level plan that future execution will run to actually add PG 18 to the catalog reader. This task only *writes the plan file* — it does not execute the Rust changes.

- [ ] **Step 1: Confirm catalog layout**

Run: `ls crates/pgevolve-core/src/catalog/queries/pg*.rs`
Expected: `pg14.rs`, `pg15.rs`, `pg16.rs`, `pg17.rs`. No `pg18.rs` yet.

Run: `grep -n "Pg14\|Pg15\|Pg16\|Pg17" crates/pgevolve-core/src/catalog/version.rs`
Expected: ~6 sites — enum, FromStr, as_str, etc.

- [ ] **Step 2: Write the plan file**

Create `docs/superpowers/plans/2026-05-26-postgres-18-support.md` with this content:

````markdown
# PG 18 Catalog Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Extend pgevolve to read v0.3 IR from a Postgres 18 server. No new IR shapes; just the version variant, the dispatch table, the query module, and the conformance matrix.

**Architecture:** Mirror the existing `pg{14,15,16,17}.rs` pattern. Initial `pg18.rs` is a thin re-export of `shared` — divergences (if any) are discovered by running tier-2 round-trip tests under PG 18 and added incrementally.

**Tech Stack:** Rust 1.x, `pg_query = "6"`, ephemeral-Postgres testkit via Docker.

---

## File Structure

| Path | Action | Responsibility |
|---|---|---|
| `crates/pgevolve-core/src/catalog/version.rs` | Modify | Add `Pg18` variant + detection |
| `crates/pgevolve-core/src/catalog/queries/pg18.rs` | Create | PG 18 SQL strings (re-exports `shared` initially) |
| `crates/pgevolve-core/src/catalog/queries/mod.rs` | Modify | Dispatch `Pg18 =>` arms |
| `crates/pgevolve-testkit/src/ephemeral_pg.rs` | Modify | `default_pg_version` + `Pg18` case |
| `.github/workflows/ci.yml` (or equivalent) | Modify | Add `pg:18` to the matrix |
| `crates/pgevolve-conformance/...` | No code change | Tier-3/4 fixtures already run version-parametrically |

---

## Task 1: Add `PgVersion::Pg18` variant

**Files:**
- Modify: `crates/pgevolve-core/src/catalog/version.rs`

- [ ] **Step 1: Write a failing test for PG 18 detection**

Add to `tests` module in `version.rs`:

```rust
#[test]
fn detects_pg18() {
    assert_eq!(
        PgVersion::detect(&MockSingle(180_000)).unwrap(),
        PgVersion::Pg18,
    );
}
```

- [ ] **Step 2: Run test, verify failure**

Run: `cargo test -p pgevolve-core catalog::version::tests::detects_pg18`
Expected: FAIL — `PgVersion::Pg18` does not exist.

- [ ] **Step 3: Add the variant + all match arms**

In `version.rs`:
- Add `Pg18` to the `PgVersion` enum (after `Pg17`).
- Add `18 => Ok(Self::Pg18),` to the `from_major` match.
- Add `Self::Pg18 => "pg18",` to `as_str`.
- Add `Self::Pg18 => 18,` to `major`.
- Update the round-trip test array to include `(180_000, PgVersion::Pg18)`.

- [ ] **Step 4: Run all version tests**

Run: `cargo test -p pgevolve-core catalog::version`
Expected: all pass, including the new `detects_pg18`.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/catalog/version.rs
git commit -m "$(cat <<'EOF'
feat(catalog): add PgVersion::Pg18

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Create `queries/pg18.rs` as a `shared` re-export

**Files:**
- Create: `crates/pgevolve-core/src/catalog/queries/pg18.rs`
- Modify: `crates/pgevolve-core/src/catalog/queries/mod.rs`

- [ ] **Step 1: Inspect `pg17.rs` for the re-export pattern**

Run: `cat crates/pgevolve-core/src/catalog/queries/pg17.rs`
Expected: short file re-exporting `shared::` constants by name.

- [ ] **Step 2: Create `pg18.rs` mirroring `pg17.rs`**

Copy each `pub use shared::*` or `pub const ... = shared::...` line verbatim. For now, every query is identical to `shared`. Divergences (if discovered in Task 4) get inlined here.

- [ ] **Step 3: Add `pub mod pg18;` to `queries/mod.rs`**

Insert after `pub mod pg17;`.

- [ ] **Step 4: Add `Pg18 =>` dispatch arms in `query_for`**

For every `(PgVersion::Pg17, CatalogQuery::X) => pg17::X,` arm, add a matching `(PgVersion::Pg18, CatalogQuery::X) => pg18::X,` arm. The pattern is fully exhaustive — `cargo check` will reject any missing arm.

- [ ] **Step 5: Run `cargo check`**

Run: `cargo check -p pgevolve-core`
Expected: clean compile.

- [ ] **Step 6: Run the queries-module tests**

Run: `cargo test -p pgevolve-core catalog::queries`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/pgevolve-core/src/catalog/queries/pg18.rs crates/pgevolve-core/src/catalog/queries/mod.rs
git commit -m "$(cat <<'EOF'
feat(catalog): add pg18.rs queries module (re-exports shared)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Add PG 18 to the testkit ephemeral-Postgres helper

**Files:**
- Modify: `crates/pgevolve-testkit/src/ephemeral_pg.rs`

- [ ] **Step 1: Locate version handling in testkit**

Run: `grep -n "Pg17\|pg:17\|17.0" crates/pgevolve-testkit/src/ephemeral_pg.rs`
Expected: matches for each call site that knows about supported versions.

- [ ] **Step 2: Mirror every `Pg17` case for `Pg18`**

For each match arm or version mapping that handles `Pg17`, add a parallel `Pg18` arm. Docker image tag is `postgres:18` (or `postgres:18-alpine` if the project uses alpine elsewhere — check `Pg17`'s tag).

- [ ] **Step 3: Build the testkit**

Run: `cargo build -p pgevolve-testkit`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-testkit/src/ephemeral_pg.rs
git commit -m "$(cat <<'EOF'
feat(testkit): add Pg18 ephemeral-postgres support

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Run tier-2 round-trip tests against PG 18; capture divergences

**Files:**
- Possibly modify: `crates/pgevolve-core/src/catalog/queries/pg18.rs`
- Possibly modify: `crates/pgevolve-core/src/catalog/queries/shared.rs`

- [ ] **Step 1: Run the full tier-2 catalog round-trip suite under PG 18**

Run: `PGEVOLVE_PG_VERSION=18 cargo test -p pgevolve-core --features pg-tests catalog`
Expected: ideally all pass with no divergences. If any fail with column-not-found / function-not-found / type-cast errors, those are the PG 18 divergences.

- [ ] **Step 2: Catalog each divergence**

For each test failure, identify the query and the divergence root cause (column renamed, function gone, new column needed for v0.3 IR). Record:
- File: which query (e.g., `SELECT_PUBLICATIONS`)
- Cause: e.g., `pg_publication.pubviaroot` renamed in PG 18 (hypothetical)
- Fix: inline a PG-18-specific variant in `pg18.rs`, leaving `shared.rs` unchanged.

- [ ] **Step 3: For each divergence, write a failing test then fix**

Per divergence:
1. Add a tier-2 fixture or assertion that specifically exercises the affected query under PG 18.
2. Run, confirm failure.
3. Replace the `re-exported` constant in `pg18.rs` with an inline PG-18-specific SQL string.
4. Re-run, confirm pass.
5. Commit with `fix(catalog): adapt SELECT_X for PG 18`.

If no divergences are found, this task is a one-line commit:

```bash
git commit --allow-empty -m "$(cat <<'EOF'
test(catalog): tier-2 round-trip clean against PG 18; no divergences

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Add PG 18 to the CI matrix

**Files:**
- Modify: `.github/workflows/ci.yml` (or the equivalent file for the `pg-matrix` job)

- [ ] **Step 1: Locate the `pg-matrix` job**

Run: `grep -rn "pg-matrix\|postgres:17" .github/`
Expected: one job with a version matrix listing `14, 15, 16, 17`.

- [ ] **Step 2: Add `18` to the version matrix**

Edit the matrix definition so the version list reads `[14, 15, 16, 17, 18]`.

- [ ] **Step 3: Verify locally that the workflow YAML parses**

Run: `yamllint .github/workflows/ci.yml` (if installed) — or just `cat` the file and visually verify.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
ci: add Postgres 18 to the pg-matrix job

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Bump `min_pg_version` upper bound in CLI config validation

**Files:**
- Modify: wherever `min_pg_version` is parsed/validated in `crates/pgevolve/src/`

- [ ] **Step 1: Locate the validator**

Run: `grep -rn "min_pg_version" crates/pgevolve/src/ crates/pgevolve-core/src/`
Expected: parser + validator that rejects values outside `14..=17`.

- [ ] **Step 2: Update the validation range to `14..=18`**

Edit the call site so the inclusive upper bound is `18`. Update any error-message string that lists supported versions to include `18`.

- [ ] **Step 3: Add a unit test that `min_pg_version = 18` parses cleanly**

Add a test alongside the existing `min_pg_version` parsing tests.

- [ ] **Step 4: Run config tests**

Run: `cargo test -p pgevolve-core -p pgevolve config min_pg_version`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/ crates/pgevolve/src/
git commit -m "$(cat <<'EOF'
feat(config): accept min_pg_version = 18

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Verification (end of plan)

- [ ] `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace` all green.
- [ ] `PGEVOLVE_PG_VERSION=18 cargo test --workspace --features pg-tests` green.
- [ ] CI's `pg-matrix` job exercises PG 18.
- [ ] `pgevolve.toml` with `min_pg_version = 18` parses without error.
- [ ] Constitution §6 reads "14, 15, 16, 17, and 18".
````

- [ ] **Step 3: Verify the file was written**

Run: `wc -l docs/superpowers/plans/2026-05-26-postgres-18-support.md`
Expected: ~280 lines.

Run: `grep -c "^### Task\|^## Task" docs/superpowers/plans/2026-05-26-postgres-18-support.md`
Expected: 6 (Tasks 1–6).

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/plans/2026-05-26-postgres-18-support.md
git commit -m "$(cat <<'EOF'
docs(plans): PG 18 catalog support implementation plan

Code-level plan targeting v0.3.6 release. Catalog read + conformance
matrix only; new PG 18 IR features (virtual generated columns,
NOT NULL NOT VALID) are tracked separately under the roadmap.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Create the `_skeleton/` directory and the v0.3.7 + v0.3.8 stubs

**Files:**
- Create: `docs/superpowers/plans/_skeleton/statistics.md`
- Create: `docs/superpowers/plans/_skeleton/view-with-check-option.md`
- Create: `docs/superpowers/plans/_skeleton/create-collation.md`
- Create: `docs/superpowers/plans/_skeleton/range-type.md`

- [ ] **Step 1: Create the directory**

Run: `mkdir -p docs/superpowers/plans/_skeleton`

- [ ] **Step 2: Write `statistics.md`**

```markdown
---
status: skeleton
target_version: v0.3.7
sub_spec: statistics
---

# `STATISTICS` — implementation plan (skeleton)

## Problem
Postgres `CREATE STATISTICS` declares multi-column statistics objects
(`ndistinct`, `dependencies`, `mcv`) that the planner uses for correlated
columns. pgevolve does not yet manage these. Currently 📋 Planned in
`objects.md`.

## Scope
- In: `CREATE STATISTICS`, `DROP STATISTICS`, `ALTER STATISTICS ...
  RENAME TO`, `ALTER STATISTICS ... SET STATISTICS n`, all three kinds
  (`ndistinct`, `dependencies`, `mcv`), expression statistics (PG 14+),
  `COMMENT ON STATISTICS`.
- Out: `CREATE STATISTICS ... INCLUDE` (PG 18+) until v0.4.x.

## IR sketch
TBD — likely a `Catalog::statistics: Vec<Statistic>` collection with
fields for kinds bitset, columns/expressions, target table, optional
`statistics_target`.

## Catalog reader notes
TBD — primary table is `pg_statistic_ext` joined with `pg_namespace` and
`pg_class`. Kind information is in `stxkind`. Expression statistics
require `pg_statistic_ext_data`.

## Conformance fixtures
TBD — `objects/statistics/create-simple`, `add-kind`, `drop`,
`alter-set-target`, `expression-stats`, `comment-on`.

## Open questions
- How are statistics that reference dropped columns handled by the
  catalog? Do we need a cascade-drop path?
- Lint rule for unmanaged statistics in managed schemas?

## Dependencies on other roadmap items
None — independent surface.
```

- [ ] **Step 3: Write `view-with-check-option.md`**

```markdown
---
status: skeleton
target_version: v0.3.7
sub_spec: view-with-check-option
---

# `CREATE VIEW ... WITH CHECK OPTION` — implementation plan (skeleton)

## Problem
Updatable views can be created with `WITH [LOCAL | CASCADED] CHECK
OPTION` so that DML through the view enforces the view's predicate.
pgevolve currently ignores this clause; it's marked 🔮 in `objects.md`.

## Scope
- In: parse `WITH LOCAL CHECK OPTION` and `WITH CASCADED CHECK OPTION`;
  model on `View` as `check_option: Option<CheckOption>` enum; emit via
  `CREATE OR REPLACE VIEW`; round-trip via `pg_views.viewdef` or
  `pg_rewrite`.
- Out: `WITH CHECK OPTION` on materialized views — Postgres does not
  support it there.

## IR sketch
TBD — add `check_option: Option<CheckOption>` to `crates/pgevolve-core/src/ir/view.rs`
with `CheckOption::{Local, Cascaded}`.

## Catalog reader notes
TBD — check-option setting lives in `pg_rewrite` rule deps or is encoded
in the rewritten action's `WithCheckOption` node; verify by querying a
known-good view.

## Conformance fixtures
TBD — `objects/views/create-with-local-check-option`,
`create-with-cascaded-check-option`, `toggle-check-option`.

## Open questions
- Does a check-option change require `CREATE OR REPLACE` or a drop +
  recreate? (Likely `CREATE OR REPLACE` works.)

## Dependencies on other roadmap items
None.
```

- [ ] **Step 4: Write `create-collation.md`**

```markdown
---
status: skeleton
target_version: v0.3.8
sub_spec: create-collation
---

# `CREATE COLLATION` — implementation plan (skeleton)

## Problem
pgevolve already references collations on columns (`per-column
collation` is supported), but `CREATE COLLATION` (defining new
collations from `lc_collate` / `lc_ctype` / `provider` / `locale` /
`deterministic`) is not managed. Text-search configurations and other
locale-sensitive objects need this prerequisite.

## Scope
- In: `CREATE COLLATION`, `DROP COLLATION`, `ALTER COLLATION ... RENAME
  TO`, `ALTER COLLATION ... REFRESH VERSION`, `COMMENT ON COLLATION`.
  Both libc and ICU providers.
- Out: `CREATE COLLATION ... FROM existing_collation` if catalog
  round-trip ambiguity proves intractable — re-evaluate during
  brainstorm.

## IR sketch
TBD — `Catalog::collations: Vec<Collation>` with fields: `qname`,
`provider` (`Libc` | `Icu`), `locale`, `lc_collate`, `lc_ctype`,
`deterministic: bool`, `version: Option<String>`, `comment`.

## Catalog reader notes
TBD — primary table is `pg_collation` joined with `pg_namespace`. Filter
out built-in collations (e.g., `default`, `C`, `POSIX`).

## Conformance fixtures
TBD — `objects/collations/create-icu`, `create-libc`,
`create-nondeterministic`, `alter-refresh-version`, `drop`, `comment-on`.

## Open questions
- How to handle collations whose `version` drifts after `pg_upgrade`?
  Probably a lint, not a diff.
- Built-in collation filter precision — exclude by `collprovider = 'c'
  AND collnamespace = 'pg_catalog'`?

## Dependencies on other roadmap items
- Unblocks `TEXT SEARCH` (v0.4.3).
```

- [ ] **Step 5: Write `range-type.md`**

```markdown
---
status: skeleton
target_version: v0.3.8
sub_spec: range-type
---

# `RANGE TYPE` — implementation plan (skeleton)

## Problem
`CREATE TYPE ... AS RANGE` defines a user range type (subtype + optional
subtype_opclass, collation, canonical, subtype_diff, multirange_type_name).
pgevolve handles enums, domains, and composites today but not ranges;
range-typed columns currently fail at IR-build with an "unknown type"
error.

## Scope
- In: `CREATE TYPE ... AS RANGE`, `DROP TYPE`, `COMMENT ON TYPE`. Range
  types are immutable once created (no `ALTER TYPE` for ranges).
- Out: multirange type customization beyond `multirange_type_name`.

## IR sketch
TBD — add `RangeType` variant to the existing `UserType` enum, with
fields `subtype: QualifiedName`, `subtype_opclass: Option<QualifiedName>`,
`collation: Option<QualifiedName>`, `canonical: Option<QualifiedName>`,
`subtype_diff: Option<QualifiedName>`,
`multirange_type_name: Option<Identifier>`.

## Catalog reader notes
TBD — primary table is `pg_range`, joined with `pg_type` for the range
type's `oid` and `pg_type` for the subtype.

## Conformance fixtures
TBD — `objects/ranges/create-simple`, `create-with-opclass`,
`create-with-canonical-fn`, `drop`, `comment-on`,
`column-with-range-type`.

## Open questions
- Drop semantics: a range type drop cascades to the multirange type;
  diff should account for this.

## Dependencies on other roadmap items
- Independent. May expose latent bugs in the column-type system for
  range-typed columns; tier-C fixture should cover that path.
```

- [ ] **Step 6: Verify all four stubs exist**

Run: `ls docs/superpowers/plans/_skeleton/`
Expected: 4 files (`statistics.md`, `view-with-check-option.md`, `create-collation.md`, `range-type.md`).

- [ ] **Step 7: Commit**

```bash
git add docs/superpowers/plans/_skeleton/
git commit -m "$(cat <<'EOF'
docs(plans): skeleton stubs for v0.3.7 + v0.3.8 roadmap entries

Adds STATISTICS, VIEW WITH CHECK OPTION, CREATE COLLATION, RANGE TYPE.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Create the v0.4.0 + v0.4.1 stubs

**Files:**
- Create: `docs/superpowers/plans/_skeleton/event-trigger.md`
- Create: `docs/superpowers/plans/_skeleton/per-partition-tablespace.md`
- Create: `docs/superpowers/plans/_skeleton/table-access-method.md`
- Create: `docs/superpowers/plans/_skeleton/aggregate.md`
- Create: `docs/superpowers/plans/_skeleton/virtual-generated-columns.md`

- [ ] **Step 1: Write `event-trigger.md`**

```markdown
---
status: skeleton
target_version: v0.4.0
sub_spec: event-trigger
---

# `EVENT TRIGGER` — implementation plan (skeleton)

## Problem
Event triggers fire on DDL events (`ddl_command_start`, `ddl_command_end`,
`table_rewrite`, `sql_drop`). pgevolve has plain triggers but not event
triggers. Used by audit tooling, schema-protection tooling, and some
extensions.

## Scope
- In: `CREATE EVENT TRIGGER`, `ALTER EVENT TRIGGER ... ENABLE/DISABLE`,
  `DROP EVENT TRIGGER`, `COMMENT ON EVENT TRIGGER`. `WHEN TAG IN (...)`
  filter.
- Out: `ALTER EVENT TRIGGER ... RENAME TO` (consistent with plain trigger
  policy — rename is drop+create).

## IR sketch
TBD — new top-level `Catalog::event_triggers: Vec<EventTrigger>`,
analogous to but separate from `Trigger`. Fields: `name`, `event`
(`DdlCommandStart` | `DdlCommandEnd` | `TableRewrite` | `SqlDrop`),
`tag_filter: Vec<String>`, `function_name`, `enabled` (`Always` |
`Replica` | `Disabled` | `Enabled`), `comment`.

## Catalog reader notes
TBD — `pg_event_trigger` joined with `pg_proc` for the function name.
Exclude extension-owned entries (`pg_depend.deptype = 'e'`).

## Conformance fixtures
TBD — `objects/event_triggers/create-simple`, `create-with-tag-filter`,
`enable-disable`, `drop`, `comment-on`,
`scenarios/extension-event-trigger-ignored`.

## Open questions
- Event trigger functions return `event_trigger`; ensure the function-IR
  validates this return type.

## Dependencies on other roadmap items
None.
```

- [ ] **Step 2: Write `per-partition-tablespace.md`**

```markdown
---
status: skeleton
target_version: v0.4.0
sub_spec: per-partition-tablespace
---

# Per-partition `TABLESPACE` — implementation plan (skeleton)

## Problem
A partitioned-table parent may specify a default tablespace; individual
partitions may override it. pgevolve's `Table` IR has a `tablespace`
field, but the per-partition override path isn't fully exercised and
the diff path for tablespace-only changes isn't implemented.

## Scope
- In: `CREATE TABLE ... PARTITION OF ... TABLESPACE foo`; `ALTER TABLE
  partition SET TABLESPACE foo`; diff path for partition-level tablespace
  changes.
- Out: cluster-level `CREATE TABLESPACE` — that's in the v0.4.2 plan.

## IR sketch
TBD — `Table::tablespace: Option<QualifiedName>` already exists.
Confirm the catalog reader returns it correctly for partitions, and
that diff emits `ALTER TABLE ... SET TABLESPACE`.

## Catalog reader notes
TBD — `pg_class.reltablespace` → `pg_tablespace.spcname`. Zero
`reltablespace` means inherit default.

## Conformance fixtures
TBD — `objects/partitions/create-with-tablespace`,
`alter-set-tablespace`, `partition-with-different-tablespace-than-parent`.

## Open questions
- Should a tablespace move be considered destructive? In Postgres it
  rewrites the partition's storage; intent-required flag is likely needed.

## Dependencies on other roadmap items
- Pairs with cluster `TABLESPACE` (v0.4.2) for the full flow, but
  per-partition assignment can ship first against existing tablespaces.
```

- [ ] **Step 3: Write `table-access-method.md`**

```markdown
---
status: skeleton
target_version: v0.4.0
sub_spec: table-access-method
---

# `TABLE ... USING <access method>` — implementation plan (skeleton)

## Problem
Postgres tables can specify a non-default table access method (`heap`,
`zheap`, columnar AMs from extensions). The IR currently assumes `heap`
implicitly; mismatches between source and catalog are silent.

## Scope
- In: parse `USING method` in `CREATE TABLE`; model as
  `Table::access_method: Option<Identifier>`; diff via `ALTER TABLE ...
  SET ACCESS METHOD method` (PG 15+) or `ReplaceWithCascade` on PG 14
  (PG 14 lacks the ALTER form).
- Out: `CREATE ACCESS METHOD` itself (extension-provided AMs only;
  pgevolve doesn't manage AM definitions).

## IR sketch
TBD — `Table::access_method: Option<Identifier>`; `None` means inherit
cluster default (`heap` in practice).

## Catalog reader notes
TBD — `pg_class.relam` → `pg_am.amname`. Filter out `heap` to keep IR
canonical (or always include — TBD during brainstorm).

## Conformance fixtures
TBD — `objects/tables/create-with-access-method` (needs an extension
that provides a non-heap AM available in the test image — likely
`pg_columnar`, or skip via `requires-extension` fixture flag),
`alter-set-access-method`.

## Open questions
- PG 14 lacks `ALTER TABLE ... SET ACCESS METHOD`; pre-PG 15 path must
  be drop + recreate. Confirm.
- Do we lint when an unknown AM is referenced (i.e., extension not
  declared)?

## Dependencies on other roadmap items
- Loose pairing with the extensions surface (extensions provide AMs).
```

- [ ] **Step 4: Write `aggregate.md`**

```markdown
---
status: skeleton
target_version: v0.4.1
sub_spec: aggregate
---

# `AGGREGATE` — implementation plan (skeleton)

## Problem
User-defined aggregates (`CREATE AGGREGATE`) wrap a state function plus
optional final/serial/deserial/combine functions to define application
aggregates (e.g., `weighted_avg(numeric, numeric)`). pgevolve doesn't
manage them.

## Scope
- In: `CREATE AGGREGATE`, `ALTER AGGREGATE ... RENAME TO`,
  `ALTER AGGREGATE ... OWNER TO`, `DROP AGGREGATE`, `COMMENT ON AGGREGATE`.
  Ordinary aggregates only (`sfunc` + `stype` + optional `finalfunc`,
  `initcond`).
- Out: ordered-set aggregates (`CREATE AGGREGATE ... ORDER BY`); moving
  aggregates (`MSFUNC` etc.); aggregates whose state function is in a PL
  language pgevolve does not yet read. Latter case is rejected at
  IR-build with a structured error; the constraint relaxes in v0.4.2
  when PL-language wiring lands.

## IR sketch
TBD — `Catalog::aggregates: Vec<Aggregate>` with fields: `qname`,
`arg_types: Vec<QualifiedName>`, `sfunc: QualifiedName`,
`stype: QualifiedName`, `finalfunc: Option<QualifiedName>`,
`initcond: Option<String>`, `comment`.

## Catalog reader notes
TBD — `pg_aggregate` joined with `pg_proc` for the wrapper proc and
again for `aggtransfn`. Identity is `(schema, name, arg_types)`.

## Conformance fixtures
TBD — `objects/aggregates/create-simple`, `create-with-finalfunc`,
`create-with-initcond`, `drop`, `comment-on`,
`failure/aggregates/reject-plpython-state-fn`.

## Open questions
- Identity collision with overloaded `FUNCTION`s: aggregates share the
  proc namespace; ensure dep graph routes correctly when an aggregate
  and a function have the same `(qname, arg_types)`.

## Dependencies on other roadmap items
- Soft dependency on the function surface for state functions
  (already supported for SQL / plpgsql).
- PL-language wiring (v0.4.2) lifts the language constraint.
```

- [ ] **Step 5: Write `virtual-generated-columns.md`**

```markdown
---
status: skeleton
target_version: v0.4.1
sub_spec: virtual-generated-columns
---

# Virtual generated columns (PG 18) — implementation plan (skeleton)

## Problem
PG 18 introduces `GENERATED ALWAYS AS (...) VIRTUAL`, computed on read
instead of stored. The current `Column::generated` field only models
stored generated columns. Source files using `VIRTUAL` fail to parse;
catalog reads of PG 18 virtual columns produce incorrect IR.

## Scope
- In: parse `GENERATED ALWAYS AS (expr) VIRTUAL`; add
  `GeneratedKind::Virtual` variant; gate via `[managed].min_pg_version
  >= 18`; diff path between `Virtual` and `Stored` triggers
  `ReplaceWithCascade` (changing storage strategy rewrites the table).
- Out: anything PG 17-and-earlier-compatible (must be a hard error
  when source uses `VIRTUAL` against PG < 18).

## IR sketch
TBD — refactor `Column::generated: Option<NormalizedExpr>` into
`Column::generated: Option<Generated>` where
`Generated { expr: NormalizedExpr, kind: GeneratedKind }` and
`GeneratedKind::{Stored, Virtual}`. Default in parser for
`GENERATED ALWAYS AS (expr) STORED` is `Stored`.

## Catalog reader notes
TBD — `pg_attribute.attgenerated` is `'s'` for stored; PG 18 adds `'v'`
for virtual. Verify the column exists in older PG versions too (it
does, just never returns `'v'`).

## Conformance fixtures
TBD — `objects/columns/create-virtual-generated`,
`alter-stored-to-virtual` (`ReplaceWithCascade` path),
`failure/columns/virtual-on-pg17` (lint).

## Open questions
- Lint name for "VIRTUAL requires PG 18+"?
  `column-virtual-generated-requires-pg-version` follows the publication
  pattern.

## Dependencies on other roadmap items
- Depends on the v0.3.6 PG 18 catalog work (`pg18.rs` must dispatch).
```

- [ ] **Step 6: Verify and commit**

Run: `ls docs/superpowers/plans/_skeleton/ | sort`
Expected: 9 files total now (4 from Task 8 + 5 from this task).

```bash
git add docs/superpowers/plans/_skeleton/event-trigger.md docs/superpowers/plans/_skeleton/per-partition-tablespace.md docs/superpowers/plans/_skeleton/table-access-method.md docs/superpowers/plans/_skeleton/aggregate.md docs/superpowers/plans/_skeleton/virtual-generated-columns.md
git commit -m "$(cat <<'EOF'
docs(plans): skeleton stubs for v0.4.0 + v0.4.1 roadmap entries

EVENT TRIGGER, per-partition TABLESPACE, TABLE access method,
AGGREGATE, and PG 18 virtual generated columns.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Create the v0.4.2 + v0.4.3 stubs

**Files:**
- Create: `docs/superpowers/plans/_skeleton/cluster-tablespace.md`
- Create: `docs/superpowers/plans/_skeleton/pl-language-wiring.md`
- Create: `docs/superpowers/plans/_skeleton/text-search.md`

- [ ] **Step 1: Write `cluster-tablespace.md`**

```markdown
---
status: skeleton
target_version: v0.4.2
sub_spec: cluster-tablespace
---

# `TABLESPACE` (cluster object) — implementation plan (skeleton)

## Problem
`CREATE TABLESPACE name OWNER role LOCATION '/path'` is currently
out-of-scope ("cluster-level admin object outside the schema-management
remit"). The 2026-05-26 roadmap reverses this: the `pgevolve cluster …`
surface already manages roles, so tablespaces fit the same model.
Filesystem-layout management (directory creation, mount points) stays
out of scope; only the SQL `CREATE TABLESPACE` step is managed.

## Scope
- In: `CREATE TABLESPACE`, `ALTER TABLESPACE ... OWNER TO`,
  `ALTER TABLESPACE ... RENAME TO`, `ALTER TABLESPACE ... SET (option)`,
  `DROP TABLESPACE`, `COMMENT ON TABLESPACE`. Owner attribution.
- Out: filesystem directory creation; `pg_tablespace_location()`
  validation that the path exists on disk; backup-relocation rules.

## IR sketch
TBD — new `cluster::tablespace::Tablespace` analogous to
`cluster::role::Role`. Fields: `name`, `owner: Identifier`,
`location: String`, `options: BTreeMap<String, String>` (seq_page_cost,
random_page_cost, effective_io_concurrency, maintenance_io_concurrency),
`comment`.

## Catalog reader notes
TBD — `pg_tablespace` joined with `pg_authid` for owner.

## Conformance fixtures
TBD — `cluster/tablespaces/create-simple`, `alter-owner`,
`alter-set-option`, `drop`, `comment-on`. Each fixture must provide a
real directory; testkit will need a temp-dir helper.

## Open questions
- Where do tablespace path strings live in `pgevolve.toml` — under
  `[cluster.tablespaces]`? Spec-level decision needed.
- Drift: what to do when the catalog has a tablespace at a different
  filesystem path than source declares? Re-create is destructive; lint
  is likely safer.

## Dependencies on other roadmap items
- Pairs with v0.4.0 per-partition `TABLESPACE` for the complete picture.
```

- [ ] **Step 2: Write `pl-language-wiring.md`**

```markdown
---
status: skeleton
target_version: v0.4.2
sub_spec: pl-language-wiring
---

# PL-language wiring → non-SQL `FUNCTION` bodies — implementation plan (skeleton)

## Problem
pgevolve manages function bodies in SQL and plpgsql today. Bodies in
PL/Python, PL/Perl, PL/Tcl, or PL/v8 fail at IR-build time because the
parser can't validate dependencies inside an opaque body string. The
extensions surface already supports `CREATE EXTENSION plpython3u`, so
the language presence is half-solved; what's missing is the parser
contract for the body.

## Scope
- In: parse and store the body verbatim for non-SQL/plpgsql languages;
  do not attempt to extract internal SQL deps; require an explicit
  `-- @pgevolve dep:` directive list for any internal references (same
  mechanism plpgsql uses for dynamic SQL today).
- Out: actual SQL-dep extraction inside foreign-language bodies (PRs
  could add per-language extractors later as separate work).

## IR sketch
TBD — extend `Function::language` to a `Language` enum
(`Sql` | `PlPgSql` | `External(Identifier)`). For `External`, the body
canonicalization is a no-op (verbatim string match).

## Catalog reader notes
TBD — `pg_proc.prolang` → `pg_language.lanname`. Already partially used
for SQL/plpgsql; extend the readout to keep the language name verbatim
for non-built-ins.

## Conformance fixtures
TBD — `objects/functions/create-plpython-simple` (gated on
`plpython3u` extension being installed in the test image),
`create-plperl-simple`, `verbatim-body-roundtrip`,
`failure/functions/plpython-without-dep-directive-rejects-internal-sql-ref`.

## Open questions
- Do we manage `CREATE LANGUAGE` directly, or rely entirely on
  `CREATE EXTENSION plpython3u` etc.? (Modern Postgres makes the
  former a no-op in favor of the latter.)
- Verbatim body comparison vs. some form of canonicalization (strip
  trailing whitespace, normalize line endings)?

## Dependencies on other roadmap items
- Loose coupling with `EXTENSION` (must be installed first).
- Unblocks `AGGREGATE` state functions in arbitrary PL languages (v0.4.1
  ships with the constraint that state functions must be SQL/plpgsql;
  this plan lifts that constraint).
```

- [ ] **Step 3: Write `text-search.md`**

```markdown
---
status: skeleton
target_version: v0.4.3
sub_spec: text-search
---

# `TEXT SEARCH` family — implementation plan (skeleton)

## Problem
Full-text-search-aware indexes (`USING gin` on `tsvector` columns)
already work, but the configuration objects driving the tokenizer +
dictionary pipeline are not managed: `TEXT SEARCH CONFIGURATION`,
`TEXT SEARCH DICTIONARY`, `TEXT SEARCH PARSER`, `TEXT SEARCH TEMPLATE`.

## Scope
- In: all four object kinds — `CREATE`, `DROP`, `ALTER`,
  `COMMENT ON`, all variants documented in Postgres.
- Out: `CREATE TEXT SEARCH TEMPLATE` (templates need C functions —
  treat like base types: ⛔ Not planned within the spec but still
  *read* from catalog as opaque references).

## IR sketch
TBD — four new `Catalog::ts_*` collections:
- `ts_configurations: Vec<TsConfiguration>`
- `ts_dictionaries: Vec<TsDictionary>`
- `ts_parsers: Vec<TsParser>`
- `ts_templates: Vec<TsTemplate>` (read-only — write surface ⛔)

## Catalog reader notes
TBD — `pg_ts_config`, `pg_ts_config_map`, `pg_ts_dict`, `pg_ts_parser`,
`pg_ts_template`.

## Conformance fixtures
TBD — `objects/text_search/create-configuration`,
`add-mapping`, `alter-mapping`, `create-dictionary`, `drop`,
`comment-on`. Plus the index-on-tsvector regression fixture that
verifies search continues to work end-to-end.

## Open questions
- Configurations reference parsers + dictionaries — ordering in the
  dep graph matters; verify cycles aren't possible.
- COLLATION inputs to dictionaries — confirm v0.3.8 CREATE COLLATION
  is sufficient prereq.

## Dependencies on other roadmap items
- Depends on `CREATE COLLATION` (v0.3.8).
- Soft coupling with the extensions surface (e.g., `pg_trgm` registers
  dictionaries).
```

- [ ] **Step 4: Verify and commit**

Run: `ls docs/superpowers/plans/_skeleton/ | wc -l`
Expected: 12.

```bash
git add docs/superpowers/plans/_skeleton/cluster-tablespace.md docs/superpowers/plans/_skeleton/pl-language-wiring.md docs/superpowers/plans/_skeleton/text-search.md
git commit -m "$(cat <<'EOF'
docs(plans): skeleton stubs for v0.4.2 + v0.4.3 roadmap entries

Cluster TABLESPACE, PL-language wiring, and TEXT SEARCH family.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Create the v0.5.x stubs

**Files:**
- Create: `docs/superpowers/plans/_skeleton/fdw-family.md`
- Create: `docs/superpowers/plans/_skeleton/operator-family.md`
- Create: `docs/superpowers/plans/_skeleton/cast.md`

- [ ] **Step 1: Write `fdw-family.md`**

```markdown
---
status: skeleton
target_version: v0.5.0
sub_spec: fdw-family
---

# FDW family — implementation plan (skeleton)

## Problem
Postgres' foreign-data ecosystem (`postgres_fdw`, `file_fdw`, etc.) lets
applications reference data outside the local cluster as if it were
local. pgevolve manages none of the moving parts:
`FOREIGN DATA WRAPPER`, `SERVER`, `USER MAPPING`, `FOREIGN TABLE`, or
`IMPORT FOREIGN SCHEMA`. Without these, schemas that use FDWs are
unmanaged.

## Scope
- In: all five object kinds, full CRUD + comment surface, plus the
  secrets-handling story for `USER MAPPING` OPTIONS (mirrors the
  `${VAR}` env-var interpolation pattern already used by subscriptions
  in v0.3.5).
- Out: `IMPORT FOREIGN SCHEMA` as a *runtime* operation (it imports
  many foreign tables at once); pgevolve declarative model lists each
  imported foreign table explicitly, with an optional lint pointing at
  the source statement.

## IR sketch
TBD — five new collections under `Catalog`:
- `foreign_data_wrappers: Vec<ForeignDataWrapper>`
- `servers: Vec<Server>`
- `user_mappings: Vec<UserMapping>` — secrets-bearing
- `foreign_tables: Vec<ForeignTable>`
Plus reuse of existing `Table` machinery where possible (foreign tables
have columns and constraints).

## Catalog reader notes
TBD — `pg_foreign_data_wrapper`, `pg_foreign_server`,
`pg_user_mapping`, `pg_foreign_table`, `pg_attribute`. The
`umoptions` array on `pg_user_mapping` contains the secret material;
the executor must redact it from any diff output the same way
subscription `CONNECTION` strings are redacted.

## Conformance fixtures
TBD — `objects/fdws/create-postgres-fdw-server-and-foreign-table` (the
golden-path), plus per-object create/drop/alter fixtures. Secrets
fixtures must verify the env-var substitution path end-to-end.

## Open questions
- Should `USER MAPPING` be considered a *cluster* object (since it
  associates with a role) or a *schema* object? Probably cluster-ish —
  follow the cluster-tablespace pattern.
- IMPORT FOREIGN SCHEMA reconciliation: how to detect drift between
  source-declared foreign tables and what the foreign server actually
  exposes? Likely a lint, not a diff.

## Dependencies on other roadmap items
- Hard dep on the existing extensions surface (FDWs ship as extensions).
- Hard dep on the cluster-roles surface (USER MAPPING references roles).
- Reuses the secrets-interpolation machinery from v0.3.5 subscriptions.
```

- [ ] **Step 2: Write `operator-family.md`**

```markdown
---
status: skeleton
target_version: v0.5.1
sub_spec: operator-family
---

# `OPERATOR` / `OPERATOR CLASS` / `OPERATOR FAMILY` — implementation plan (skeleton)

## Problem
User-defined operators and their opclass/family membership (driving
index access methods' understanding of custom types) are unmanaged.
The most common use case is custom types that want to be indexable —
without managing the opclass/family, an index on a custom-typed column
silently breaks.

## Scope
- In: `CREATE OPERATOR`, `ALTER OPERATOR`, `DROP OPERATOR`,
  `CREATE OPERATOR CLASS`, `ALTER OPERATOR CLASS`, `DROP OPERATOR CLASS`,
  `CREATE OPERATOR FAMILY`, `ALTER OPERATOR FAMILY` (add/drop members),
  `DROP OPERATOR FAMILY`, `COMMENT ON` all three.
- Out: hash opclasses for non-standard hash functions in the first
  iteration; revisit if demand surfaces.

## IR sketch
TBD — three new `Catalog::operators*` collections. Identity for operators
is `(schema, name, left_type, right_type)`.

## Catalog reader notes
TBD — `pg_operator`, `pg_opclass`, `pg_opfamily`, `pg_amop` (operator
membership in a family), `pg_amproc` (support procedures).

## Conformance fixtures
TBD — `objects/operators/create-simple`,
`create-opclass-for-custom-type`, `alter-family-add-operator`,
`scenarios/custom-type-with-btree-opclass-roundtrip`.

## Open questions
- Cross-schema operator references in index DDL — verify the dep graph
  catches these.
- Should we lint operators with no matching opclass (i.e., unusable for
  indexing)?

## Dependencies on other roadmap items
- Loose dep on user-defined types being solid (already true for v0.2).
- Loose dep on `CAST` (v0.5.2) for some operator-driven implicit casts.
```

- [ ] **Step 3: Write `cast.md`**

```markdown
---
status: skeleton
target_version: v0.5.2
sub_spec: cast
---

# `CAST` — implementation plan (skeleton)

## Problem
User-defined casts between custom types (or between built-ins via a
user function) are not managed. Common with custom types that want to
participate in coercion paths.

## Scope
- In: `CREATE CAST (source AS target) WITH FUNCTION fn`,
  `CREATE CAST ... WITHOUT FUNCTION`, `CREATE CAST ... WITH INOUT`,
  `AS ASSIGNMENT` / `AS IMPLICIT` flags, `DROP CAST`,
  `COMMENT ON CAST`.
- Out: cast removal on built-in types (catalog reader excludes them).

## IR sketch
TBD — `Catalog::casts: Vec<Cast>` with fields `source: QualifiedName`,
`target: QualifiedName`, `method: CastMethod` (`Function(QualifiedName)`
| `Inout` | `Binary`), `context: CastContext` (`Explicit` | `Assignment`
| `Implicit`), `comment`. Identity: `(source, target)`.

## Catalog reader notes
TBD — `pg_cast` joined with `pg_type` (twice) and `pg_proc`. Filter out
built-in casts (`castcontext = 'i'` from built-ins or `castsource`/`casttarget`
in `pg_catalog`).

## Conformance fixtures
TBD — `objects/casts/create-with-function`, `create-without-function`,
`create-with-inout`, `drop`, `comment-on`,
`scenarios/custom-type-implicit-cast-roundtrip`.

## Open questions
- Identity collision when source = target = same type (legal in PG for
  domain → base coercion); ensure `Cast` identity handles this.

## Dependencies on other roadmap items
- Hard dep on the function surface and custom types (both already in).
- Soft dep on `OPERATOR` family (operators sometimes drive implicit
  casts).
```

- [ ] **Step 4: Verify and commit**

Run: `ls docs/superpowers/plans/_skeleton/ | wc -l`
Expected: 15.

Run: `ls docs/superpowers/plans/_skeleton/ | sort`
Expected (alphabetical):
```
aggregate.md
cast.md
cluster-tablespace.md
create-collation.md
event-trigger.md
fdw-family.md
operator-family.md
per-partition-tablespace.md
pl-language-wiring.md
range-type.md
statistics.md
table-access-method.md
text-search.md
view-with-check-option.md
virtual-generated-columns.md
```

```bash
git add docs/superpowers/plans/_skeleton/fdw-family.md docs/superpowers/plans/_skeleton/operator-family.md docs/superpowers/plans/_skeleton/cast.md
git commit -m "$(cat <<'EOF'
docs(plans): skeleton stubs for v0.5.x roadmap entries

FDW family, OPERATOR family, and CAST.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Cross-link verification

**Files:** none modified — verification only.

- [ ] **Step 1: Every roadmap link resolves to an existing file**

Run:
```bash
grep -oE '\(\.\./superpowers/plans/_skeleton/[a-z-]+\.md\)' docs/spec/roadmap.md \
  | sed 's|[()]||g; s|^\.\./|docs/|' \
  | while read -r path; do test -f "$path" || echo "MISSING: $path"; done
```
Expected: no output.

- [ ] **Step 2: Every roadmap entry has a corresponding `_skeleton/` file**

Run:
```bash
diff \
  <(grep -oE '_skeleton/[a-z-]+\.md' docs/spec/roadmap.md | sort -u) \
  <(ls docs/superpowers/plans/_skeleton/ | sed 's|^|_skeleton/|' | sort -u)
```
Expected: no output (the two lists match).

- [ ] **Step 3: Every objects.md row now links to roadmap.md where it should**

Run: `grep -c "roadmap.md" docs/spec/objects.md`
Expected: >= 14 (one per status-flipped row).

- [ ] **Step 4: Constitution + user docs agree on version range**

Run: `grep -l "14.*18" docs/CONSTITUTION.md docs/user/installation.md docs/user/configuration.md`
Expected: all three filenames listed.

- [ ] **Step 5: No commit needed if everything checks out**

If any verification step fails, fix and commit with `docs: fix
broken cross-link in <path>`.

---

## End-of-plan verification

- [ ] `git log --oneline -15` shows ~12 new commits covering this plan.
- [ ] `docs/spec/roadmap.md` exists and lists 15 skeleton entries.
- [ ] `docs/superpowers/plans/_skeleton/` contains 15 files.
- [ ] `docs/superpowers/plans/2026-05-26-postgres-18-support.md` exists.
- [ ] CONSTITUTION §6, installation.md, configuration.md, README.md, objects.md all reference PG 18.
- [ ] Working tree has no uncommitted plan/spec changes (subscription
      WIP from before this plan was started is untouched).
