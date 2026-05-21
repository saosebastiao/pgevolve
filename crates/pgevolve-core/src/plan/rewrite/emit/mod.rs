//! Per-object-family dispatchers for the rewrite pass.
//!
//! Each file in this module handles the `Change::*` variants for one
//! family. The top-level [`super::emit_change`] dispatcher routes each
//! variant to a `pub(super) fn` here. Future v0.2 sub-specs (extensions,
//! triggers, partitioning) add new files in this directory.

pub(super) mod constraint;
pub(super) mod deferred_fk;
pub(super) mod extension;
pub(super) mod function;
pub(super) mod index;
pub(super) mod mv;
pub(super) mod procedure;
pub(super) mod schema;
pub(super) mod sequence;
pub(super) mod table;
pub(super) mod trigger;
pub(super) mod user_type;
pub(super) mod view;
