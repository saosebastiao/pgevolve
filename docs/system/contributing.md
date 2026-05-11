# Contributing

How to work inside the repo: tests, fixtures, common change patterns.

## Setup

```sh
git clone https://github.com/saosebastiao/pgevolve.git
cd pgevolve
cargo build --workspace
```

`rust-toolchain.toml` pins Rust 1.95. `rustup` picks it up
automatically; no `rustup default` needed.

Optional but recommended: install Docker if you want to run the tier-3
through tier-5 test suites locally. They skip cleanly when Docker
isn't available.

## Running tests

| Goal | Command | Notes |
|---|---|---|
| Everything that doesn't need Docker | `cargo test --workspace --lib --tests` with `PGEVOLVE_DISABLE_DOCKER_TESTS=1` | The default for a fast iteration loop. |
| Everything (Docker-bound included) | `cargo test --workspace` | ~30s if Docker is warm. |
| Just the core | `cargo test -p pgevolve-core` | |
| Just the CLI / executor | `cargo test -p pgevolve` | |
| Property tests at higher coverage | `PGEVOLVE_PROPERTY_CASES=50 cargo test -p pgevolve --test pg_property_tests --release` | Default is 3 cases per test; CI uses 50; soak uses 5000. |
| Just shadow validation | `cargo test -p pgevolve --test shadow_validate` | Needs Docker. |

The test tier definitions are in [`docs/spec/testing.md`](../spec/testing.md).

## Linting and formatting

```sh
cargo fmt --all                                # apply
cargo fmt --all -- --check                     # check (what CI runs)
cargo clippy --workspace --all-targets -- -D warnings
```

The workspace lints are strict (clippy `all` + `pedantic` + `nursery`,
plus rust `unsafe_code = deny`). Locally-targeted `#[allow(...)]` is
acceptable; blanket allows at the crate level need justification.

## Regenerating tier-3 catalog goldens

The tier-3 round-trip tests in
`crates/pgevolve-core/tests/catalog_round_trip.rs` are snapshot tests.
When the catalog reader or canonicalization changes, the goldens need
to be re-blessed:

```sh
cargo run -p xtask -- bless
```

`xtask bless` walks every fixture under
`crates/pgevolve-core/tests/fixtures/catalog/<pg-major>/<case>/`,
applies the `source.sql` to an ephemeral container of the matching
major version, introspects, canonical-JSON-serializes, and overwrites
the `expected.json`.

**Review the diff carefully.** If the bless produces an unexpected
change in a fixture, that's the signal that you've changed
introspection semantics in a user-visible way and should think about
backward compatibility.

## Adding a new object kind

The general shape, by example: imagine adding `VIEW` support.

1. **IR.** Add `View` to `pgevolve_core::ir`. Implement `Diff` and
   wire it into `Catalog`'s `Diff` impl and `canonicalize`.
2. **Parser.** Add a `Statement::CreateView` variant. Add a
   `parse::builder::create_view_stmt` module that translates the
   `pg_query` AST into a `View` IR.
3. **Catalog reader.** Add a `CatalogQuery::Views` variant + a
   per-version SQL string in `catalog/queries/`. Add an
   `assemble::build_view` function. Bless tier-3 goldens.
4. **Differ.** Add `Change::CreateView / DropView / AlterView` variants
   to `diff::change::Change` and the corresponding pair-by-key logic
   in `diff/views.rs`.
5. **Planner.** Add `NodeId::View` to `plan::edges`, add the
   dependency edges (views depend on their referenced tables), and
   wire the dispatcher in `plan::rewrite::mod.rs`. Decide whether any
   online rewrites apply.
6. **Plan format.** Add a `StepKind::CreateView / DropView` etc., plus
   `kind_name` / `parse_kind_name` entries.
7. **Lint.** Add layout rules in `lint::profile::*` for views (e.g.,
   schema-mirror wants `schema/<schema>/views/<name>.sql`).
8. **Testing.** Add a tier-2 fixture in
   `crates/pgevolve-core/tests/fixtures/`; add a tier-3 fixture under
   `tests/fixtures/catalog/<pg-major>/`; consider adding to the IR
   generator's strategy so the property tests exercise views too.
