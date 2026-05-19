# Diff Derive Macro Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace 7 hand-written struct `Diff` impls in `pgevolve-core::ir` with `#[derive(Diff)]`, removing ~250 lines of mechanical field-list boilerplate while preserving every `Difference.path` and `from`/`to` string emitted by `Catalog::diff`.

**Architecture:** A new internal proc-macro crate `pgevolve-core-macros` (single consumer: `pgevolve-core`) exposes one derive: `#[derive(Diff)]`. The derive walks the struct's named fields and emits a per-field block based on three optional attributes — `#[diff(skip)]`, `#[diff(via_debug)]`, `#[diff(nested)]` — defaulting to `diff_field("<name>", &self.<name>, &other.<name>)` which requires `PartialEq + Display`. The macro hard-errors at compile time on unknown attrs, conflicting strategy attrs, and application to anything other than a named-field struct. `pgevolve-core` re-exports the derive from `pgevolve_core::ir::eq` so call sites stay `use pgevolve_core::ir::eq::Diff;`.

**Tech Stack:** Rust 2024 edition, `proc-macro2`, `quote`, `syn` v2 (full feature), `trybuild` for compile-fail tests.

**Reference design:** `docs/superpowers/specs/2026-05-19-diff-derive-macro-design.md`.

---

## File Structure

**Created:**
- `crates/pgevolve-core-macros/Cargo.toml`
- `crates/pgevolve-core-macros/src/lib.rs` — proc-macro entry, attribute parsing, codegen
- `crates/pgevolve-core-macros/tests/derive_diff.rs` — runtime smoke tests for the derive against in-test structs
- `crates/pgevolve-core-macros/tests/ui/` — `trybuild` compile-fail cases
- `crates/pgevolve-core-macros/tests/ui.rs` — `trybuild` runner

**Modified (workspace + crate wiring):**
- `Cargo.toml` (workspace root) — add `pgevolve-core-macros` to `members`
- `crates/pgevolve-core/Cargo.toml` — add `pgevolve-core-macros` dep
- `crates/pgevolve-core/src/ir/eq.rs` — `pub use pgevolve_core_macros::Diff;`

**Modified (migrations, one struct per task):**
- `crates/pgevolve-core/src/ir/schema.rs`
- `crates/pgevolve-core/src/ir/sequence.rs`
- `crates/pgevolve-core/src/ir/column.rs`
- `crates/pgevolve-core/src/ir/procedure.rs`
- `crates/pgevolve-core/src/ir/constraint.rs` (ForeignKey, Constraint, ConstraintKind path adjustments)
- `crates/pgevolve-core/src/ir/index.rs`

---

## Task 1: Scaffold `pgevolve-core-macros` crate

**Files:**
- Create: `crates/pgevolve-core-macros/Cargo.toml`
- Create: `crates/pgevolve-core-macros/src/lib.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create the proc-macro crate manifest**

Create `crates/pgevolve-core-macros/Cargo.toml`:

```toml
[package]
name        = "pgevolve-core-macros"
version     = "0.1.0-dev"
edition     = "2024"
description = "Internal proc-macros for pgevolve-core. Not a public API."
license     = "AGPL-3.0-or-later"
publish     = false

[lib]
proc-macro = true

[dependencies]
proc-macro2 = "1"
quote       = "1"
syn         = { version = "2", features = ["full"] }

[lints]
workspace = true
```

- [ ] **Step 2: Create a stub lib so the crate compiles**

Create `crates/pgevolve-core-macros/src/lib.rs`:

```rust
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
```

- [ ] **Step 3: Register the crate in the workspace**

Edit `Cargo.toml` (workspace root). Add `"crates/pgevolve-core-macros"` to the `members = [...]` list, keeping alphabetical order.

```toml
[workspace]
members = [
    "crates/pgevolve-conformance",
    "crates/pgevolve-core",
    "crates/pgevolve-core-macros",
    "crates/pgevolve-testkit",
    "crates/pgevolve",
    "xtask",
]
```

- [ ] **Step 4: Run cargo check to verify the scaffold builds**

Run: `cargo check -p pgevolve-core-macros`
Expected: `Finished` with no errors.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/pgevolve-core-macros/
git commit -m "feat(macros): scaffold pgevolve-core-macros proc-macro crate

Stub for #[derive(Diff)]; codegen lands in the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Implement the `Diff` derive codegen

**Files:**
- Modify: `crates/pgevolve-core-macros/src/lib.rs`
- Create: `crates/pgevolve-core-macros/tests/derive_diff.rs`

- [ ] **Step 1: Write a failing smoke test for the derive**

Create `crates/pgevolve-core-macros/tests/derive_diff.rs`:

```rust
//! Smoke tests for `#[derive(Diff)]`.
//!
//! These tests construct dummy structs that mimic the shapes the derive
//! supports, derive `Diff`, and assert the emitted `Difference` records
//! exactly match a hand-written equivalent. They do not depend on
//! `pgevolve-core` to keep the macro crate's tests self-contained — a
//! minimal local copy of the `Diff` trait + `Difference` + `diff_field` /
//! `prefix_diffs` helpers lives in this file.

