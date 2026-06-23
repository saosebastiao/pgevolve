//! `ColumnType` — the canonical normalized form of a Postgres data type.
//!
//! Every column type seen in source SQL or in the live catalog is translated
//! into this enum. Equivalence is decided by the `Equiv` impl in this module;
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
    /// `numeric` / `decimal` with optional precision (and, only then, optional scale).
    /// `precision: None` = unbounded `numeric`.
    Numeric {
        /// `None` = unbounded `numeric`; `Some` constrains precision (and
        /// optionally scale). Wrapping the pair in [`NumericPrecision`] makes
        /// the old `precision: None, scale: Some(_)` illegal state
        /// unrepresentable.
        precision: Option<NumericPrecision>,
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
        element: Box<Self>,
        /// Number of dimensions (Postgres treats arrays as flat at the type level,
        /// but stores dim count in `pg_attribute.attndims`).
        dims: u8,
    },
    /// Reference to a user-defined type (enum, domain, composite).
    /// Structure is *not* introspected in v0.1.
    UserDefined(QualifiedName),
    /// Catch-all for types we don't yet model.
    /// Equivalence treats `Other` strictly: equal iff `raw` strings match exactly.
    Other {
        /// Raw type string from source or catalog.
        raw: String,
    },
}

/// Precision/scale for a constrained `numeric(p[, s])`.
///
/// Scale is representable only *with* a precision — Postgres has no
/// `numeric(,s)` form — so the previous `precision: None, scale: Some(_)`
/// illegal state cannot be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NumericPrecision {
    /// Total digits (1..=1000).
    pub precision: u16,
    /// Digits to the right of the decimal point; `None` = scale 0.
    pub scale: Option<i16>,
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
    /// Returns `true` if this type has a default btree operator class in
    /// Postgres, making it usable in `CREATE STATISTICS` and B-tree indexes
    /// without an explicit `USING` opclass clause.
    ///
    /// Types that lack a default btree opclass (json, jsonb, arrays, bit/varbit,
    /// user-defined composite/range types, and unknown `Other` types) return
    /// `false`. PG rejects `CREATE STATISTICS` on such columns with error 0A000.
    ///
    /// # Notes
    ///
    /// - `UserDefined` is conservatively treated as **ineligible**: we cannot
    ///   determine opclass availability without live catalog introspection.
    ///   User-defined enums do get a btree opclass automatically, but we cannot
    ///   distinguish enums from composites/ranges/domains at this level of the IR.
    /// - `Array` types have no default btree opclass in any supported PG version.
    /// - `Bit` / `bit varying` have no default btree opclass.
    #[must_use]
    pub const fn has_default_btree_opclass(&self) -> bool {
        matches!(
            self,
            Self::Boolean
                | Self::SmallInt
                | Self::Integer
                | Self::BigInt
                | Self::Real
                | Self::DoublePrecision
                | Self::Numeric { .. }
                | Self::Text
                | Self::Varchar { .. }
                | Self::Char { .. }
                | Self::Bytea
                | Self::Date
                | Self::Time { .. }
                | Self::Timestamp { .. }
                | Self::Interval { .. }
                | Self::Uuid
                | Self::NetAddress(_)
        )
    }

    /// Render this type as canonical Postgres syntax.
    /// The output round-trips through [`Self::parse_from_pg_type_string`] back to `self`.
    #[allow(clippy::too_many_lines)] // exhaustive variant match by design
    pub fn render_sql(&self) -> String {
        match self {
            Self::Boolean => "boolean".into(),
            Self::SmallInt => "smallint".into(),
            Self::Integer => "integer".into(),
            Self::BigInt => "bigint".into(),
            Self::Real => "real".into(),
            Self::DoublePrecision => "double precision".into(),
            Self::Numeric { precision: None } => "numeric".into(),
            Self::Numeric {
                precision:
                    Some(NumericPrecision {
                        precision,
                        scale: None,
                    }),
            } => {
                format!("numeric({precision})")
            }
            Self::Numeric {
                precision:
                    Some(NumericPrecision {
                        precision,
                        scale: Some(s),
                    }),
            } => {
                format!("numeric({precision},{s})")
            }
            Self::Text => "text".into(),
            Self::Varchar { len: None } => "varchar".into(),
            Self::Varchar { len: Some(n) } => format!("varchar({n})"),
            Self::Char { len: None } => "char".into(),
            Self::Char { len: Some(n) } => format!("char({n})"),
            Self::Bytea => "bytea".into(),
            Self::Date => "date".into(),
            Self::Time { precision, with_tz } => match (precision, with_tz) {
                (None, false) => "time".into(),
                (Some(p), false) => format!("time({p})"),
                (None, true) => "time with time zone".into(),
                (Some(p), true) => format!("time({p}) with time zone"),
            },
            Self::Timestamp { precision, with_tz } => match (precision, with_tz) {
                (None, false) => "timestamp".into(),
                (Some(p), false) => format!("timestamp({p})"),
                (None, true) => "timestamp with time zone".into(),
                (Some(p), true) => format!("timestamp({p}) with time zone"),
            },
            Self::Interval {
                fields: None,
                precision: None,
            } => "interval".into(),
            Self::Interval {
                fields: None,
                precision: Some(p),
            } => format!("interval({p})"),
            Self::Interval {
                fields: Some(f),
                precision: None,
            } => format!("interval {f}"),
            Self::Interval {
                fields: Some(f),
                precision: Some(p),
            } => format!("interval {f}({p})"),
            Self::Bit {
                len,
                varying: false,
            } => format!("bit({len})"),
            Self::Bit { len, varying: true } => format!("bit varying({len})"),
            Self::Uuid => "uuid".into(),
            Self::Json => "json".into(),
            Self::Jsonb => "jsonb".into(),
            Self::NetAddress(NetAddressKind::Inet) => "inet".into(),
            Self::NetAddress(NetAddressKind::Cidr) => "cidr".into(),
            Self::NetAddress(NetAddressKind::MacAddr) => "macaddr".into(),
            Self::NetAddress(NetAddressKind::MacAddr8) => "macaddr8".into(),
            Self::Array { element, dims } => {
                let mut s = element.render_sql();
                for _ in 0..*dims {
                    s.push_str("[]");
                }
                s
            }
            Self::UserDefined(qname) => qname.render_sql(),
            Self::Other { raw } => raw.clone(),
        }
    }

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
            // PostGIS geometry/geography subtypes are case-insensitive, but the
            // source parser (pg_query) lowercases the bareword subtype while
            // pg_catalog.format_type emits canonical TitleCase. Store the
            // lowercase form so source and catalog IR compare equal (Other
            // equality is exact-string), avoiding a perpetual spurious diff.
            // Matches on the unqualified geometry/geography head only; all other
            // Other types keep their original case. (issue #40)
            raw: if is_postgis_geo_type(&lower) {
                lower
            } else {
                trimmed.to_string()
            },
        });
        Ok(parsed)
    }
}

