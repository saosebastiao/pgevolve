//! Assemble `pg_ts_dict` rows into `Vec<TsDictionary>`.
//!
//! Each row represents one user-defined text-search dictionary in a managed
//! schema. Rows are decoded 1:1 into [`crate::ir::text_search::TsDictionary`]
//! IR entries; there is no cross-row stitching.
//!
//! The `dictinitoption` column is a `text` blob in PG's canonical format:
//!
//! ```text
//! key = 'value'[, key = 'value', …]
//! ```
//!
//! `parse_dict_options` splits it into an ordered `Vec<(String, String)>`.
//! The split is **quote-aware**: commas inside single-quoted values do not
//! terminate an entry (e.g. `stopwords = 'a,b'` yields one pair with value
//! `a,b`).

// `CatalogError` embeds `IrError` and `ParseError`, both large. Boxing them
// would add indirection noise without benefit — these are cold-path catalog
// reads, not hot loops.
#![allow(clippy::result_large_err)]

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::text_search::TsDictionary;

const Q: CatalogQuery = CatalogQuery::TsDictionaries;

/// Decode all `pg_ts_dict` rows into [`TsDictionary`] IR entries.
pub(super) fn assemble_ts_dictionaries(rows: &[Row]) -> Result<Vec<TsDictionary>, CatalogError> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(decode_row(row)?);
    }
    Ok(out)
}

/// Decode a single `pg_ts_dict` row.
fn decode_row(row: &Row) -> Result<TsDictionary, CatalogError> {
    let schema_name = row.get_text(Q, "schema_name")?;
    let name = row.get_text(Q, "name")?;
    let qname = QualifiedName::new(ident(&schema_name, "schema_name")?, ident(&name, "name")?);

    let template_schema = row.get_text(Q, "template_schema")?;
    let template_name = row.get_text(Q, "template_name")?;
    let template = QualifiedName::new(
        ident(&template_schema, "template_schema")?,
        ident(&template_name, "template_name")?,
    );

    let options_raw = row.get_opt_text(Q, "options")?;
    let options = match options_raw.as_deref() {
        None | Some("") => Vec::new(),
        Some(s) => parse_dict_options(s),
    };

    let owner_str = row.get_text(Q, "owner")?;
    let owner = if owner_str.is_empty() {
        None
    } else {
        Some(ident(&owner_str, "owner")?)
    };

    let comment = match row.get_opt_text(Q, "comment")? {
        Some(s) if !s.is_empty() => Some(s),
        _ => None,
    };

    Ok(TsDictionary {
        qname,
        template,
        options,
        owner,
        comment,
    })
}

/// Parse a `dictinitoption` blob into an ordered list of `(key, value)` pairs.
///
/// PG stores options in the canonical form `key = 'value'[, key = 'value', …]`.
/// Values are single-quoted; single quotes inside a value are escaped as `''`.
/// The split on `,` is quote-aware: commas inside single-quoted values do **not**
/// terminate an entry.
///
/// Returns an empty `Vec` for an empty or whitespace-only input.
fn parse_dict_options(s: &str) -> Vec<(String, String)> {
    // Split on commas that are NOT inside single-quoted strings.
    let parts = split_options(s);
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // Split on the first `=` to separate key from value.
        if let Some(eq_pos) = part.find('=') {
            let key = part[..eq_pos].trim().to_string();
            let raw_val = part[eq_pos + 1..].trim();
            let value = strip_and_unescape_quotes(raw_val);
            if !key.is_empty() {
                out.push((key, value));
            }
        }
    }
    out
}

/// Split `s` on commas that appear outside single-quoted strings.
///
/// This handles the case where a value contains a literal comma inside quotes,
/// e.g. `stopwords = 'a,b'` must yield a single entry, not two.
fn split_options(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_quotes = false;
    let bytes = s.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b'\'' if in_quotes => {
                // A `''` inside a quoted string is an escaped single-quote —
                // it does NOT end the quoted region.
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    // Skip over both quote characters.
                    i += 2;
                    continue;
                }
                // Closing quote.
                in_quotes = false;
            }
            b'\'' => {
                in_quotes = true;
            }
            b',' if !in_quotes => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    // Push the final segment.
    parts.push(&s[start..]);
    parts
}

/// Strip surrounding single quotes from `s` and unescape `''` → `'`.
///
/// If the value is not surrounded by single quotes, returns it as-is.
fn strip_and_unescape_quotes(s: &str) -> String {
    if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
        s[1..s.len() - 1].replace("''", "'")
    } else {
        s.to_string()
    }
}

