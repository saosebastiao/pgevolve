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
    /// `ALTER SUBSCRIPTION x OWNER TO`.
    Subscription,
    /// `ALTER STATISTICS x OWNER TO`.
    Statistic,
    /// `ALTER COLLATION x OWNER TO`.
    Collation,
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
            Self::Subscription => "SUBSCRIPTION",
            Self::Statistic => "STATISTICS",
            Self::Collation => "COLLATION",
        }
    }
}

/// Identifies the object being re-owned by name shape.
///
/// Replaces the older convention of stuffing every kind into a
/// [`QualifiedName`] (which forced workarounds like
/// `QualifiedName::new(name, name)` for schemas and
/// `QualifiedName::new("__cluster__", name)` for publications /
/// subscriptions). The renderer dispatches on this enum directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum OwnedObjectId {
    /// A schema-qualified object: table, view, MV, sequence, function,
    /// procedure, user-type, statistic.
    Qualified(QualifiedName),
    /// A schema itself. Rendered as the bare schema name.
    Schema(Identifier),
    /// A cluster-level object — publication or subscription. Rendered
    /// as the bare name (no schema qualifier; PG does not schema-qualify
    /// these).
    Cluster(Identifier),
}

impl OwnedObjectId {
    /// Render the object's target name for use in
    /// `ALTER <kind> <here>{signature} OWNER TO <role>;`.
    #[must_use]
    pub fn render_sql(&self) -> String {
        match self {
            Self::Qualified(q) => q.render_sql(),
            Self::Schema(name) | Self::Cluster(name) => name.render_sql(),
        }
    }
}

impl std::fmt::Display for OwnedObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Qualified(q) => write!(f, "{q}"),
            Self::Schema(name) | Self::Cluster(name) => write!(f, "{name}"),
        }
    }
}

/// An `ALTER <kind> <id> OWNER TO <to>` statement, paired with the previous
/// owner for audit / rollback purposes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlterObjectOwner {
    /// Which kind of object is being re-owned.
    pub kind: OwnerObjectKind,
    /// Identifies the object by name shape (qualified / schema / cluster).
    pub id: OwnedObjectId,
    /// Optional argument-signature suffix for routines (e.g., `(int, text)`).
    /// Empty for non-routine kinds.
    #[serde(default)]
    pub signature: String,
    /// Previous owner (taken from the target catalog; `None` when the
    /// catalog did not record an owner).
    pub from: Option<Identifier>,
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
        assert_eq!(OwnerObjectKind::Subscription.sql_keyword(), "SUBSCRIPTION");
        assert_eq!(OwnerObjectKind::Statistic.sql_keyword(), "STATISTICS");
        assert_eq!(OwnerObjectKind::Collation.sql_keyword(), "COLLATION");
    }

    #[test]
    fn owned_object_id_renders_each_shape() {
        let q = OwnedObjectId::Qualified(QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted("users").unwrap(),
        ));
        assert_eq!(q.render_sql(), "app.users");

        let s = OwnedObjectId::Schema(Identifier::from_unquoted("billing").unwrap());
        assert_eq!(s.render_sql(), "billing");

        let c = OwnedObjectId::Cluster(Identifier::from_unquoted("my_pub").unwrap());
        assert_eq!(c.render_sql(), "my_pub");
    }
}
