//! `CREATE COLLATION` IR — first-class managed collation kind.
//!
//! Source `lc_collate` and `lc_ctype` are always stored separately, even
//! when the user wrote `locale = 'X'` shorthand. The renderer collapses
//! back to `locale = '...'` when the two are equal.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::difference::Difference;
use crate::ir::eq::{Equiv, field_difference};

/// A user-defined collation managed by pgevolve.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collation {
    /// Schema-qualified collation name.
    pub qname: QualifiedName,
    /// libc / icu / PG 17+ builtin.
    pub provider: CollationProvider,
    /// `lc_collate` from `pg_collation.collcollate`.
    pub lc_collate: String,
    /// `lc_ctype` from `pg_collation.collctype`.
    pub lc_ctype: String,
    /// `deterministic` toggle — default `true`. PG 12+, ICU only when false.
    pub deterministic: bool,
    /// Read-only `pg_collation.collversion`. Source declares as `None`;
    /// the differ ignores this field. REFRESH VERSION management deferred
    /// to v0.3.9.
    pub version: Option<String>,
    /// Lenient owner field (per v0.3.1 cross-cutting state pattern).
    pub owner: Option<Identifier>,
    /// `COMMENT ON COLLATION qname IS '...'`.
    pub comment: Option<String>,
}

impl Equiv for Collation {
    fn differences(&self, other: &Self) -> Vec<Difference> {
        // Field-completeness guard: the compiler errors if a field is added
        // without being handled below. `version` is read-only (`pg_collation.
        // collversion`); source declares it `None` and the differ ignores it,
        // so it is intentionally excluded from equivalence.
        let Self {
            qname: _,
            provider: _,
            lc_collate: _,
            lc_ctype: _,
            deterministic: _,
            version: _, // read-only collversion, ignored by the differ
            owner: _,
            comment: _,
        } = self;
        let mut out = Vec::new();
        out.extend(field_difference("qname", &self.qname, &other.qname));
        out.extend(field_difference(
            "provider",
            &format!("{:?}", self.provider),
            &format!("{:?}", other.provider),
        ));
        out.extend(field_difference(
            "lc_collate",
            &self.lc_collate,
            &other.lc_collate,
        ));
        out.extend(field_difference(
            "lc_ctype",
            &self.lc_ctype,
            &other.lc_ctype,
        ));
        out.extend(field_difference(
            "deterministic",
            &format!("{:?}", self.deterministic),
            &format!("{:?}", other.deterministic),
        ));
        out.extend(field_difference(
            "owner",
            &format!("{:?}", self.owner),
            &format!("{:?}", other.owner),
        ));
        out.extend(field_difference(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out
    }
}

/// Locale-data provider — controls which OS / library produces the
/// sort + ctype tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollationProvider {
    /// `pg_collation.collprovider = 'c'`.
    Libc,
    /// `pg_collation.collprovider = 'i'`.
    Icu,
    /// `pg_collation.collprovider = 'b'` — PG 17+ only.
    Builtin,
}

/// Collation shortnames that bypass `column-references-unmanaged-collation`
/// even when they have no schema qualifier. PG seeds these at initdb.
pub const BUILTIN_COLLATIONS: &[&str] =
    &["default", "C", "POSIX", "und-x-icu", "unicode", "ucs_basic"];

impl CollationProvider {
    /// SQL keyword used in `CREATE COLLATION … (provider = …)`.
    #[must_use]
    pub const fn sql_keyword(self) -> &'static str {
        match self {
            Self::Libc => "libc",
            Self::Icu => "icu",
            Self::Builtin => "builtin",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(id(s), id(n))
    }

    #[test]
    fn collation_serde_round_trip() {
        let c = Collation {
            qname: qn("app", "case_insensitive"),
            provider: CollationProvider::Icu,
            lc_collate: "und".into(),
            lc_ctype: "und".into(),
            deterministic: false,
            version: None,
            owner: None,
            comment: Some("CI collation".into()),
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: Collation = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn provider_sql_keywords() {
        assert_eq!(CollationProvider::Libc.sql_keyword(), "libc");
        assert_eq!(CollationProvider::Icu.sql_keyword(), "icu");
        assert_eq!(CollationProvider::Builtin.sql_keyword(), "builtin");
    }
}
