//! Decode `pg_collation` rows into [`Collation`] IR.
//!
//! Only one query is involved (see `queries/collations.rs`); the assembler
//! in [`crate::catalog::assemble::collations`] is a thin wrapper that maps
//! [`decode_collation_row`] across every fetched row.
//!
//! `collprovider` is decoded from PG's single-byte char (`'c' | 'i' | 'b'`)
//! into [`CollationProvider`]. Any other value is rejected with
//! [`CatalogError::BadColumnType`] — the established pattern for unexpected
//! catalog column values across this crate (see `catalog/subscriptions.rs`,
//! `catalog/statistics.rs`).

// `CatalogError` embeds `IrError` and `ParseError`, both of which are large.
// Cold-path catalog reads; boxing adds noise without benefit.
#![allow(clippy::result_large_err)]

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::collation::{Collation, CollationProvider};

const Q: CatalogQuery = CatalogQuery::Collations;

/// Decode one `pg_collation` row into a [`Collation`].
pub fn decode_collation_row(row: &Row) -> Result<Collation, CatalogError> {
    let schema_str = row.get_text(Q, "schema")?;
    let name_str = row.get_text(Q, "name")?;

    let schema = ident(&schema_str, "schema")?;
    let name = ident(&name_str, "name")?;
    let qname = QualifiedName::new(schema, name);

    let provider = decode_provider(&row.get_text(Q, "provider")?)?;

    let owner = match row.get_opt_text(Q, "owner")? {
        Some(s) if !s.is_empty() => Some(ident(&s, "owner")?),
        _ => None,
    };

    Ok(Collation {
        qname,
        provider,
        lc_collate: row.get_text(Q, "lc_collate")?,
        lc_ctype: row.get_text(Q, "lc_ctype")?,
        deterministic: row.get_bool(Q, "deterministic")?,
        version: row.get_opt_text(Q, "version")?,
        owner,
        comment: row.get_opt_text(Q, "comment")?,
    })
}

fn ident(s: &str, column: &str) -> Result<Identifier, CatalogError> {
    Identifier::from_unquoted(s).map_err(|e| CatalogError::BadColumnType {
        query: Q,
        column: column.to_string(),
        message: format!("invalid identifier {s:?}: {e}"),
    })
}

/// Decode the single-byte `collprovider` value: `'c'` → libc, `'i'` → icu,
/// `'b'` → builtin (PG 17+). Any other byte is rejected.
fn decode_provider(s: &str) -> Result<CollationProvider, CatalogError> {
    match s {
        "c" => Ok(CollationProvider::Libc),
        "i" => Ok(CollationProvider::Icu),
        "b" => Ok(CollationProvider::Builtin),
        other => Err(CatalogError::BadColumnType {
            query: Q,
            column: "provider".to_string(),
            message: format!("expected one of 'c'/'i'/'b', got {other:?}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::Value;

    fn base_row() -> Row {
        Row::new()
            .with("schema", Value::Text("app".to_string()))
            .with("name", Value::Text("ci".to_string()))
            .with("provider", Value::Text("i".to_string()))
            .with("lc_collate", Value::Text("und".to_string()))
            .with("lc_ctype", Value::Text("und".to_string()))
            .with("deterministic", Value::Bool(false))
            .with("version", Value::Null)
            .with("owner", Value::Text("app_owner".to_string()))
            .with("comment", Value::Null)
    }

    #[test]
    fn decode_icu_nondeterministic() {
        let c = decode_collation_row(&base_row()).unwrap();
        assert_eq!(c.qname.schema.as_str(), "app");
        assert_eq!(c.qname.name.as_str(), "ci");
        assert_eq!(c.provider, CollationProvider::Icu);
        assert_eq!(c.lc_collate, "und");
        assert_eq!(c.lc_ctype, "und");
        assert!(!c.deterministic);
        assert!(c.version.is_none());
        assert_eq!(c.owner.as_ref().map(Identifier::as_str), Some("app_owner"));
        assert!(c.comment.is_none());
    }

    #[test]
    fn decode_libc_with_version_and_comment() {
        let row = base_row()
            .with("provider", Value::Text("c".to_string()))
            .with("lc_collate", Value::Text("en_US.utf8".to_string()))
            .with("lc_ctype", Value::Text("en_US.utf8".to_string()))
            .with("deterministic", Value::Bool(true))
            .with("version", Value::Text("153.128".to_string()))
            .with("comment", Value::Text("pinned".to_string()));
        let c = decode_collation_row(&row).unwrap();
        assert_eq!(c.provider, CollationProvider::Libc);
        assert!(c.deterministic);
        assert_eq!(c.version.as_deref(), Some("153.128"));
        assert_eq!(c.comment.as_deref(), Some("pinned"));
    }

    #[test]
    fn decode_builtin_provider() {
        let row = base_row().with("provider", Value::Text("b".to_string()));
        let c = decode_collation_row(&row).unwrap();
        assert_eq!(c.provider, CollationProvider::Builtin);
    }

    #[test]
    fn decode_unknown_provider_errors() {
        let row = base_row().with("provider", Value::Text("x".to_string()));
        let err = decode_collation_row(&row).unwrap_err();
        assert!(
            matches!(err, CatalogError::BadColumnType { ref column, .. } if column == "provider"),
            "expected BadColumnType on provider column, got {err:?}"
        );
    }

    #[test]
    fn decode_owner_null_yields_none() {
        let row = base_row().with("owner", Value::Null);
        let c = decode_collation_row(&row).unwrap();
        assert!(c.owner.is_none());
    }

    #[test]
    fn decode_owner_empty_yields_none() {
        let row = base_row().with("owner", Value::Text(String::new()));
        let c = decode_collation_row(&row).unwrap();
        assert!(c.owner.is_none());
    }
}
