# Phase 1 — IR foundations

**Goal:** Land all v0.1 IR types in `pgevolve-core`, plus the `Diff` trait and equivalence machinery. This phase produces no behavior visible from the CLI yet — its output is a fully tested in-memory data model that everything else (parser, catalog reader, differ) builds on.

**Spec coverage:** §5 (entire IR design), §13 (error type scaffolding for `IrError`).

**Depends on:** Phase 0 complete.

**Exit criteria:**

- All IR types land with `#[derive(Debug, Clone)]`, `serde::Serialize` + `Deserialize`, and `PartialEq`/`Eq`/`Hash` where appropriate.
- `canonical_eq` and `Diff::diff` are implemented and tested for every IR type.
- `ColumnType` covers every type listed in spec §5.2 (Integer, Boolean, Varchar with optional length, Numeric, Text, Time, Timestamp, Bit, Interval, Date, etc.) and round-trips with normalization rules.
- `DefaultExpr` round-trips and normalizes redundant casts.
- `cargo test -p pgevolve-core` runs > 50 unit tests, all passing.
- `cargo clippy` and `cargo fmt --check` clean.

---

## File structure introduced this phase

```
crates/pgevolve-core/src/
├── lib.rs                      # add module declarations
├── identifier.rs               # Identifier, QualifiedName
├── error.rs                    # top-level error scaffold
└── ir/
    ├── mod.rs                  # public re-exports
    ├── catalog.rs              # Catalog struct
    ├── schema.rs               # Schema
    ├── table.rs                # Table
    ├── column.rs               # Column, Identity, Generated
    ├── column_type.rs          # ColumnType + normalization
    ├── default_expr.rs         # DefaultExpr, LiteralValue, NormalizedExpr
    ├── constraint.rs           # Constraint, ConstraintKind, ForeignKey, ReferentialAction
    ├── index.rs                # Index, IndexMethod, IndexColumn, NullsOrder
    ├── sequence.rs             # Sequence
    ├── difference.rs           # Difference enum + helpers
    └── eq.rs                   # Diff trait + canonical_eq helpers
```

`error.rs` only adds the scaffolding for `IrError` here; per-phase error variants are added by their respective phases.

---

### Task 1.1: Add the IR module skeleton

**Files:**
- Modify: `crates/pgevolve-core/src/lib.rs`
- Create: `crates/pgevolve-core/src/identifier.rs`
- Create: `crates/pgevolve-core/src/error.rs`
- Create: `crates/pgevolve-core/src/ir/mod.rs`

- [ ] **Step 1: Add module declarations to `lib.rs`**

`crates/pgevolve-core/src/lib.rs` (replace the existing content):

```rust
//! `pgevolve-core` — the declarative-schema-management engine.
//!
//! See `docs/superpowers/specs/2026-05-09-pgevolve-design.md` for the design.
#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod error;
pub mod identifier;
pub mod ir;

/// Crate version, exposed for embedding in plan manifests.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_nonempty() {
        assert!(!VERSION.is_empty());
    }
}
```

- [ ] **Step 2: Add error scaffold**

`crates/pgevolve-core/src/error.rs`:

```rust
//! Top-level error types for `pgevolve-core`.
//!
//! Per-phase error variants (parse, catalog, diff, plan) are added by their
//! respective modules. This file declares the umbrella type and re-exports.

use thiserror::Error;

/// Top-level error type. Each variant carries the typed error from one phase.
#[derive(Debug, Error)]
pub enum Error {
    /// IR-construction error (e.g., invalid identifier).
    #[error(transparent)]
    Ir(#[from] crate::ir::IrError),
    // Parse, Catalog, Diff, Plan variants added by later phases.
}

/// Result alias for crate-level operations.
pub type Result<T> = std::result::Result<T, Error>;
```

- [ ] **Step 3: Add IR module skeleton**

`crates/pgevolve-core/src/ir/mod.rs`:

```rust
//! In-memory representation of a Postgres schema.
//!
//! The IR is the contract between every other component. Both the source-side
//! parser and the catalog reader produce these types; the differ, dependency
//! analyzer, and planner consume them.

pub mod catalog;
pub mod column;
pub mod column_type;
pub mod constraint;
pub mod default_expr;
pub mod difference;
pub mod eq;
pub mod index;
pub mod schema;
pub mod sequence;
pub mod table;

pub use catalog::Catalog;
pub use column::{Column, Generated, GeneratedKind, Identity, IdentityKind};
pub use column_type::ColumnType;
pub use constraint::{Constraint, ConstraintKind, ForeignKey, ReferentialAction};
pub use default_expr::{DefaultExpr, LiteralValue, NormalizedExpr};
pub use difference::Difference;
pub use eq::Diff;
pub use index::{Index, IndexColumn, IndexMethod, NullsOrder, SortOrder};
pub use schema::Schema;
pub use sequence::Sequence;
pub use table::Table;

use thiserror::Error;

/// Errors raised when constructing IR values.
#[derive(Debug, Error)]
pub enum IrError {
    /// An identifier did not satisfy validation rules.
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),

    /// A type definition was not representable in our IR.
    #[error("invalid column type: {0}")]
    InvalidColumnType(String),

    /// A required field was missing or empty.
    #[error("missing required field: {0}")]
    MissingField(&'static str),
}
```

- [ ] **Step 4: Add identifier module (placeholder)**

`crates/pgevolve-core/src/identifier.rs`:

