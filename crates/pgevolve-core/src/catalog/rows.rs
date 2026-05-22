//! Row types returned by [`crate::catalog::CatalogQuerier`].
//!
//! The trait deliberately does not depend on `tokio-postgres` or any other
//! Postgres driver; the binary's adapter performs the I/O and constructs
//! [`Row`] values from whatever rows the driver returns. Tests construct
//! [`Row`] values directly from a literal map.

use std::collections::BTreeMap;

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;

/// A SQL value variant supported by the catalog reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// SQL `NULL`.
    Null,
    /// `boolean`.
    Bool(bool),
    /// `smallint`.
    SmallInt(i16),
    /// `integer` / `bigint` / any oid we don't unpack further.
    Integer(i64),
    /// Single-character "char" (used by `pg_constraint.contype`, etc.).
    Char(char),
    /// Text-like (`text`, `name`, `varchar`).
    Text(String),
    /// `text[]` / `name[]`.
    TextArray(Vec<String>),
    /// `int2[]` / `int4[]` / `int8[]`.
    IntegerArray(Vec<i64>),
    /// Bytes (used for `pg_node_tree` payloads we don't decode in IR).
    Bytes(Vec<u8>),
}

/// A single row returned by a catalog query, keyed by column name.
#[derive(Debug, Clone, Default)]
pub struct Row {
    cols: BTreeMap<String, Value>,
}

impl Row {
    /// Construct an empty row.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cols: BTreeMap::new(),
        }
    }

    /// Insert a column.
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: Value) -> Self {
        self.cols.insert(key.into(), value);
        self
    }

    /// Insert a column in place.
    pub fn insert(&mut self, key: impl Into<String>, value: Value) {
        self.cols.insert(key.into(), value);
    }

    /// Borrow a column value by name.
    pub fn get_value(&self, query: CatalogQuery, key: &str) -> Result<&Value, CatalogError> {
        self.cols
            .get(key)
            .ok_or_else(|| CatalogError::MissingColumn {
                query,
                column: key.to_string(),
            })
    }

    /// Whether the column is absent or NULL.
    pub fn is_null(&self, key: &str) -> bool {
        matches!(self.cols.get(key), None | Some(Value::Null))
    }

    /// Read a non-null integer column (any width).
    pub fn get_int(&self, query: CatalogQuery, key: &str) -> Result<i64, CatalogError> {
        match self.get_value(query, key)? {
            Value::Integer(i) => Ok(*i),
            Value::SmallInt(i) => Ok(i64::from(*i)),
            other => Err(CatalogError::BadColumnType {
                query,
                column: key.to_string(),
                message: format!("expected integer, got {other:?}"),
            }),
        }
    }

    /// Read an optional integer column.
    pub fn get_opt_int(&self, query: CatalogQuery, key: &str) -> Result<Option<i64>, CatalogError> {
        if self.is_null(key) {
            return Ok(None);
        }
        self.get_int(query, key).map(Some)
    }

    /// Read a non-null text column.
    pub fn get_text(&self, query: CatalogQuery, key: &str) -> Result<String, CatalogError> {
        match self.get_value(query, key)? {
            Value::Text(s) => Ok(s.clone()),
            other => Err(CatalogError::BadColumnType {
                query,
                column: key.to_string(),
                message: format!("expected text, got {other:?}"),
            }),
        }
    }

    /// Read an optional text column.
    pub fn get_opt_text(
        &self,
        query: CatalogQuery,
        key: &str,
    ) -> Result<Option<String>, CatalogError> {
        if self.is_null(key) {
            return Ok(None);
        }
        self.get_text(query, key).map(Some)
    }

    /// Read a non-null bool column.
    pub fn get_bool(&self, query: CatalogQuery, key: &str) -> Result<bool, CatalogError> {
        match self.get_value(query, key)? {
            Value::Bool(b) => Ok(*b),
            other => Err(CatalogError::BadColumnType {
                query,
                column: key.to_string(),
                message: format!("expected boolean, got {other:?}"),
            }),
        }
    }

    /// Read a non-null char column (e.g., `pg_constraint.contype`).
    pub fn get_char(&self, query: CatalogQuery, key: &str) -> Result<char, CatalogError> {
        match self.get_value(query, key)? {
            Value::Char(c) => Ok(*c),
            Value::Text(s) if s.chars().count() == 1 => {
                // SAFETY: count() == 1 guarantees next() yields Some.
                Ok(s.chars()
                    .next()
                    .unwrap_or_else(|| unreachable!("count==1 has one char")))
            }
            other => Err(CatalogError::BadColumnType {
                query,
                column: key.to_string(),
                message: format!("expected single-char, got {other:?}"),
            }),
        }
    }

    /// Read a non-null `int2[]`/`int4[]`/`int8[]` column.
    pub fn get_int_array(&self, query: CatalogQuery, key: &str) -> Result<Vec<i64>, CatalogError> {
        match self.get_value(query, key)? {
            Value::IntegerArray(v) => Ok(v.clone()),
            other => Err(CatalogError::BadColumnType {
                query,
                column: key.to_string(),
                message: format!("expected integer array, got {other:?}"),
            }),
        }
    }

    /// Read a non-null `text[]`/`name[]` column.
    pub fn get_text_array(
        &self,
        query: CatalogQuery,
        key: &str,
    ) -> Result<Vec<String>, CatalogError> {
        match self.get_value(query, key)? {
            Value::TextArray(v) => Ok(v.clone()),
            other => Err(CatalogError::BadColumnType {
                query,
                column: key.to_string(),
                message: format!("expected text array, got {other:?}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::CatalogQuery;

    #[test]
    fn missing_column_errors() {
        let r = Row::new();
        let err = r.get_int(CatalogQuery::Schemas, "oid").unwrap_err();
        assert!(matches!(err, CatalogError::MissingColumn { .. }));
    }

    #[test]
    fn integer_widening_works() {
        let r = Row::new()
            .with("a", Value::SmallInt(7))
            .with("b", Value::Integer(42));
        assert_eq!(r.get_int(CatalogQuery::Schemas, "a").unwrap(), 7);
        assert_eq!(r.get_int(CatalogQuery::Schemas, "b").unwrap(), 42);
    }

    #[test]
    fn null_is_treated_as_absent() {
        let r = Row::new().with("c", Value::Null);
        assert!(r.is_null("c"));
        assert!(
            r.get_opt_text(CatalogQuery::Schemas, "c")
                .unwrap()
                .is_none()
        );
    }
}
