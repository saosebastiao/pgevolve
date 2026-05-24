# Column types

Every Postgres type family with pgevolve's support status. The IR's
`ColumnType` enum is the source of truth; entries below mirror its
variants and document the gaps.

See [`../README.md`](./README.md) for the status legend.

## Numeric

**Tests (whole section):** tier-1: `crates/pgevolve-core/src/ir/column_type.rs::tests` (variant parse + display); tier-2: `crates/pgevolve-core/tests/fixtures/parser/equivalent_pairs/0001-int-aliases`, `0004-timestamp-tz`; tier-C: `objects/columns/alter-column-type-widening`, `alter-column-type-narrowing`.

| Type | Status | Notes |
|---|---|---|
| `boolean` | ✅ Implemented | change_kinds: [add, change_type] |
| `smallint` (`int2`) | ✅ Implemented | change_kinds: [add, change_type] |
| `integer` (`int`, `int4`) | ✅ Implemented | change_kinds: [add, change_type] |
| `bigint` (`int8`) | ✅ Implemented | change_kinds: [add, change_type] |
| `real` (`float4`) | ✅ Implemented | change_kinds: [add, change_type] |
| `double precision` (`float8`) | ✅ Implemented | change_kinds: [add, change_type] |
| `numeric` (`decimal`) | ✅ Implemented | Including precision and scale (`numeric(p)`, `numeric(p, s)`). Unbounded `numeric` round-trips. change_kinds: [add, change_type] |
| `smallserial` / `serial` / `bigserial` | ✅ Implemented | Desugared at parse time into the underlying integer column + owned sequence; round-trips through introspection by detecting the `nextval(...)` default.<br>**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/desugar_serial.rs::tests`; tier-2: fixture `parser/equivalent_pairs/0002-serial-desugar` |
| `money` | ⛔ Not planned | Locale-dependent representation; discouraged by Postgres docs. Use `numeric` instead. |

## Character

**Tests (whole section):** tier-1: `crates/pgevolve-core/src/ir/column_type.rs::tests`; tier-2: `crates/pgevolve-core/tests/fixtures/parser/equivalent_pairs/0003-varchar-aliases`.

| Type | Status | Notes |
|---|---|---|
| `text` | ✅ Implemented | change_kinds: [add, change_type] |
| `varchar(n)` / `character varying(n)` | ✅ Implemented | change_kinds: [add, change_type] |
| `varchar` (unbounded) | ✅ Implemented | change_kinds: [add, change_type] |
| `char(n)` / `character(n)` | ✅ Implemented | change_kinds: [add, change_type] |
| `char` (unbounded; single-char) | ✅ Implemented | change_kinds: [add, change_type] |
| `name` | ⛔ Not planned | Internal PG type; you should not be using this in application schemas. |
| `"char"` (single-byte; quoted) | ⛔ Not planned | Internal PG type. |

## Binary

**Tests:** tier-1: `crates/pgevolve-core/src/ir/column_type.rs::tests`; tier-3: `crates/pgevolve-core/tests/catalog_round_trip.rs`.

| Type | Status | Notes |
|---|---|---|
| `bytea` | ✅ Implemented | change_kinds: [add, change_type] |

## Date / time

**Tests (whole section):** tier-1: `crates/pgevolve-core/src/ir/column_type.rs::tests`; tier-2: `crates/pgevolve-core/tests/fixtures/parser/equivalent_pairs/0004-timestamp-tz`.

| Type | Status | Notes |
|---|---|---|
| `date` | ✅ Implemented | change_kinds: [add, change_type] |
| `time` | ✅ Implemented | Sub-second precision (`time(p)`); no time zone. change_kinds: [add, change_type] |
| `time with time zone` (`timetz`) | ✅ Implemented | Including `time(p) with time zone`. change_kinds: [add, change_type] |
| `timestamp` | ✅ Implemented | With sub-second precision. change_kinds: [add, change_type] |
| `timestamp with time zone` (`timestamptz`) | ✅ Implemented | The recommended type for most workflows. change_kinds: [add, change_type] |
| `interval` | ✅ Implemented | Including field constraints (`interval year`, `interval day to hour`, etc.) and sub-second precision. change_kinds: [add, change_type] |

## Networking

**Tests:** tier-1: `crates/pgevolve-core/src/ir/column_type.rs::tests`; tier-3: `crates/pgevolve-core/tests/catalog_round_trip.rs`.

| Type | Status | Notes |
|---|---|---|
| `inet` | ✅ Implemented | change_kinds: [add, change_type] |
| `cidr` | ✅ Implemented | change_kinds: [add, change_type] |
| `macaddr` | ✅ Implemented | change_kinds: [add, change_type] |
| `macaddr8` | ✅ Implemented | change_kinds: [add, change_type] |

## UUID, JSON, XML

**Tests:** tier-1: `crates/pgevolve-core/src/ir/column_type.rs::tests`; tier-3: `crates/pgevolve-core/tests/catalog_round_trip.rs`.

| Type | Status | Notes |
|---|---|---|
| `uuid` | ✅ Implemented | change_kinds: [add, change_type] |
| `json` | ✅ Implemented | Lacks a default btree opclass, so the IR generator deliberately avoids indexing `json` columns. change_kinds: [add, change_type] |
| `jsonb` | ✅ Implemented | change_kinds: [add, change_type] |
| `jsonpath` | 🔮 Future | Rarely used as a column type; mostly an expression-level value. |
| `xml` | 🔮 Future | Requires `--with-libxml` Postgres build; less common in modern stacks. |

## Bit strings

**Tests:** tier-1: `crates/pgevolve-core/src/ir/column_type.rs::tests`.

| Type | Status | Notes |
|---|---|---|
| `bit(n)` | ✅ Implemented | Fixed-length. change_kinds: [add, change_type] |
| `bit varying(n)` (`varbit`) | ✅ Implemented | Variable-length. change_kinds: [add, change_type] |

## Arrays

**Tests:** tier-1: `crates/pgevolve-core/src/ir/column_type.rs::tests`.

| Type | Status | Notes |
|---|---|---|
| `<element>[]` (single-dimension) | ✅ Implemented | Element type and dimension count modeled in IR. change_kinds: [add, change_type] |
| Multi-dimensional arrays (`int[][]`) | 🟡 Partial | The IR carries `dims: u8` so multi-dimensional declarations parse, but the differ treats all dimensions ≥ 1 identically (Postgres itself does not enforce dimension counts). change_kinds: [add, change_type] |
| Array element constraints | 🔮 Future | E.g., `CHECK (array_length(col, 1) = 3)`. Modeled as a CHECK constraint, not as an array attribute. |

## Range and multirange types

| Type | Status | Notes |
|---|---|---|
| Built-in range types (`int4range`, `int8range`, `numrange`, `tsrange`, `tstzrange`, `daterange`) | 🔮 Future | Lands with user-defined range types. |
| Built-in multirange types (`int4multirange`, etc.) | 🔮 Future | Lands with range types. |
| User-defined range types | 🔮 Future | Depends on `CREATE TYPE ... AS RANGE` (see [`objects.md`](./objects.md)). |

## Geometric

| Type | Status | Notes |
|---|---|---|
| `point`, `line`, `lseg`, `box`, `path`, `polygon`, `circle` | 🔮 Future | Niche; lands when there is concrete user demand. |

## Text search

| Type | Status | Notes |
|---|---|---|
| `tsvector` | 🔮 Future | Useful as a `STORED` generated column; lands with the broader text-search story. |
| `tsquery` | 🔮 Future | Mostly an expression-level type. |

## Object identifier types

| Type | Status | Notes |
|---|---|---|
| `oid`, `regclass`, `regtype`, `regnamespace`, etc. | ⛔ Not planned | These are catalog-internal types; user schemas should not depend on them. Catalogs returning `regclass` are converted to qualified-name strings at introspection time. |

## User-defined types

**Tests:** tier-1: `crates/pgevolve-core/src/ir/user_type.rs::tests`; tier-C: `objects/enums/`, `objects/domains/`, `objects/composites/` (see [`objects.md`](./objects.md) for fixture list).

| Type | Status | Notes |
|---|---|---|
| Enum (`CREATE TYPE ... AS ENUM`) | 📋 Planned, v0.2 | A column typed as an enum is modeled as `ColumnType::UserDefined(qname)` today; v0.2 adds first-class enum diff (including `ALTER TYPE ... ADD VALUE`). |
| Composite (`CREATE TYPE ... AS (...)`) | 📋 Planned, v0.2 | Same as enum: typed columns work today as `UserDefined`; v0.2 adds first-class composite diff. |
| Domain (`CREATE DOMAIN`) | 📋 Planned, v0.2 | First-class diff including `NOT NULL`, `CHECK`, default. |
| Range (`CREATE TYPE ... AS RANGE`) | 🔮 Future | Lands with range columns. |
| Base type (`CREATE TYPE ... ( INPUT = c_func, OUTPUT = c_func )`) | ⛔ Not planned | Requires C-language functions. |

## Catch-all fallback

| Variant | Status | Notes |
|---|---|---|
| `ColumnType::Other { raw: String }` | ✅ Implemented | Any type pgevolve doesn't recognize is preserved as a raw string. Two `Other` types compare equal iff their strings match — pgevolve makes no claim about semantic equivalence. Lets the system parse unknown types without aborting, which is essential for adopting an existing DB.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/column_type.rs::tests` |
| `ColumnType::UserDefined(QualifiedName)` | ✅ Implemented | Schema-qualified reference to a user-defined type. The IR doesn't introspect the type's structure in v0.1 — that lands with first-class custom types in v0.2.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/column_type.rs::tests`, `user_type.rs::tests` |

## Type-level attributes

| Attribute | Status | Notes |
|---|---|---|
| `NOT NULL` | ✅ Implemented | Column-level; not a constraint. The `SET NOT NULL via CHECK pattern` rewrite avoids long locks (see [`pipeline.md`](./pipeline.md)).<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/tests::set_not_null_on_existing_column_emits_four_steps`, `crates/pgevolve-core/src/diff/columns.rs::tests`; tier-2: `parser/equivalent_pairs/0007-not-null-via-pk` |
| `DEFAULT <literal>` | ✅ Implemented | Booleans, integers, floats, text, bytea, NULL.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/default_expr.rs::tests`; tier-C: `objects/columns/set-default`, `drop-default` |
| `DEFAULT <sequence>` | ✅ Implemented | `nextval('seq')` recognized; canonicalized at parse + introspect time.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/default_expr.rs::tests`; tier-C: `objects/columns/add-column-with-default` |
| `DEFAULT <expression>` | ✅ Implemented | Any other expression preserved as canonical text (lowercased keywords, sorted commutative operands, paren-folded).<br>**Tests:** tier-1: `crates/pgevolve-core/src/parse/normalize_expr.rs::tests`; tier-2: `parser/equivalent_pairs/0005-default-cast-strip` |
| `COLLATE <collation>` | ✅ Implemented | Per-column collation. `pg_catalog.default` is treated as "no collation" so it doesn't appear as drift.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs::tests` |
| `GENERATED ALWAYS AS IDENTITY` / `GENERATED BY DEFAULT AS IDENTITY` | ✅ Implemented | Including sequence option overrides (`START`, `INCREMENT`, `MINVALUE`, `MAXVALUE`, `CACHE`, `CYCLE`).<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/sequence.rs::tests`, `parse/builder/create_stmt.rs::tests` |
| `GENERATED ALWAYS AS (expr) STORED` (computed columns) | ✅ Implemented | Stored generated columns.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/column.rs::tests`; tier-C: `objects/columns/add-generated-column` |
| `GENERATED ALWAYS AS (expr) VIRTUAL` | ⛔ Not planned | Postgres only supports `STORED` through at least PG 17; this row will move to ✅ Implemented if/when Postgres adds it. |
| Per-column comments | ✅ Implemented | **Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/comment_stmt.rs::tests`; tier-C: `objects/tables/comment-on-column` |