```rust
//! Identifier and qualified-name types.
//!
//! Real implementation lands in task 1.2.

#![allow(dead_code)] // populated in task 1.2
```

- [ ] **Step 5: Verify build**

Run: `cargo check -p pgevolve-core`
Expected: succeeds. Empty IR submodules are OK because nothing references them yet — but the re-exports do reference them, so each submodule must at least exist.

Create empty placeholder files for each IR submodule referenced by `mod.rs`:

```bash
touch crates/pgevolve-core/src/ir/catalog.rs
touch crates/pgevolve-core/src/ir/column.rs
touch crates/pgevolve-core/src/ir/column_type.rs
touch crates/pgevolve-core/src/ir/constraint.rs
touch crates/pgevolve-core/src/ir/default_expr.rs
touch crates/pgevolve-core/src/ir/difference.rs
touch crates/pgevolve-core/src/ir/eq.rs
touch crates/pgevolve-core/src/ir/index.rs
touch crates/pgevolve-core/src/ir/schema.rs
touch crates/pgevolve-core/src/ir/sequence.rs
touch crates/pgevolve-core/src/ir/table.rs
```

Then add minimal `pub use` placeholders so the `mod.rs` re-exports compile. **Easier alternative:** strip the `pub use` lines for now and re-add as each submodule lands. Use this approach — it lets each subsequent task land independently.

Replace `mod.rs` to remove the broken re-exports (we'll add them back per-task):

```rust
//! In-memory representation of a Postgres schema.

pub mod catalog;
pub mod column;
pub mod column_type;
pub mod constraint;
pub mod default_expr;
pub mod difference;
pub mod eq;
pub mod index;
pub mod schema;
pub mod sequence;
pub mod table;

use thiserror::Error;

/// Errors raised when constructing IR values.
#[derive(Debug, Error)]
pub enum IrError {
    /// An identifier did not satisfy validation rules.
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),

    /// A type definition was not representable in our IR.
    #[error("invalid column type: {0}")]
    InvalidColumnType(String),

    /// A required field was missing or empty.
    #[error("missing required field: {0}")]
    MissingField(&'static str),
}
```

- [ ] **Step 6: Verify**

Run: `cargo check -p pgevolve-core`
Expected: succeeds.

- [ ] **Step 7: Commit**

```bash
git add crates/pgevolve-core
git commit -m "feat(core): scaffold IR module tree and IrError

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 1.2: `Identifier` and `QualifiedName`

**Files:**
- Modify: `crates/pgevolve-core/src/identifier.rs`
- Modify: `crates/pgevolve-core/src/lib.rs` (already declares `pub mod identifier`)

- [ ] **Step 1: Write failing tests**

`crates/pgevolve-core/src/identifier.rs`:

```rust
//! Identifier and qualified-name types.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A single Postgres identifier (e.g., a table name).
///
/// Postgres identifier rules:
/// - Length: 1..=63 bytes (NAMEDATALEN).
/// - Unquoted: starts with `[A-Za-z_]` followed by `[A-Za-z0-9_$]*`.
/// - Quoted: any UTF-8 except `"` (we accept any non-empty UTF-8 here; pg_query
///   will reject anything postgres can't actually accept at parse time).
///
/// We store identifiers in their *case-folded canonical form* for unquoted
/// inputs (Postgres lowercases unquoted identifiers) and in their original
/// form for quoted inputs. The constructor distinguishes the two cases via
/// [`Identifier::from_unquoted`] vs [`Identifier::from_quoted`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Identifier(String);

/// Errors raised when constructing an [`Identifier`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdentifierError {
    /// The identifier was empty.
    #[error("identifier is empty")]
    Empty,
    /// The identifier exceeded Postgres's 63-byte limit.
    #[error("identifier exceeds 63 bytes: got {0}")]
    TooLong(usize),
    /// The unquoted identifier contained invalid characters.
    #[error("unquoted identifier contains invalid characters: {0:?}")]
    InvalidUnquotedChars(String),
}

impl Identifier {
    /// Construct from an unquoted identifier source — lowercases per Postgres rules.
    pub fn from_unquoted(s: &str) -> Result<Self, IdentifierError> {
        if s.is_empty() {
            return Err(IdentifierError::Empty);
        }
        if s.len() > 63 {
            return Err(IdentifierError::TooLong(s.len()));
        }
        let mut chars = s.chars();
        let first = chars.next().expect("non-empty checked above");
        if !(first.is_ascii_alphabetic() || first == '_') {
            return Err(IdentifierError::InvalidUnquotedChars(s.to_string()));
        }
        for c in chars {
            if !(c.is_ascii_alphanumeric() || c == '_' || c == '$') {
                return Err(IdentifierError::InvalidUnquotedChars(s.to_string()));
            }
        }
        Ok(Self(s.to_ascii_lowercase()))
    }

    /// Construct from a quoted identifier — preserves case.
    pub fn from_quoted(s: &str) -> Result<Self, IdentifierError> {
        if s.is_empty() {
            return Err(IdentifierError::Empty);
        }
        if s.len() > 63 {
            return Err(IdentifierError::TooLong(s.len()));
        }
        Ok(Self(s.to_string()))
    }

    /// Returns the inner canonical string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Renders this identifier as it would appear in SQL — quoted iff necessary.
    pub fn render_sql(&self) -> String {
        if needs_quoting(&self.0) {
            format!("\"{}\"", self.0.replace('"', "\"\""))
        } else {
            self.0.clone()
        }
    }
}

