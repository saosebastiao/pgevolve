# Canonicalization consolidation — design

**Status:** Approved 2026-05-19. Implementation plan to follow.

## Goal

Move every "filter PG defaults to None" rule out of the catalog reader
(`catalog/assemble.rs`) and the source-side parser builders
(`parse/builder/*.rs`) into a single ordered pipeline under
`crates/pgevolve-core/src/ir/canon/`. `Catalog::canonicalize` keeps its
existing signature but becomes a thin wrapper that delegates to the new
pipeline. Result: one discoverable home for every IR-value normalization
rule. The next time PG returns a default we didn't expect, the fix lands
in `ir/canon/filter_pg_defaults.rs`, not in two or three different
places.

## Non-goals

- Changing the public `Catalog::canonicalize` signature.
- Defining any new normalization rules. Pure refactor — every existing
  behavior is preserved.
- Touching `parse/normalize_body.rs`, `parse/normalize_expr.rs`, or
  `parse/ast_canon.rs`. Those operate on AST/text at a different layer
  and stay where they are.
- Touching `Catalog::diff` or any `Diff` impls.

## Architecture

A new module `crates/pgevolve-core/src/ir/canon/` with one file per
pass:

```
ir/canon/
  mod.rs                              ← `pub fn canonicalize(&mut Catalog) -> Result<(), IrError>`
  filter_pg_defaults.rs               ← Sequence min/max, Function cost/rows, Column collation 'pg_catalog.default' → None
  sentinel_view_columns.rs            ← view/MV column type → `ColumnType::Other { raw: "view_column" }`
  renumber_enum_sort_orders.rs        ← sort by current order; renumber 1.0, 2.0, …
  sort_and_dedupe.rs                  ← sort each Vec<T> by canonical key; reject duplicates
```

`Catalog::canonicalize` becomes:

```rust
pub fn canonicalize(mut self) -> Result<Self, IrError> {
    crate::ir::canon::canonicalize(&mut self)?;
    Ok(self)
}
```

`ir::canon::canonicalize` runs the passes in a fixed documented order:

1. `filter_pg_defaults` — values matching PG's documented default become
   `None`. Order-insensitive within the pass.
2. `sentinel_view_columns` — replace view/MV column types with the
   shared `view_column` sentinel.
3. `renumber_enum_sort_orders` — re-index every enum's `sort_order` to
   `1.0, 2.0, ...` in current order.
4. `sort_and_dedupe` — final pass; sorts every IR collection by its
   canonical key and errors on duplicates. Runs last so duplicate
   detection sees post-normalization values.

## Passes in detail

### `filter_pg_defaults`

Operates on three IR shapes:

1. **`Sequence`**: `min_value: Option<i64>` and `max_value: Option<i64>`
   that equal the PG-implied default for `(data_type, increment)`
   become `None`. Today this rule lives in two places: the catalog
   reader (`catalog/assemble.rs::build_sequence`, lines ~707–711) and
   `Sequence::canonicalize` in `ir/sequence.rs`. Both call sites are
   deleted; the rule lives once here.

2. **`Function`**: `cost: Option<f32>` equal to `100.0` (the PG default
   for SQL/PLpgSQL) becomes `None`. `rows: Option<f32>` equal to
   `1000.0` (SETOF default) or `0.0` (non-SETOF default) becomes
   `None`. Today this lives in the catalog reader
   (`catalog/assemble.rs::build_functions_and_procedures`) and in the
   source-side builder (`parse/builder/create_function_stmt.rs`).
   Both call sites are deleted; the rule lives once here.

3. **`Column.collation`**: `Some(QualifiedName)` where
   `schema == "pg_catalog" && name == "default"` becomes `None`. Today
   this lives in the catalog reader
   (`catalog/assemble.rs::build_table_columns`, around lines 268–272).
   The reader is changed to return the raw value; this pass normalizes.

### `sentinel_view_columns`

Currently lives inline inside `Catalog::canonicalize`
(ir/catalog.rs:139–151). Moved verbatim into its own file. The rule is
unchanged: every view and materialized view column's `column_type` is
replaced with `ColumnType::Other { raw: "view_column" }`.

### `renumber_enum_sort_orders`

Currently lives inline inside `Catalog::canonicalize`
(ir/catalog.rs:167–181). Moved verbatim. Sort each enum's values by
current `sort_order`, then assign `1.0, 2.0, 3.0, ...`.

### `sort_and_dedupe`

The scaffolding currently sprawls across `Catalog::canonicalize`
(ir/catalog.rs:71–214). Move it to one focused file. Each collection
gets a single `sort_collection!(catalog.foo, key_fn, "foo")` call (or a
plain function — macro is YAGNI if a function over `&mut Vec<T>` with a
closure works). Duplicate detection (`first_duplicate`) moves here too.
This is the only fallible pass — returns `Result<(), IrError>`.

## What moves out of where

| Currently in                                                                       | Moves to                              |
|------------------------------------------------------------------------------------|---------------------------------------|
| `catalog/assemble.rs::build_sequence` (min/max filter)                             | `canon/filter_pg_defaults.rs`         |
| `catalog/assemble.rs::build_functions_and_procedures` (cost/rows filter)           | `canon/filter_pg_defaults.rs`         |
| `catalog/assemble.rs::build_table_columns` (`pg_catalog.default` collation → None) | `canon/filter_pg_defaults.rs`         |
| `parse/builder/create_function_stmt.rs` (cost/rows filter)                         | `canon/filter_pg_defaults.rs`         |
| `ir/sequence.rs::Sequence::canonicalize` + `default_bounds` helper                  | `canon/filter_pg_defaults.rs`         |
| `ir/catalog.rs` view/MV sentinel block                                              | `canon/sentinel_view_columns.rs`      |
| `ir/catalog.rs` enum sort_order renumbering                                         | `canon/renumber_enum_sort_orders.rs`  |
| `ir/catalog.rs` sort + `first_duplicate` scaffolding                                | `canon/sort_and_dedupe.rs`            |

