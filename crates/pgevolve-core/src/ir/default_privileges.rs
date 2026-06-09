//! `ALTER DEFAULT PRIVILEGES` — future-object grants.
//!
//! `pg_default_acl` rows. Distinct from per-object `grants`: these say
//! "future objects of type X in schema Y created by role Z get these
//! grants automatically."

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::difference::Difference;
use crate::ir::eq::{Equiv, field_difference};
use crate::ir::grant::Grant;

/// One `ALTER DEFAULT PRIVILEGES` rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultPrivilegeRule {
    /// `FOR ROLE x` — whose future objects this applies to.
    pub target_role: Identifier,
    /// `IN SCHEMA y` — scope. `None` = "all schemas owned by `target_role`".
    pub schema: Option<Identifier>,
    /// Object type this rule applies to.
    pub object_type: DefaultPrivObjectType,
    /// Grants applied. Canonicalized (sorted, deduped).
    pub grants: Vec<Grant>,
}

impl Equiv for DefaultPrivilegeRule {
    fn differences(&self, other: &Self) -> Vec<Difference> {
        let Self {
            target_role: _,
            schema: _,
            object_type: _,
            grants: _,
        } = self;
        let mut out = Vec::new();
        out.extend(field_difference(
            "target_role",
            &self.target_role,
            &other.target_role,
        ));
        out.extend(field_difference(
            "schema",
            &format!("{:?}", self.schema),
            &format!("{:?}", other.schema),
        ));
        out.extend(field_difference(
            "object_type",
            &format!("{:?}", self.object_type),
            &format!("{:?}", other.object_type),
        ));
        out.extend(field_difference(
            "grants",
            &format!("{:?}", self.grants),
            &format!("{:?}", other.grants),
        ));
        out
    }
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
    use crate::ir::eq::Equiv;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn base() -> DefaultPrivilegeRule {
        DefaultPrivilegeRule {
            target_role: id("app_owner"),
            schema: None,
            object_type: DefaultPrivObjectType::Tables,
            grants: Vec::new(),
        }
    }

    #[test]
    fn equal_rules_have_no_diff() {
        assert!(base().canonical_eq(&base()));
    }

    #[test]
    fn object_type_change_diffs() {
        let mut b = base();
        b.object_type = DefaultPrivObjectType::Sequences;
        let d = base().differences(&b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "object_type");
    }

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