fn needs_quoting(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    if RESERVED_KEYWORDS.binary_search(&s).is_ok() {
        return true;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_lowercase() || first == '_') {
        return true;
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return true;
        }
    }
    false
}

// Sorted; binary searched. Source: Postgres docs (reserved + reserved-non-function-or-type).
// This list is intentionally conservative — it errs on the side of quoting.
static RESERVED_KEYWORDS: &[&str] = &[
    "all", "analyse", "analyze", "and", "any", "array", "as", "asc", "asymmetric",
    "both", "case", "cast", "check", "collate", "column", "constraint", "create",
    "current_catalog", "current_date", "current_role", "current_time",
    "current_timestamp", "current_user", "default", "deferrable", "desc",
    "distinct", "do", "else", "end", "except", "false", "fetch", "for", "foreign",
    "from", "grant", "group", "having", "in", "initially", "intersect", "into",
    "lateral", "leading", "limit", "localtime", "localtimestamp", "not", "null",
    "offset", "on", "only", "or", "order", "placing", "primary", "references",
    "returning", "select", "session_user", "some", "symmetric", "table",
    "then", "to", "trailing", "true", "union", "unique", "user", "using",
    "variadic", "when", "where", "window", "with",
];

impl fmt::Display for Identifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Identifier {
    type Err = IdentifierError;
    /// Parses as an unquoted identifier.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_unquoted(s)
    }
}

/// A schema-qualified identifier — e.g., `app.users` or `"AppSchema".users`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QualifiedName {
    /// The schema component.
    pub schema: Identifier,
    /// The object name.
    pub name: Identifier,
}

impl QualifiedName {
    /// Construct from two identifiers.
    pub fn new(schema: Identifier, name: Identifier) -> Self {
        Self { schema, name }
    }

    /// Renders as it would appear in SQL.
    pub fn render_sql(&self) -> String {
        format!("{}.{}", self.schema.render_sql(), self.name.render_sql())
    }
}

impl fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.schema, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn unquoted_lowercases() {
        let id = Identifier::from_unquoted("Users").unwrap();
        assert_eq!(id.as_str(), "users");
    }

    #[test]
    fn quoted_preserves_case() {
        let id = Identifier::from_quoted("Users").unwrap();
        assert_eq!(id.as_str(), "Users");
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(Identifier::from_unquoted(""), Err(IdentifierError::Empty));
        assert_eq!(Identifier::from_quoted(""), Err(IdentifierError::Empty));
    }

    #[test]
    fn rejects_overlong() {
        let long = "a".repeat(64);
        assert!(matches!(
            Identifier::from_unquoted(&long),
            Err(IdentifierError::TooLong(64))
        ));
    }

    #[test]
    fn rejects_unquoted_starting_with_digit() {
        assert!(matches!(
            Identifier::from_unquoted("1foo"),
            Err(IdentifierError::InvalidUnquotedChars(_))
        ));
    }

    #[test]
    fn quoted_allows_special_chars() {
        let id = Identifier::from_quoted("foo bar").unwrap();
        assert_eq!(id.as_str(), "foo bar");
    }

    #[test]
    fn render_sql_quotes_when_necessary() {
        assert_eq!(Identifier::from_unquoted("users").unwrap().render_sql(), "users");
        assert_eq!(Identifier::from_quoted("Users").unwrap().render_sql(), "\"Users\"");
        assert_eq!(Identifier::from_quoted("select").unwrap().render_sql(), "\"select\"");
    }

    #[test]
    fn render_sql_escapes_embedded_quotes() {
        let id = Identifier::from_quoted("a\"b").unwrap();
        assert_eq!(id.render_sql(), "\"a\"\"b\"");
    }

    #[test]
    fn qualified_name_renders() {
        let qn = QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted("users").unwrap(),
        );
        assert_eq!(qn.render_sql(), "app.users");
        assert_eq!(qn.to_string(), "app.users");
    }
}
```

- [ ] **Step 2: Run tests, expect them to pass first time**

Run: `cargo test -p pgevolve-core --lib identifier`
Expected: 9 passing tests.

- [ ] **Step 3: Commit**

```bash
git add crates/pgevolve-core/src/identifier.rs
git commit -m "feat(core): Identifier and QualifiedName with quoting/lowercasing rules

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 1.3: `ColumnType` enum (variants only, no normalization yet)

**Files:**
- Modify: `crates/pgevolve-core/src/ir/column_type.rs`
- Modify: `crates/pgevolve-core/src/ir/mod.rs` (re-export)

- [ ] **Step 1: Write the type definition + a small variant test**

`crates/pgevolve-core/src/ir/column_type.rs`:

```rust
//! `ColumnType` — the canonical normalized form of a Postgres data type.
//!
//! Every column type seen in source SQL or in the live catalog is translated
//! into this enum. Equivalence is decided by [`canonical_eq`]; rendering back
//! to SQL is via [`render_sql`].

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
```

> The serde round-trip test requires `serde_json` as a dev-dep. Add it:

`crates/pgevolve-core/Cargo.toml` — under `[dev-dependencies]`, add:

```toml
serde_json = { workspace = true }
```

- [ ] **Step 2: Re-export from `ir/mod.rs`**

Add to `crates/pgevolve-core/src/ir/mod.rs` (after `IrError`):

```rust
pub use column_type::{ColumnType, NetAddressKind};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p pgevolve-core --lib ir::column_type`
Expected: 3 passing tests.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core
git commit -m "feat(core): ColumnType enum with all v0.1 variants

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 1.4: `ColumnType::parse_from_pg_type_string` + alias normalization

