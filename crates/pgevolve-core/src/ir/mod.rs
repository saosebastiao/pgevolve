//! In-memory representation of a Postgres schema.
//!
//! The IR is the contract between every other component. Both the source-side
//! parser and the catalog reader produce these types; the differ, dependency
//! analyzer, and planner consume them.

pub mod aggregate;
pub mod canon;
pub mod cast;
pub mod catalog;
pub mod cluster;
pub mod collation;
pub mod column;
pub mod column_type;
pub mod constraint;
pub mod default_expr;
pub mod default_privileges;
pub mod difference;
pub mod eq;
pub mod extension;
pub mod function;
pub mod grant;
pub mod index;
pub mod partition;
pub mod policy;
pub mod publication;
pub mod reloptions;
pub mod schema;
pub mod sequence;
pub mod statistic;
pub mod subscription;
pub mod table;
pub use function::*;
pub mod procedure;
pub use procedure::*;
pub mod event_trigger;
pub mod text_search;
pub mod trigger;
pub mod user_type;
pub use user_type::*;
pub mod view;
pub use view::*;

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

    /// A `PublicationScope::Selective` had no schemas and no tables.
    #[error("publication {0:?}: empty Selective scope (no tables, no schemas)")]
    EmptyPublication(crate::identifier::Identifier),
    /// A `PublishedTable.columns` was `Some(vec![])`.
    #[error("publication {0:?} table {1:?}: empty column list (use None to publish all columns)")]
    EmptyColumnList(
        crate::identifier::Identifier,
        crate::identifier::QualifiedName,
    ),
    /// A `PublishKinds` had all four DML flags false.
    #[error("publication {0:?}: empty publish bitset (must enable at least one DML kind)")]
    EmptyPublishBitset(crate::identifier::Identifier),
    /// A `PublishedTable.columns` contained a duplicate column name.
    #[error("publication {0:?} table {1:?}: duplicate column {2:?} in column list")]
    DuplicateColumnInPublication(
        crate::identifier::Identifier,
        crate::identifier::QualifiedName,
        crate::identifier::Identifier,
    ),

    /// A view column's type was still unresolved at canon time.
    #[error("view {view}: column {column} has an unresolved type (internal resolver bug)")]
    UnresolvedViewColumn {
        /// The view whose column is unresolved.
        view: crate::identifier::QualifiedName,
        /// The unresolved column.
        column: crate::identifier::Identifier,
    },

    /// A `Subscription.publications` was empty.
    #[error("subscription {0:?}: empty publication list (PG requires at least one)")]
    EmptySubscriptionPublications(crate::identifier::Identifier),
    /// A `Subscription.connection` was empty or whitespace-only.
    #[error("subscription {0:?}: empty connection string")]
    EmptyConnection(crate::identifier::Identifier),

    /// A `StatisticKinds` had all three flags false.
    #[error(
        "statistic {0}: empty kinds bitset (must enable at least one of ndistinct, dependencies, mcv)"
    )]
    EmptyStatisticKinds(crate::identifier::QualifiedName),
    /// A `Statistic.columns` was empty.
    #[error("statistic {0}: empty column list")]
    EmptyStatisticColumns(crate::identifier::QualifiedName),

    /// Two event triggers share a name.
    #[error("duplicate event trigger: {0}")]
    DuplicateEventTrigger(crate::identifier::Identifier),

    /// Two aggregates share the same `(qname, arg_types)` overload identity.
    #[error("duplicate aggregate overload: {0} (same name and argument types)")]
    DuplicateAggregate(crate::identifier::QualifiedName),

    /// Two `TEXT SEARCH DICTIONARY` objects share the same `qname`.
    #[error("duplicate text search dictionary: {0}")]
    DuplicateTsDictionary(crate::identifier::QualifiedName),

    /// Two `TEXT SEARCH CONFIGURATION` objects share the same `qname`.
    #[error("duplicate text search configuration: {0}")]
    DuplicateTsConfiguration(crate::identifier::QualifiedName),

    /// Two casts share the same `(source, target)` identity.
    #[error("duplicate cast ({src} AS {tgt})")]
    DuplicateCast {
        /// Source type of the duplicate cast.
        src: crate::identifier::QualifiedName,
        /// Target type of the duplicate cast.
        tgt: crate::identifier::QualifiedName,
    },

    /// Two tablespaces share a name.
    #[error("duplicate tablespace: {0}")]
    DuplicateTablespace(crate::identifier::Identifier),

    /// Two objects in the same collection share a key (name / qualified name /
    /// overload identity).
    #[error("duplicate {kind}: {key}")]
    DuplicateObject {
        /// Human-readable object kind, e.g. "table", "index", "schema".
        kind: &'static str,
        /// The duplicated key, formatted for display.
        key: String,
    },

    /// A `Collation` failed canon validation (e.g. nondeterministic libc).
    #[error("collation {qname}: invalid — {reason}")]
    InvalidCollation {
        /// Schema-qualified collation name.
        qname: crate::identifier::QualifiedName,
        /// Why the collation is invalid.
        reason: String,
    },
}
