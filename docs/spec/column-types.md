# Column types

Every Postgres type family with pgevolve's support status. The IR's
`ColumnType` enum is the source of truth; entries below mirror its
variants and document the gaps.

See [`../README.md`](./README.md) for the status legend.

## Numeric

| Type | Status | Notes |
|---|---|---|
| `boolean` | ✅ Implemented | |
| `smallint` (`int2`) | ✅ Implemented | |
| `integer` (`int`, `int4`) | ✅ Implemented | |
| `bigint` (`int8`) | ✅ Implemented | |
| `real` (`float4`) | ✅ Implemented | |
| `double precision` (`float8`) | ✅ Implemented | |
| `numeric` (`decimal`) | ✅ Implemented | Including precision and scale (`numeric(p)`, `numeric(p, s)`). Unbounded `numeric` round-trips. |
| `smallserial` / `serial` / `bigserial` | ✅ Implemented | Desugared at parse time into the underlying integer column + owned sequence; round-trips through introspection by detecting the `nextval(...)` default. |
| `money` | ⛔ Not planned | Locale-dependent representation; discouraged by Postgres docs. Use `numeric` instead. |

## Character

| Type | Status | Notes |
|---|---|---|
| `text` | ✅ Implemented | |
| `varchar(n)` / `character varying(n)` | ✅ Implemented | |
| `varchar` (unbounded) | ✅ Implemented | |
| `char(n)` / `character(n)` | ✅ Implemented | |
| `char` (unbounded; single-char) | ✅ Implemented | |
| `name` | ⛔ Not planned | Internal PG type; you should not be using this in application schemas. |
| `"char"` (single-byte; quoted) | ⛔ Not planned | Internal PG type. |

## Binary

| Type | Status | Notes |
|---|---|---|
| `bytea` | ✅ Implemented | |

## Date / time

| Type | Status | Notes |
|---|---|---|
| `date` | ✅ Implemented | |
| `time` | ✅ Implemented | Sub-second precision (`time(p)`); no time zone. |
| `time with time zone` (`timetz`) | ✅ Implemented | Including `time(p) with time zone`. |
| `timestamp` | ✅ Implemented | With sub-second precision. |
| `timestamp with time zone` (`timestamptz`) | ✅ Implemented | The recommended type for most workflows. |
| `interval` | ✅ Implemented | Including field constraints (`interval year`, `interval day to hour`, etc.) and sub-second precision. |

## Networking

| Type | Status | Notes |
|---|---|---|
| `inet` | ✅ Implemented | |
| `cidr` | ✅ Implemented | |
| `macaddr` | ✅ Implemented | |
| `macaddr8` | ✅ Implemented | |

## UUID, JSON, XML

| Type | Status | Notes |
|---|---|---|
| `uuid` | ✅ Implemented | |
| `json` | ✅ Implemented | Lacks a default btree opclass, so the IR generator deliberately avoids indexing `json` columns. |
| `jsonb` | ✅ Implemented | |
| `jsonpath` | 🔮 Future | Rarely used as a column type; mostly an expression-level value. |
| `xml` | 🔮 Future | Requires `--with-libxml` Postgres build; less common in modern stacks. |

## Bit strings

| Type | Status | Notes |
|---|---|---|
| `bit(n)` | ✅ Implemented | Fixed-length. |
| `bit varying(n)` (`varbit`) | ✅ Implemented | Variable-length. |

## Arrays

| Type | Status | Notes |
|---|---|---|
| `<element>[]` (single-dimension) | ✅ Implemented | Element type and dimension count modeled in IR. |
| Multi-dimensional arrays (`int[][]`) | 🟡 Partial | The IR carries `dims: u8` so multi-dimensional declarations parse, but the differ treats all dimensions ≥ 1 identically (Postgres itself does not enforce dimension counts). |
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
| `ColumnType::Other { raw: String }` | ✅ Implemented | Any type pgevolve doesn't recognize is preserved as a raw string. Two `Other` types compare equal iff their strings match — pgevolve makes no claim about semantic equivalence. Lets the system parse unknown types without aborting, which is essential for adopting an existing DB. |
| `ColumnType::UserDefined(QualifiedName)` | ✅ Implemented | Schema-qualified reference to a user-defined type. The IR doesn't introspect the type's structure in v0.1 — that lands with first-class custom types in v0.2. |

## Type-level attributes

| Attribute | Status | Notes |
|---|---|---|
| `NOT NULL` | ✅ Implemented | Column-level; not a constraint. |
| `DEFAULT <literal>` | ✅ Implemented | Booleans, integers, floats, text, bytea, NULL. |
| `DEFAULT <sequence>` | ✅ Implemented | `nextval('seq')` recognized; canonicalized at parse + introspect time. |
| `DEFAULT <expression>` | ✅ Implemented | Any other expression preserved as canonical text (lowercased keywords, sorted commutative operands, paren-folded). |
| `COLLATE <collation>` | ✅ Implemented | Per-column collation. `pg_catalog.default` is treated as "no collation" so it doesn't appear as drift. |
| `GENERATED ALWAYS AS IDENTITY` / `GENERATED BY DEFAULT AS IDENTITY` | ✅ Implemented | Including sequence option overrides (`START`, `INCREMENT`, `MINVALUE`, `MAXVALUE`, `CACHE`, `CYCLE`). |
| `GENERATED ALWAYS AS (expr) STORED` (computed columns) | ✅ Implemented | Stored generated columns. |
| `GENERATED ALWAYS AS (expr) VIRTUAL` | ⛔ Not planned | Postgres only supports `STORED` through at least PG 17; this row will move to ✅ Implemented if/when Postgres adds it. |
| Per-column comments | ✅ Implemented | |
