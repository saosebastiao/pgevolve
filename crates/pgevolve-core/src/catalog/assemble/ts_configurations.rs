//! Assemble `pg_ts_config` + `pg_ts_config_map` rows into
//! `Vec<TsConfiguration>`.
//!
//! Two result sets are consumed:
//!
//! 1. **Config rows** (from [`CatalogQuery::TsConfigurations`]): one row per
//!    configuration, carrying the parser, owner, and comment. These are decoded
//!    into `TsConfiguration` values with an empty `mappings` vec.
//!
//! 2. **Mapping rows** (from [`CatalogQuery::TsConfigMappings`]): one row per
//!    (config, `token_type`, dictionary) triple, ordered by
//!    `(config_schema, config_name, token_alias, mapseqno)`. The assembler
//!    groups them by config qname and then by `token_type`, preserving `mapseqno`
//!    order (which is the order of the dictionary fallback chain). The resulting
//!    `Vec<TsMapping>` is attached to the matching `TsConfiguration`.
//!
//! A configuration with no mapping rows produces an empty `mappings` vec.

// `CatalogError` embeds `IrError` and `ParseError`, both large. Boxing them
// would add indirection noise without benefit — these are cold-path catalog
// reads, not hot loops.
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::text_search::{TsConfiguration, TsMapping};

const Q_CFG: CatalogQuery = CatalogQuery::TsConfigurations;
const Q_MAP: CatalogQuery = CatalogQuery::TsConfigMappings;

/// Decode all `pg_ts_config` rows and attach mappings from `pg_ts_config_map`
/// rows.
///
/// `config_rows` contains one row per configuration (from
/// [`CatalogQuery::TsConfigurations`]).
/// `mapping_rows` contains one row per (config, `token_type`, dictionary) triple
/// (from [`CatalogQuery::TsConfigMappings`]), ordered by
/// `(config_schema, config_name, token_alias, mapseqno)`.
pub(super) fn assemble_ts_configurations(
    config_rows: &[Row],
    mapping_rows: &[Row],
) -> Result<Vec<TsConfiguration>, CatalogError> {
    // --- Pass 1: decode config rows into TsConfiguration (empty mappings). ---
    let mut configs: Vec<TsConfiguration> = Vec::with_capacity(config_rows.len());
    for row in config_rows {
        configs.push(decode_config_row(row)?);
    }

    // --- Pass 2: group mapping rows by config qname, then by token_type. ---
    //
    // The SQL ORDER BY guarantees: for a given (config_schema, config_name,
    // token_alias), rows arrive in ascending mapseqno order. We rely on that
    // ordering for the dictionary chain.
    //
    // Key: config qname canonical string → BTreeMap<token_type → Vec<dict qname>>
    // BTreeMap for token_type so the resulting TsMapping vec is deterministically
    // ordered (alphabetical by token_type alias, matching the SQL ORDER BY).
    let mut mappings_by_cfg: BTreeMap<String, BTreeMap<String, Vec<QualifiedName>>> =
        BTreeMap::new();

    for row in mapping_rows {
        let cfg_schema = row.get_text(Q_MAP, "config_schema")?;
        let cfg_name = row.get_text(Q_MAP, "config_name")?;
        let token_type = row.get_text(Q_MAP, "token_type")?;
        let dict_schema = row.get_text(Q_MAP, "dict_schema")?;
        let dict_name = row.get_text(Q_MAP, "dict_name")?;

        let cfg_key = format!("{cfg_schema}.{cfg_name}");
        let dict = qname_from_strings(&dict_schema, "dict_schema", &dict_name, "dict_name")?;

        mappings_by_cfg
            .entry(cfg_key)
            .or_default()
            .entry(token_type)
            .or_default()
            .push(dict);
    }

    // --- Pass 3: attach mappings to their configuration. ---
    for cfg in &mut configs {
        let cfg_key = format!("{}.{}", cfg.qname.schema.as_str(), cfg.qname.name.as_str());
        if let Some(token_map) = mappings_by_cfg.remove(&cfg_key) {
            cfg.mappings = token_map
                .into_iter()
                .map(|(token_type, dictionaries)| TsMapping {
                    token_type,
                    dictionaries,
                })
                .collect();
        }
    }

    Ok(configs)
}

/// Decode a single `pg_ts_config` row into a [`TsConfiguration`].
fn decode_config_row(row: &Row) -> Result<TsConfiguration, CatalogError> {
    let schema_name = row.get_text(Q_CFG, "schema_name")?;
    let name = row.get_text(Q_CFG, "name")?;
    let qname = QualifiedName::new(ident(&schema_name, "schema_name")?, ident(&name, "name")?);

    let parser_schema = row.get_text(Q_CFG, "parser_schema")?;
    let parser_name = row.get_text(Q_CFG, "parser_name")?;
    let parser = QualifiedName::new(
        ident(&parser_schema, "parser_schema")?,
        ident(&parser_name, "parser_name")?,
    );

    let owner_str = row.get_text(Q_CFG, "owner")?;
    let owner = if owner_str.is_empty() {
        None
    } else {
        Some(ident(&owner_str, "owner")?)
    };

    let comment = match row.get_opt_text(Q_CFG, "comment")? {
        Some(s) if !s.is_empty() => Some(s),
        _ => None,
    };

    Ok(TsConfiguration {
        qname,
        parser,
        mappings: Vec::new(),
        owner,
        comment,
    })
}