/// Parse a raw string as an unquoted identifier, mapping the error to
/// [`CatalogError`].
fn ident(s: &str, column: &str) -> Result<Identifier, CatalogError> {
    Identifier::from_unquoted(s).map_err(|e| CatalogError::BadColumnType {
        query: Q,
        column: column.to_string(),
        message: format!("invalid identifier {s:?}: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::Value;

    /// Build a minimal valid dictionary row with two options.
    fn dict_row() -> Row {
        Row::new()
            .with("schema_name", Value::Text("app".to_string()))
            .with("name", Value::Text("english_stem".to_string()))
            .with("template_schema", Value::Text("pg_catalog".to_string()))
            .with("template_name", Value::Text("snowball".to_string()))
            .with(
                "options",
                Value::Text("language = 'english', stopwords = 'english'".to_string()),
            )
            .with("owner", Value::Text("app_owner".to_string()))
            .with("comment", Value::Null)
    }

    #[test]
    fn decode_dict_with_template_and_two_options() {
        let dict = assemble_ts_dictionaries(&[dict_row()]).unwrap();
        assert_eq!(dict.len(), 1);
        let d = &dict[0];
        assert_eq!(d.qname.to_string(), "app.english_stem");
        assert_eq!(d.template.to_string(), "pg_catalog.snowball");
        assert_eq!(d.options.len(), 2);
        assert_eq!(
            d.options[0],
            ("language".to_string(), "english".to_string())
        );
        assert_eq!(
            d.options[1],
            ("stopwords".to_string(), "english".to_string())
        );
        assert_eq!(d.owner.as_ref().map(Identifier::as_str), Some("app_owner"));
        assert!(d.comment.is_none());
    }

    #[test]
    fn parse_options_comma_inside_quotes_is_single_entry() {
        // A naive split(',') would incorrectly produce two entries here.
        let opts = parse_dict_options("stopwords = 'a,b'");
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0], ("stopwords".to_string(), "a,b".to_string()));
    }

    #[test]
    fn parse_options_multiple_entries_with_comma_in_value() {
        let opts = parse_dict_options("language = 'english', stopwords = 'a,b'");
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0], ("language".to_string(), "english".to_string()));
        assert_eq!(opts[1], ("stopwords".to_string(), "a,b".to_string()));
    }

    #[test]
    fn parse_options_empty_string_yields_no_options() {
        assert!(parse_dict_options("").is_empty());
    }

    #[test]
    fn decode_dict_null_options_yields_empty_vec() {
        let mut row = dict_row();
        row.insert("options", Value::Null);
        let dict = assemble_ts_dictionaries(&[row]).unwrap();
        assert!(dict[0].options.is_empty());
    }

    #[test]
    fn decode_dict_empty_options_yields_empty_vec() {
        let mut row = dict_row();
        row.insert("options", Value::Text(String::new()));
        let dict = assemble_ts_dictionaries(&[row]).unwrap();
        assert!(dict[0].options.is_empty());
    }

    #[test]
    fn decode_dict_with_owner_and_comment() {
        let mut row = dict_row();
        row.insert(
            "comment",
            Value::Text("snowball english stemmer".to_string()),
        );
        let dict = assemble_ts_dictionaries(&[row]).unwrap();
        let d = &dict[0];
        assert_eq!(d.owner.as_ref().map(Identifier::as_str), Some("app_owner"));
        assert_eq!(d.comment.as_deref(), Some("snowball english stemmer"));
    }

    #[test]
    fn decode_dict_null_owner_yields_none() {
        let mut row = dict_row();
        row.insert("owner", Value::Text(String::new()));
        let dict = assemble_ts_dictionaries(&[row]).unwrap();
        assert!(dict[0].owner.is_none());
    }

    #[test]
    fn decode_empty_rows_returns_empty_vec() {
        assert!(assemble_ts_dictionaries(&[]).unwrap().is_empty());
    }

    #[test]
    fn parse_options_unescapes_double_single_quotes() {
        // PG escapes a literal ' inside a value as '' — e.g. ``key = 'it''s'``
        // should yield value `it's`.
        let opts = parse_dict_options("key = 'it''s'");
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0], ("key".to_string(), "it's".to_string()));
    }

    #[test]
    fn template_qname_is_schema_qualified() {
        // Verify the template is assembled as a schema-qualified QualifiedName
        // (not just the bare name).
        let dict = assemble_ts_dictionaries(&[dict_row()]).unwrap();
        assert_eq!(dict[0].template.schema.as_str(), "pg_catalog");
        assert_eq!(dict[0].template.name.as_str(), "snowball");
    }
}
