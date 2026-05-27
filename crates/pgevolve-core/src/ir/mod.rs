//! In-memory representation of a Postgres schema.
//!
//! The IR is the contract between every other component. Both the source-side
//! parser and the catalog reader produce these types; the differ, dependency
//! analyzer, and planner consume them.

pub mod canon;
pub mod catalog;
pub mod cluster;
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
pub mod subscription;
pub mod table;
pub use function::*;
pub mod procedure;
pub use procedure::*;
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

    /// A `Subscription.publications` was empty.
    #[error("subscription {0:?}: empty publication list (PG requires at least one)")]
    EmptySubscriptionPublications(crate::identifier::Identifier),
    /// A `Subscription.connection` was empty or whitespace-only.
    #[error("subscription {0:?}: empty connection string")]
    EmptyConnection(crate::identifier::Identifier),
}