/// Parse a raw string as an unquoted identifier, mapping the error to
/// [`CatalogError`].
fn ident(s: &str, column: &str) -> Result<Identifier, CatalogError> {
    Identifier::from_unquoted(s).map_err(|e| CatalogError::BadColumnType {
        query: Q_CFG,
        column: column.to_string(),
        message: format!("invalid identifier {s:?}: {e}"),
    })
}

/// Build a [`QualifiedName`] from two raw strings, mapping errors to
/// [`CatalogError`].
fn qname_from_strings(
    schema: &str,
    schema_col: &str,
    name: &str,
    name_col: &str,
) -> Result<QualifiedName, CatalogError> {
    let s = Identifier::from_unquoted(schema).map_err(|e| CatalogError::BadColumnType {
        query: Q_MAP,
        column: schema_col.to_string(),
        message: format!("invalid identifier {schema:?}: {e}"),
    })?;
    let n = Identifier::from_unquoted(name).map_err(|e| CatalogError::BadColumnType {
        query: Q_MAP,
        column: name_col.to_string(),
        message: format!("invalid identifier {name:?}: {e}"),
    })?;
    Ok(QualifiedName::new(s, n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::Value;

    /// Build a minimal valid configuration row.
    fn config_row(schema: &str, name: &str, parser_schema: &str, parser_name: &str) -> Row {
        Row::new()
            .with("schema_name", Value::Text(schema.to_string()))
            .with("name", Value::Text(name.to_string()))
            .with("parser_schema", Value::Text(parser_schema.to_string()))
            .with("parser_name", Value::Text(parser_name.to_string()))
            .with("owner", Value::Text("app_owner".to_string()))
            .with("comment", Value::Null)
    }

    /// Build a mapping row: (`config_schema`, `config_name`, `token_type`, `dict_schema`,
    /// `dict_name`, `mapseqno`).
    fn mapping_row(
        cfg_schema: &str,
        cfg_name: &str,
        token_type: &str,
        dict_schema: &str,
        dict_name: &str,
        seqno: i64,
    ) -> Row {
        Row::new()
            .with("config_schema", Value::Text(cfg_schema.to_string()))
            .with("config_name", Value::Text(cfg_name.to_string()))
            .with("token_type", Value::Text(token_type.to_string()))
            .with("dict_schema", Value::Text(dict_schema.to_string()))
            .with("dict_name", Value::Text(dict_name.to_string()))
            .with("mapseqno", Value::Integer(seqno))
    }

    // ---- Basic config decode ----

    #[test]
    fn decode_config_with_parser_owner_comment() {
        let mut row = config_row("app", "english_cfg", "pg_catalog", "default");
        row.insert(
            "comment",
            Value::Text("English full-text search".to_string()),
        );

        let cfgs = assemble_ts_configurations(&[row], &[]).unwrap();
        assert_eq!(cfgs.len(), 1);
        let c = &cfgs[0];
        assert_eq!(c.qname.to_string(), "app.english_cfg");
        assert_eq!(c.parser.schema.as_str(), "pg_catalog");
        assert_eq!(c.parser.name.as_str(), "default");
        assert_eq!(c.owner.as_ref().map(Identifier::as_str), Some("app_owner"));
        assert_eq!(c.comment.as_deref(), Some("English full-text search"));
        assert!(c.mappings.is_empty());
    }

    #[test]
    fn decode_config_null_owner_yields_none() {
        let mut row = config_row("app", "cfg", "pg_catalog", "default");
        row.insert("owner", Value::Text(String::new()));
        let cfgs = assemble_ts_configurations(&[row], &[]).unwrap();
        assert!(cfgs[0].owner.is_none());
    }

    #[test]
    fn decode_config_null_comment_yields_none() {
        let row = config_row("app", "cfg", "pg_catalog", "default");
        let cfgs = assemble_ts_configurations(&[row], &[]).unwrap();
        assert!(cfgs[0].comment.is_none());
    }

    // ---- Config with no mappings → empty mappings vec ----

    #[test]
    fn config_with_no_mapping_rows_has_empty_mappings() {
        let row = config_row("app", "minimal_cfg", "pg_catalog", "default");
        let cfgs = assemble_ts_configurations(&[row], &[]).unwrap();
        assert_eq!(cfgs.len(), 1);
        assert!(cfgs[0].mappings.is_empty());
    }

    // ---- Single token type with two dicts: seqno order preserved ----

    #[test]
    fn single_token_type_two_dicts_seqno_order_preserved() {
        let cfg_row = config_row("app", "english_cfg", "pg_catalog", "default");
        // seqno=1: english_stem, seqno=2: simple — row order already correct
        let map_rows = vec![
            mapping_row("app", "english_cfg", "word", "app", "english_stem", 1),
            mapping_row("app", "english_cfg", "word", "pg_catalog", "simple", 2),
        ];

        let cfgs = assemble_ts_configurations(&[cfg_row], &map_rows).unwrap();
        assert_eq!(cfgs.len(), 1);
        let c = &cfgs[0];
        assert_eq!(c.mappings.len(), 1);
        let m = &c.mappings[0];
        assert_eq!(m.token_type, "word");
        assert_eq!(m.dictionaries.len(), 2);
        assert_eq!(m.dictionaries[0].to_string(), "app.english_stem");
        assert_eq!(m.dictionaries[1].to_string(), "pg_catalog.simple");
    }

    // ---- Two token types ----

    #[test]
    fn two_token_types_both_attached() {
        let cfg_row = config_row("app", "english_cfg", "pg_catalog", "default");
        let map_rows = vec![
            mapping_row("app", "english_cfg", "asciiword", "app", "english_stem", 1),
            mapping_row("app", "english_cfg", "word", "app", "english_stem", 1),
            mapping_row("app", "english_cfg", "word", "pg_catalog", "simple", 2),
        ];

        let cfgs = assemble_ts_configurations(&[cfg_row], &map_rows).unwrap();
        let c = &cfgs[0];
        assert_eq!(c.mappings.len(), 2);

        // BTreeMap iteration is alphabetical: asciiword < word
        let asciiword = c
            .mappings
            .iter()
            .find(|m| m.token_type == "asciiword")
            .unwrap();
        assert_eq!(asciiword.dictionaries.len(), 1);
        assert_eq!(asciiword.dictionaries[0].to_string(), "app.english_stem");

        let word = c.mappings.iter().find(|m| m.token_type == "word").unwrap();
        assert_eq!(word.dictionaries.len(), 2);
        assert_eq!(word.dictionaries[0].to_string(), "app.english_stem");
        assert_eq!(word.dictionaries[1].to_string(), "pg_catalog.simple");
    }

    // ---- Multi-dict seqno order: rows arrive already ordered by SQL ----

    #[test]
    fn multi_dict_seqno_order_is_preserved_when_rows_in_order() {
        // SQL guarantees ORDER BY … mapseqno so rows arrive seqno 1,2,3.
        let cfg_row = config_row("app", "cfg", "pg_catalog", "default");
        let map_rows = vec![
            mapping_row("app", "cfg", "word", "app", "dict_a", 1),
            mapping_row("app", "cfg", "word", "app", "dict_b", 2),
            mapping_row("app", "cfg", "word", "app", "dict_c", 3),
        ];
        let cfgs = assemble_ts_configurations(&[cfg_row], &map_rows).unwrap();
        let m = &cfgs[0].mappings[0];
        assert_eq!(m.dictionaries[0].name.as_str(), "dict_a");
        assert_eq!(m.dictionaries[1].name.as_str(), "dict_b");
        assert_eq!(m.dictionaries[2].name.as_str(), "dict_c");
    }

    // ---- Two configs, mappings split correctly ----

    #[test]
    fn two_configs_mappings_split_correctly() {
        let row_a = config_row("app", "cfg_a", "pg_catalog", "default");
        let row_b = config_row("app", "cfg_b", "pg_catalog", "default");
        let map_rows = vec![
            mapping_row("app", "cfg_a", "word", "app", "dict1", 1),
            mapping_row("app", "cfg_b", "asciiword", "pg_catalog", "simple", 1),
        ];
        let cfgs = assemble_ts_configurations(&[row_a, row_b], &map_rows).unwrap();
        assert_eq!(cfgs.len(), 2);

        let a = cfgs
            .iter()
            .find(|c| c.qname.name.as_str() == "cfg_a")
            .unwrap();
        assert_eq!(a.mappings.len(), 1);
        assert_eq!(a.mappings[0].token_type, "word");

        let b = cfgs
            .iter()
            .find(|c| c.qname.name.as_str() == "cfg_b")
            .unwrap();
        assert_eq!(b.mappings.len(), 1);
        assert_eq!(b.mappings[0].token_type, "asciiword");
    }

    // ---- Empty inputs ----

    #[test]
    fn empty_config_rows_returns_empty_vec() {
        let cfgs = assemble_ts_configurations(&[], &[]).unwrap();
        assert!(cfgs.is_empty());
    }

    #[test]
    fn empty_mapping_rows_all_configs_have_empty_mappings() {
        let rows = vec![
            config_row("app", "a", "pg_catalog", "default"),
            config_row("app", "b", "pg_catalog", "default"),
        ];
        let cfgs = assemble_ts_configurations(&rows, &[]).unwrap();
        assert!(cfgs.iter().all(|c| c.mappings.is_empty()));
    }

    // ---- Parser qname is schema-qualified ----

    #[test]
    fn parser_qname_is_schema_qualified() {
        let row = config_row("app", "cfg", "pg_catalog", "default");
        let cfgs = assemble_ts_configurations(&[row], &[]).unwrap();
        assert_eq!(cfgs[0].parser.schema.as_str(), "pg_catalog");
        assert_eq!(cfgs[0].parser.name.as_str(), "default");
    }
}
