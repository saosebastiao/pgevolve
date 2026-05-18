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
}