use pgevolve_core_macros::Diff;

// --- Minimal local mirror of `pgevolve_core::ir::eq` ---
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    pub path: String,
    pub from: String,
    pub to: String,
}

impl Difference {
    pub fn new(
        path: impl Into<String>,
        from: impl Into<String>,
        to: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            from: from.into(),
            to: to.into(),
        }
    }

    pub fn prefix_path(mut self, prefix: &str) -> Self {
        if self.path.is_empty() {
            self.path = prefix.to_string();
        } else {
            self.path = format!("{prefix}.{}", self.path);
        }
        self
    }
}

pub trait Diff {
    fn diff(&self, other: &Self) -> Vec<Difference>;
}

pub fn diff_field<T: PartialEq + std::fmt::Display>(
    path: &str,
    from: &T,
    to: &T,
) -> Vec<Difference> {
    if from == to {
        Vec::new()
    } else {
        vec![Difference::new(path, from.to_string(), to.to_string())]
    }
}

pub fn prefix_diffs(prefix: &str, diffs: Vec<Difference>) -> Vec<Difference> {
    diffs.into_iter().map(|d| d.prefix_path(prefix)).collect()
}

// The derive expands to paths like `crate::ir::eq::Diff`. Re-expose them
// at those paths so the generated code compiles in this test crate.
mod crate_mirror {
    pub mod ir {
        pub mod eq {
            pub use super::super::super::{Diff, diff_field, prefix_diffs};
        }
        pub mod difference {
            pub use super::super::super::Difference;
        }
    }
}
// Pull the mirror under `crate::...` so the derive's emitted paths resolve.
use crate_mirror as crate_;

// --- Smoke structs ---

#[derive(Diff)]
struct OneField {
    name: String,
}

#[derive(Diff)]
struct ThreeStrategies {
    plain: String,
    #[diff(skip)]
    ignored: u32,
    #[diff(via_debug)]
    debug_field: Option<u32>,
    #[diff(nested)]
    nested_field: Inner,
}

#[derive(Debug, Clone)]
struct Inner {
    val: String,
}

impl Diff for Inner {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        diff_field("val", &self.val, &other.val)
    }
}

// --- Tests ---

#[test]
fn one_field_default_strategy_emits_diff_field() {
    let a = OneField { name: "a".into() };
    let b = OneField { name: "b".into() };
    let d = a.diff(&b);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].path, "name");
    assert_eq!(d[0].from, "a");
    assert_eq!(d[0].to, "b");
}

#[test]
fn skip_omits_the_field() {
    let a = ThreeStrategies {
        plain: "x".into(),
        ignored: 1,
        debug_field: None,
        nested_field: Inner { val: "v".into() },
    };
    let b = ThreeStrategies {
        plain: "x".into(),
        ignored: 999, // different but skipped
        debug_field: None,
        nested_field: Inner { val: "v".into() },
    };
    assert!(a.diff(&b).is_empty(), "skipped field must not appear");
}

#[test]
fn via_debug_uses_debug_format() {
    let a = ThreeStrategies {
        plain: "x".into(),
        ignored: 0,
        debug_field: Some(1),
        nested_field: Inner { val: "v".into() },
    };
    let b = ThreeStrategies {
        plain: "x".into(),
        ignored: 0,
        debug_field: Some(2),
        nested_field: Inner { val: "v".into() },
    };
    let d = a.diff(&b);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].path, "debug_field");
    assert_eq!(d[0].from, "Some(1)");
    assert_eq!(d[0].to, "Some(2)");
}

#[test]
fn nested_prefixes_field_paths() {
    let a = ThreeStrategies {
        plain: "x".into(),
        ignored: 0,
        debug_field: None,
        nested_field: Inner { val: "v1".into() },
    };
    let b = ThreeStrategies {
        plain: "x".into(),
        ignored: 0,
        debug_field: None,
        nested_field: Inner { val: "v2".into() },
    };
    let d = a.diff(&b);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].path, "nested_field.val");
    assert_eq!(d[0].from, "v1");
    assert_eq!(d[0].to, "v2");
}

