//! `tokio_postgres::Client`-backed [`CatalogQuerier`] adapter.
//!
//! Gated behind the `tokio-postgres-querier` feature. Shared by the `pgevolve`
//! CLI and the `pgevolve-testkit` tier-3 fixture runner; library consumers that
//! build their own [`CatalogQuerier`] from a different driver can omit the
//! feature to avoid the tokio + tokio-postgres dependency.
//!
//! Construct on a multi-threaded Tokio runtime — single-threaded runtimes
//! cannot satisfy [`tokio::task::block_in_place`].

use std::error::Error as StdError;
use std::sync::{Arc, Mutex};

use tokio::runtime::Handle;
use tokio_postgres::Row as PgRow;
use tokio_postgres::error::DbError;
use tokio_postgres::types::Type;

use super::queries::query_for;
use super::{CatalogError, CatalogQuerier, CatalogQuery, PgVersion, Row, Value};

/// Adapter that runs catalog queries against a live `tokio_postgres::Client`.
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

/// Error returned when constructing a [`PgCatalogQuerier`] off a Tokio runtime.
#[derive(Debug, thiserror::Error)]
#[error("PgCatalogQuerier must be constructed from within a Tokio runtime: {0}")]
pub struct NoRuntimeError(#[from] tokio::runtime::TryCurrentError);

impl PgCatalogQuerier {
    /// Wrap an open client.
    pub fn new(client: tokio_postgres::Client) -> Result<Self, NoRuntimeError> {
        Ok(Self {
            client: Arc::new(client),
            runtime: Handle::try_current()?,
            version: Mutex::new(None),
        })
    }

    /// Wrap a shared `Arc<Client>`. Used by callers that hold the client behind
    /// a shared reference and cannot transfer ownership (e.g. preflight passes
    /// the same client to multiple queriers).
    pub fn from_arc(client: Arc<tokio_postgres::Client>) -> Result<Self, NoRuntimeError> {
        Ok(Self {
            client,
            runtime: Handle::try_current()?,
            version: Mutex::new(None),
        })
    }

    /// Detect the major version once and cache it.
    pub fn version(&self) -> Result<PgVersion, CatalogError> {
        // Mutex poison is unrecoverable — it means another thread holding this
        // lock panicked. Propagate the panic; no meaningful recovery path.
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
        text_array_param: &[&str],
    ) -> Result<Vec<Row>, CatalogError> {
        // PgVersion is the bootstrap query — pick the shared SQL irrespective
        // of cached version (which we don't yet know).
        let sql = if matches!(query, CatalogQuery::PgVersion) {
            query_for(PgVersion::Pg16, query)
        } else {
            query_for(self.version()?, query)
        };

        let client = Arc::clone(&self.client);
        let owned: Vec<String> = text_array_param.iter().map(ToString::to_string).collect();
        let pg_rows: Vec<PgRow> = tokio::task::block_in_place(|| {
            self.runtime.block_on(async move {
                if query.takes_text_array_param() {
                    client.query(sql, &[&owned]).await
                } else {
                    client.query(sql, &[]).await
                }
            })
        })
        .map_err(|e| {
            // `tokio_postgres::Error::to_string()` for a DB-level error only
            // returns "db error". The PG sqlstate code and message are in the
            // error source chain. Build a richer message so that callers (e.g.
            // the 42501 / insufficient_privilege detection in read_catalog) can
            // inspect the code.
            let message = StdError::source(&e)
                .and_then(|s| s.downcast_ref::<DbError>())
                .map_or_else(
                    || e.to_string(),
                    |db_err| {
                        format!(
                            "{} (code={}, detail={})",
                            db_err.message(),
                            db_err.code().code(),
                            db_err.detail().unwrap_or("")
                        )
                    },
                );
            CatalogError::QueryFailed { query, message }
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

#[allow(clippy::too_many_lines)] // one arm per supported PG type-code; decoder is intentionally exhaustive.
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
            // `pg_constraint.contype` is a single-byte char in Postgres but
            // tokio-postgres decodes it as i8; map to a single-character text.
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
        _ => {
            // Unknown — attempt to read as text fall-back. Many catalog
            // columns are domains/regtypes that decode as Text/String.
            row.try_get::<_, Option<String>>(idx)
                .map(|v| v.map_or(Value::Null, Value::Text))
                .map_err(|e| bad(format!("unsupported type {} ({e})", ty.name())))
        }
    }
}
