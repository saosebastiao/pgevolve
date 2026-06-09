//! Catalog-object reference ([`CatalogObjectRef`]) and the uniform
//! owner-change op ([`AlterObjectOwner`]) shared by the grant and owner diffs.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};

/// A routine's argument-type signature, e.g. `(integer, text)`.
///
/// Rendered verbatim — the leading/trailing parens are part of the stored
/// string (constructed as `format!("({args_label})")` at the diff sites),
/// matching the old `signature` field exactly so rendered SQL is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineSignature(String);

impl RoutineSignature {
    /// Construct from the parenthesized argument-type signature, e.g.
    /// `(integer, text)`. The string is stored verbatim — no paren-wrapping
    /// or normalization is applied.
    #[must_use]
    pub const fn new(s: String) -> Self {
        Self(s)
    }

    /// Returns the inner signature string, parens included.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// A grantable / ownable schema object, carrying exactly the data its kind
/// needs.
///
/// A routine argument-signature is representable ONLY on
/// [`CatalogObjectRef::Function`] / [`CatalogObjectRef::Procedure`], so the old
/// `signature: String` field (documented as "empty for non-routine kinds") —
/// an illegal state — is gone.
///
/// This subsumes the former `OwnerObjectKind` (the SQL keyword discriminant)
/// and `OwnedObjectId` (the name shape): schema / publication / subscription
/// render a bare [`Identifier`]; every other kind renders a [`QualifiedName`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CatalogObjectRef {
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

impl CatalogObjectRef {
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
    /// (`format!("{}{}", name.render_sql(), signature.as_str())`). Schema /
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
                format!("{}{}", name.render_sql(), signature.as_str())
            }
        }
    }

    /// Human-readable label used in the grant observation side-channels
    /// (`UnmanagedGrantObservation` / `RevokeWithOwnerObservation`), e.g.
    /// `"table app.users"` or `"function app.f(integer)"`.
    ///
    /// The name is rendered via the identifier `Display` impl (raw, *not*
    /// SQL-quoted) to byte-match the pre-dedup inline labels that built this
    /// string with `format!("table {qname}")` and friends. This is distinct
    /// from [`CatalogObjectRef::render_target`], which SQL-quotes for emitted
    /// statements.
    #[must_use]
    pub fn observation_label(&self) -> String {
        let kind = self.sql_keyword().to_lowercase();
        match self {
            Self::Schema(name) | Self::Publication(name) | Self::Subscription(name) => {
                format!("{kind} {name}")
            }
            Self::Sequence(q)
            | Self::Table(q)
            | Self::View(q)
            | Self::MaterializedView(q)
            | Self::UserType(q)
            | Self::Statistic(q)
            | Self::Collation(q) => format!("{kind} {q}"),
            Self::Function { name, signature } | Self::Procedure { name, signature } => {
                format!("{kind} {name}{}", signature.as_str())
            }
        }
    }
}

impl std::fmt::Display for CatalogObjectRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render_target())
    }
}

/// An `ALTER <kind> <object> OWNER TO <to>` statement, paired with the previous
/// owner for audit / rollback purposes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlterObjectOwner {
    /// The object being re-owned (kind + name + optional routine signature).
    pub object: CatalogObjectRef,
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
        assert_eq!(CatalogObjectRef::Schema(id("s")).sql_keyword(), "SCHEMA");
        assert_eq!(
            CatalogObjectRef::Sequence(qn("s", "x")).sql_keyword(),
            "SEQUENCE"
        );
        assert_eq!(CatalogObjectRef::Table(qn("s", "x")).sql_keyword(), "TABLE");
        assert_eq!(CatalogObjectRef::View(qn("s", "x")).sql_keyword(), "VIEW");
        assert_eq!(
            CatalogObjectRef::MaterializedView(qn("s", "x")).sql_keyword(),
            "MATERIALIZED VIEW"
        );
        assert_eq!(
            CatalogObjectRef::Function {
                name: qn("s", "x"),
                signature: RoutineSignature::new("()".to_string()),
            }
            .sql_keyword(),
            "FUNCTION"
        );
        assert_eq!(
            CatalogObjectRef::Procedure {
                name: qn("s", "x"),
                signature: RoutineSignature::new("()".to_string()),
            }
            .sql_keyword(),
            "PROCEDURE"
        );
        assert_eq!(
            CatalogObjectRef::UserType(qn("s", "x")).sql_keyword(),
            "TYPE"
        );
        assert_eq!(
            CatalogObjectRef::Publication(id("p")).sql_keyword(),
            "PUBLICATION"
        );
        assert_eq!(
            CatalogObjectRef::Subscription(id("p")).sql_keyword(),
            "SUBSCRIPTION"
        );
        assert_eq!(
            CatalogObjectRef::Statistic(qn("s", "x")).sql_keyword(),
            "STATISTICS"
        );
        assert_eq!(
            CatalogObjectRef::Collation(qn("s", "x")).sql_keyword(),
            "COLLATION"
        );
    }

    #[test]
    fn render_target_each_shape() {
        assert_eq!(
            CatalogObjectRef::Table(qn("app", "users")).render_target(),
            "app.users"
        );
        assert_eq!(
            CatalogObjectRef::Schema(id("billing")).render_target(),
            "billing"
        );
        assert_eq!(
            CatalogObjectRef::Publication(id("my_pub")).render_target(),
            "my_pub"
        );
        assert_eq!(
            CatalogObjectRef::Function {
                name: qn("app", "do_thing"),
                signature: RoutineSignature::new("(integer, text)".to_string()),
            }
            .render_target(),
            "app.do_thing(integer, text)"
        );
        assert_eq!(
            CatalogObjectRef::Procedure {
                name: qn("app", "do_work"),
                signature: RoutineSignature::new("(integer)".to_string()),
            }
            .render_target(),
            "app.do_work(integer)"
        );
    }

    #[test]
    fn observation_label_each_shape() {
        assert_eq!(
            CatalogObjectRef::Table(qn("app", "users")).observation_label(),
            "table app.users"
        );
        assert_eq!(
            CatalogObjectRef::MaterializedView(qn("app", "mv")).observation_label(),
            "materialized view app.mv"
        );
        assert_eq!(
            CatalogObjectRef::Schema(id("billing")).observation_label(),
            "schema billing"
        );
        assert_eq!(
            CatalogObjectRef::UserType(qn("app", "status")).observation_label(),
            "type app.status"
        );
        assert_eq!(
            CatalogObjectRef::Function {
                name: qn("app", "do_thing"),
                signature: RoutineSignature::new("(integer, text)".to_string()),
            }
            .observation_label(),
            "function app.do_thing(integer, text)"
        );
    }

    /// The observation label uses the identifier `Display` impl (raw, unquoted),
    /// matching the pre-dedup inline `format!("type {qname}")` labels — *not*
    /// the SQL-quoting `render_target`. A name needing quotes must NOT be quoted
    /// in the observation label.
    #[test]
    fn observation_label_is_unquoted_unlike_render_target() {
        // "select" is a reserved keyword → render_sql quotes it.
        let q = qn("app", "select");
        let obj = CatalogObjectRef::Table(q);
        assert_eq!(obj.render_target(), "app.\"select\"");
        assert_eq!(obj.observation_label(), "table app.select");
    }
}
