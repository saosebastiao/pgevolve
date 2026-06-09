//! `AlterObjectOwner` — uniform owner-change op across grantable families.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};

/// A routine's argument-type signature, e.g. `(integer, text)`.
///
/// Rendered verbatim — the leading/trailing parens are part of the stored
/// string (constructed as `format!("({args_label})")` at the diff sites),
/// matching the old `signature` field exactly so rendered SQL is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineSignature(pub String);

/// A grantable / ownable schema object, carrying exactly the data its kind
/// needs.
///
/// A routine argument-signature is representable ONLY on
/// [`GrantableObject::Function`] / [`GrantableObject::Procedure`], so the old
/// `signature: String` field (documented as "empty for non-routine kinds") —
/// an illegal state — is gone.
///
/// This subsumes the former `OwnerObjectKind` (the SQL keyword discriminant)
/// and `OwnedObjectId` (the name shape): schema / publication / subscription
/// render a bare [`Identifier`]; every other kind renders a [`QualifiedName`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrantableObject {
    /// `SCHEMA x` — rendered as the bare schema name.
    Schema(Identifier),
    /// `SEQUENCE x`.
    Sequence(QualifiedName),
    /// `TABLE x`.
    Table(QualifiedName),
    /// `VIEW x`.
    View(QualifiedName),
    /// `MATERIALIZED VIEW x`.
    MaterializedView(QualifiedName),
    /// `TYPE x`.
    UserType(QualifiedName),
    /// `FUNCTION x(<signature>)`.
    Function {
        /// Qualified routine name.
        name: QualifiedName,
        /// Argument-type signature suffix (with parens).
        signature: RoutineSignature,
    },
    /// `PROCEDURE x(<signature>)`.
    Procedure {
        /// Qualified routine name.
        name: QualifiedName,
        /// Argument-type signature suffix (with parens).
        signature: RoutineSignature,
    },
    /// `STATISTICS x`.
    Statistic(QualifiedName),
    /// `COLLATION x`.
    Collation(QualifiedName),
    /// `PUBLICATION x` — cluster-level, rendered as the bare name.
    Publication(Identifier),
    /// `SUBSCRIPTION x` — cluster-level, rendered as the bare name.
    Subscription(Identifier),
}

impl GrantableObject {
    /// The SQL keyword(s) used in `GRANT ... ON <keyword> <name>` /
    /// `ALTER <keyword> <name> OWNER TO <role>`.
    #[must_use]
    pub const fn sql_keyword(&self) -> &'static str {
        match self {
            Self::Schema(_) => "SCHEMA",
            Self::Sequence(_) => "SEQUENCE",
            Self::Table(_) => "TABLE",
            Self::View(_) => "VIEW",
            Self::MaterializedView(_) => "MATERIALIZED VIEW",
            Self::UserType(_) => "TYPE",
            Self::Function { .. } => "FUNCTION",
            Self::Procedure { .. } => "PROCEDURE",
            Self::Statistic(_) => "STATISTICS",
            Self::Collation(_) => "COLLATION",
            Self::Publication(_) => "PUBLICATION",
            Self::Subscription(_) => "SUBSCRIPTION",
        }
    }

    /// Render the object's target name for use in
    /// `ALTER <kw> <here> OWNER TO <role>;` / `GRANT ... ON <kw> <here> ...`.
    ///
    /// For routines this includes the signature suffix exactly as before
    /// (`format!("{}{}", name.render_sql(), signature.0)`). Schema /
    /// publication / subscription render the bare identifier; the rest render
    /// the [`QualifiedName`].
    #[must_use]
    pub fn render_target(&self) -> String {
        match self {
            Self::Schema(name) | Self::Publication(name) | Self::Subscription(name) => {
                name.render_sql()
            }
            Self::Sequence(q)
            | Self::Table(q)
            | Self::View(q)
            | Self::MaterializedView(q)
            | Self::UserType(q)
            | Self::Statistic(q)
            | Self::Collation(q) => q.render_sql(),
            Self::Function { name, signature } | Self::Procedure { name, signature } => {
                format!("{}{}", name.render_sql(), signature.0)
            }
        }
    }
}

impl std::fmt::Display for GrantableObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render_target())
    }
}

/// An `ALTER <kind> <object> OWNER TO <to>` statement, paired with the previous
/// owner for audit / rollback purposes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlterObjectOwner {
    /// The object being re-owned (kind + name + optional routine signature).
    pub object: GrantableObject,
    /// Previous owner (taken from the target catalog; `None` when the
    /// catalog did not record an owner).
    pub from: Option<Identifier>,
    /// Desired new owner (taken from the source catalog).
    pub to: Identifier,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    #[test]
    fn sql_keywords_match_pg() {
        assert_eq!(GrantableObject::Schema(id("s")).sql_keyword(), "SCHEMA");
        assert_eq!(
            GrantableObject::Sequence(qn("s", "x")).sql_keyword(),
            "SEQUENCE"
        );
        assert_eq!(GrantableObject::Table(qn("s", "x")).sql_keyword(), "TABLE");
        assert_eq!(GrantableObject::View(qn("s", "x")).sql_keyword(), "VIEW");
        assert_eq!(
            GrantableObject::MaterializedView(qn("s", "x")).sql_keyword(),
            "MATERIALIZED VIEW"
        );
        assert_eq!(
            GrantableObject::Function {
                name: qn("s", "x"),
                signature: RoutineSignature("()".to_string()),
            }
            .sql_keyword(),
            "FUNCTION"
        );
        assert_eq!(
            GrantableObject::Procedure {
                name: qn("s", "x"),
                signature: RoutineSignature("()".to_string()),
            }
            .sql_keyword(),
            "PROCEDURE"
        );
        assert_eq!(
            GrantableObject::UserType(qn("s", "x")).sql_keyword(),
            "TYPE"
        );
        assert_eq!(
            GrantableObject::Publication(id("p")).sql_keyword(),
            "PUBLICATION"
        );
        assert_eq!(
            GrantableObject::Subscription(id("p")).sql_keyword(),
            "SUBSCRIPTION"
        );
        assert_eq!(
            GrantableObject::Statistic(qn("s", "x")).sql_keyword(),
            "STATISTICS"
        );
        assert_eq!(
            GrantableObject::Collation(qn("s", "x")).sql_keyword(),
            "COLLATION"
        );
    }

    #[test]
    fn render_target_each_shape() {
        assert_eq!(
            GrantableObject::Table(qn("app", "users")).render_target(),
            "app.users"
        );
        assert_eq!(
            GrantableObject::Schema(id("billing")).render_target(),
            "billing"
        );
        assert_eq!(
            GrantableObject::Publication(id("my_pub")).render_target(),
            "my_pub"
        );
        assert_eq!(
            GrantableObject::Function {
                name: qn("app", "do_thing"),
                signature: RoutineSignature("(integer, text)".to_string()),
            }
            .render_target(),
            "app.do_thing(integer, text)"
        );
        assert_eq!(
            GrantableObject::Procedure {
                name: qn("app", "do_work"),
                signature: RoutineSignature("(integer)".to_string()),
            }
            .render_target(),
            "app.do_work(integer)"
        );
    }
}