/// Whether `lower` (already lowercased) names a `PostGIS` `geometry`/`geography`
/// type, bare or parameterized (`geometry(Point,4326)`), optionally
/// schema-qualified (`public.geometry(point,4326)`).
///
/// Matches on the *last* dot-separated segment of the head so that both
/// `geometry(Point,4326)` (issue #40) and `public.geometry(Point,4326)`
/// (issue #42) are recognised.
fn is_postgis_geo_type(lower: &str) -> bool {
    let head = lower.split_once('(').map_or(lower, |(h, _)| h).trim();
    let last = head.rsplit('.').next().unwrap_or(head);
    last == "geometry" || last == "geography"
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
        "numeric" | "decimal" => Some(ColumnType::Numeric { precision: None }),
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

    // `interval <fields>` — no parentheses; e.g. `interval hour to minute`.
    // Must come before the split_paren path so we don't confuse `interval year`
    // with a parameterized type whose head is `interval year`.
    if let Some(fields_part) = s.strip_prefix("interval ") {
        let fields_part = fields_part.trim();
        // Must not look like `interval(N)` — those have a `(` immediately after `interval`.
        // `fields_part` here is everything after "interval " (with a space), so it
        // is safe to treat as a fields qualifier.  If the suffix contains a `(…)`
        // at the end, that is the precision: `interval hour(6)`.
        return Some(parse_interval_with_fields(fields_part));
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
            let precision: u16 = parts.next()?.parse().ok()?;
            let scale = parts
                .next()
                .map(str::trim)
                .map(str::parse)
                .transpose()
                .ok()?;
            Some(ColumnType::Numeric {
                precision: Some(NumericPrecision { precision, scale }),
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

/// Parse `interval <fields>` or `interval <fields>(p)` where `rest` is
/// everything after the `"interval "` prefix (already trimmed, already lowercased).
///
/// Examples:
/// - `"year to month"` → `Interval { fields: Some("year to month"), precision: None }`
/// - `"hour(6)"` → `Interval { fields: Some("hour"), precision: Some(6) }`
/// - `"day to second(3)"` → `Interval { fields: Some("day to second"), precision: Some(3) }`
fn parse_interval_with_fields(rest: &str) -> ColumnType {
    // Check whether the qualifier ends with `(p)` — a trailing precision.
    // Use rfind so nested parens in unusual inputs don't confuse us.
    if let (Some(open), Some(close)) = (rest.rfind('('), rest.rfind(')'))
        && close == rest.len() - 1
        && open < close
    {
        let fields_part = rest[..open].trim();
        let prec_str = &rest[open + 1..close];
        if let Ok(p) = prec_str.trim().parse::<u8>() {
            let fields = if fields_part.is_empty() {
                None
            } else {
                Some(fields_part.to_string())
            };
            return ColumnType::Interval {
                fields,
                precision: Some(p),
            };
        }
    }
    // No precision — the whole rest is the fields qualifier.
    ColumnType::Interval {
        fields: Some(rest.to_string()),
        precision: None,
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

impl crate::ir::eq::Equiv for ColumnType {
    fn differences(&self, other: &Self) -> Vec<crate::ir::difference::Difference> {
        if self == other {
            Vec::new()
        } else {
            vec![crate::ir::difference::Difference::new(
                "",
                self.render_sql(),
                other.render_sql(),
            )]
        }
    }
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
                precision: Some(NumericPrecision {
                    precision: 10,
                    scale: Some(2)
                })
            }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("numeric(10)").unwrap(),
            ColumnType::Numeric {
                precision: Some(NumericPrecision {
                    precision: 10,
                    scale: None
                })
            }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("decimal").unwrap(),
            ColumnType::Numeric { precision: None }
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

    /// Unit tests for the `is_postgis_geo_type` helper, including schema-qualified
    /// forms (issue #42).
    #[test]
    fn is_postgis_geo_type_recognises_schema_qualified() {
        // Bare unqualified — #40 cases must still work.
        assert!(is_postgis_geo_type("geometry"));
        assert!(is_postgis_geo_type("geography"));
        assert!(is_postgis_geo_type("geometry(point,4326)"));

        // Schema-qualified — new #42 cases.
        assert!(is_postgis_geo_type("public.geometry"));
        assert!(is_postgis_geo_type("public.geometry(point,4326)"));
        assert!(is_postgis_geo_type("myschema.geography(multipolygon,4326)"));

        // Negative: other user types must NOT match.
        assert!(!is_postgis_geo_type("public.mytype"));
        assert!(!is_postgis_geo_type("mytype(foo,1)"));
        assert!(!is_postgis_geo_type("public.mygeometry")); // suffix "geometry" not exact last segment
    }

    /// Schema-qualified `PostGIS` types normalise their subtype casing (issue #42).
    #[test]
    fn schema_qualified_postgis_geo_subtype_casing_is_normalized() {
        // `public.geometry(Point,4326)` (TitleCase, as from pg_catalog.format_type) must
        // normalise to `public.geometry(point,4326)` (lowercase subtype).
        let from_catalog =
            ColumnType::parse_from_pg_type_string("public.geometry(Point,4326)").unwrap();
        assert!(
            matches!(&from_catalog, ColumnType::Other { raw } if raw == "public.geometry(point,4326)"),
            "got {from_catalog:?}"
        );
        // Already-lowercase form is a no-op.
        let from_source =
            ColumnType::parse_from_pg_type_string("public.geometry(point,4326)").unwrap();
        assert_eq!(from_catalog, from_source);

        // Same for geography.
        let geo_catalog =
            ColumnType::parse_from_pg_type_string("public.geography(MultiPolygon,4326)").unwrap();
        let geo_source =
            ColumnType::parse_from_pg_type_string("public.geography(multipolygon,4326)").unwrap();
        assert_eq!(geo_catalog, geo_source);
    }

    #[test]
    fn postgis_geo_subtype_casing_is_normalized() {
        // The source parser (pg_query) lowercases the bareword subtype while the
        // catalog's `format_type` emits canonical TitleCase. Both must produce an
        // equal `Other` so there is no perpetual spurious ALTER COLUMN TYPE diff
        // against a live PostGIS database. (issue #40)
        let from_catalog = ColumnType::parse_from_pg_type_string("geometry(Point,4326)").unwrap();
        let from_source = ColumnType::parse_from_pg_type_string("geometry(point,4326)").unwrap();
        assert_eq!(from_catalog, from_source);
        assert!(
            matches!(&from_catalog, ColumnType::Other { raw } if raw == "geometry(point,4326)"),
            "got {from_catalog:?}"
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("geography(MultiPolygon,4326)").unwrap(),
            ColumnType::parse_from_pg_type_string("geography(multipolygon,4326)").unwrap(),
        );
        // The array form normalizes too: parse strips "[]" and recurses, so the
        // element's geo casing is reconciled inside the recursive call.
        let arr_catalog = ColumnType::parse_from_pg_type_string("geometry(Point,4326)[]").unwrap();
        assert_eq!(
            arr_catalog,
            ColumnType::parse_from_pg_type_string("geometry(point,4326)[]").unwrap(),
        );
        match arr_catalog {
            ColumnType::Array { element, dims } => {
                assert_eq!(dims, 1);
                assert!(
                    matches!(&*element, ColumnType::Other { raw } if raw == "geometry(point,4326)"),
                    "got {element:?}"
                );
            }
            other => panic!("expected Array, got {other:?}"),
        }
    }

    #[test]
    fn non_geo_other_preserves_case() {
        // Normalization is scoped to geometry/geography only — every other Other
        // type keeps its original case, bare or parameterized.
        let bare = ColumnType::parse_from_pg_type_string("MyDomain").unwrap();
        assert!(matches!(bare, ColumnType::Other { ref raw } if raw == "MyDomain"));
        let parameterized = ColumnType::parse_from_pg_type_string("MyType(Foo,4326)").unwrap();
        assert!(
            matches!(parameterized, ColumnType::Other { ref raw } if raw == "MyType(Foo,4326)")
        );
    }

    #[test]
    fn render_sql_round_trips_canonical() {
        let cases = [
            ColumnType::Boolean,
            ColumnType::Integer,
            ColumnType::BigInt,
            ColumnType::Text,
            ColumnType::Varchar { len: None },
            ColumnType::Varchar { len: Some(50) },
            ColumnType::Char { len: Some(8) },
            ColumnType::Numeric { precision: None },
            ColumnType::Numeric {
                precision: Some(NumericPrecision {
                    precision: 10,
                    scale: Some(2),
                }),
            },
            ColumnType::Timestamp {
                precision: None,
                with_tz: false,
            },
            ColumnType::Timestamp {
                precision: Some(3),
                with_tz: true,
            },
            ColumnType::Time {
                precision: None,
                with_tz: true,
            },
            ColumnType::Uuid,
            ColumnType::Jsonb,
            ColumnType::NetAddress(NetAddressKind::Inet),
            ColumnType::Bit {
                len: 8,
                varying: false,
            },
            ColumnType::Bit {
                len: 8,
                varying: true,
            },
            ColumnType::Array {
                element: Box::new(ColumnType::Integer),
                dims: 2,
            },
            // Normalized PostGIS geo Other must be a render->parse fixed point,
            // or the diff engine would oscillate. raw is already lowercase, so
            // the geo re-normalization on reparse is a no-op.
            ColumnType::Other {
                raw: "geometry(point,4326)".to_string(),
            },
        ];
        for t in cases {
            let rendered = t.render_sql();
            let parsed = ColumnType::parse_from_pg_type_string(&rendered).unwrap();
            assert_eq!(parsed, t, "rendered: {rendered}");
        }
    }

    #[test]
    fn numeric_constrained_round_trips() {
        let n = ColumnType::Numeric {
            precision: Some(NumericPrecision {
                precision: 10,
                scale: Some(2),
            }),
        };
        let j = serde_json::to_string(&n).unwrap();
        assert_eq!(
            ColumnType::parse_from_pg_type_string(&n.render_sql()).unwrap(),
            n
        );
        assert_eq!(serde_json::from_str::<ColumnType>(&j).unwrap(), n);
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

    /// Exhaustive table-driven test: every `ColumnType` variant is listed and
    /// its expected `has_default_btree_opclass()` value is asserted.
    #[test]
    fn has_default_btree_opclass_all_variants() {
        // Eligible types — PG has a built-in default btree opclass.
        let eligible: &[ColumnType] = &[
            ColumnType::Boolean,
            ColumnType::SmallInt,
            ColumnType::Integer,
            ColumnType::BigInt,
            ColumnType::Real,
            ColumnType::DoublePrecision,
            ColumnType::Numeric { precision: None },
            ColumnType::Numeric {
                precision: Some(NumericPrecision {
                    precision: 10,
                    scale: Some(2),
                }),
            },
            ColumnType::Text,
            ColumnType::Varchar { len: None },
            ColumnType::Varchar { len: Some(50) },
            ColumnType::Char { len: None },
            ColumnType::Char { len: Some(8) },
            ColumnType::Bytea,
            ColumnType::Date,
            ColumnType::Time {
                precision: None,
                with_tz: false,
            },
            ColumnType::Time {
                precision: None,
                with_tz: true,
            },
            ColumnType::Timestamp {
                precision: None,
                with_tz: false,
            },
            ColumnType::Timestamp {
                precision: None,
                with_tz: true,
            },
            ColumnType::Interval {
                fields: None,
                precision: None,
            },
            ColumnType::Uuid,
            ColumnType::NetAddress(NetAddressKind::Inet),
            ColumnType::NetAddress(NetAddressKind::Cidr),
            ColumnType::NetAddress(NetAddressKind::MacAddr),
            ColumnType::NetAddress(NetAddressKind::MacAddr8),
        ];
        for ty in eligible {
            assert!(
                ty.has_default_btree_opclass(),
                "expected eligible but got ineligible: {ty:?}"
            );
        }

        // Ineligible types — PG rejects CREATE STATISTICS / btree index without
        // an explicit opclass.
        let ineligible: &[ColumnType] = &[
            ColumnType::Json,
            ColumnType::Jsonb,
            ColumnType::Bit {
                len: 8,
                varying: false,
            },
            ColumnType::Bit {
                len: 8,
                varying: true,
            },
            ColumnType::Array {
                element: Box::new(ColumnType::Integer),
                dims: 1,
            },
            ColumnType::UserDefined(crate::identifier::QualifiedName::new(
                crate::identifier::Identifier::from_unquoted("public").unwrap(),
                crate::identifier::Identifier::from_unquoted("my_enum").unwrap(),
            )),
            ColumnType::Other {
                raw: "circle".into(),
            },
        ];
        for ty in ineligible {
            assert!(
                !ty.has_default_btree_opclass(),
                "expected ineligible but got eligible: {ty:?}"
            );
        }
    }

    #[test]
    fn columntype_diff_empty_when_equal() {
        use crate::ir::eq::Equiv;
        let a = ColumnType::Varchar { len: Some(50) };
        let b = ColumnType::Varchar { len: Some(50) };
        assert!(a.differences(&b).is_empty());
        assert!(a.canonical_eq(&b));
    }

    #[test]
    fn columntype_diff_reports_difference() {
        use crate::ir::eq::Equiv;
        let a = ColumnType::Varchar { len: Some(50) };
        let b = ColumnType::Varchar { len: Some(100) };
        let diffs = a.differences(&b);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].from, "varchar(50)");
        assert_eq!(diffs[0].to, "varchar(100)");
    }

    /// Table-driven: parse each canonical `interval` string and check it produces
    /// the expected `ColumnType::Interval`. Covers issue #41: `interval(N)` and
    /// `interval <fields>` must parse to typed `Interval`, not fall through to `Other`.
    #[test]
    fn interval_canonical_forms_parse() {
        let cases: &[(&str, ColumnType)] = &[
            (
                "interval",
                ColumnType::Interval {
                    fields: None,
                    precision: None,
                },
            ),
            (
                "interval(6)",
                ColumnType::Interval {
                    fields: None,
                    precision: Some(6),
                },
            ),
            (
                "interval year",
                ColumnType::Interval {
                    fields: Some("year".to_string()),
                    precision: None,
                },
            ),
            (
                "interval month",
                ColumnType::Interval {
                    fields: Some("month".to_string()),
                    precision: None,
                },
            ),
            (
                "interval day",
                ColumnType::Interval {
                    fields: Some("day".to_string()),
                    precision: None,
                },
            ),
            (
                "interval hour",
                ColumnType::Interval {
                    fields: Some("hour".to_string()),
                    precision: None,
                },
            ),
            (
                "interval minute",
                ColumnType::Interval {
                    fields: Some("minute".to_string()),
                    precision: None,
                },
            ),
            (
                "interval second",
                ColumnType::Interval {
                    fields: Some("second".to_string()),
                    precision: None,
                },
            ),
            (
                "interval year to month",
                ColumnType::Interval {
                    fields: Some("year to month".to_string()),
                    precision: None,
                },
            ),
            (
                "interval day to second",
                ColumnType::Interval {
                    fields: Some("day to second".to_string()),
                    precision: None,
                },
            ),
            (
                "interval hour to minute",
                ColumnType::Interval {
                    fields: Some("hour to minute".to_string()),
                    precision: None,
                },
            ),
            (
                "interval hour(6)",
                ColumnType::Interval {
                    fields: Some("hour".to_string()),
                    precision: Some(6),
                },
            ),
        ];
        for (src, expected) in cases {
            let got = ColumnType::parse_from_pg_type_string(src).unwrap();
            assert_eq!(got, *expected, "input: {src}");
        }
    }

    /// Case-insensitive input collapses to lowercase canonical fields.
    #[test]
    fn interval_fields_are_case_insensitive() {
        let upper = ColumnType::parse_from_pg_type_string("interval YEAR TO MONTH").unwrap();
        let lower = ColumnType::parse_from_pg_type_string("interval year to month").unwrap();
        assert_eq!(upper, lower);
        assert_eq!(
            upper,
            ColumnType::Interval {
                fields: Some("year to month".to_string()),
                precision: None,
            }
        );
    }

    /// Display round-trip: every `Interval` variant renders to a string that
    /// parses back to the same value.
    #[test]
    fn interval_display_round_trips() {
        let cases: &[ColumnType] = &[
            ColumnType::Interval {
                fields: None,
                precision: None,
            },
            ColumnType::Interval {
                fields: None,
                precision: Some(6),
            },
            ColumnType::Interval {
                fields: Some("year".to_string()),
                precision: None,
            },
            ColumnType::Interval {
                fields: Some("hour to minute".to_string()),
                precision: None,
            },
            ColumnType::Interval {
                fields: Some("day to second".to_string()),
                precision: None,
            },
            ColumnType::Interval {
                fields: Some("hour".to_string()),
                precision: Some(6),
            },
        ];
        for t in cases {
            let rendered = t.render_sql();
            let parsed = ColumnType::parse_from_pg_type_string(&rendered).unwrap();
            assert_eq!(parsed, *t, "rendered: {rendered}");
        }
    }
}
