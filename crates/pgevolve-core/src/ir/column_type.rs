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

impl ColumnType {
    /// Parse a Postgres type string (as found in `pg_type.typname` or in source DDL)
    /// into the canonical `ColumnType`.
    ///
    /// Aliases are collapsed (e.g., `int4` → `Integer`). Unknown types fall through
    /// to [`ColumnType::Other`].
    pub fn parse_from_pg_type_string(raw: &str) -> Result<Self, ParseTypeError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ParseTypeError::Empty);
        }

        // Array suffix: count trailing "[]" pairs.
        let (base, dims) = strip_array_suffix(trimmed);
        if dims > 0 {
            let inner = Self::parse_from_pg_type_string(base)?;
            return Ok(Self::Array {
                element: Box::new(inner),
                dims,
            });
        }

        let lower = trimmed.to_ascii_lowercase();
        let parsed = parse_canonical(&lower).unwrap_or_else(|| Self::Other {
            raw: trimmed.to_string(),
        });
        Ok(parsed)
    }
}

fn strip_array_suffix(s: &str) -> (&str, u8) {
    let mut dims: u8 = 0;
    let mut cur = s;
    loop {
        let trimmed = cur.trim_end();
        if let Some(stripped) = trimmed.strip_suffix("[]") {
            dims = dims.saturating_add(1);
            cur = stripped;
        } else {
            return (trimmed, dims);
        }
    }
}

#[allow(clippy::too_many_lines)] // exhaustive type-name match by design
fn parse_canonical(s: &str) -> Option<ColumnType> {
    // Bare names first.
    let bare = match s {
        "int" | "integer" | "int4" => Some(ColumnType::Integer),
        "smallint" | "int2" => Some(ColumnType::SmallInt),
        "bigint" | "int8" => Some(ColumnType::BigInt),
        "bool" | "boolean" => Some(ColumnType::Boolean),
        "real" | "float4" => Some(ColumnType::Real),
        "double precision" | "float8" => Some(ColumnType::DoublePrecision),
        "text" => Some(ColumnType::Text),
        "bytea" => Some(ColumnType::Bytea),
        "date" => Some(ColumnType::Date),
        "uuid" => Some(ColumnType::Uuid),
        "json" => Some(ColumnType::Json),
        "jsonb" => Some(ColumnType::Jsonb),
        "inet" => Some(ColumnType::NetAddress(NetAddressKind::Inet)),
        "cidr" => Some(ColumnType::NetAddress(NetAddressKind::Cidr)),
        "macaddr" => Some(ColumnType::NetAddress(NetAddressKind::MacAddr)),
        "macaddr8" => Some(ColumnType::NetAddress(NetAddressKind::MacAddr8)),
        "varchar" | "character varying" => Some(ColumnType::Varchar { len: None }),
        "char" | "character" | "bpchar" => Some(ColumnType::Char { len: None }),
        "numeric" | "decimal" => Some(ColumnType::Numeric {
            precision: None,
            scale: None,
        }),
        "timestamp" | "timestamp without time zone" => Some(ColumnType::Timestamp {
            precision: None,
            with_tz: false,
        }),
        "timestamptz" | "timestamp with time zone" => Some(ColumnType::Timestamp {
            precision: None,
            with_tz: true,
        }),
        "time" | "time without time zone" => Some(ColumnType::Time {
            precision: None,
            with_tz: false,
        }),
        "timetz" | "time with time zone" => Some(ColumnType::Time {
            precision: None,
            with_tz: true,
        }),
        "interval" => Some(ColumnType::Interval {
            fields: None,
            precision: None,
        }),
        _ => None,
    };
    if let Some(v) = bare {
        return Some(v);
    }

    // Parameterized: <name>(<args>)[ <suffix>]
    let (head, args, suffix) = split_paren(s)?;
    let head = head.trim();
    let suffix = suffix.trim();

    match head {
        "varchar" | "character varying" => {
            let n: u32 = args.trim().parse().ok()?;
            Some(ColumnType::Varchar { len: Some(n) })
        }
        "char" | "character" | "bpchar" => {
            let n: u32 = args.trim().parse().ok()?;
            Some(ColumnType::Char { len: Some(n) })
        }
        "numeric" | "decimal" => {
            let mut parts = args.split(',').map(str::trim);
            let p: u16 = parts.next()?.parse().ok()?;
            let scale = parts
                .next()
                .map(str::trim)
                .map(str::parse)
                .transpose()
                .ok()?;
            Some(ColumnType::Numeric {
                precision: Some(p),
                scale,
            })
        }
        "timestamp" | "timestamp without time zone" => {
            let p: u8 = args.trim().parse().ok()?;
            let with_tz = suffix.eq_ignore_ascii_case("with time zone");
            Some(ColumnType::Timestamp {
                precision: Some(p),
                with_tz,
            })
        }
        "timestamptz" | "timestamp with time zone" => {
            let p: u8 = args.trim().parse().ok()?;
            Some(ColumnType::Timestamp {
                precision: Some(p),
                with_tz: true,
            })
        }
        "time" | "time without time zone" => {
            let p: u8 = args.trim().parse().ok()?;
            let with_tz = suffix.eq_ignore_ascii_case("with time zone");
            Some(ColumnType::Time {
                precision: Some(p),
                with_tz,
            })
        }
        "timetz" | "time with time zone" => {
            let p: u8 = args.trim().parse().ok()?;
            Some(ColumnType::Time {
                precision: Some(p),
                with_tz: true,
            })
        }
        "interval" => {
            // interval([fields,] precision) — pg_type stores this as `_interval` or with typmod;
            // for v0.1 we just accept a precision int.
            let p: u8 = args.trim().parse().ok()?;
            Some(ColumnType::Interval {
                fields: None,
                precision: Some(p),
            })
        }
        "bit" => {
            let n: u32 = args.trim().parse().ok()?;
            Some(ColumnType::Bit {
                len: n,
                varying: false,
            })
        }
        "bit varying" | "varbit" => {
            let n: u32 = args.trim().parse().ok()?;
            Some(ColumnType::Bit {
                len: n,
                varying: true,
            })
        }
        _ => None,
    }
}

