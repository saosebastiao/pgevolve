//! Internal proc-macros for pgevolve-core.
//!
//! Currently exposes a single derive, [`macro@Diff`], that generates a
//! [`pgevolve_core::ir::eq::Diff`] impl for plain field-list structs in the
//! pgevolve IR. See the design doc at
//! `docs/superpowers/specs/2026-05-19-diff-derive-macro-design.md`.

use proc_macro::TokenStream;

/// Derive a `Diff` impl for a named-field struct.
///
/// Per-field attributes:
/// - `#[diff(skip)]`      — omit the field entirely.
/// - `#[diff(via_debug)]` — compare via `format!("{:?}", _)`.
/// - `#[diff(nested)]`    — recurse into the field's own `Diff` impl
///                          and prefix with the field name.
///
/// Default (no attribute) requires the field type to implement
/// `PartialEq + std::fmt::Display`.
#[proc_macro_derive(Diff, attributes(diff))]
pub fn derive_diff(_input: TokenStream) -> TokenStream {
    // Filled in by Task 2.
    TokenStream::new()
}
