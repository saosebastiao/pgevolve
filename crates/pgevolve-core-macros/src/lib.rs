//! Internal proc-macros for pgevolve-core.
//!
//! See `docs/superpowers/specs/2026-05-19-diff-derive-macro-design.md`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DataStruct, DeriveInput, Fields, Ident, parse_macro_input, spanned::Spanned};

/// Derive a `Diff` impl for a named-field struct.
///
/// Per-field attributes:
/// - `#[diff(skip)]`      — omit the field entirely.
/// - `#[diff(via_debug)]` — compare via `format!("{:?}", _)`.
/// - `#[diff(nested)]`    — recurse into the field's own `Diff` impl
///   and prefix with the field name.
///
/// Default (no attribute) requires the field type to implement
/// `PartialEq + std::fmt::Display`.
#[proc_macro_derive(Diff, attributes(diff))]
pub fn derive_diff(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match derive_diff_impl(&ast) {
        Ok(ts) => ts.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strategy {
    Plain,
    Skip,
    ViaDebug,
    Nested,
}

fn derive_diff_impl(ast: &DeriveInput) -> syn::Result<TokenStream2> {
    let struct_data: &DataStruct = match &ast.data {
        Data::Struct(s) => s,
        _ => {
            return Err(syn::Error::new(
                ast.span(),
                "#[derive(Diff)] only supports structs with named fields",
            ));
        }
    };

    let named = match &struct_data.fields {
        Fields::Named(n) => &n.named,
        _ => {
            return Err(syn::Error::new(
                struct_data.fields.span(),
                "#[derive(Diff)] only supports structs with named fields",
            ));
        }
    };

    let mut field_blocks: Vec<TokenStream2> = Vec::with_capacity(named.len());
    for field in named {
        let ident: &Ident = field
            .ident
            .as_ref()
            .expect("Fields::Named guarantees idents");
        let strategy = parse_strategy(field)?;
        if let Some(block) = emit_field_block(ident, strategy) {
            field_blocks.push(block);
        }
    }

    let name = &ast.ident;
    let (impl_g, ty_g, where_c) = ast.generics.split_for_impl();

    Ok(quote! {
        impl #impl_g crate::ir::eq::Diff for #name #ty_g #where_c {
            fn diff(
                &self,
                other: &Self,
            ) -> ::std::vec::Vec<crate::ir::difference::Difference> {
                let mut out = ::std::vec::Vec::new();
                #(#field_blocks)*
                out
            }
        }
    })
}

fn parse_strategy(field: &syn::Field) -> syn::Result<Strategy> {
    let mut strategy = Strategy::Plain;
    for attr in &field.attrs {
        if !attr.path().is_ident("diff") {
            continue;
        }
        let mut new_strategy: Option<Strategy> = None;
        attr.parse_nested_meta(|meta| {
            let candidate = if meta.path.is_ident("skip") {
                Strategy::Skip
            } else if meta.path.is_ident("via_debug") {
                Strategy::ViaDebug
            } else if meta.path.is_ident("nested") {
                Strategy::Nested
            } else {
                return Err(meta
                    .error("unknown #[diff(...)] attribute; supported: skip, via_debug, nested"));
            };
            if new_strategy.is_some() {
                return Err(
                    meta.error("only one #[diff(...)] strategy attribute is allowed per field")
                );
            }
            new_strategy = Some(candidate);
            Ok(())
        })?;
        if let Some(s) = new_strategy {
            if strategy != Strategy::Plain {
                return Err(syn::Error::new(
                    attr.span(),
                    "only one #[diff(...)] attribute is allowed per field",
                ));
            }
            strategy = s;
        }
    }
    Ok(strategy)
}

fn emit_field_block(ident: &Ident, strategy: Strategy) -> Option<TokenStream2> {
    let name_lit = ident.to_string();
    match strategy {
        Strategy::Skip => None,
        Strategy::Plain => Some(quote! {
            out.extend(crate::ir::eq::diff_field(
                #name_lit,
                &self.#ident,
                &other.#ident,
            ));
        }),
        Strategy::ViaDebug => Some(quote! {
            out.extend(crate::ir::eq::diff_field(
                #name_lit,
                &format!("{:?}", self.#ident),
                &format!("{:?}", other.#ident),
            ));
        }),
        Strategy::Nested => Some(quote! {
            out.extend(crate::ir::eq::prefix_diffs(
                #name_lit,
                crate::ir::eq::Diff::diff(&self.#ident, &other.#ident),
            ));
        }),
    }
}
