//! `AlterObjectOwner` — uniform owner-change op across grantable families.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};

/// Object kind discriminant for the renderer (`ALTER TABLE x OWNER TO`,
/// `ALTER SCHEMA x OWNER TO`, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerObjectKind {
    /// `ALTER SCHEMA x OWNER TO`.
    Schema,
    /// `ALTER SEQUENCE x OWNER TO`.
    Sequence,
    /// `ALTER TABLE x OWNER TO`.
    Table,
    /// `ALTER VIEW x OWNER TO`.
    View,
    /// `ALTER MATERIALIZED VIEW x OWNER TO`.
    MaterializedView,
    /// `ALTER FUNCTION x() OWNER TO`.
    Function,
    /// `ALTER PROCEDURE x() OWNER TO`.
    Procedure,
    /// `ALTER TYPE x OWNER TO`.
    UserType,
    /// `ALTER PUBLICATION x OWNER TO`.
    Publication,
}

impl OwnerObjectKind {
    /// The SQL keyword(s) used in `ALTER <keyword> <name> OWNER TO <role>`.
    #[must_use]
    pub const fn sql_keyword(self) -> &'static str {
        match self {
            Self::Schema => "SCHEMA",
            Self::Sequence => "SEQUENCE",
            Self::Table => "TABLE",
            Self::View => "VIEW",
            Self::MaterializedView => "MATERIALIZED VIEW",
            Self::Function => "FUNCTION",
            Self::Procedure => "PROCEDURE",
            Self::UserType => "TYPE",
            Self::Publication => "PUBLICATION",
        }
    }
}

/// An `ALTER <kind> <qname> OWNER TO <to>` statement, paired with the previous
/// owner for audit / rollback purposes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlterObjectOwner {
    /// Which kind of object is being re-owned.
    pub kind: OwnerObjectKind,
    /// Qualified name of the object.
    pub qname: QualifiedName,
    /// Optional argument-signature suffix for routines (e.g., `(int, text)`).
    /// Empty for non-routine kinds.
    #[serde(default)]
    pub signature: String,
    /// Previous owner (taken from the target catalog; `__unknown_owner__` when
    /// the catalog did not record an owner).
    pub from: Identifier,
    /// Desired new owner (taken from the source catalog).
    pub to: Identifier,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_keywords_match_pg() {
        assert_eq!(OwnerObjectKind::Schema.sql_keyword(), "SCHEMA");
        assert_eq!(OwnerObjectKind::Sequence.sql_keyword(), "SEQUENCE");
        assert_eq!(OwnerObjectKind::Table.sql_keyword(), "TABLE");
        assert_eq!(OwnerObjectKind::View.sql_keyword(), "VIEW");
        assert_eq!(
            OwnerObjectKind::MaterializedView.sql_keyword(),
            "MATERIALIZED VIEW"
        );
        assert_eq!(OwnerObjectKind::Function.sql_keyword(), "FUNCTION");
        assert_eq!(OwnerObjectKind::Procedure.sql_keyword(), "PROCEDURE");
        assert_eq!(OwnerObjectKind::UserType.sql_keyword(), "TYPE");
        assert_eq!(OwnerObjectKind::Publication.sql_keyword(), "PUBLICATION");
    }
}