**Files:**
- Modify: `crates/pgevolve-core/src/ir/column_type.rs`

This task implements the normalization that collapses Postgres type aliases (`int4` ≡ `integer`, `bool` ≡ `boolean`, etc.) and parses parameterized types from their `pg_type.typname`-style strings. Both source-side and catalog-side go through this function.

- [ ] **Step 1: Write failing tests**

Append to `crates/pgevolve-core/src/ir/column_type.rs` `tests` module:

```rust
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
            ColumnType::Numeric { precision: Some(10), scale: Some(2) }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("numeric(10)").unwrap(),
            ColumnType::Numeric { precision: Some(10), scale: None }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("decimal").unwrap(),
            ColumnType::Numeric { precision: None, scale: None }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("timestamp").unwrap(),
            ColumnType::Timestamp { precision: None, with_tz: false }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("timestamp(3) with time zone").unwrap(),
            ColumnType::Timestamp { precision: Some(3), with_tz: true }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("timestamptz").unwrap(),
            ColumnType::Timestamp { precision: None, with_tz: true }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("time(6) without time zone").unwrap(),
            ColumnType::Time { precision: Some(6), with_tz: false }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("timetz").unwrap(),
            ColumnType::Time { precision: None, with_tz: true }
        );
    }

    #[test]
    fn array_types_parse() {
        assert_eq!(
            ColumnType::parse_from_pg_type_string("integer[]").unwrap(),
            ColumnType::Array { element: Box::new(ColumnType::Integer), dims: 1 }
        );
        assert_eq!(
            ColumnType::parse_from_pg_type_string("text[][]").unwrap(),
            ColumnType::Array { element: Box::new(ColumnType::Text), dims: 2 }
        );
    }

    #[test]
    fn unknown_type_falls_through_to_other() {
        let t = ColumnType::parse_from_pg_type_string("nonexistent_type").unwrap();
        assert!(matches!(t, ColumnType::Other { ref raw } if raw == "nonexistent_type"));
    }
```

- [ ] **Step 2: Run tests, expect them to fail**

Run: `cargo test -p pgevolve-core --lib ir::column_type`
Expected: failing tests for `parse_from_pg_type_string` (function not found).

- [ ] **Step 3: Implement `parse_from_pg_type_string`**

Append to `crates/pgevolve-core/src/ir/column_type.rs` (above the `tests` module):

```rust
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
            return Ok(ColumnType::Array { element: Box::new(inner), dims });
        }

        let lower = trimmed.to_ascii_lowercase();
        let parsed = parse_canonical(&lower).unwrap_or(ColumnType::Other { raw: trimmed.to_string() });
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
        "numeric" | "decimal" => Some(ColumnType::Numeric { precision: None, scale: None }),
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
        "interval" => Some(ColumnType::Interval { fields: None, precision: None }),
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
            let scale = parts.next().map(str::trim).map(str::parse).transpose().ok()?;
            Some(ColumnType::Numeric { precision: Some(p), scale })
        }
        "timestamp" | "timestamp without time zone" => {
            let p: u8 = args.trim().parse().ok()?;
            let with_tz = suffix.eq_ignore_ascii_case("with time zone");
            Some(ColumnType::Timestamp { precision: Some(p), with_tz })
        }
        "timestamptz" | "timestamp with time zone" => {
            let p: u8 = args.trim().parse().ok()?;
            Some(ColumnType::Timestamp { precision: Some(p), with_tz: true })
        }
        "time" | "time without time zone" => {
            let p: u8 = args.trim().parse().ok()?;
            let with_tz = suffix.eq_ignore_ascii_case("with time zone");
            Some(ColumnType::Time { precision: Some(p), with_tz })
        }
        "timetz" | "time with time zone" => {
            let p: u8 = args.trim().parse().ok()?;
            Some(ColumnType::Time { precision: Some(p), with_tz: true })
        }
        "interval" => {
            // interval([fields,] precision) — pg_type stores this as `_interval` or with typmod;
            // for v0.1 we just accept a precision int.
            let p: u8 = args.trim().parse().ok()?;
            Some(ColumnType::Interval { fields: None, precision: Some(p) })
        }
        "bit" => {
            let n: u32 = args.trim().parse().ok()?;
            Some(ColumnType::Bit { len: n, varying: false })
        }
        "bit varying" | "varbit" => {
            let n: u32 = args.trim().parse().ok()?;
            Some(ColumnType::Bit { len: n, varying: true })
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p pgevolve-core --lib ir::column_type`
Expected: 7 passing tests.

- [ ] **Step 5: Run clippy**

Run: `cargo clippy -p pgevolve-core --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/pgevolve-core
git commit -m "feat(core): ColumnType::parse_from_pg_type_string with alias normalization

Collapses int4 ≡ int ≡ integer, bool ≡ boolean, varchar ≡ character varying,
timestamptz ≡ timestamp with time zone, etc. Falls through to ColumnType::Other
for unknown inputs.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 1.5: `ColumnType::render_sql` (round-trip back to canonical Postgres syntax)

**Files:**
- Modify: `crates/pgevolve-core/src/ir/column_type.rs`

- [ ] **Step 1: Write failing tests**

Append to the `tests` module:

```rust
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
            ColumnType::Numeric { precision: None, scale: None },
            ColumnType::Numeric { precision: Some(10), scale: Some(2) },
            ColumnType::Timestamp { precision: None, with_tz: false },
            ColumnType::Timestamp { precision: Some(3), with_tz: true },
            ColumnType::Time { precision: None, with_tz: true },
            ColumnType::Uuid,
            ColumnType::Jsonb,
            ColumnType::NetAddress(NetAddressKind::Inet),
            ColumnType::Bit { len: 8, varying: false },
            ColumnType::Bit { len: 8, varying: true },
            ColumnType::Array { element: Box::new(ColumnType::Integer), dims: 2 },
        ];
        for t in cases {
            let rendered = t.render_sql();
            let parsed = ColumnType::parse_from_pg_type_string(&rendered).unwrap();
            assert_eq!(parsed, t, "rendered: {rendered}");
        }
    }
