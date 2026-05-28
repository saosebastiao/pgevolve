# Statistics

pgevolve models `CREATE STATISTICS` as a first-class declarative IR object.
A statistic is a schema-qualified object (`pg_statistic_ext`) that captures
cross-column correlations for the Postgres query planner.

## Source surface

Four syntactic forms are supported:

```sql
-- Form 1: basic — all three kinds enabled (pg default)
CREATE STATISTICS app.orders_s ON (status, region) FROM app.orders;

-- Form 2: explicit kinds — only the requested kinds
CREATE STATISTICS app.orders_dep ON (status, region) FROM app.orders
    (dependencies);

-- Form 3: expression statistic (PG 14+)
CREATE STATISTICS app.orders_expr ON (lower(region), status) FROM app.orders;

-- Form 4: mixed columns + expression, explicit kinds
CREATE STATISTICS app.orders_mixed ON (status, lower(region)) FROM app.orders
    (ndistinct, mcv);
```

The kinds clause accepts any non-empty subset of:
- `ndistinct` — multi-column distinct-value counts
- `dependencies` — functional dependencies between columns
- `mcv` — most-common-value lists per column combination

Omitting the kinds clause enables all three (Postgres default).

**Explicit names required.** The anonymous form (`CREATE STATISTICS ON (...) FROM t`)
is rejected at parse time, mirroring the no-anonymous-indexes policy. Every
statistic managed by pgevolve must carry a schema-qualified name.

## Semantics — lenient at the statistic grain

pgevolve applies leniency at the **whole-statistic** level:

| source | catalog | differ action |
|---|---|---|
| Statistic absent | Statistic absent | no-op |
| Statistic absent | Statistic present | **no-op** — surfaced as `unmanaged-statistic` lint warning |
| Statistic present | Statistic absent | `CREATE STATISTICS` |
| Statistic present, same | Statistic present, same | no-op |
| Statistic present, `statistics_target` differs | Statistic present, differs | `ALTER STATISTICS … SET STATISTICS n` (granular cheap path) |
| Statistic present, any other field differs | Statistic present, differs | `DROP STATISTICS` + `CREATE STATISTICS` (ReplaceStatistic) |

A statistic present in source is fully managed. A statistic absent from source
is left alone and surfaced via lint.

## Granular differ — two paths

Postgres has no `ALTER STATISTICS` for column lists or kinds (it would require
rebuilding the statistics object anyway). pgevolve therefore emits:

- **`AlterStatisticSetTarget`** (`ALTER STATISTICS name SET STATISTICS n`) when
  only `statistics_target` changed. This is cheap and non-destructive.
- **`ReplaceStatistic`** (`DROP STATISTICS` + `CREATE STATISTICS`) for any other
  structural change (columns, kinds, target table rename). This is destructive
  and requires intent approval.

## 5 StepKind variants

| Step kind | SQL emitted |
|---|---|
| `CreateStatistic` | `CREATE STATISTICS …` |
| `DropStatistic` | `DROP STATISTICS name` (destructive; intent required) |
| `ReplaceStatistic` | `DROP STATISTICS name` + `CREATE STATISTICS …` (structural change; destructive) |
| `AlterStatisticSetTarget` | `ALTER STATISTICS name SET STATISTICS n` |
| `CommentOnStatistic` | `COMMENT ON STATISTICS name IS '…'` |

## 1 lint rule

| Rule | Severity | Condition | Waivable? |
|---|---|---|---|
| `unmanaged-statistic` | Warning | A statistic is in the catalog but not in source | Yes |

**Tests:** tier-1: `crates/pgevolve-core/src/lint/rules/unmanaged_statistic.rs::tests`;
tier-C: `objects/statistics/lint-unmanaged`.

## Out of scope

- **Anonymous form** (`CREATE STATISTICS ON (...) FROM t`) — rejected at parse time.
  Explicit names are required so pgevolve can track identity across migrations.
- **`INCLUDE` clause (PG 18+)** — not yet modeled. Deferred to a future patch.
- **`ALTER STATISTICS … RENAME TO`** — not supported. Rename is treated as
  Drop + Create (old name disappears, new name appears).
- **`GRANT` on statistics** — Postgres does not support object-level grants on
  `pg_statistic_ext` objects. Out of scope by PG design.

## Catalog reader

Statistics are read from `pg_statistic_ext` joined with `pg_namespace`:

- `stxname`, `stxnamespace` — name + schema.
- `stxrelid` — target table OID (resolved to `QualifiedName` via `pg_class`).
- `stxkind` — char[] with `'d'` (dependencies), `'f'` (ndistinct), `'m'` (mcv).
- `stxkeys` — int2vector of column attribute numbers (resolved to `Identifier`
  via `pg_attribute`).
- Expression statistics (PG 14+): `pg_get_statisticsobjdef_expressions(oid)`
  returns a text[] of expression SQL; each entry is parsed + canonicalized
  via `NormalizedExpr`.
- `stxstattarget` — the `statistics_target` override; `-1` maps to `None`.
- `pg_description` — `COMMENT ON STATISTICS`.

**Tests:** tier-2: `crates/pgevolve-core/tests/statistics_round_trip.rs`;
tier-C: `objects/statistics/`.

## Conformance fixtures

9 fixtures under `crates/pgevolve-conformance/tests/cases/objects/statistics/`:

| Fixture | Covers |
|---|---|
| `create-simple` | Basic two-column, all-kinds statistic |
| `create-explicit-kinds` | `(ndistinct)` only |
| `create-expression` | PG 14+ expression column |
| `alter-set-target` | `AlterStatisticSetTarget` cheap path |
| `replace-columns` | Structural change → ReplaceStatistic |
| `replace-kinds` | Kinds change → ReplaceStatistic |
| `drop-simple` | `DropStatistic` |
| `comment-on` | `COMMENT ON STATISTICS` |
| `lint-unmanaged` | `unmanaged-statistic` warning |
