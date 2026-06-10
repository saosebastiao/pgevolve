# pgevolve roadmap

This document orders every remaining 🔮 Future / 📋 Planned object kind
in [`objects.md`](./objects.md) into target releases. The ordering
principle is **Postgres dependency order × user impact**: prerequisite
objects ship first; within a dep-respecting slot, the objects that
unblock the most real applications go earlier.

Version numbers may slip; the **order** does not. Each row links to a
plan stub under [`../superpowers/plans/_skeleton/`](../superpowers/plans/_skeleton/);
the stub is promoted to a dated plan when brainstorming begins.

**See also [`../v1.md`](../v1.md)** — the v1.0 charter defines the
gate that triggers the 0.x → 1.0 cut, the stability commitments, and
the quality bar. The roadmap below is the slotted feature schedule;
the charter is the meaning of "done".

## Shipped

| Released | Object / sub-feature | Plan |
|---|---|---|
| v0.3.4 | `PUBLICATION` | [`2026-05-26-publications.md`](../superpowers/plans/2026-05-26-publications.md) |
| v0.3.5 | `SUBSCRIPTION` | [`2026-05-26-subscriptions.md`](../superpowers/plans/2026-05-26-subscriptions.md) |
| v0.3.6 | PG 18 catalog support | [`2026-05-26-postgres-18-support.md`](../superpowers/plans/2026-05-26-postgres-18-support.md) |
| v0.3.7 | `STATISTICS` + `VIEW ... WITH CHECK OPTION` | [`2026-05-27-statistics-and-check-option.md`](../superpowers/plans/2026-05-27-statistics-and-check-option.md) |
| v0.3.8 | `CREATE COLLATION` + `RANGE TYPE` | [`2026-05-28-collation-and-range-type.md`](../superpowers/plans/2026-05-28-collation-and-range-type.md) |
| v0.4.0 | `EVENT TRIGGER` | [`2026-06-04-event-trigger.md`](../superpowers/plans/2026-06-04-event-trigger.md) |
| v0.4.0 | `TABLESPACE` (cluster object) | [`2026-06-05-tablespace.md`](../superpowers/plans/2026-06-05-tablespace.md) |
| v0.4.0 | `TABLE ... USING <access method>` | [`2026-06-06-table-access-method.md`](../superpowers/plans/2026-06-06-table-access-method.md) |
| v0.4.1 | `AGGREGATE` (ordinary: SFUNC + STYPE) | [`2026-06-06-aggregate.md`](../superpowers/plans/2026-06-06-aggregate.md) |
| v0.4.2 | `CAST` | [`2026-06-07-cast.md`](../superpowers/plans/2026-06-07-cast.md) |
| v0.4.3 | Per-partition `TABLESPACE` | [`2026-06-08-table-tablespace.md`](../superpowers/plans/2026-06-08-table-tablespace.md) |
| v0.4.3 | `TEXT SEARCH` family (`DICTIONARY` + `CONFIGURATION`) | [`2026-06-08-text-search.md`](../superpowers/plans/2026-06-08-text-search.md) |
| unreleased | Recursive views (`WITH RECURSIVE`) — already supported (verified PG 14–18); conformance coverage added, no code change needed | [`2026-06-10-recursive-views-design.md`](../superpowers/specs/2026-06-10-recursive-views-design.md) |

## Active matrix

**The 1.0 cut happens when this matrix is empty.** See
[`../v1.md`](../v1.md) §4 for the full v1.0 feature checklist (this
matrix is the source of truth; the charter restates it).

| Target | Object / sub-feature | Plan | Notes |
|---|---|---|---|
| blocked | PG 18 virtual generated columns | [`2026-06-07-virtual-generated-columns-design.md`](../superpowers/specs/2026-06-07-virtual-generated-columns-design.md) | Design complete; **blocked upstream**. The `pg_query` crate (latest 6.1.1) wraps libpg_query 17 and rejects `VIRTUAL` syntax; libpg_query C has an `18.0.0` tag but no Rust release wraps it, and `cargo publish` forbids git deps. Unblock by bumping `pg_query` once a PG-18 crates.io release lands, then proceed to writing-plans. |
| v0.4.2 | PL-language wiring → non-SQL `FUNCTION` bodies | [`_skeleton/pl-language-wiring.md`](../superpowers/plans/_skeleton/pl-language-wiring.md) | Enables PL/Python, PL/Perl, etc. Depends on: `CREATE EXTENSION` (shipped v0.2.x) for the language extension. |
| v0.5.0 | FDW family | [`_skeleton/fdw-family.md`](../superpowers/plans/_skeleton/fdw-family.md) | `FDW`, `SERVER`, `USER MAPPING`, `FOREIGN TABLE`, `IMPORT FOREIGN SCHEMA`; includes secrets handling. Internal slot order within v0.5.0: FDW → SERVER → USER MAPPING → FOREIGN TABLE → IMPORT FOREIGN SCHEMA. |
| v0.5.1 | `OPERATOR` / `OPERATOR CLASS` / `OPERATOR FAMILY` | [`_skeleton/operator-family.md`](../superpowers/plans/_skeleton/operator-family.md) | Heavy admin surface. Depends on: functions + custom types (both shipped v0.2.x). |

## Future (no version commitment)

| Object / feature | Why deferred |
|---|---|
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
