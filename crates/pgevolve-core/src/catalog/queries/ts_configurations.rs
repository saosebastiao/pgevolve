//! `pg_ts_config` + `pg_ts_config_map` catalog queries.
//!
//! Two queries are registered:
//!
//! 1. [`SELECT_TS_CONFIGURATIONS`] — one row per text-search configuration in
//!    the managed schemas, with the parser schema+name resolved via a join to
//!    `pg_ts_parser` and its namespace.
//!
//! 2. [`SELECT_TS_CONFIG_MAPPINGS`] — one row per (config, `token_type`, dict)
//!    triple, ordered by `(config_schema, config_name, token_alias, mapseqno)`.
//!    `ts_token_type(cfgparser)` is called via a `LATERAL` to resolve the
//!    integer `maptokentype` to its human-readable alias (e.g. `word`,
//!    `asciiword`). Dictionaries within a token type are in ascending
//!    `mapseqno` order so the assembler can rely on row order for the
//!    fallback chain.
//!
//! Extension-owned configurations (`pg_depend.deptype = 'e'`) are excluded at
//! the SQL layer. Both queries take `$1::text[]` (managed-schema names).

/// Fetch one row per user-defined text-search configuration in the managed
/// schemas.
///
/// Columns returned:
/// - `schema_name` — configuration schema (`pg_namespace.nspname`)
/// - `name` — configuration name (`pg_ts_config.cfgname`)
/// - `parser_schema` — parser schema (`pg_namespace.nspname` for parser)
/// - `parser_name` — parser name (`pg_ts_parser.prsname`)
/// - `owner` — role name from `pg_get_userbyid(cfgowner)`
/// - `comment` — from `pg_description` (NULL when absent)
///
/// Extension-owned configurations are excluded via a `NOT EXISTS` subquery on
/// `pg_depend`. Takes `$1::text[]` (managed-schema names).
pub const SELECT_TS_CONFIGURATIONS: &str = "\
SELECT \
    n.nspname::text                                              AS schema_name, \
    c.cfgname::text                                              AS name, \
    pn.nspname::text                                             AS parser_schema, \
    p.prsname::text                                              AS parser_name, \
    pg_catalog.pg_get_userbyid(c.cfgowner)::text                 AS owner, \
    de.description::text                                         AS comment \
FROM pg_catalog.pg_ts_config c \
JOIN pg_catalog.pg_namespace n   ON n.oid  = c.cfgnamespace \
JOIN pg_catalog.pg_ts_parser p   ON p.oid  = c.cfgparser \
JOIN pg_catalog.pg_namespace pn  ON pn.oid = p.prsnamespace \
LEFT JOIN pg_catalog.pg_description de \
       ON de.objoid   = c.oid \
      AND de.classoid = 'pg_catalog.pg_ts_config'::regclass \
      AND de.objsubid = 0 \
WHERE n.nspname = ANY($1::text[]) \
  AND NOT EXISTS ( \
      SELECT 1 \
      FROM pg_catalog.pg_depend dep \
      WHERE dep.classid = 'pg_catalog.pg_ts_config'::regclass \
        AND dep.objid   = c.oid \
        AND dep.deptype = 'e' \
  ) \
ORDER BY n.nspname, c.cfgname";

/// Fetch one row per (config, `token_type`, dictionary) triple in the managed
/// schemas, ordered so that dictionaries within each (config, `token_type`) are
/// in ascending `mapseqno` order.
///
/// Columns returned:
/// - `config_schema` — config's schema name
/// - `config_name` — config name
/// - `token_type` — token alias (e.g. `word`, `asciiword`) from `ts_token_type`
/// - `dict_schema` — dictionary schema
/// - `dict_name` — dictionary name
/// - `mapseqno` — sequence number within the mapping (1-based, ascending)
///
/// The `LATERAL ts_token_type(c.cfgparser)` call resolves the integer
/// `maptokentype` to its canonical alias string. Extension-owned configs are
/// excluded by mirroring the same `NOT EXISTS` filter used in
/// [`SELECT_TS_CONFIGURATIONS`]. Takes `$1::text[]` (managed-schema names).
pub const SELECT_TS_CONFIG_MAPPINGS: &str = "\
SELECT \
    n.nspname::text   AS config_schema, \
    c.cfgname::text   AS config_name, \
    tt.alias::text    AS token_type, \
    dn.nspname::text  AS dict_schema, \
    d.dictname::text  AS dict_name, \
    m.mapseqno::bigint AS mapseqno \
FROM pg_catalog.pg_ts_config_map m \
JOIN pg_catalog.pg_ts_config c   ON c.oid   = m.mapcfg \
JOIN pg_catalog.pg_namespace n   ON n.oid   = c.cfgnamespace \
JOIN pg_catalog.pg_ts_dict d     ON d.oid   = m.mapdict \
JOIN pg_catalog.pg_namespace dn  ON dn.oid  = d.dictnamespace \
JOIN LATERAL ts_token_type(c.cfgparser) tt ON tt.tokid = m.maptokentype \
WHERE n.nspname = ANY($1::text[]) \
  AND NOT EXISTS ( \
      SELECT 1 \
      FROM pg_catalog.pg_depend dep \
      WHERE dep.classid = 'pg_catalog.pg_ts_config'::regclass \
        AND dep.objid   = c.oid \
        AND dep.deptype = 'e' \
  ) \
ORDER BY n.nspname, c.cfgname, tt.alias, m.mapseqno";
