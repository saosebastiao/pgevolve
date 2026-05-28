//! User-defined type catalog queries — enums, domains, composites.
//!
//! All five queries target PG 14–17 identically; no version-specific SQL is
//! required for the type catalog in v0.2.

/// Enumerate every user-defined type in the managed schemas.
///
/// `typtype` values: `'e'` = enum, `'d'` = domain, `'c'` = composite,
/// `'r'` = range. Auto-generated multirange types (`'m'`) are excluded —
/// they are companions of ranges, materialized implicitly when the range
/// is created.
///
/// Auto-generated row types backing ordinary tables (`relkind != 'c'`) are
/// excluded: a `pg_class` entry backing a composite *type* has `relkind='c'`;
/// one backing a table has `relkind='r'` (etc.). We keep only composites whose
/// `typrelid` references a `relkind='c'` class (or is zero, which domains,
/// enums, and ranges have).
///
/// Range-specific columns come from `pg_range`. When `rngtypid IS NULL`, the
/// type is not a range and the assembler ignores the range-* columns.
pub const SELECT_USER_TYPES: &str = "\
SELECT \
    n.nspname  AS schema_name, \
    t.typname  AS name, \
    t.typtype::text  AS kind, \
    owner_role.rolname AS owner, \
    coalesce(t.typacl::text[], '{}'::text[]) AS acl, \
    obj_description(t.oid, 'pg_type') AS comment, \
    r.rngtypid IS NOT NULL AS is_range, \
    stn.nspname AS rng_subtype_schema, st.typname AS rng_subtype_name, \
    on_.nspname AS rng_subopc_schema, o.opcname AS rng_subopc_name, \
    cn.nspname AS rng_collation_schema, c.collname AS rng_collation_name, \
    cann.nspname AS rng_canonical_schema, canon.proname AS rng_canonical_name, \
    difn.nspname AS rng_subdiff_schema, dif.proname AS rng_subdiff_name, \
    mr.typname AS rng_multirange_name \
FROM pg_type t \
JOIN pg_namespace n ON t.typnamespace = n.oid \
JOIN pg_authid owner_role ON owner_role.oid = t.typowner \
LEFT JOIN pg_range r ON r.rngtypid = t.oid \
LEFT JOIN pg_type st ON st.oid = r.rngsubtype \
LEFT JOIN pg_namespace stn ON stn.oid = st.typnamespace \
LEFT JOIN pg_opclass o ON o.oid = r.rngsubopc \
LEFT JOIN pg_namespace on_ ON on_.oid = o.opcnamespace \
LEFT JOIN pg_collation c ON c.oid = r.rngcollation AND c.oid <> 0 \
LEFT JOIN pg_namespace cn ON cn.oid = c.collnamespace \
LEFT JOIN pg_proc canon ON canon.oid = r.rngcanonical AND canon.oid <> 0 \
LEFT JOIN pg_namespace cann ON cann.oid = canon.pronamespace \
LEFT JOIN pg_proc dif ON dif.oid = r.rngsubdiff AND dif.oid <> 0 \
LEFT JOIN pg_namespace difn ON difn.oid = dif.pronamespace \
LEFT JOIN pg_type mr ON mr.oid = r.rngmultitypid \
WHERE t.typtype IN ('e','d','c','r') \
  AND n.nspname = ANY($1::text[]) \
  AND NOT (t.typtype = 'c' AND EXISTS ( \
      SELECT 1 FROM pg_class c2 \
      WHERE c2.oid = t.typrelid AND c2.relkind <> 'c' \
  )) \
  AND NOT EXISTS ( \
      SELECT 1 \
      FROM pg_catalog.pg_depend dep \
      WHERE dep.classid = 'pg_catalog.pg_type'::regclass \
        AND dep.objid = t.oid \
        AND dep.deptype = 'e' \
  ) \
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
///
/// v0.2 does not model domain collation (`pg_type.typcollation`); both source
/// parser and assembler default `collation: None`. A future sub-spec can add a
/// `pg_collation` join here and a matching column in the source parser to lift
/// that limitation.
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
  AND c.contype = 'c' \
ORDER BY n.nspname, t.typname, c.conname";

/// Attributes (fields) of every composite type in the managed schemas.
///
/// Rows arrive ordered by `attnum` so the assembler can append them directly
/// without a secondary sort. v0.2 does not model attribute collation
/// (`pg_attribute.attcollation`); both source parser and assembler default
/// each attribute's `collation: None`. Lifting this is a future sub-spec.
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
