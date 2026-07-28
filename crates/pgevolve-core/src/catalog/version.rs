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
    /// Postgres 18.
    Pg18,
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
    /// outside the 14–18 range.
    pub fn from_server_version_num(n: i64) -> Result<Self, CatalogError> {
        let major = n / 10_000;
        match major {
            14 => Ok(Self::Pg14),
            15 => Ok(Self::Pg15),
            16 => Ok(Self::Pg16),
            17 => Ok(Self::Pg17),
            18 => Ok(Self::Pg18),
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
            Self::Pg18 => "pg18",
        }
    }

    /// Major-version integer (14, 15, 16, 17, 18).
    #[must_use]
    pub const fn major(self) -> u32 {
        match self {
            Self::Pg14 => 14,
            Self::Pg15 => 15,
            Self::Pg16 => 16,
            Self::Pg17 => 17,
            Self::Pg18 => 18,
        }
    }

    /// Every supported major, ascending.
    pub const ALL: [Self; 5] = [Self::Pg14, Self::Pg15, Self::Pg16, Self::Pg17, Self::Pg18];

    /// The supported majors as they appear in user-facing error text.
    ///
    /// Kept in sync with [`Self::ALL`] by `supported_list_matches_all`, so
    /// adding a variant without updating this string fails the test suite
    /// rather than shipping a stale message.
    pub const SUPPORTED_LIST: &'static str = "14, 15, 16, 17, 18";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::{Row, Value};

    struct MockSingle(i64);
    impl CatalogQuerier for MockSingle {
        fn fetch(&self, _: CatalogQuery, _: &[&str]) -> Result<Vec<Row>, CatalogError> {
            Ok(vec![
                Row::new().with("server_version_num", Value::Integer(self.0)),
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
            (180_000, PgVersion::Pg18),
        ] {
            assert_eq!(PgVersion::detect(&MockSingle(n)).unwrap(), v);
        }
    }

    #[test]
    fn detects_pg18() {
        assert_eq!(
            PgVersion::detect(&MockSingle(180_000)).unwrap(),
            PgVersion::Pg18,
        );
    }

    #[test]
    fn supported_list_matches_all() {
        let derived = PgVersion::ALL
            .iter()
            .map(|v| v.major().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(derived, PgVersion::SUPPORTED_LIST);
    }

    #[test]
    fn unsupported_error_names_every_supported_major() {
        let msg = CatalogError::UnsupportedPgVersion(13).to_string();
        for v in PgVersion::ALL {
            assert!(
                msg.contains(&v.major().to_string()),
                "error text {msg:?} omits supported major {}",
                v.major(),
            );
        }
    }

    #[test]
    fn rejects_unsupported() {
        let err = PgVersion::detect(&MockSingle(130_004)).unwrap_err();
        assert!(matches!(err, CatalogError::UnsupportedPgVersion(13)));
        let err2 = PgVersion::detect(&MockSingle(190_000)).unwrap_err();
        assert!(matches!(err2, CatalogError::UnsupportedPgVersion(19)));
    }
}
