# `plan/rewrite/mod.rs` split — design

**Status:** Approved 2026-05-20. Implementation plan to follow.

## Goal

Reduce `crates/pgevolve-core/src/plan/rewrite/mod.rs` from **2694
lines** to a thin top-level module by:

1. Moving per-object-family `Change` dispatcher logic into a new
   `emit/` submodule (one file per family).
2. Moving the ~1294-line end-to-end `#[cfg(test)] mod tests` block to
   a sibling `tests.rs` file.

Pure refactor — zero behavior change, no public-API change, every
existing test passes byte-identical, conformance suite stays green.

## Motivation

The remaining v0.2 sub-specs (#3 Extensions, #5 Triggers,
#6 Declarative partitioning + table reloptions) each add an
`emit_<family>_change` function to this dispatcher. Adding three more
families to a 2694-line file would push it past 3500 lines. Splitting
now means each sub-spec lands in its own file (`emit/extension.rs`,
`emit/trigger.rs`, etc.) and subagents can focus a single file in
context per task.

## Non-goals

- Renaming, restructuring, or splitting the existing SQL-emission
  helpers: `sql.rs`, `functions.rs`, `views.rs`, `types.rs`.
- Touching the post-emit rewrite passes: `concurrent_index.rs`,
  `refresh_mv_concurrently.rs`, `set_not_null_check_pattern.rs`,
  `check_not_valid_validate.rs`, `fk_not_valid_validate.rs`.
- Changing any public API (`rewrite`, `rewrite_with_source`, or any
  re-exports).
- Splitting tests by family. Tests are end-to-end via `rewrite()` and
  don't cleanly disambiguate; they stay as one block in `tests.rs`.

## Architecture

```
plan/rewrite/
  mod.rs              # public API + Ctx + emit_change dispatcher + tiny helpers (~120 lines)
  tests.rs            # the 40 existing end-to-end tests (~1294 lines, moved verbatim)
  emit/
    mod.rs            # pub(super) mod schema, table, sequence, ...
    schema.rs         # Change::CreateSchema, DropSchema, AlterSchema
    table.rs          # Change::CreateTable + emit_table_op (~320 lines combined)
    sequence.rs       # Change::CreateSequence + emit_sequence_op
    index.rs          # Change::CreateIndex, DropIndex, RecreateIndex
    constraint.rs     # AddConstraint, DropConstraint, ValidateConstraint
    view.rs           # emit_view_change (~155 lines)
    mv.rs             # emit_mv_change (~143 lines)
    user_type.rs      # emit_user_type_change (~258 lines)
    function.rs       # emit_function_change (~107 lines)
    procedure.rs      # emit_procedure_change (~88 lines)
    deferred_fk.rs    # emit_deferred_fk
```

**Unchanged sibling files (stay where they are):**

- SQL-emission helpers: `sql.rs`, `functions.rs`, `views.rs`,
  `types.rs`.
- Post-emit rewrite passes: `check_not_valid_validate.rs`,
  `concurrent_index.rs`, `fk_not_valid_validate.rs`,
  `refresh_mv_concurrently.rs`, `set_not_null_check_pattern.rs`.

## Calling convention

Each `emit/<family>.rs` exposes one or more `pub(super) fn` entry
points. The most common shape, for families with a single
`Change::<Family>(<FamilyChange>)` variant (View, Mv, UserType,
Function, Procedure):

```rust
pub(super) fn emit(
    change: <FamilyChange>,
    ctx: &super::Ctx<'_>,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
);
```

For families that span multiple `Change::*` variants (Schema, Table,
Sequence, Index, Constraint), the module exposes one function per
variant. Names mirror the variant: `create_schema`, `drop_schema`,
`alter_schema`, `create_table`, `create_sequence`, etc. Cleaner than a
per-family dispatch function for those small-arm cases.

After the split, `mod.rs::emit_change` becomes a thin match:

```rust
fn emit_change(entry: ChangeEntry, ctx: &Ctx<'_>, out: &mut Vec<RawStep>) {
    let destructive_reason = destructive_reason(&entry.destructiveness);
    let destructive = entry.destructiveness.requires_approval();
    match entry.change {
        Change::CreateSchema(s) => emit::schema::create(&s, destructive, destructive_reason, out),
        Change::DropSchema(name) => emit::schema::drop(&name, destructive, destructive_reason, out),
        Change::AlterSchema { name, comment } => emit::schema::alter(&name, comment.as_deref(), out),
        Change::CreateTable(t) => emit::table::create(t, destructive, destructive_reason, out),
        Change::CreateSequence(s) => emit::sequence::create(s, destructive, destructive_reason, out),
        // … one short arm per Change variant …
        Change::View(c) => emit::view::emit(c, ctx, destructive, destructive_reason, out),
        Change::Mv(c) => emit::mv::emit(c, ctx, destructive, destructive_reason, out),
        Change::UserType(c) => emit::user_type::emit(c, ctx, destructive, destructive_reason, out),
        Change::Function(c) => emit::function::emit(c, ctx, destructive, destructive_reason, out),
        Change::Procedure(c) => emit::procedure::emit(c, ctx, destructive, destructive_reason, out),
        Change::TableOp(e) => emit::table::op(e, ctx, destructive, destructive_reason, out),
        Change::SequenceOp(qname, e) => emit::sequence::op(&qname, e, out),
        // …
    }
}
```

Total dispatcher body shrinks from ~318 lines (with inline arms) to
~50 lines (one line per arm). The exact arm names match the existing
`Change` variants; the table above is illustrative.

## Shared symbols

`mod.rs` retains and re-exposes for the `emit/*` submodule:

- `pub(super) struct Ctx<'a>` (was `struct`; visibility widened so
  `super::Ctx` resolves from inside `emit/`).
- `pub(super) fn destructive_reason(d: &Destructiveness) -> Option<String>`.
- `pub(super) fn schema_target(name: &Identifier) -> QualifiedName`.

The `#![allow(clippy::too_many_lines)]` + friends at module scope of
`mod.rs` move to whichever `emit/*.rs` files actually need them
(probably `user_type.rs` and `table.rs`).

## Migration order

Each step is one commit; `cargo test --workspace --lib --tests` and
`cargo clippy --workspace --all-targets -- -D warnings` stay green at
every step. Start with the smallest, isolated families to validate
the pattern.

1. **Scaffold `emit/` with empty modules.** Create `emit/mod.rs` with
   `pub(super) mod <name>;` declarations and empty `<name>.rs` files
   for every planned family. Widen `Ctx`, `destructive_reason`,
   `schema_target` to `pub(super)`. No callers yet. Build green.
2. **Move `emit::schema`** (smallest, 3 arms ~50 lines).
3. **Move `emit::sequence`** (CreateSequence arm + emit_sequence_op).
4. **Move `emit::deferred_fk`** (1 fn ~17 lines).
5. **Move `emit::index`** (3 inline arms).
6. **Move `emit::constraint`** (3 inline arms).
7. **Move `emit::view`** (emit_view_change ~155 lines).
8. **Move `emit::mv`** (emit_mv_change ~143 lines).
9. **Move `emit::user_type`** (emit_user_type_change ~258 lines).
10. **Move `emit::function`** (emit_function_change ~107 lines).
11. **Move `emit::procedure`** (emit_procedure_change ~88 lines).
12. **Move `emit::table`** (CreateTable arm + emit_table_op, ~320
    combined). Largest, intentionally last.
13. **Move the `#[cfg(test)] mod tests` block to `tests.rs`.** Change
    `mod.rs` to declare `#[cfg(test)] mod tests;`.
14. **Final verify.** `mod.rs` is around 120 lines, all four
    workspace checks green (test, clippy, fmt, conformance).

## Testing

- All 40 existing rewrite tests must pass byte-identical after the
  move. They stay end-to-end via `rewrite()`.
- Conformance suite is the integration check; runs at the end of the
  migration.
- No new tests added by this refactor.

## Risk and rollback

Each migration step is isolated. Risk is import or visibility errors
that compile-fail; mitigation is the per-step build check. If a step
introduces a regression that escapes the test suite, `git revert`
that commit leaves prior steps intact.

## Out of scope (explicit)

- Splitting `sql.rs` / `functions.rs` / `views.rs` / `types.rs`.
- Splitting `tests.rs` per family. May be revisited if future
  contributions show test growth concentrated by family.
- Renaming the existing `Change` enum or any of its variants.
- Removing the `#![allow(clippy::too_many_lines)]` attribute from
  `mod.rs` if some `emit/*.rs` still need it (it travels with the
  function, not the file). Acceptable as-is.

## Open questions

None. Migration mechanics are fully covered by the implementation
plan.
