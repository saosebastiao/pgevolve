# `Diff` derive macro — design

**Status:** Approved 2026-05-19. Implementation plan to follow.

## Goal

Replace the ~10 hand-written struct `Diff` impls in `pgevolve-core::ir` with a
`#[derive(Diff)]` proc-macro. No behavior change — every existing `Diff` test
keeps passing, and `Catalog::diff` produces the same `Difference` paths and
strings.

The win is volume: the current hand-written impls total ~250 lines of mechanical
`out.extend(diff_field(...))` calls. Each new IR field today needs a
correspondingly mechanical diff entry, easy to forget — `Function::diff` already
had to grow a `if out.is_empty() { push "<unknown field divergence>" }` escape
hatch because at least one field was historically left out. A derive makes the
field-by-field shape compile-time complete.

## Non-goals

- Replacing `Function::diff`, `Catalog::diff`, or any enum `Diff` impl
  (`ConstraintKind`, `DefaultExpr`, `ColumnType`). Those have custom keying or
  variant-dispatch logic that doesn't fit a field-list shape.
- Changing the `Diff` trait, the `Difference` struct, or any helper in
  `pgevolve-core::ir::eq` (`diff_field`, `prefix_diffs`, `diff_keyed`).
- Changing the `Difference` paths emitted by the existing hand-written impls.
  Migration is invariant-preserving.

## Architecture

A new internal proc-macro crate `pgevolve-core-macros` lives next to
`pgevolve-core` in the workspace:

```
crates/
  pgevolve-core/
  pgevolve-core-macros/        ← new
```

`pgevolve-core-macros/Cargo.toml`:

```toml
[package]
name    = "pgevolve-core-macros"
version = "0.1.0-dev"
edition = "2024"

[lib]
proc-macro = true

[dependencies]
proc-macro2 = "1"
quote       = "1"
syn         = { version = "2", features = ["full"] }
```

The crate exports one derive:

```rust
#[proc_macro_derive(Diff, attributes(diff))]
pub fn derive_diff(input: TokenStream) -> TokenStream { ... }
```

`pgevolve-core` depends on it and re-exports the derive from
`pgevolve_core::ir::eq` so call sites only see the existing module:

```rust
// crates/pgevolve-core/src/ir/eq.rs
pub use pgevolve_core_macros::Diff;
```

## Generated code

For each named-field struct `S` annotated with `#[derive(Diff)]`, the macro
emits:

```rust
impl crate::ir::eq::Diff for S {
    fn diff(&self, other: &Self) -> Vec<crate::ir::difference::Difference> {
        let mut out = Vec::new();
        // one block per non-skipped field — strategy depends on attrs
        out
    }
}
```

The per-field block depends on the attribute on that field:

| Attribute        | Generated block                                                                                        |
|------------------|---------------------------------------------------------------------------------------------------------|
| *(none)*         | `out.extend(crate::ir::eq::diff_field("<name>", &self.<name>, &other.<name>));`                          |
| `#[diff(skip)]`  | *(field omitted)*                                                                                       |
| `#[diff(via_debug)]` | `out.extend(crate::ir::eq::diff_field("<name>", &format!("{:?}", self.<name>), &format!("{:?}", other.<name>)));` |
| `#[diff(nested)]` | `out.extend(crate::ir::eq::prefix_diffs("<name>", self.<name>.diff(&other.<name>)));`                   |

The macro never invents path names — the `Difference.path` matches the field
identifier verbatim.

## Attribute grammar

Only one strategy attribute per field. The macro errors at compile time on:

- Unknown names inside `#[diff(...)]` (e.g., `#[diff(rename = "...")]`).
- Multiple strategies on one field (e.g., `#[diff(skip, nested)]`).
- Application of `#[derive(Diff)]` to anything other than a struct with named
  fields (tuple structs, unit structs, enums all reject).

Errors surface via `syn::Error` so they show up at the field's source span.

## Worked example

Before:

```rust
impl Diff for Sequence {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let mut out = Vec::new();
        out.extend(diff_field("qname", &self.qname, &other.qname));
        out.extend(diff_field(
            "data_type",
            &self.data_type.render_sql(),
            &other.data_type.render_sql(),
        ));
        out.extend(diff_field("start", &self.start, &other.start));
        out.extend(diff_field("increment", &self.increment, &other.increment));
        out.extend(diff_field(
            "min_value",
            &format!("{:?}", self.min_value),
            &format!("{:?}", other.min_value),
        ));
        // ... 6 more fields ...
        out
    }
}
```

After:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Diff)]
pub struct Sequence {
    pub qname: QualifiedName,
    #[diff(via_debug)]
    pub data_type: ColumnType,
    pub start: i64,
    pub increment: i64,
    #[diff(via_debug)]
    pub min_value: Option<i64>,
    #[diff(via_debug)]
    pub max_value: Option<i64>,
    pub cache: i64,
    pub cycle: bool,
    #[diff(via_debug)]
    pub owned_by: Option<SequenceOwner>,
    #[diff(via_debug)]
    pub comment: Option<String>,
}
```

The `Difference.path` strings are identical to the hand-written impl. The
`data_type` text changes from `bigint` (render_sql) to `BigInt`
(Debug) — see "Display divergence" below.

## Display divergence note

Two fields today format via `render_sql()` rather than Display:
`Sequence.data_type: ColumnType` and `Column.ty: ColumnType`. The minimal
attribute set has no `via = "method"` escape hatch, so those become
`#[diff(via_debug)]`. The user-visible `Difference.from`/`to` strings go from
`bigint` / `integer` to `BigInt` / `Integer` — slightly more verbose, fully
structured, and still readable in conformance-failure output.

If we later want SQL-style strings in those two places, the cheapest fix is
adding `impl Display for ColumnType` that delegates to `render_sql()` and
dropping the attribute. Out of scope for this change.

## Coverage

**Derived (10 structs):**

| Struct                                     | File                              |
|--------------------------------------------|-----------------------------------|
| `Schema`                                   | `ir/schema.rs`                    |
| `Sequence`                                 | `ir/sequence.rs`                  |
| `Column`                                   | `ir/column.rs`                    |
| `Constraint`                               | `ir/constraint.rs`                |
| `ForeignKey`                               | `ir/constraint.rs`                |
| `Table`                                    | `ir/table.rs`                     |
| `Index`                                    | `ir/index.rs`                     |
| `View`                                     | `ir/view.rs`                      |
| `MaterializedView`                         | `ir/view.rs`                      |
| `Procedure`                                | `ir/procedure.rs`                 |

**Stays hand-written (5 impls):**

| Item                                       | Why                                      |
|--------------------------------------------|------------------------------------------|
| `Function`                                 | Custom `qname(args)` key in `path`       |
| `Catalog`                                  | Orchestrates `diff_keyed` over `Vec<T>`  |
| `ConstraintKind`                           | Enum variant dispatch                    |
| `DefaultExpr`                              | Enum variant dispatch                    |
| `ColumnType`                               | Enum variant dispatch                    |
| `UserType` and sub-structs                 | Verify per-struct during migration       |

`user_type.rs` is reviewed during migration — its sub-structs may or may not
fit the derive depending on their shape; whichever do, get the derive.

## Testing

1. **Existing tests stay green.** Every IR module already has
   `diff_reports_*` and `canonical_eq` tests. They pass post-migration
   without modification — that's the equivalence guarantee.
2. **`pgevolve-core-macros` unit tests** using `trybuild` for compile-fail
   cases:
   - Unknown attribute name.
   - Two strategy attrs on one field.
   - Derive on enum.
   - Derive on tuple struct.
3. **One temporary parity test** in `ir/sequence.rs::tests` that constructs
   two `Sequence`s differing in each field and asserts the derived `Diff`
   produces the same `Vec<Difference>` (same paths, same from/to strings) as
   the historical hand-written one. Delete the test once the migration is
   committed.

## Migration order

Smallest first, so problems with the macro surface against the simplest IR
types. Each migration is its own commit with green tests:

1. `Schema` (one field).
2. `Sequence` (10 fields, mostly `via_debug`).
3. `Column` (mixed: default `Diff` types, `via_debug` for `Option<*>`).
4. `Procedure`.
5. `ForeignKey`, `Constraint`.
6. `Table`.
7. `Index`.
8. `View`, `MaterializedView`.
9. `UserType` sub-structs (whichever fit).
10. Delete the temporary parity test.

## Error handling and robustness

- Compile-time errors only — no runtime branches.
- Macro output is span-tagged so `cargo build` errors point to the IR field,
  not the proc-macro internals.
- Macro tested via `trybuild` UI snapshots, so accidental changes to its
  error messages get caught in PR review.

## Open questions

None. Everything else is migration mechanics covered by the implementation
plan.