## What stays where (deliberately not moved)

- **`parse/ast_canon.rs::canonicalize_view_bodies`** — runs on
  `raw_body` SQL text before the IR settles. Source-side only.
- **`parse/ast_canon.rs::promote_mv_index_parents`** — needs the
  source-side raw parse output (an `IndexParent::Table` that we now
  know is actually an MV) before catalog canonicalization runs.
- **`parse/normalize_body.rs`** — body-level canonicalization
  (qualifier-strip, whitespace) is text/AST-level, not IR-value-level.
- **`parse/normalize_expr.rs`** — same.

These three are conceptually "canonicalization" but operate at the
AST/text layer above `Catalog` IR. Moving them would either require
plumbing parsed-AST state into the canon pipeline (large) or
restructuring the parse path (out of scope per the brainstorm).

## Data flow after consolidation

```
Source side:                                  Catalog side:
parse::parse_directory                        catalog::read_catalog
  ├─ builders produce raw IR                    ├─ assemble produces raw IR
  ├─ ast_resolution (unchanged)                 │   (no more PG-default filters in builders)
  ├─ ast_canon::canonicalize_view_bodies        │
  ├─ ast_canon::promote_mv_index_parents        │
  └─ catalog.canonicalize()  ─────┬─────────────┘
                                  ▼
                  ir::canon::canonicalize(&mut Catalog)
                    1. filter_pg_defaults
                    2. sentinel_view_columns
                    3. renumber_enum_sort_orders
                    4. sort_and_dedupe   ← fallible
```

The same canon pipeline runs on both sides. Anything that should be
equal between source and catalog goes here.

## Error handling

`canon::canonicalize` returns `Result<(), IrError>`. Only
`sort_and_dedupe` is fallible. The other three passes mutate in place
and cannot fail.

Each pass's `pub fn run(cat: &mut Catalog)` (or
`Result<(), IrError>` for `sort_and_dedupe`) is the unit-of-test
boundary.

## Testing strategy

1. **Per-pass unit tests in `ir/canon/<pass>.rs::tests`** — each pass
   gets focused tests for just its invariant:
   - `filter_pg_defaults`: build a `Sequence` with explicit `Some(1)`
     min for bigint; run pass; assert `None`. Build with `Some(5)`; run
     pass; assert `Some(5)`. Same shape for function cost/rows and
     column collation.
   - `sentinel_view_columns`: build a `View` with `BigInt` column;
     run; assert sentinel.
   - `renumber_enum_sort_orders`: build with `[0.0, 0.5, 1.7]`; run;
     assert `[1.0, 2.0, 3.0]`.
   - `sort_and_dedupe`: build a catalog with duplicate table qnames;
     run; assert `IrError`.

2. **`Catalog::canonicalize` keeps its existing tests** in
   `ir/catalog.rs::tests`. They pass unchanged — the public API is
   identical.

3. **Conformance suite is the integration test.** Source and catalog
   produce the same `Catalog`; canon runs on both; they stay
   byte-equal. The suite already verifies this and will catch any
   regression.

4. **Catalog reader / source builder tests** that previously asserted
   "the reader/builder filters PG defaults to None" need updating to
   assert "produces the raw value; canonicalize filters." Where those
   tests exist (e.g., `catalog::assemble::tests`), update them. Where
   the filtering was only verified indirectly through canonical
   equality, no change.

## Migration order

Each step is one commit; full workspace test suite + conformance suite
must stay green.

1. **Scaffold:** create `ir/canon/mod.rs` with `pub fn canonicalize`
   that's an empty `Ok(())`. Change `Catalog::canonicalize` to delegate
   to it. Existing rules still inline in `Catalog::canonicalize` — the
   delegation is a no-op for now. Tests green.
2. **Move `sort_and_dedupe`** (smallest semantic unit; isolated). The
   existing inline sort/dedup code moves to `ir/canon/sort_and_dedupe.rs`;
   `Catalog::canonicalize`'s body shrinks. Tests green.
3. **Move `sentinel_view_columns`.** Tests green.
4. **Move `renumber_enum_sort_orders`.** Tests green.
5. **Move `filter_pg_defaults` — phase A:** create the file with the
   `Sequence` and `Column.collation` rules. Move those filters out of
   `Sequence::canonicalize` and `catalog/assemble.rs::build_sequence` /
   `build_table_columns`. Delete `Sequence::canonicalize`. Tests green.
6. **Move `filter_pg_defaults` — phase B:** add the `Function`
   cost/rows rules to the pass. Move those filters out of
   `catalog/assemble.rs::build_functions_and_procedures` and
   `parse/builder/create_function_stmt.rs`. Tests green.
7. **Cleanup:** verify the `default_bounds` helper in `ir/sequence.rs`
   is gone, no orphan imports in any file. Run clippy clean. Final
   commit.

## Out-of-scope items (for clarity)

- The `ast_canon` source-side passes (`canonicalize_view_bodies`,
  `promote_mv_index_parents`).
- Body and expression canonicalization (`NormalizedBody`,
  `NormalizedExpr`).
- Catalog and IR struct definitions; this is a pure code-motion
  refactor.

## Open questions

None. Migration mechanics are covered by the implementation plan.
