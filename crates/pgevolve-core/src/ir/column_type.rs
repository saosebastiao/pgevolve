//! `ColumnType` — the canonical normalized form of a Postgres data type.
//!
//! Every column type seen in source SQL or in the live catalog is translated
//! into this enum. Equivalence is decided by the `Diff` impl in this module;
//! rendering back to SQL is via [`ColumnType::render_sql`].

use serde::{Deserialize, Serialize};

use crate::identifier::QualifiedName;

/// Canonical normalized form of a Postgres column type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ColumnType {
    /// Boolean.
    Boolean,
    /// `smallint` / `int2`.
    SmallInt,
    /// `integer` / `int` / `int4`.
    Integer,
    /// `bigint` / `int8`.
    BigInt,
    /// `real` / `float4`.
    Real,
    /// `double precision` / `float8`.
    DoublePrecision,
    /// `numeric` / `decimal` with optional precision and scale.
    Numeric {
        /// Total digits (1..=1000); `None` = unbounded.
        precision: Option<u16>,
        /// Digits to the right of the decimal point; `None` = 0 by default.
        scale: Option<i16>,
    },
    /// `text`.
    Text,
    /// `varchar(N)` or unbounded `varchar`.
    Varchar {
        /// Maximum character length; `None` = unbounded (distinct from `Text`).
        len: Option<u32>,
    },
    /// `char(N)` (blank-padded). `len = None` is a 1-char default.
    Char {
        /// Character length.
        len: Option<u32>,
    },
    /// `bytea`.
    Bytea,
    /// `date`.
    Date,
    /// `time` / `time with time zone`.
    Time {
        /// Sub-second precision (0..=6).
        precision: Option<u8>,
        /// True for `WITH TIME ZONE`.
        with_tz: bool,
    },
    /// `timestamp` / `timestamp with time zone`.
    Timestamp {
        /// Sub-second precision (0..=6).
        precision: Option<u8>,
        /// True for `WITH TIME ZONE`.
        with_tz: bool,
    },
    /// `interval` with optional fields and precision.
    Interval {
        /// E.g., `YEAR`, `YEAR TO MONTH`, `DAY TO HOUR`. `None` = unconstrained.
        fields: Option<String>,
        /// Sub-second precision (0..=6).
        precision: Option<u8>,
    },
    /// `bit(N)` / `bit varying(N)`.
    Bit {
        /// Bit length.
        len: u32,
        /// True for `bit varying`.
        varying: bool,
    },
    /// `uuid`.
    Uuid,
    /// `json`.
    Json,
    /// `jsonb`.
    Jsonb,
    /// `inet` / `cidr` / `macaddr` / `macaddr8`.
    NetAddress(NetAddressKind),
    /// Array type — element + dimension count.
    Array {
        /// Element type.
        element: Box<ColumnType>,
        /// Number of dimensions (Postgres treats arrays as flat at the type level,
        /// but stores dim count in `pg_attribute.attndims`).
        dims: u8,
    },
    /// Reference to a user-defined type (enum, domain, composite).
    /// Structure is *not* introspected in v0.1.
    UserDefined(QualifiedName),
    /// Catch-all for types we don't yet model.
    /// Diff treats `Other` strictly: equal iff `raw` strings match exactly.
    Other {
        /// Raw type string from source or catalog.
        raw: String,
    },
}

/// Network-address subtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetAddressKind {
    /// `inet`.
    Inet,
    /// `cidr`.
    Cidr,
    /// `macaddr`.
    MacAddr,
    /// `macaddr8`.
    MacAddr8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_variants_are_distinct() {
        assert_ne!(ColumnType::SmallInt, ColumnType::Integer);
        assert_ne!(ColumnType::Integer, ColumnType::BigInt);
    }

    #[test]
    fn varchar_unbounded_distinct_from_text() {
        assert_ne!(ColumnType::Varchar { len: None }, ColumnType::Text);
    }

    #[test]
    fn array_recursive() {
        let nested = ColumnType::Array {
            element: Box::new(ColumnType::Array {
                element: Box::new(ColumnType::Integer),
                dims: 1,
            }),
            dims: 1,
        };
        // Smoke check: serde round-trip works for nested arrays.
        let json = serde_json::to_string(&nested).unwrap();
        let back: ColumnType = serde_json::from_str(&json).unwrap();
        assert_eq!(nested, back);
    }
}
