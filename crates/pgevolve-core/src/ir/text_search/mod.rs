//! `TEXT SEARCH` IR types — `TsDictionary` and `TsConfiguration`.
//!
//! Both are schema-scoped managed objects. `TEXT SEARCH PARSER` and
//! `TEXT SEARCH TEMPLATE` are unmanaged environment references (require C
//! functions) and are represented only as [`QualifiedName`] references within
//! `template` and `parser` fields.
//!
//! [`QualifiedName`]: crate::identifier::QualifiedName

pub mod configuration;
pub mod dictionary;

pub use configuration::{TsConfiguration, TsMapping};
pub use dictionary::TsDictionary;
