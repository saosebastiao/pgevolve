//! Postgres major-version detection.

use crate::catalog::error::CatalogError;
use crate::catalog::{CatalogQuerier, CatalogQuery};

/// Major Postgres versions supported by the v0.1 catalog reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PgVersion {
    /// Postgres 14.
    Pg14,
    /// Postgres 15.
    Pg15,
    /// Postgres 16.
    Pg16,
    /// Postgres 17.
    Pg17,
}

impl PgVersion {
    /// Detect the major version from `server_version_num`.
    pub fn detect(querier: &dyn CatalogQuerier) -> Result<Self, CatalogError> {
        let rows = querier.fetch(CatalogQuery::PgVersion, &[])?;
        let row = rows.into_iter().next().ok_or(CatalogError::MissingResult {
            query: CatalogQuery::PgVersion,
        })?;
        let n = row.get_int(CatalogQuery::PgVersion, "server_version_num")?;
        Self::from_server_version_num(n)
    }

    /// Convert a `server_version_num` integer (e.g., `160000`) into the major
    /// version. Returns [`CatalogError::UnsupportedPgVersion`] for anything
    /// outside the 14–17 range.
    pub fn from_server_version_num(n: i64) -> Result<Self, CatalogError> {
        let major = n / 10_000;
        match major {
            14 => Ok(Self::Pg14),
            15 => Ok(Self::Pg15),
            16 => Ok(Self::Pg16),
            17 => Ok(Self::Pg17),
            v => Err(CatalogError::UnsupportedPgVersion(
                v.try_into().unwrap_or(0),
            )),
        }
    }

    /// Display this version as a short tag (e.g., `pg16`).
    #[must_use]
    pub const fn as_tag(self) -> &'static str {
        match self {
            Self::Pg14 => "pg14",
            Self::Pg15 => "pg15",
            Self::Pg16 => "pg16",
            Self::Pg17 => "pg17",
        }
    }

    /// Major-version integer (14, 15, 16, 17).
    #[must_use]
    pub const fn major(self) -> u32 {
        match self {
            Self::Pg14 => 14,
            Self::Pg15 => 15,
            Self::Pg16 => 16,
            Self::Pg17 => 17,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::{Row, Value};

    struct MockSingle(i64);
    impl CatalogQuerier for MockSingle {
        fn fetch(&self, _: CatalogQuery, _: &[&str]) -> Result<Vec<Row>, CatalogError> {
            Ok(vec![
                Row::new().with("server_version_num", Value::Integer(self.0))
            ])
        }
    }

    #[test]
    fn detects_each_supported_major() {
        for (n, v) in [
            (140_005, PgVersion::Pg14),
            (150_002, PgVersion::Pg15),
            (160_000, PgVersion::Pg16),
            (170_001, PgVersion::Pg17),
        ] {
            assert_eq!(PgVersion::detect(&MockSingle(n)).unwrap(), v);
        }
    }

    #[test]
    fn rejects_unsupported() {
        let err = PgVersion::detect(&MockSingle(130_004)).unwrap_err();
        assert!(matches!(err, CatalogError::UnsupportedPgVersion(13)));
        let err2 = PgVersion::detect(&MockSingle(180_000)).unwrap_err();
        assert!(matches!(err2, CatalogError::UnsupportedPgVersion(18)));
    }
}
