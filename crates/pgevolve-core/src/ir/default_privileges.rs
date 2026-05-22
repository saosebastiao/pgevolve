//! `ALTER DEFAULT PRIVILEGES` — future-object grants.
//!
//! `pg_default_acl` rows. Distinct from per-object `grants`: these say
//! "future objects of type X in schema Y created by role Z get these
//! grants automatically."

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::eq::DiffMacro;
use crate::ir::grant::Grant;

/// One `ALTER DEFAULT PRIVILEGES` rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct DefaultPrivilegeRule {
    /// `FOR ROLE x` — whose future objects this applies to.
    pub target_role: Identifier,
    /// `IN SCHEMA y` — scope. `None` = "all schemas owned by `target_role`".
    #[diff(via_debug)]
    pub schema: Option<Identifier>,
    /// Object type this rule applies to.
    #[diff(via_debug)]
    pub object_type: DefaultPrivObjectType,
    /// Grants applied. Canonicalized (sorted, deduped).
    #[diff(via_debug)]
    pub grants: Vec<Grant>,
}

/// Object-type discriminant for default-privilege rules.
///
/// PG's grouping: `TABLES` covers tables + views + MVs;
/// `FUNCTIONS` covers functions + procedures (alias `ROUTINES` in PG 11+).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultPrivObjectType {
    /// `TABLES` — covers tables, views, and materialized views.
    Tables,
    /// `SEQUENCES`.
    Sequences,
    /// `FUNCTIONS` — covers procedures too (PG alias `ROUTINES`).
    Functions,
    /// `TYPES`.
    Types,
    /// `SCHEMAS` — PG 14+ only.
    Schemas,
}

impl DefaultPrivObjectType {
    /// PG `pg_default_acl.defaclobjtype` single-char code.
    #[must_use]
    pub const fn pg_char(self) -> char {
        match self {
            Self::Tables => 'r',
            Self::Sequences => 'S',
            Self::Functions => 'f',
            Self::Types => 'T',
            Self::Schemas => 'n',
        }
    }

    /// Decode from `pg_default_acl.defaclobjtype`.
    #[must_use]
    pub const fn from_pg_char(c: char) -> Option<Self> {
        Some(match c {
            'r' => Self::Tables,
            'S' => Self::Sequences,
            'f' => Self::Functions,
            'T' => Self::Types,
            'n' => Self::Schemas,
            _ => return None,
        })
    }

    /// SQL keyword in `ALTER DEFAULT PRIVILEGES ... GRANT ... ON <KIND> ...`.
    #[must_use]
    pub const fn sql_keyword(self) -> &'static str {
        match self {
            Self::Tables => "TABLES",
            Self::Sequences => "SEQUENCES",
            Self::Functions => "FUNCTIONS",
            Self::Types => "TYPES",
            Self::Schemas => "SCHEMAS",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_char_roundtrips() {
        for kind in [
            DefaultPrivObjectType::Tables,
            DefaultPrivObjectType::Sequences,
            DefaultPrivObjectType::Functions,
            DefaultPrivObjectType::Types,
            DefaultPrivObjectType::Schemas,
        ] {
            assert_eq!(
                DefaultPrivObjectType::from_pg_char(kind.pg_char()),
                Some(kind)
            );
        }
    }

    #[test]
    fn from_pg_char_rejects_unknown() {
        assert_eq!(DefaultPrivObjectType::from_pg_char('q'), None);
    }
}