```

- [ ] **Step 2: Run, expect failure**

Run: `cargo test -p pgevolve-core --lib ir::column_type::tests::render_sql_round_trips_canonical`
Expected: fails (method not found).

- [ ] **Step 3: Implement `render_sql`**

Add inside `impl ColumnType { ... }`:

```rust
    /// Render this type as canonical Postgres syntax.
    /// The output round-trips through [`Self::parse_from_pg_type_string`] back to `self`.
    pub fn render_sql(&self) -> String {
        match self {
            Self::Boolean => "boolean".into(),
            Self::SmallInt => "smallint".into(),
            Self::Integer => "integer".into(),
            Self::BigInt => "bigint".into(),
            Self::Real => "real".into(),
            Self::DoublePrecision => "double precision".into(),
            Self::Numeric { precision: None, scale: None } => "numeric".into(),
            Self::Numeric { precision: Some(p), scale: None } => format!("numeric({p})"),
            Self::Numeric { precision: Some(p), scale: Some(s) } => format!("numeric({p},{s})"),
            Self::Numeric { precision: None, scale: Some(_) } => unreachable!(
                "scale without precision should never be constructed"
            ),
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
            Self::Interval { fields: None, precision: None } => "interval".into(),
            Self::Interval { fields: None, precision: Some(p) } => format!("interval({p})"),
            Self::Interval { fields: Some(f), precision: None } => format!("interval {f}"),
            Self::Interval { fields: Some(f), precision: Some(p) } => format!("interval {f}({p})"),
            Self::Bit { len, varying: false } => format!("bit({len})"),
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p pgevolve-core --lib ir::column_type`
Expected: 8 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core
git commit -m "feat(core): ColumnType::render_sql with parse round-trip property

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 1.6: `Diff` trait + `Difference` enum + helpers

**Files:**
- Modify: `crates/pgevolve-core/src/ir/eq.rs`
- Modify: `crates/pgevolve-core/src/ir/difference.rs`
- Modify: `crates/pgevolve-core/src/ir/mod.rs` (re-exports)

These are the building blocks every IR type below uses. We define them once and reuse.

- [ ] **Step 1: Write `Difference` enum**

`crates/pgevolve-core/src/ir/difference.rs`:

```rust
//! Structured representation of one or more differences between two IR values.

use serde::{Deserialize, Serialize};

/// A single named difference between two IR values.
///
/// Fields capture the path to the differing position (e.g., a column name)
/// and a JSON-encoded representation of the two values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Difference {
    /// Dotted path to the differing field (e.g., `columns.email.ty`).
    pub path: String,
    /// Old value, as displayed (Display impl, not Debug).
    pub from: String,
    /// New value, as displayed.
    pub to: String,
}

impl Difference {
    /// Construct a `Difference` from displayable values.
    pub fn new<F: std::fmt::Display, T: std::fmt::Display>(path: impl Into<String>, from: F, to: T) -> Self {
        Self {
            path: path.into(),
            from: from.to_string(),
            to: to.to_string(),
        }
    }

    /// Prefix the path of each entry with the given prefix.
    pub fn prefix_path(self, prefix: &str) -> Self {
        Self {
            path: if self.path.is_empty() {
                prefix.into()
            } else {
                format!("{prefix}.{}", self.path)
            },
            ..self
        }
    }
}
```

- [ ] **Step 2: Write `Diff` trait**

`crates/pgevolve-core/src/ir/eq.rs`:

```rust
//! `Diff` trait — produces structured differences between two IR values.

use super::difference::Difference;

/// Compute the structured difference between two IR values.
///
/// Equivalence is the inverse of `diff(...).is_empty()`. Implementors derive
/// equivalence from `Diff` rather than from `PartialEq` so that equivalence
/// rules can diverge from structural equality (e.g., field reordering inside
/// a `Vec<Constraint>` doesn't matter, but `PartialEq` would say it does).
pub trait Diff {
    /// List the differences between `self` and `other`. Empty list = equivalent.
    fn diff(&self, other: &Self) -> Vec<Difference>;

    /// Convenience: `true` iff `self.diff(other).is_empty()`.
    fn canonical_eq(&self, other: &Self) -> bool {
        self.diff(other).is_empty()
    }
}

/// Helper: produces a single-element `Vec<Difference>` if `from != to`, else empty.
pub fn diff_field<T: PartialEq + std::fmt::Display>(
    path: &str,
    from: &T,
    to: &T,
) -> Vec<Difference> {
    if from == to {
        Vec::new()
    } else {
        vec![Difference::new(path, from, to)]
    }
}

/// Helper: prefix every element's path.
pub fn prefix_diffs(prefix: &str, diffs: Vec<Difference>) -> Vec<Difference> {
    diffs.into_iter().map(|d| d.prefix_path(prefix)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_field_matches() {
        let r = diff_field("name", &1, &1);
        assert!(r.is_empty());
    }

    #[test]
    fn diff_field_reports() {
        let r = diff_field("name", &1, &2);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].path, "name");
    }

    #[test]
    fn prefix_diffs_simple() {
        let d = vec![Difference::new("len", "5", "10")];
        let p = prefix_diffs("ty", d);
        assert_eq!(p[0].path, "ty.len");
    }
}
```

- [ ] **Step 3: Re-export from `ir/mod.rs`**

Add to `crates/pgevolve-core/src/ir/mod.rs`:

```rust
pub use difference::Difference;
pub use eq::{diff_field, prefix_diffs, Diff};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p pgevolve-core --lib ir::eq`
Expected: 3 passing tests.

- [ ] **Step 5: Implement `Diff` for `ColumnType`**

Append to `crates/pgevolve-core/src/ir/column_type.rs`:

```rust
impl crate::ir::eq::Diff for ColumnType {
    fn diff(&self, other: &Self) -> Vec<crate::ir::difference::Difference> {
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
```

(For `ColumnType`, `PartialEq` is the right equivalence — `parse_from_pg_type_string` already collapses aliases, so two equal-after-parse values are *structurally* equal. We use `render_sql` for the human-readable from/to.)

- [ ] **Step 6: Test `Diff` for `ColumnType`**

Append to `column_type.rs` `tests`:

```rust
    #[test]
    fn columntype_diff_empty_when_equal() {
        use crate::ir::eq::Diff;
        let a = ColumnType::Varchar { len: Some(50) };
        let b = ColumnType::Varchar { len: Some(50) };
        assert!(a.diff(&b).is_empty());
        assert!(a.canonical_eq(&b));
    }

    #[test]
    fn columntype_diff_reports_difference() {
        use crate::ir::eq::Diff;
        let a = ColumnType::Varchar { len: Some(50) };
        let b = ColumnType::Varchar { len: Some(100) };
        let diffs = a.diff(&b);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].from, "varchar(50)");
        assert_eq!(diffs[0].to, "varchar(100)");
    }
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p pgevolve-core --lib`
Expected: all passing.

- [ ] **Step 8: Commit**

```bash
git add crates/pgevolve-core
git commit -m "feat(core): Diff trait, Difference, and helpers; impl Diff for ColumnType

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 1.7: `Schema` and `Sequence` IR types