fn split_paren(s: &str) -> Option<(&str, &str, &str)> {
    let open = s.find('(')?;
    let close = s.rfind(')')?;
    if close <= open {
        return None;
    }
    Some((&s[..open], &s[open + 1..close], &s[close + 1..]))
}

/// Errors from [`ColumnType::parse_from_pg_type_string`].
#[derive(Debug, thiserror::Error)]
pub enum ParseTypeError {
    /// The input was empty after trimming.
    #[error("type string is empty")]
    Empty,
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

    #[test]
    fn aliases_collapse_to_canonical() {
        let cases = [
            ("int", ColumnType::Integer),
            ("integer", ColumnType::Integer),
            ("int4", ColumnType::Integer),
            ("int2", ColumnType::SmallInt),
            ("smallint", ColumnType::SmallInt),
            ("int8", ColumnType::BigInt),
            ("bigint", ColumnType::BigInt),
            ("bool", ColumnType::Boolean),
            ("boolean", ColumnType::Boolean),
            ("float4", ColumnType::Real),
            ("real", ColumnType::Real),
            ("float8", ColumnType::DoublePrecision),
            ("double precision", ColumnType::DoublePrecision),
            ("text", ColumnType::Text),
            ("bytea", ColumnType::Bytea),
            ("date", ColumnType::Date),
            ("uuid", ColumnType::Uuid),
            ("json", ColumnType::Json),
            ("jsonb", ColumnType::Jsonb),
            ("inet", ColumnType::NetAddress(NetAddressKind::Inet)),
            ("cidr", ColumnType::NetAddress(NetAddressKind::Cidr)),
            ("macaddr", ColumnType::NetAddress(NetAddressKind::MacAddr)),
            ("macaddr8", ColumnType::NetAddress(NetAddressKind::MacAddr8)),
        ];
        for (src, expected) in cases {
            assert_eq!(
                ColumnType::parse_from_pg_type_string(src).unwrap(),
                expected,
                "input: {src}"
            );
        }
    }

    #[test]
    fn parameterized_types_parse() {
        assert_eq!(
            ColumnType::parse_from_pg_type_string("varchar(50)").unwrap(),
            ColumnType::Varchar { len: Some(50) }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("varchar").unwrap(),
            ColumnType::Varchar { len: None }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("character varying(10)").unwrap(),
            ColumnType::Varchar { len: Some(10) }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("character(8)").unwrap(),
            ColumnType::Char { len: Some(8) }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("char(8)").unwrap(),
            ColumnType::Char { len: Some(8) }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("numeric(10,2)").unwrap(),
            ColumnType::Numeric {
                precision: Some(10),
                scale: Some(2)
            }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("numeric(10)").unwrap(),
            ColumnType::Numeric {
                precision: Some(10),
                scale: None
            }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("decimal").unwrap(),
            ColumnType::Numeric {
                precision: None,
                scale: None
            }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("timestamp").unwrap(),
            ColumnType::Timestamp {
                precision: None,
                with_tz: false
            }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("timestamp(3) with time zone").unwrap(),
            ColumnType::Timestamp {
                precision: Some(3),
                with_tz: true
            }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("timestamptz").unwrap(),
            ColumnType::Timestamp {
                precision: None,
                with_tz: true
            }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("time(6) without time zone").unwrap(),
            ColumnType::Time {
                precision: Some(6),
                with_tz: false
            }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("timetz").unwrap(),
            ColumnType::Time {
                precision: None,
                with_tz: true
            }
        );
    }

    #[test]
    fn array_types_parse() {
        assert_eq!(
            ColumnType::parse_from_pg_type_string("integer[]").unwrap(),
            ColumnType::Array {
                element: Box::new(ColumnType::Integer),
                dims: 1
            }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("text[][]").unwrap(),
            ColumnType::Array {
                element: Box::new(ColumnType::Text),
                dims: 2
            }
        );
    }

    #[test]
    fn unknown_type_falls_through_to_other() {
        let t = ColumnType::parse_from_pg_type_string("nonexistent_type").unwrap();
        assert!(matches!(t, ColumnType::Other { ref raw } if raw == "nonexistent_type"));
    }

    #[test]
    fn empty_returns_error() {
        assert!(matches!(
            ColumnType::parse_from_pg_type_string(""),
            Err(ParseTypeError::Empty)
        ));
        assert!(matches!(
            ColumnType::parse_from_pg_type_string("   "),
            Err(ParseTypeError::Empty)
        ));
    }
}