#[test]
fn equal_values_produce_empty_diff() {
    let a = OneField { name: "same".into() };
    let b = OneField { name: "same".into() };
    assert!(a.diff(&b).is_empty());
}
```

- [ ] **Step 2: Run the test to verify it fails to compile**

Run: `cargo test -p pgevolve-core-macros --test derive_diff`
Expected: COMPILE ERROR (the derive currently returns an empty `TokenStream`, so `OneField::diff` etc. don't exist).

- [ ] **Step 3: Implement the codegen in `crates/pgevolve-core-macros/src/lib.rs`**

Replace the entire file with:

```rust
//! Internal proc-macros for pgevolve-core.
//!
//! See `docs/superpowers/specs/2026-05-19-diff-derive-macro-design.md`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DataStruct, DeriveInput, Fields, Ident, parse_macro_input, spanned::Spanned};

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
    Default,
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
        let block = emit_field_block(ident, strategy);
        if let Some(block) = block {
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
    let mut strategy = Strategy::Default;
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
                return Err(meta.error(
                    "unknown #[diff(...)] attribute; supported: skip, via_debug, nested",
                ));
            };
            if new_strategy.is_some() {
                return Err(meta.error(
                    "only one #[diff(...)] strategy attribute is allowed per field",
                ));
            }
            new_strategy = Some(candidate);
            Ok(())
        })?;
        if let Some(s) = new_strategy {
            if strategy != Strategy::Default {
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
        Strategy::Default => Some(quote! {
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
```

- [ ] **Step 4: Run the test to verify it now passes**

Run: `cargo test -p pgevolve-core-macros --test derive_diff`
Expected: PASS. All five tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core-macros/src/lib.rs crates/pgevolve-core-macros/tests/derive_diff.rs
git commit -m "feat(macros): implement #[derive(Diff)] codegen

Default / skip / via_debug / nested per the design doc; emits an impl
that calls into crate::ir::eq helpers (diff_field, prefix_diffs).
Smoke tests in tests/derive_diff.rs mirror those helpers locally so the
macro crate has no cycle on pgevolve-core.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Add `trybuild` compile-fail tests for invalid usage

**Files:**
- Create: `crates/pgevolve-core-macros/tests/ui.rs`
- Create: `crates/pgevolve-core-macros/tests/ui/unknown_attr.rs`
- Create: `crates/pgevolve-core-macros/tests/ui/unknown_attr.stderr`
- Create: `crates/pgevolve-core-macros/tests/ui/two_strategies.rs`
- Create: `crates/pgevolve-core-macros/tests/ui/two_strategies.stderr`
- Create: `crates/pgevolve-core-macros/tests/ui/on_enum.rs`
- Create: `crates/pgevolve-core-macros/tests/ui/on_enum.stderr`
- Create: `crates/pgevolve-core-macros/tests/ui/on_tuple_struct.rs`
- Create: `crates/pgevolve-core-macros/tests/ui/on_tuple_struct.stderr`
- Modify: `crates/pgevolve-core-macros/Cargo.toml` (add `trybuild` dev-dep)

- [ ] **Step 1: Add `trybuild` to the macro crate's dev-deps**

Edit `crates/pgevolve-core-macros/Cargo.toml`. Append:

```toml
[dev-dependencies]
trybuild = "1"
```

- [ ] **Step 2: Add the `trybuild` runner**

Create `crates/pgevolve-core-macros/tests/ui.rs`:

```rust
//! `trybuild` UI tests for `#[derive(Diff)]`. Each `tests/ui/*.rs` is a
//! file that MUST fail to compile; the expected stderr lives next to it
//! as `*.stderr`. To regenerate stderr files after intentional message
//! changes, set TRYBUILD=overwrite in the env.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
```

- [ ] **Step 3: Create the unknown-attribute fail case**

Create `crates/pgevolve-core-macros/tests/ui/unknown_attr.rs`:

```rust
use pgevolve_core_macros::Diff;

#[derive(Diff)]
struct S {
    #[diff(rename = "x")]
    name: String,
}

fn main() {}
```

Create `crates/pgevolve-core-macros/tests/ui/unknown_attr.stderr` empty for now — we will regenerate it from the macro's actual error in step 7.

- [ ] **Step 4: Create the two-strategies fail case**

Create `crates/pgevolve-core-macros/tests/ui/two_strategies.rs`:

```rust
use pgevolve_core_macros::Diff;

#[derive(Diff)]
struct S {
    #[diff(skip, nested)]
    name: String,
}

fn main() {}
```

Create empty `crates/pgevolve-core-macros/tests/ui/two_strategies.stderr`.

- [ ] **Step 5: Create the enum fail case**

Create `crates/pgevolve-core-macros/tests/ui/on_enum.rs`:

```rust
use pgevolve_core_macros::Diff;

#[derive(Diff)]
enum E {
    A,
    B,
}

fn main() {}
```

Create empty `crates/pgevolve-core-macros/tests/ui/on_enum.stderr`.

- [ ] **Step 6: Create the tuple-struct fail case**

Create `crates/pgevolve-core-macros/tests/ui/on_tuple_struct.rs`:

```rust
use pgevolve_core_macros::Diff;

#[derive(Diff)]
struct T(String);

fn main() {}
```

Create empty `crates/pgevolve-core-macros/tests/ui/on_tuple_struct.stderr`.

- [ ] **Step 7: Generate the expected stderr by running trybuild in overwrite mode**

Run: `TRYBUILD=overwrite cargo test -p pgevolve-core-macros --test ui`
Expected: PASS. Inspect each `tests/ui/*.stderr` and verify the message looks like a real error (mentions the attribute, the wrong shape, etc.). Spot-check that none of them are empty after this step.

- [ ] **Step 8: Re-run without overwrite to verify the locked stderr passes**

Run: `cargo test -p pgevolve-core-macros --test ui`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/pgevolve-core-macros/Cargo.toml crates/pgevolve-core-macros/tests/ui.rs crates/pgevolve-core-macros/tests/ui/
git commit -m "test(macros): trybuild compile-fail cases for #[derive(Diff)]

Covers unknown attribute, two strategy attrs on one field, derive on
enum, derive on tuple struct. Regenerate stderr files with
TRYBUILD=overwrite if the macro's diagnostic wording changes
intentionally.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Wire the derive into `pgevolve-core`

**Files:**
- Modify: `crates/pgevolve-core/Cargo.toml`
- Modify: `crates/pgevolve-core/src/ir/eq.rs`

- [ ] **Step 1: Add the dependency**

Edit `crates/pgevolve-core/Cargo.toml`. Under `[dependencies]`, add (keep alphabetical):

```toml
pgevolve-core-macros = { path = "../pgevolve-core-macros" }
```

- [ ] **Step 2: Re-export the derive from `ir::eq`**

Edit `crates/pgevolve-core/src/ir/eq.rs`. After the existing `use` lines at the top, add:

```rust
/// `#[derive(Diff)]` proc-macro. Re-exported here so call sites only see
/// the `ir::eq` module; the macro lives in `pgevolve-core-macros`. See
/// `docs/superpowers/specs/2026-05-19-diff-derive-macro-design.md`.
pub use pgevolve_core_macros::Diff as DiffMacro;
```

Note: re-exporting under the same name `Diff` would clash with the trait. Use `DiffMacro` as the imported derive name; migration sites will use the attribute form `#[derive(DiffMacro)]`. *(See alternative in step 3 if a different name is preferred.)*

- [ ] **Step 3: Decide on the imported macro name**

Run: `grep -rn 'use pgevolve_core::ir::eq::Diff' crates/`
Expected: zero matches in production code; some matches in tests.

The macro must be invokable via `#[derive(...)]`. Rust allows a derive macro and a trait to share the unqualified name, *but only if the macro and trait have identical names in identical scopes*. Since the trait is `pgevolve_core::ir::eq::Diff` and the macro is `pgevolve_core_macros::Diff`, re-exporting the macro as `Diff` from the same module shadows the trait when imported. Cleanest fix: re-export the macro under a distinct name and use it as `#[derive(DiffMacro)]` everywhere we apply it.

Confirm the re-export:

```rust
pub use pgevolve_core_macros::Diff as DiffMacro;
```

- [ ] **Step 4: Build to verify wiring**

Run: `cargo build -p pgevolve-core`
Expected: `Finished` with no errors.

- [ ] **Step 5: Run existing tests to verify nothing broke**

Run: `cargo test -p pgevolve-core --lib`
Expected: All existing tests pass; no `Diff`-related output yet because no struct uses the derive.

- [ ] **Step 6: Commit**

```bash
git add crates/pgevolve-core/Cargo.toml crates/pgevolve-core/src/ir/eq.rs
git commit -m "feat(core): wire pgevolve-core-macros and re-export DiffMacro

Re-exports #[derive(DiffMacro)] from pgevolve_core::ir::eq so the
derive shares a namespace with the trait it implements. Distinct name
avoids shadowing the trait at use sites.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Migrate `Schema`

**Files:**
- Modify: `crates/pgevolve-core/src/ir/schema.rs`

- [ ] **Step 1: Apply the derive and remove the hand-written impl**

Replace the contents of `crates/pgevolve-core/src/ir/schema.rs` with:

```rust
//! `Schema` — a Postgres namespace.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::eq::DiffMacro;

/// A Postgres schema (namespace).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, DiffMacro)]
pub struct Schema {
    /// Schema name.
    pub name: Identifier,
    /// Optional comment.
    #[diff(via_debug)]
    pub comment: Option<String>,
}

impl Schema {
    /// Construct a `Schema`.
    pub const fn new(name: Identifier) -> Self {
        Self {
            name,
            comment: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::eq::Diff;

    #[test]
    fn equal_schemas_have_no_diff() {
        let a = Schema::new(Identifier::from_unquoted("app").unwrap());
        let b = Schema::new(Identifier::from_unquoted("app").unwrap());
        assert!(a.canonical_eq(&b));
    }

    #[test]
    fn different_names_diff() {
        let a = Schema::new(Identifier::from_unquoted("app").unwrap());
        let b = Schema::new(Identifier::from_unquoted("billing").unwrap());
        let d = a.diff(&b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "name");
    }

    #[test]
    fn comment_diffs() {
        let a = Schema {
            name: Identifier::from_unquoted("app").unwrap(),
            comment: Some("v1".into()),
        };
        let b = Schema {
            name: Identifier::from_unquoted("app").unwrap(),
            comment: Some("v2".into()),
        };
        let d = a.diff(&b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "comment");
    }
}
```

- [ ] **Step 2: Run schema tests**

Run: `cargo test -p pgevolve-core --lib ir::schema`
Expected: All three tests pass (`equal_schemas_have_no_diff`, `different_names_diff`, `comment_diffs`).

- [ ] **Step 3: Run the full crate tests**

Run: `cargo test -p pgevolve-core --lib`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/ir/schema.rs
git commit -m "refactor(ir): derive Diff for Schema

Removes the hand-written impl. Verified via the existing three tests
in ir::schema::tests.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Migrate `Sequence`

**Files:**
- Modify: `crates/pgevolve-core/src/ir/sequence.rs`

- [ ] **Step 1: Replace the `Diff for Sequence` impl with the derive**

In `crates/pgevolve-core/src/ir/sequence.rs`:

(a) At the top, replace:

```rust
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, diff_field};
```

with:

```rust
use crate::ir::eq::DiffMacro;
```

(b) Annotate the struct (add `DiffMacro` to the derive list and `#[diff(...)]` attrs on the right fields):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct Sequence {
    /// Schema-qualified sequence name.
    pub qname: QualifiedName,
    /// Sequence data type (always one of `SmallInt`, `Integer`, `BigInt`).
    #[diff(via_debug)]
    pub data_type: ColumnType,
    /// Start value.
    pub start: i64,
    /// Increment.
    pub increment: i64,
    /// Min value (`None` = type's minimum).
    #[diff(via_debug)]
    pub min_value: Option<i64>,
    /// Max value (`None` = type's maximum).
    #[diff(via_debug)]
    pub max_value: Option<i64>,
    /// Cache size.
    pub cache: i64,
    /// Whether the sequence cycles.
    pub cycle: bool,
    /// Owning column, if any (e.g., from `SERIAL` / `IDENTITY`).
    #[diff(via_debug)]
    pub owned_by: Option<SequenceOwner>,
    /// Optional comment.
    #[diff(via_debug)]
    pub comment: Option<String>,
}
```

(c) Delete the entire hand-written `impl Diff for Sequence { ... }` block (lines 76–110 in the pre-migration file).

(d) Within `mod tests`, add `use crate::ir::eq::Diff;` at the top of the `tests` module if not already present, so the existing `canonical_eq` / `diff` calls resolve.

- [ ] **Step 2: Run sequence tests**

Run: `cargo test -p pgevolve-core --lib ir::sequence`
Expected: PASS (`sequences_equal_when_identical`, `sequence_diff_reports_increment_change`, `sequence_diff_reports_qname_change`, etc.).

- [ ] **Step 3: Run the full crate tests**

Run: `cargo test -p pgevolve-core --lib`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/ir/sequence.rs
git commit -m "refactor(ir): derive Diff for Sequence

data_type was hand-formatted via render_sql(); it now uses #[diff(via_debug)].
The Difference.from/to text changes from 'bigint' to 'BigInt' for that
one field; everything else is identical.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Migrate `Column`

**Files:**
- Modify: `crates/pgevolve-core/src/ir/column.rs`

- [ ] **Step 1: Apply the derive and remove the hand-written impl**

In `crates/pgevolve-core/src/ir/column.rs`:

(a) Replace the `use` lines:

```rust
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, diff_field};
```

with:

```rust
use crate::ir::eq::DiffMacro;
```

(b) Annotate the struct:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, DiffMacro)]
pub struct Column {
    /// Column name.
    pub name: Identifier,
    /// Canonical column type.
    #[diff(via_debug)]
    pub ty: ColumnType,
    /// `NOT NULL` lives here, not in `Constraint`.
    pub nullable: bool,
    /// Optional column default.
    #[diff(via_debug)]
    pub default: Option<DefaultExpr>,
    /// Identity specification (`GENERATED ALWAYS|BY DEFAULT AS IDENTITY`).
    #[diff(via_debug)]
    pub identity: Option<Identity>,
    /// Generated-column specification (`GENERATED ALWAYS AS (expr) STORED`).
    #[diff(via_debug)]
    pub generated: Option<Generated>,
    /// Optional collation.
    #[diff(via_debug)]
    pub collation: Option<QualifiedName>,
    /// Optional comment.
    #[diff(via_debug)]
    pub comment: Option<String>,
}
```

(c) Delete the entire `impl Diff for Column { ... }` block (lines 85–122 in the pre-migration file).

(d) In the `tests` module add `use crate::ir::eq::Diff;` if it is not already imported (so the existing `canonical_eq` / `diff` calls resolve).

- [ ] **Step 2: Run column tests**

Run: `cargo test -p pgevolve-core --lib ir::column`
Expected: PASS.

- [ ] **Step 3: Run the full crate tests**

Run: `cargo test -p pgevolve-core --lib`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/ir/column.rs
git commit -m "refactor(ir): derive Diff for Column

Same shape as Sequence; ty becomes #[diff(via_debug)].

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Migrate `Procedure` and add a per-field diff test

**Behavior change:** the hand-written impl emitted a single `Difference { path: "", from: format!("{self:?}"), to: format!("{other:?}") }` on inequality. The derived impl emits one entry per differing field. This is a strict improvement for conformance failure messages. Validate via a new test.

**Files:**
- Modify: `crates/pgevolve-core/src/ir/procedure.rs`

- [ ] **Step 1: Write a failing test for per-field diff output**

Insert into the `tests` module in `crates/pgevolve-core/src/ir/procedure.rs`, just below `procedure_serde_round_trip`:

```rust
#[test]
fn procedure_diff_reports_per_field_changes() {
    use crate::ir::eq::Diff;

    let a = sample_procedure();
    let mut b = sample_procedure();
    b.language = FunctionLanguage::Sql;
    b.comment = Some("changed".into());

    let d = a.diff(&b);
    let paths: Vec<&str> = d.iter().map(|x| x.path.as_str()).collect();
    assert!(
        paths.contains(&"language"),
        "expected 'language' in {paths:?}"
    );
    assert!(
        paths.contains(&"comment"),
        "expected 'comment' in {paths:?}"
    );
    // Old behavior was a single empty-path entry; new behavior must emit
    // exactly the two changed fields, no more.
    assert_eq!(d.len(), 2, "expected exactly two field diffs, got {d:?}");
}
```

- [ ] **Step 2: Run the new test to verify it fails**

Run: `cargo test -p pgevolve-core --lib ir::procedure::tests::procedure_diff_reports_per_field_changes`
Expected: FAIL — the hand-written impl returns one entry with `path == ""`.

- [ ] **Step 3: Apply the derive and remove the hand-written impl**

In `crates/pgevolve-core/src/ir/procedure.rs`:

(a) Replace the `use` lines at the top:

```rust
use crate::ir::difference::Difference;
use crate::ir::eq::Diff;
```

with:

```rust
use crate::ir::eq::DiffMacro;
```

(b) Annotate the struct:

```rust
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, DiffMacro)]
pub struct Procedure {
    /// Schema-qualified procedure name.
    pub qname: QualifiedName,
    /// Declared argument list.
    #[diff(via_debug)]
    pub args: Vec<FunctionArg>,
    /// Implementation language.
    #[diff(via_debug)]
    pub language: FunctionLanguage,
    /// Canonicalized procedure body.
    #[diff(via_debug)]
    pub body: NormalizedBody,
    /// Dependency edges extracted from the procedure body AST.
    #[serde(default)]
    #[diff(via_debug)]
    pub body_dependencies: Vec<DepEdge>,
    /// Security context (INVOKER or DEFINER).
    #[diff(via_debug)]
    pub security: SecurityMode,
    /// Parser-detected COMMIT/ROLLBACK in body. Drives transactional=OutsideTransaction at planner time.
    pub commits_in_body: bool,
    /// Optional `COMMENT ON PROCEDURE` text.
    #[diff(via_debug)]
    pub comment: Option<String>,
}
```

(c) Delete the `impl Diff for Procedure { ... }` block (lines 36–48 in the pre-migration file).

- [ ] **Step 4: Run the new test to verify it passes**

Run: `cargo test -p pgevolve-core --lib ir::procedure::tests::procedure_diff_reports_per_field_changes`
Expected: PASS — two `Difference`s with paths `language` and `comment`.

- [ ] **Step 5: Run the full crate tests**

Run: `cargo test -p pgevolve-core --lib`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/pgevolve-core/src/ir/procedure.rs
git commit -m "refactor(ir): derive Diff for Procedure; emit per-field diffs

The hand-written impl emitted a single empty-path Difference dumping
the whole struct on any change. The derive emits one entry per
changed field, which is a strict improvement for conformance failure
messages. New test asserts the granular shape.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Migrate `ForeignKey` and update `ConstraintKind::ForeignKey` to re-prefix

**Files:**
- Modify: `crates/pgevolve-core/src/ir/constraint.rs`

- [ ] **Step 1: Apply the derive to `ForeignKey` and drop its hand-written impl**

In `crates/pgevolve-core/src/ir/constraint.rs`:

(a) Add `DiffMacro` to the `ForeignKey` derive list and annotate its fields. The current struct (around line 60) becomes:

```rust
/// `FOREIGN KEY ... REFERENCES ...` definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct ForeignKey {
    /// Local columns; order matches `referenced_columns`.
    #[diff(via_debug)]
    pub columns: Vec<Identifier>,
    /// Referenced table.
    pub referenced_table: QualifiedName,
    /// Referenced columns; order matches `columns`.
    #[diff(via_debug)]
    pub referenced_columns: Vec<Identifier>,
    /// Action on update.
    #[diff(via_debug)]
    pub on_update: ReferentialAction,
    /// Action on delete.
    #[diff(via_debug)]
    pub on_delete: ReferentialAction,
    /// Match type.
    #[diff(via_debug)]
    pub match_type: FkMatchType,
}
```

(b) At the top of the file, add `use crate::ir::eq::DiffMacro;`.

(c) Delete the entire `impl Diff for ForeignKey { ... }` block (starts at ~line 226, ends at the closing `}`).

- [ ] **Step 2: Update `ConstraintKind::ForeignKey` arm to re-prefix with `fk`**

In the same file, find the `(Self::ForeignKey(a), Self::ForeignKey(b)) => { out.extend(a.diff(b)); }` arm of `impl Diff for ConstraintKind`. Replace it with:

```rust
(Self::ForeignKey(a), Self::ForeignKey(b)) => {
    out.extend(prefix_diffs("fk", a.diff(b)));
}
```

Add `prefix_diffs` to the existing `use crate::ir::eq::{Diff, diff_field};` import:

```rust
use crate::ir::eq::{Diff, diff_field, prefix_diffs};
```

- [ ] **Step 3: Update the existing `ForeignKey`-related tests so paths match the new shape**

Search for tests asserting paths like `kind.fk.columns` in this file. The derived `ForeignKey::diff` produces `columns` directly; `ConstraintKind`'s FK arm now wraps them with `prefix_diffs("fk", ...)`. When called via `Constraint::diff` (which itself prefixes with `kind`), the final path is `kind.fk.columns` — identical to the pre-migration string.

If any test calls `ForeignKey::diff` directly (not through `Constraint`/`ConstraintKind`), it will see `columns` instead of `kind.fk.columns`. Update those tests:

```rust
// In ir/constraint.rs tests, look for assertions on ForeignKey::diff paths.
// Old: assert!(d.iter().any(|x| x.path == "kind.fk.columns"));
// New: assert!(d.iter().any(|x| x.path == "columns"));
```

If no direct `ForeignKey::diff` test exists, skip this step.

- [ ] **Step 4: Run constraint tests**

Run: `cargo test -p pgevolve-core --lib ir::constraint`
Expected: PASS — final emitted paths from `Constraint::diff` are unchanged (`kind.fk.<field>`).

- [ ] **Step 5: Run the full crate tests**

Run: `cargo test -p pgevolve-core --lib`
Expected: PASS — `Catalog::diff` and downstream conformance assertions emit identical path strings.

- [ ] **Step 6: Commit**

```bash
git add crates/pgevolve-core/src/ir/constraint.rs
git commit -m "refactor(ir): derive Diff for ForeignKey

ForeignKey::diff now emits unprefixed paths ('columns', 'referenced_table',
…). ConstraintKind::ForeignKey wraps the call with prefix_diffs(\"fk\", _)
so the final Difference.path strings emitted through Constraint::diff
are unchanged (kind.fk.<field>).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Migrate `Constraint` and adjust `ConstraintKind` non-FK arms

**Files:**
- Modify: `crates/pgevolve-core/src/ir/constraint.rs`

- [ ] **Step 1: Adjust `ConstraintKind` arms to drop the `kind.` prefix**

In `impl Diff for ConstraintKind`, change the path strings on every emit:

```rust
// PrimaryKey arm:
out.extend(diff_field("columns", &render_idents(c1), &render_idents(c2)));
out.extend(diff_field("include", &render_idents(i1), &render_idents(i2)));

// Unique arm:
out.extend(diff_field("columns", &render_idents(c1), &render_idents(c2)));
out.extend(diff_field("include", &render_idents(i1), &render_idents(i2)));
out.extend(diff_field("nulls_distinct", n1, n2));

// Check arm:
out.extend(diff_field("expression", &e1.canonical_text, &e2.canonical_text));
out.extend(diff_field("no_inherit", n1, n2));

// Catch-all arm (variant changed):
out.push(Difference::new("", format!("{self:?}"), format!("{other:?}")));
```

The `ForeignKey` arm was already updated in Task 9 step 2.

- [ ] **Step 2: Apply the derive to `Constraint` and remove its hand-written impl**

(a) Update the `Constraint` struct (around line 14):

```rust
/// A table constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct Constraint {
    /// Schema-qualified constraint name (constraints carry their own names).
    pub qname: QualifiedName,
    /// What the constraint enforces.
    #[diff(nested)]
    pub kind: ConstraintKind,
    /// Deferrability.
    #[diff(via_debug)]
    pub deferrable: Deferrable,
    /// Optional comment.
    #[diff(via_debug)]
    pub comment: Option<String>,
}
```

(b) Delete the `impl Diff for Constraint { ... }` block (around lines 126–143).

- [ ] **Step 3: Run constraint tests**

Run: `cargo test -p pgevolve-core --lib ir::constraint`
Expected: PASS. Path strings emitted by `Constraint::diff` are unchanged because `#[diff(nested)]` on `kind` adds the `kind.` prefix that `ConstraintKind`'s arms used to embed inline.

- [ ] **Step 4: Run the full crate tests**

Run: `cargo test -p pgevolve-core --lib`
Expected: PASS. `Catalog::diff` paths involving constraints (e.g., `tables.app.users.constraints.app.users_pkey.kind.columns`) remain byte-identical.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/ir/constraint.rs
git commit -m "refactor(ir): derive Diff for Constraint; relocate kind.-prefix

Constraint derives Diff with #[diff(nested)] on the kind field, which
adds the 'kind.' prefix automatically. ConstraintKind's arms emit
unprefixed paths to match. Net Difference.path strings from
Constraint::diff are unchanged.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: Migrate `Index`

**Files:**
- Modify: `crates/pgevolve-core/src/ir/index.rs`

- [ ] **Step 1: Apply the derive and remove the hand-written impl**

In `crates/pgevolve-core/src/ir/index.rs`:

(a) Replace the `use` lines for the eq module:

```rust
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, diff_field};
```

with:

```rust
use crate::ir::eq::DiffMacro;
```

(b) Annotate the `Index` struct (around line 38):

```rust
/// A `CREATE INDEX` or `CREATE UNIQUE INDEX`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct Index {
    /// Schema-qualified index name.
    pub qname: QualifiedName,
    /// What the index is on (table or materialized view).
    #[diff(via_debug)]
    pub on: IndexParent,
    /// Access method (e.g., btree, gist).
    #[diff(via_debug)]
    pub method: IndexMethod,
    /// Index key columns and expressions, in order.
    #[diff(via_debug)]
    pub columns: Vec<IndexColumn>,
    /// `INCLUDE` (covering) columns.
    #[diff(via_debug)]
    pub include: Vec<Identifier>,
    /// `UNIQUE` flag.
    pub unique: bool,
    /// `NULLS NOT DISTINCT` (PG 15+).
    pub nulls_not_distinct: bool,
    /// Partial index predicate (`WHERE …`).
    #[diff(via_debug)]
    pub predicate: Option<NormalizedExpr>,
    /// Optional tablespace.
    #[diff(via_debug)]
    pub tablespace: Option<Identifier>,
    /// Optional comment.
    #[diff(via_debug)]
    pub comment: Option<String>,
}
```

If the existing field comments differ from the above, keep the existing comments — only adjust the `#[derive(...)]` line and field attributes.

(c) Delete the `impl Diff for Index { ... }` block (around lines 136–183).

(d) If `render_idents` is no longer used anywhere else in the file, delete its definition (around lines 119–134). Check with grep first.

- [ ] **Step 2: Verify `render_idents` is unused**

Run: `grep -n render_idents crates/pgevolve-core/src/ir/index.rs`
Expected: zero matches after the migration. If matches remain, leave `render_idents` in place.

- [ ] **Step 3: Run index tests**

Run: `cargo test -p pgevolve-core --lib ir::index`
Expected: PASS. The `include` field's diff strings change from `[a,b]` style (render_idents) to `[Identifier("a"), Identifier("b")]` style (Debug), but existing tests check only the `path == "include"` assertion, not the from/to strings.

- [ ] **Step 4: Run the full crate tests**

Run: `cargo test -p pgevolve-core --lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/ir/index.rs
git commit -m "refactor(ir): derive Diff for Index

include is now #[diff(via_debug)] (was hand-rendered via render_idents);
the path stays 'include' but the from/to strings are Debug format. No
test depends on the exact from/to text. render_idents is dropped if no
longer used.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: Workspace-wide verification

**Files:** none modified — verification only.

- [ ] **Step 1: Run the full workspace test suite**

Run: `cargo test --workspace --lib --tests`
Expected: All tests pass. No `Difference.path` change has cascaded into a conformance fixture mismatch.

- [ ] **Step 2: Run clippy with `-D warnings`**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: `Finished` with no errors.

- [ ] **Step 3: Run fmt check**

Run: `cargo fmt --check`
Expected: No output.

- [ ] **Step 4: Run the conformance suite to verify Difference paths unchanged**

Run: `cargo test -p pgevolve-conformance --test run`
Expected: PASS.

- [ ] **Step 5: Commit the verification (if any clippy/fmt fixes were needed)**

If steps 2 or 3 produced changes that you applied, commit them:

```bash
git add -A
git commit -m "chore: post-derive clippy and fmt fixups

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

If no changes were needed, skip the commit step.

---

## Self-review pre-flight checklist for the implementing agent

After all tasks complete and before declaring the work done:

- [ ] Search the IR module for stale `use crate::ir::eq::{Diff, diff_field};` imports in files that no longer use those helpers; remove unused ones. `cargo clippy` flags these as warnings — fix them.
- [ ] Verify `git grep "impl .*Diff for" crates/pgevolve-core/src/ir/` returns 9 matches (Catalog, Function, Table, View, MaterializedView, UserType, ConstraintKind, DefaultExpr, ColumnType). The pre-migration count is 16; subtract 7 for the migrated structs (Schema, Sequence, Column, Procedure, ForeignKey, Constraint, Index) = expect 9 remaining.
- [ ] Re-run `cargo test --workspace --lib --tests` one final time.

---

## Out-of-scope (do NOT touch)

- `Function::diff` — custom keying logic, hand-written intentionally.
- `Catalog::diff` — orchestrates `diff_keyed` over many `Vec<T>` collections, hand-written intentionally.
- `Table::diff`, `View::diff`, `MaterializedView::diff` — pair columns/constraints by name with order-drift reporting; hand-written intentionally.
- `UserType::diff` — intentional single-`Difference` dump-all per in-file comment.
- `ConstraintKind`, `DefaultExpr`, `ColumnType` enum `Diff` impls — enum variant dispatch, hand-written intentionally (though `ConstraintKind` is *adjusted* in Task 9/10 to drop the `kind.` prefix; the rest of its body stays).
