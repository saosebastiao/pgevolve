# pgevolve roadmap

This document orders every remaining 🔮 Future / 📋 Planned object kind
in [`objects.md`](./objects.md) into target releases. The ordering
principle is **Postgres dependency order × user impact**: prerequisite
objects ship first; within a dep-respecting slot, the objects that
unblock the most real applications go earlier.

Version numbers may slip; the **order** does not. Each row links to a
plan stub under [`../superpowers/plans/_skeleton/`](../superpowers/plans/_skeleton/);
the stub is promoted to a dated plan when brainstorming begins.

## Shipped

| Released | Object / sub-feature | Plan |
|---|---|---|
| v0.3.4 | `PUBLICATION` | [`2026-05-26-publications.md`](../superpowers/plans/2026-05-26-publications.md) |
| v0.3.5 | `SUBSCRIPTION` | [`2026-05-26-subscriptions.md`](../superpowers/plans/2026-05-26-subscriptions.md) |
| v0.3.6 | PG 18 catalog support | [`2026-05-26-postgres-18-support.md`](../superpowers/plans/2026-05-26-postgres-18-support.md) |
| v0.3.7 | `STATISTICS` + `VIEW ... WITH CHECK OPTION` | [`2026-05-27-statistics-and-check-option.md`](../superpowers/plans/2026-05-27-statistics-and-check-option.md) |
| v0.3.8 | `CREATE COLLATION` + `RANGE TYPE` | [`2026-05-28-collation-and-range-type.md`](../superpowers/plans/2026-05-28-collation-and-range-type.md) |

## Active matrix

| Target | Object / sub-feature | Plan | Notes |
|---|---|---|---|
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
