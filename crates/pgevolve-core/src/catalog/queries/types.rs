//! User-defined type catalog queries — enums, domains, composites.
//!
//! All five queries target PG 14–17 identically; no version-specific SQL is
//! required for the type catalog in v0.2.

/// Enumerate every user-defined type in the managed schemas.
///
/// `typtype` values: `'e'` = enum, `'d'` = domain, `'c'` = composite.
/// Auto-generated row types backing ordinary tables (`relkind != 'c'`) are
/// excluded: a `pg_class` entry backing a composite *type* has `relkind='c'`;
/// one backing a table has `relkind='r'` (etc.). We keep only composites whose
/// `typrelid` references a `relkind='c'` class (or is zero, which domains and
/// enums have).
pub const SELECT_USER_TYPES: &str = "\
SELECT \
    n.nspname  AS schema_name, \
    t.typname  AS name, \
    t.typtype::text  AS kind, \
    obj_description(t.oid, 'pg_type') AS comment \
FROM pg_type t \
JOIN pg_namespace n ON t.typnamespace = n.oid \
WHERE t.typtype IN ('e','d','c') \
  AND n.nspname = ANY($1::text[]) \
  AND NOT (t.typtype = 'c' AND EXISTS ( \
      SELECT 1 FROM pg_class c \
      WHERE c.oid = t.typrelid AND c.relkind <> 'c' \
  )) \
ORDER BY n.nspname, t.typname";

/// Enum labels for every enum type in the managed schemas.
///
/// The `sort_order` column is cast to `text` so that it decodes through the
/// existing [`crate::catalog::rows::Value::Text`] variant; the assembler
/// parses it back to `f32`.
pub const SELECT_ENUM_VALUES: &str = "\
SELECT \
    n.nspname            AS schema_name, \
    t.typname            AS type_name, \
    e.enumlabel          AS value_name, \
    e.enumsortorder::text AS sort_order \
FROM pg_enum e \
JOIN pg_type t ON e.enumtypid = t.oid \
JOIN pg_namespace n ON t.typnamespace = n.oid \
WHERE n.nspname = ANY($1::text[]) \
ORDER BY n.nspname, t.typname, e.enumsortorder";

/// Base-type and nullability metadata for every domain type.
pub const SELECT_DOMAIN_DETAILS: &str = "\
SELECT \
    n.nspname                                AS schema_name, \
    t.typname                                AS name, \
    format_type(t.typbasetype, t.typtypmod)  AS base_type, \
    t.typnotnull                             AS not_null, \
    t.typdefault                             AS default_expr \
FROM pg_type t \
JOIN pg_namespace n ON t.typnamespace = n.oid \
WHERE t.typtype = 'd' \
  AND n.nspname = ANY($1::text[])";

/// Named CHECK constraints attached to domain types.
///
/// `pg_get_constraintdef(oid, true)` returns `CHECK (<expr>)` (or sometimes
/// `CHECK ((<expr>))` with an extra paren layer). The assembler strips the
/// outer `CHECK (…)` wrapper before normalization.
pub const SELECT_DOMAIN_CHECKS: &str = "\
SELECT \
    n.nspname                          AS schema_name, \
    t.typname                          AS type_name, \
    c.conname                          AS constraint_name, \
    pg_get_constraintdef(c.oid, true)  AS expression \
FROM pg_constraint c \
JOIN pg_type t ON c.contypid = t.oid \
JOIN pg_namespace n ON t.typnamespace = n.oid \
WHERE t.typtype = 'd' \
  AND n.nspname = ANY($1::text[]) \
ORDER BY n.nspname, t.typname, c.conname";

/// Attributes (fields) of every composite type in the managed schemas.
///
/// Rows arrive ordered by `attnum` so the assembler can append them directly
/// without a secondary sort.
pub const SELECT_COMPOSITE_ATTRIBUTES: &str = "\
SELECT \
    n.nspname                            AS schema_name, \
    t.typname                            AS type_name, \
    a.attname                            AS attribute_name, \
    format_type(a.atttypid, a.atttypmod) AS attribute_type, \
    a.attnum                             AS attnum \
FROM pg_attribute a \
JOIN pg_class c ON a.attrelid = c.oid \
JOIN pg_type t ON c.reltype = t.oid \
JOIN pg_namespace n ON t.typnamespace = n.oid \
WHERE t.typtype = 'c' \
  AND c.relkind = 'c' \
  AND a.attnum > 0 \
  AND NOT a.attisdropped \
  AND n.nspname = ANY($1::text[]) \
ORDER BY n.nspname, t.typname, a.attnum";
