//! Decode `pg_class.reloptions::text[]` into typed storage-options structs.

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::ir::reloptions::{BufferingMode, IndexStorageOptions, TableStorageOptions};

/// Decode reloptions for a table or materialized view.
pub fn decode_table_reloptions(
    raw: &[String],
    q: CatalogQuery,
) -> Result<TableStorageOptions, CatalogError> {
    let mut out = TableStorageOptions::default();
    for entry in raw {
        let (key, value) = split_kv(entry, q)?;
        match key {
            "fillfactor" => out.fillfactor = Some(parse_u32(&value, key, q)?),
            "parallel_workers" => out.parallel_workers = Some(parse_u32(&value, key, q)?),
            "toast_tuple_target" => out.toast_tuple_target = Some(parse_u32(&value, key, q)?),
            "user_catalog_table" => out.user_catalog_table = Some(parse_bool(&value, key, q)?),
            "vacuum_truncate" => out.vacuum_truncate = Some(parse_bool(&value, key, q)?),
            _ => {
                out.extra.insert(key.to_owned(), value);
            }
        }
    }
    Ok(out)
}

/// Decode reloptions for an index. Unknown keys land in `extra` regardless
/// of access method — validation is parser-side, not reader-side.
pub fn decode_index_reloptions(
    raw: &[String],
    q: CatalogQuery,
) -> Result<IndexStorageOptions, CatalogError> {
    let mut out = IndexStorageOptions::default();
    for entry in raw {
        let (key, value) = split_kv(entry, q)?;
        match key {
            "fillfactor" => out.fillfactor = Some(parse_u32(&value, key, q)?),
            "fastupdate" => out.fastupdate = Some(parse_bool(&value, key, q)?),
            "gin_pending_list_limit" => {
                out.gin_pending_list_limit = Some(parse_u64(&value, key, q)?);
            }
            "buffering" => {
                out.buffering = Some(value.parse::<BufferingMode>().map_err(|()| {
                    CatalogError::BadColumnType {
                        query: q,
                        column: "reloptions".to_owned(),
                        message: format!("buffering value {value:?} invalid"),
                    }
                })?);
            }
            "deduplicate_items" => out.deduplicate_items = Some(parse_bool(&value, key, q)?),
            "pages_per_range" => out.pages_per_range = Some(parse_u32(&value, key, q)?),
            "autosummarize" => out.autosummarize = Some(parse_bool(&value, key, q)?),
            _ => {
                out.extra.insert(key.to_owned(), value);
            }
        }
    }
    Ok(out)
}

fn split_kv(entry: &str, q: CatalogQuery) -> Result<(&str, String), CatalogError> {
    let (k, v) = entry
        .split_once('=')
        .ok_or_else(|| CatalogError::BadColumnType {
            query: q,
            column: "reloptions".to_owned(),
            message: format!("malformed reloption {entry:?}"),
        })?;
    Ok((k, v.to_owned()))
}

fn parse_u32(v: &str, key: &str, q: CatalogQuery) -> Result<u32, CatalogError> {
    v.parse().map_err(|e| CatalogError::BadColumnType {
        query: q,
        column: "reloptions".to_owned(),
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })
}

fn parse_u64(v: &str, key: &str, q: CatalogQuery) -> Result<u64, CatalogError> {
    v.parse().map_err(|e| CatalogError::BadColumnType {
        query: q,
        column: "reloptions".to_owned(),
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })
}

fn parse_bool(v: &str, key: &str, q: CatalogQuery) -> Result<bool, CatalogError> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "on" | "1" => Ok(true),
        "false" | "off" | "0" => Ok(false),
        _ => Err(CatalogError::BadColumnType {
            query: q,
            column: "reloptions".to_owned(),
            message: format!("reloption {key} = {v:?} not a recognized bool"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Arbitrary choice — doesn't affect decode logic.
    const Q: CatalogQuery = CatalogQuery::Indexes;

    #[test]
    fn decodes_fillfactor() {
        let s = decode_table_reloptions(&["fillfactor=80".into()], Q).unwrap();
        assert_eq!(s.fillfactor, Some(80));
    }

    #[test]
    fn decodes_autovacuum_enabled() {
        // autovacuum_* keys flow through the generic `extra` bag verbatim.
        let s = decode_table_reloptions(&["autovacuum_enabled=false".into()], Q).unwrap();
        assert_eq!(
            s.extra.get("autovacuum_enabled").map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn decodes_autovacuum_scale_factor() {
        let s =
            decode_table_reloptions(&["autovacuum_vacuum_scale_factor=0.05".into()], Q).unwrap();
        assert_eq!(
            s.extra
                .get("autovacuum_vacuum_scale_factor")
                .map(String::as_str),
            Some("0.05")
        );
    }

    #[test]
    fn unknown_keys_flow_into_extra() {
        let s = decode_table_reloptions(&["pg_partman.something=value".into()], Q).unwrap();
        assert_eq!(
            s.extra.get("pg_partman.something").map(String::as_str),
            Some("value")
        );
    }

    #[test]
    fn index_decode_buffering_on() {
        let s = decode_index_reloptions(&["buffering=auto".into()], Q).unwrap();
        assert_eq!(s.buffering, Some(BufferingMode::Auto));
    }

    #[test]
    fn malformed_entry_errors() {
        assert!(decode_table_reloptions(&["no_equals".into()], Q).is_err());
    }

    #[test]
    fn autovacuum_value_stored_verbatim() {
        // No typed validation: whatever the catalog yields is stored as-is.
        let s = decode_table_reloptions(&["autovacuum_enabled=on".into()], Q).unwrap();
        assert_eq!(
            s.extra.get("autovacuum_enabled").map(String::as_str),
            Some("on")
        );
    }
}