**Files:**
- Modify: `crates/pgevolve-core/src/ir/schema.rs`
- Modify: `crates/pgevolve-core/src/ir/sequence.rs`
- Modify: `crates/pgevolve-core/src/ir/mod.rs`

- [ ] **Step 1: Write `Schema`**

`crates/pgevolve-core/src/ir/schema.rs`:

```rust
//! `Schema` — a Postgres namespace.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::difference::Difference;
use crate::ir::eq::{diff_field, Diff};

/// A Postgres schema (namespace).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Schema {
    /// Schema name.
    pub name: Identifier,
    /// Optional comment.
    pub comment: Option<String>,
}

impl Schema {
    /// Construct a `Schema`.
    pub fn new(name: Identifier) -> Self {
        Self { name, comment: None }
    }
}

impl Diff for Schema {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let mut out = Vec::new();
        out.extend(diff_field("name", &self.name, &other.name));
        out.extend(diff_field(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;

    #[test]
    fn equal_schemas_have_no_diff() {
        let a = Schema::new(Identifier::from_unquoted("app").unwrap());
        let b = Schema::new(Identifier::from_unquoted("app").unwrap());
        assert!(a.canonical_eq(&b));
    }

    #[test]
    fn different_names_diff() {
        let a = Schema::new(Identifier::from_unquoted("app").unwrap());
        let b = Schema::new(Identifier::from_unquoted("billing").unwrap());
        let d = a.diff(&b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "name");
    }

    #[test]
    fn comment_diffs() {
        let a = Schema {
            name: Identifier::from_unquoted("app").unwrap(),
            comment: Some("v1".into()),
        };
        let b = Schema {
            name: Identifier::from_unquoted("app").unwrap(),
            comment: Some("v2".into()),
        };
        let d = a.diff(&b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "comment");
    }
}
```

- [ ] **Step 2: Write `Sequence`**

`crates/pgevolve-core/src/ir/sequence.rs`:

```rust
//! `Sequence` — a standalone or column-owned Postgres sequence.

use serde::{Deserialize, Serialize};

use crate::identifier::QualifiedName;
use crate::ir::column_type::ColumnType;
use crate::ir::difference::Difference;
use crate::ir::eq::{diff_field, Diff};

/// A Postgres sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sequence {
    /// Schema-qualified sequence name.
    pub qname: QualifiedName,
    /// Sequence data type (always one of `SmallInt`, `Integer`, `BigInt`).
    pub data_type: ColumnType,
    /// Start value.
    pub start: i64,
    /// Increment.
    pub increment: i64,
    /// Min value (`None` = type's minimum).
    pub min_value: Option<i64>,
    /// Max value (`None` = type's maximum).
    pub max_value: Option<i64>,
    /// Cache size.
    pub cache: i64,
    /// Whether the sequence cycles.
    pub cycle: bool,
    /// Owning column, if any (e.g., from `SERIAL` / `IDENTITY`).
    pub owned_by: Option<SequenceOwner>,
    /// Optional comment.
    pub comment: Option<String>,
}

/// Identifies a column that owns this sequence (Postgres `OWNED BY`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceOwner {
    /// Owning table.
    pub table: QualifiedName,
    /// Owning column name.
    pub column: crate::identifier::Identifier,
}

impl Diff for Sequence {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let mut out = Vec::new();
        out.extend(diff_field("qname", &self.qname, &other.qname));
        out.extend(diff_field(
            "data_type",
            &self.data_type.render_sql(),
            &other.data_type.render_sql(),
        ));
        out.extend(diff_field("start", &self.start, &other.start));
        out.extend(diff_field("increment", &self.increment, &other.increment));
        out.extend(diff_field(
            "min_value",
            &format!("{:?}", self.min_value),
            &format!("{:?}", other.min_value),
        ));
        out.extend(diff_field(
            "max_value",
            &format!("{:?}", self.max_value),
            &format!("{:?}", other.max_value),
        ));
        out.extend(diff_field("cache", &self.cache, &other.cache));
        out.extend(diff_field("cycle", &self.cycle, &other.cycle));
        out.extend(diff_field(
            "owned_by",
            &format!("{:?}", self.owned_by),
            &format!("{:?}", other.owned_by),
        ));
        out.extend(diff_field(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};

    fn s(name: &str) -> QualifiedName {
        QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted(name).unwrap(),
        )
    }

    fn base() -> Sequence {
        Sequence {
            qname: s("seq1"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cache: 1,
            cycle: false,
            owned_by: None,
            comment: None,
        }
    }

    #[test]
    fn sequences_equal_when_identical() {
        assert!(base().canonical_eq(&base()));
    }

    #[test]
    fn sequence_diff_reports_increment_change() {
        let mut other = base();
        other.increment = 2;
        let d = base().diff(&other);
        assert!(d.iter().any(|x| x.path == "increment"));
    }
}
```

- [ ] **Step 3: Re-export from `ir/mod.rs`**

Add:

```rust
pub use schema::Schema;
pub use sequence::{Sequence, SequenceOwner};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p pgevolve-core --lib`
Expected: all passing.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core
git commit -m "feat(core): Schema and Sequence IR types with Diff impls

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Tasks 1.8 — 1.13: remaining IR types

The pattern is identical for the remaining types: write the struct, derive serde + PartialEq, hand-write `Diff`, write 3-5 tests covering equality + each significant difference, run, commit. The full code for these is omitted from this plan in the interest of length but each follows the exact pattern in tasks 1.6 and 1.7. Below is the contract for each — fields, variants, and key tests.

#### Task 1.8: `DefaultExpr`, `LiteralValue`, `NormalizedExpr`

**File:** `crates/pgevolve-core/src/ir/default_expr.rs`

```rust
pub enum DefaultExpr {
    Literal(LiteralValue),
    Sequence(QualifiedName),
    Expr(NormalizedExpr),
}

pub enum LiteralValue {
    Bool(bool),
    Integer(i64),
    Float(f64),
    Text(String),
    Bytea(Vec<u8>),
    Null,
    // ... add as fixtures discover them
}

pub struct NormalizedExpr {
    /// pg_query AST after normalization passes:
    /// - strip casts to the column's own type
    /// - fold parens
    /// - sort commutative operands deterministically
    /// - lowercase keywords
    pub canonical_text: String,    // canonical text form
    pub ast_hash:       [u8; 32],  // BLAKE3 of canonical AST bytes
}
```

The actual normalization passes use `pg_query.rs`. Since pg_query lands in phase 2, this task may stub `NormalizedExpr` to just `canonical_text: String` and add the AST hash logic in phase 2. **Recommended:** stub here, fully wire in task 2.5.

Tests:
- `DefaultExpr::Literal(Text("foo"))` ≡ itself.
- `DefaultExpr::Literal(Text("foo"))` differs from `DefaultExpr::Literal(Text("bar"))`.
- `DefaultExpr::Sequence(qn)` differs from `DefaultExpr::Literal(...)`.

Re-export `DefaultExpr`, `LiteralValue`, `NormalizedExpr` from `ir/mod.rs`.

#### Task 1.9: `Constraint`, `ConstraintKind`, `ForeignKey`, `ReferentialAction`

**File:** `crates/pgevolve-core/src/ir/constraint.rs`

```rust
pub struct Constraint {
    pub qname:    QualifiedName,           // constraints carry their own names
    pub kind:     ConstraintKind,
    pub deferrable: Deferrable,
    pub comment:  Option<String>,
}

pub enum ConstraintKind {
    PrimaryKey { columns: Vec<Identifier>, include: Vec<Identifier> },
    Unique     { columns: Vec<Identifier>, include: Vec<Identifier>, nulls_distinct: bool },
    ForeignKey(ForeignKey),
    Check      { expression: NormalizedExpr, no_inherit: bool },
    Exclusion  { /* deferred to phase 2 */ },  // OOS for v0.1
}

pub struct ForeignKey {
    pub columns:           Vec<Identifier>,
    pub referenced_table:  QualifiedName,
    pub referenced_columns: Vec<Identifier>,
    pub on_update:         ReferentialAction,
    pub on_delete:         ReferentialAction,
    pub match_type:        FkMatchType, // Simple | Full | Partial (Partial unsupported in pg)
}

pub enum ReferentialAction {
    NoAction, Restrict, Cascade, SetNull(Vec<Identifier>), SetDefault(Vec<Identifier>),
}

pub enum FkMatchType { Simple, Full }

pub enum Deferrable { NotDeferrable, Deferrable { initially_deferred: bool } }
```

