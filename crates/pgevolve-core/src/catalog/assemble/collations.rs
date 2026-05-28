//! Assemble `pg_collation` rows into `Vec<Collation>`.
//!
//! Only one query (`CatalogQuery::Collations`) feeds this assembler. Each
//! row maps 1:1 to a [`Collation`] via [`decode_collation_row`]; there is
//! no cross-row stitching to do (compare with publications, which join
//! `pg_publication_rel` + `pg_publication_namespace` + attribute resolution).

// `CatalogError` embeds `IrError` and `ParseError`, both of which are large.
// Cold-path catalog reads; boxing adds noise without benefit.
#![allow(clippy::result_large_err)]

use crate::catalog::collations::decode_collation_row;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::ir::collation::Collation;

/// Build the IR's `collations` field from pre-fetched `pg_collation` rows.
pub(in crate::catalog) fn build_collations(rows: &[Row]) -> Result<Vec<Collation>, CatalogError> {
    rows.iter().map(decode_collation_row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::Value;
    use crate::ir::collation::CollationProvider;

    fn row(schema: &str, name: &str, provider: &str) -> Row {
        Row::new()
            .with("schema", Value::Text(schema.to_string()))
            .with("name", Value::Text(name.to_string()))
            .with("provider", Value::Text(provider.to_string()))
            .with("lc_collate", Value::Text("und".to_string()))
            .with("lc_ctype", Value::Text("und".to_string()))
            .with("deterministic", Value::Bool(true))
            .with("version", Value::Null)
            .with("owner", Value::Null)
            .with("comment", Value::Null)
    }

    #[test]
    fn empty_rows_returns_empty_vec() {
        assert!(build_collations(&[]).unwrap().is_empty());
    }

    #[test]
    fn maps_one_row_per_collation() {
        let rows = vec![
            row("app", "a", "c"),
            row("app", "b", "i"),
            row("app", "c", "b"),
        ];
        let out = build_collations(&rows).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].qname.name.as_str(), "a");
        assert_eq!(out[0].provider, CollationProvider::Libc);
        assert_eq!(out[1].provider, CollationProvider::Icu);
        assert_eq!(out[2].provider, CollationProvider::Builtin);
    }

    #[test]
    fn propagates_decode_error() {
        let bad = vec![row("app", "x", "z")];
        let err = build_collations(&bad).unwrap_err();
        assert!(
            matches!(err, CatalogError::BadColumnType { ref column, .. } if column == "provider"),
            "expected provider decode error, got {err:?}"
        );
    }
}
