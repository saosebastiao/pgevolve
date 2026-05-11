//! `tokio_postgres`-backed [`CatalogQuerier`] adapter for the binary.
//!
//! Phase 9 path. The trait is sync; this adapter runs queries via
//! [`tokio::task::block_in_place`] on the caller's runtime. Mirrors the
//! testkit variant used in tier-3 tests; duplicated here to avoid pulling
//! `testcontainers` (a testkit dep) into the binary.

use std::sync::{Arc, Mutex};

use anyhow::Context;
use tokio::runtime::Handle;
use tokio_postgres::types::Type;
use tokio_postgres::Row as PgRow;

use pgevolve_core::catalog::queries::query_for;
use pgevolve_core::catalog::{CatalogError, CatalogQuerier, CatalogQuery, PgVersion, Row, Value};

/// Adapter that runs catalog queries against a live `tokio_postgres::Client`.
///
/// Construct on a multi-threaded Tokio runtime — single-threaded runtimes
/// cannot satisfy `block_in_place`.
pub struct PgCatalogQuerier {
    client: Arc<tokio_postgres::Client>,
    runtime: Handle,
    version: Mutex<Option<PgVersion>>,
}

impl std::fmt::Debug for PgCatalogQuerier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgCatalogQuerier").finish_non_exhaustive()
    }
}

impl PgCatalogQuerier {
    /// Wrap an open client in an adapter.
    pub fn new(client: tokio_postgres::Client) -> anyhow::Result<Self> {
        Ok(Self {
            client: Arc::new(client),
            runtime: Handle::try_current()
                .with_context(|| "PgCatalogQuerier::new must be called from a Tokio runtime")?,
            version: Mutex::new(None),
        })
    }

    /// Detect the major version once and cache it.
    pub fn version(&self) -> Result<PgVersion, CatalogError> {
        let cached = *self.version.lock().expect("poisoned");
        if let Some(v) = cached {
            return Ok(v);
        }
        let v = PgVersion::detect(self)?;
        *self.version.lock().expect("poisoned") = Some(v);
        Ok(v)
    }
}

impl CatalogQuerier for PgCatalogQuerier {
    fn fetch(
        &self,
        query: CatalogQuery,
        managed_schemas: &[&str],
    ) -> Result<Vec<Row>, CatalogError> {
        let sql = if matches!(query, CatalogQuery::PgVersion) {
            query_for(PgVersion::Pg16, query)
        } else {
            query_for(self.version()?, query)
        };

        let client = Arc::clone(&self.client);
        let owned: Vec<String> = managed_schemas.iter().map(ToString::to_string).collect();
        let pg_rows: Vec<PgRow> = tokio::task::block_in_place(|| {
            self.runtime.block_on(async move {
                if matches!(query, CatalogQuery::PgVersion) {
                    client.query(sql, &[]).await
                } else {
                    client.query(sql, &[&owned]).await
                }
            })
        })
        .map_err(|e| CatalogError::QueryFailed {
            query,
            message: e.to_string(),
        })?;

        pg_rows
            .into_iter()
            .map(|r| pg_row_to_row(&r, query))
            .collect()
    }
}

fn pg_row_to_row(row: &PgRow, query: CatalogQuery) -> Result<Row, CatalogError> {
    let mut out = Row::new();
    for (i, col) in row.columns().iter().enumerate() {
        let name = col.name();
        let value = pg_value(row, i, col.type_(), name, query)?;
        out.insert(name, value);
    }
    Ok(out)
}

#[allow(clippy::too_many_lines)]
fn pg_value(
    row: &PgRow,
    idx: usize,
    ty: &Type,
    column_name: &str,
    query: CatalogQuery,
) -> Result<Value, CatalogError> {
    let bad = |msg: String| CatalogError::BadColumnType {
        query,
        column: column_name.to_string(),
        message: msg,
    };

    macro_rules! get_opt {
        ($t:ty) => {
            row.try_get::<_, Option<$t>>(idx)
                .map_err(|e| bad(format!("decode {} as {}: {e}", ty.name(), stringify!($t))))?
        };
    }

    match *ty {
        Type::BOOL => Ok(get_opt!(bool).map_or(Value::Null, Value::Bool)),
        Type::INT2 => Ok(get_opt!(i16).map_or(Value::Null, Value::SmallInt)),
        Type::INT4 => Ok(get_opt!(i32).map_or(Value::Null, |v| Value::Integer(i64::from(v)))),
        Type::INT8 => Ok(get_opt!(i64).map_or(Value::Null, Value::Integer)),
        Type::OID => Ok(get_opt!(u32).map_or(Value::Null, |v| Value::Integer(i64::from(v)))),
        Type::TEXT | Type::VARCHAR | Type::NAME | Type::BPCHAR => {
            Ok(get_opt!(String).map_or(Value::Null, Value::Text))
        }
        Type::CHAR => {
            let v = get_opt!(i8);
            #[allow(clippy::cast_sign_loss)]
            Ok(v.map_or(Value::Null, |b| Value::Char(b as u8 as char)))
        }
        Type::INT2_ARRAY => Ok(get_opt!(Vec<i16>).map_or(Value::Null, |v| {
            Value::IntegerArray(v.into_iter().map(i64::from).collect())
        })),
        Type::INT4_ARRAY => Ok(get_opt!(Vec<i32>).map_or(Value::Null, |v| {
            Value::IntegerArray(v.into_iter().map(i64::from).collect())
        })),
        Type::INT8_ARRAY => Ok(get_opt!(Vec<i64>).map_or(Value::Null, Value::IntegerArray)),
        Type::TEXT_ARRAY | Type::NAME_ARRAY | Type::VARCHAR_ARRAY => {
            Ok(get_opt!(Vec<String>).map_or(Value::Null, Value::TextArray))
        }
        Type::BYTEA => Ok(get_opt!(Vec<u8>).map_or(Value::Null, Value::Bytes)),
        _ => row
            .try_get::<_, Option<String>>(idx)
            .map(|v| v.map_or(Value::Null, Value::Text))
            .map_err(|e| bad(format!("unsupported type {} ({e})", ty.name()))),
    }
}