`Diff` for `Constraint`: compares each field using `diff_field`, recursing into `kind`. For `Vec<Identifier>` columns, **order matters** (PK column order is significant in Postgres). Tests should cover: equal-by-name, different column lists, different ON DELETE actions, NotValid handling (NotValid is *not* on `Constraint` — it's on `Change::AddConstraint`'s state at apply time, not on the IR which represents the *valid* end state).

**Note:** the IR represents fully-validated constraints. The `NOT VALID` intermediate state is a planner concern, not an IR concern — see phase 6.

#### Task 1.10: `Index`, `IndexColumn`, `IndexMethod`, `SortOrder`, `NullsOrder`

**File:** `crates/pgevolve-core/src/ir/index.rs`

```rust
pub struct Index {
    pub qname:        QualifiedName,
    pub table:        QualifiedName,
    pub method:       IndexMethod,           // BTree, Hash, Gin, Gist, Brin, Spgist
    pub columns:      Vec<IndexColumn>,
    pub include:      Vec<Identifier>,       // INCLUDE (cols)
    pub unique:       bool,
    pub nulls_not_distinct: bool,            // PG 15+
    pub predicate:    Option<NormalizedExpr>,// partial index WHERE clause
    pub tablespace:   Option<Identifier>,
    pub comment:      Option<String>,
}

pub struct IndexColumn {
    pub expr:        IndexColumnExpr,        // bare column or expression
    pub collation:   Option<QualifiedName>,
    pub opclass:     Option<QualifiedName>,
    pub sort_order:  SortOrder,
    pub nulls_order: NullsOrder,
}

pub enum IndexColumnExpr {
    Column(Identifier),
    Expression(NormalizedExpr),
}

pub enum IndexMethod { BTree, Hash, Gin, Gist, Brin, Spgist }
pub enum SortOrder   { Asc, Desc }
pub enum NullsOrder  { NullsFirst, NullsLast }
```

`Diff` notes: column order matters. Tests cover unique/non-unique, include columns, partial predicate, opclass changes.

#### Task 1.11: `Column`, `Identity`, `IdentityKind`, `Generated`, `GeneratedKind`

**File:** `crates/pgevolve-core/src/ir/column.rs`

```rust
pub struct Column {
    pub name:      Identifier,
    pub ty:        ColumnType,
    pub nullable:  bool,
    pub default:   Option<DefaultExpr>,
    pub identity:  Option<Identity>,
    pub generated: Option<Generated>,
    pub collation: Option<QualifiedName>,
    pub comment:   Option<String>,
}

pub struct Identity {
    pub kind:     IdentityKind,
    pub sequence: SequenceOptions,
}

pub enum IdentityKind { Always, ByDefault }

pub struct Generated { pub kind: GeneratedKind, pub expression: NormalizedExpr }
pub enum   GeneratedKind { Stored }

pub struct SequenceOptions {
    pub start:     i64,
    pub increment: i64,
    pub min_value: Option<i64>,
    pub max_value: Option<i64>,
    pub cache:     i64,
    pub cycle:     bool,
}
```

#### Task 1.12: `Table`

**File:** `crates/pgevolve-core/src/ir/table.rs`

```rust
pub struct Table {
    pub qname:        QualifiedName,
    pub columns:      Vec<Column>,        // logical order matters
    pub constraints:  Vec<Constraint>,    // sorted by qname for canonical form
    pub comment:      Option<String>,
}
```

`Diff` walks `columns` paired by `name` (not by index), reports `removed`/`added`/`changed` columns explicitly. For `constraints`, pairs by `qname`. Test coverage:
- Equal tables ≡.
- Add/remove column.
- Reorder columns (a *position* difference, not a structural one — we report it but the planner treats it as a real change).
- Add/remove constraint.
- Constraint definition change.

#### Task 1.13: `Catalog`

**File:** `crates/pgevolve-core/src/ir/catalog.rs`

```rust
pub struct Catalog {
    pub schemas:   Vec<Schema>,
    pub tables:    Vec<Table>,
    pub indexes:   Vec<Index>,
    pub sequences: Vec<Sequence>,
}
```

Constructors:
- `Catalog::empty()` → all-empty.
- `Catalog::canonicalize(self) -> Self` — sorts each `Vec` by qname, deduplicates by qname, returns `Result<Self, IrError>` if duplicates exist.

`Diff` walks each collection paired by qname.

Tests:
- Empty ≡ empty.
- Adding a table reports.
- Removing a table reports.
- Tables with same qname but different columns report column-level diffs under `tables.<qname>.columns`.

---

### Task 1.14: Phase 1 self-review

- [ ] **Step 1: Re-run gauntlet**

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

All must pass.

- [ ] **Step 2: Spec coverage check**

Walk through spec §5 line by line and confirm every type/property is represented. Notable items to verify:

- `ColumnType::Other` exists for unknown types (§5.2).
- `DefaultExpr` has `Literal | Sequence | Expr` variants (§5.3).
- Constraint qnames are first-class (§5.4).
- NOT NULL is on `Column.nullable`, not in `Constraint` (§5.4).
- `SequenceOwner` is wired (§5.5 — desugaring lands in phase 2 but the type to support it lives here).
- `Diff` trait + `canonical_eq` (§5.6).

- [ ] **Step 3: Commit any fixes and proceed.**

Phase 1 complete.