9. **Docs.** Update `docs/spec/objects.md` to flip `VIEW`'s status to
   ✅ Implemented and add `docs/spec/column-types.md`,
   `docs/spec/cli.md` rows where relevant.

Each of these is a small commit; bundling them per "tier" of the
pipeline tends to keep PRs reviewable.

## Adding a new layout profile

`pgevolve_core::lint::profile::Profile`:

1. Add a new variant `Profile::MyProfile`.
2. Create `pgevolve_core::lint::profile::my_profile::check(tree, schema_dir) -> Vec<Finding>`.
3. Wire it into `check_profile` and `Profile::from_name`.
4. Tests: a passing fixture and a failing fixture per rule.
5. Document in `docs/spec/lint-and-layout.md`.

For custom profiles, prefer extending the existing regex+assertion
mechanism (`profile/custom.rs`) over adding a new built-in. New
assertions go in the `Assertion` enum.

## Working with the IR generator

`pgevolve_testkit::ir_generator` is the proptest strategy that produces
random valid `Catalog`s for tier-5 tests. To extend it:

1. Find the right sub-strategy (`arbitrary_table`, `arbitrary_column_type`,
   `arbitrary_indexes_for_table`).
2. Add the new variant. Make sure it produces objects that
   `Catalog::canonicalize` accepts.
3. Add a sanity check to the existing distribution tests
   (`generator_produces_valid_catalogs`, `generator_covers_a_variety_of_column_types`).

The generator's job is to produce inputs that *survive Postgres
round-trip*. The two biggest gotchas:

- **Type defaults.** PG normalizes some forms on introspection (e.g.,
  `pg_catalog.default` collation, type-default sequence min/max). The
  catalog reader normalizes them back; the generator should produce
  values that round-trip cleanly.
- **Indexable types.** Not every type has a default opclass for every
  index method. The current generator filters indexable columns
  through `is_btree_indexable`; extend that filter when adding types
  that aren't btree-friendly.

## Common change patterns

### Adding a new online-rewrite rule

1. Add the rule's source in `plan/rewrite/`.
2. Add a `policy::OnlineRewrites` field controlling it (default `true`).
3. Wire the dispatcher in `plan/rewrite/mod.rs` to call your rule's
   `should_rewrite` and emit the multi-step output.
4. Add a `pgevolve.toml` row in the
   `[planner.online_rewrites]` section of `docs/spec/cli.md`.
5. Tests in `plan/rewrite/mod.rs` for: rewrite fires on the right
   conditions, doesn't fire on the wrong conditions, atomic policy
   disables it.

### Changing a plan-directory file format

This is **breaking**. Bump the file's identity marker (e.g.,
`pgevolve-plan-id-v1` → `pgevolve-plan-id-v2`) and document the
migration. Plans written with the old format are not forward-
compatible.

### Adding a new CLI command

1. Add a `<Cmd>Args` struct to `cli.rs`.
2. Add a `commands::<cmd>` module with `run(args, cfg) -> Result<i32>`.
3. Wire the dispatcher in `main.rs`.
4. Update `docs/spec/cli.md` and `docs/user/commands.md`.

## Submitting changes

1. Run the full local check:
   ```sh
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
2. Open a PR. CI runs:
   - `fmt` (rustfmt check)
   - `clippy` (no warnings)
   - `test` (workspace, no Docker)
   - `pg-matrix` (tier 3-5 across PG 14/15/16/17 at `PROPTEST_CASES=50`)
3. The weekly `soak` workflow runs property tests at
   `PROPTEST_CASES=5000` per PG major; that's where rare
   normalization bugs surface.

## Releases

v0.1 release process (target — not yet automated):

1. Bump `version` in `Cargo.toml`.
2. Update `docs/user/installation.md` and `README.md` if installation
   semantics change.
3. Run a soak workflow on the release branch.
4. Tag `v0.1.0` on `main`.
5. GitHub Actions builds and attaches binaries for Linux x86_64 and
   macOS arm64.
6. Publish `pgevolve-core` and `pgevolve` to crates.io.
   `pgevolve-testkit` and `xtask` stay private.

`docs/spec/` is the living source of truth for *what's released*; the
release notes summarize what changed since the last tag.
