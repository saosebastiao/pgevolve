# pgevolve-pgquery

Parse SQL with the real Postgres grammar, and deparse it back.

Statically links [libpg_query], which is the Postgres server's own parser and
deparser extracted into a standalone C library. Parsing is not reimplemented
here and never should be — the only implementation that agrees with Postgres in
every corner is Postgres.

**Vendored:** Postgres **18.4** (`PG_VERSION_NUM 180004`). The C sources live in
`libpg_query/` **as files in-tree** — not a submodule (`cargo package` does not
follow them) and not a download (docs.rs builds with networking disabled).

[libpg_query]: https://github.com/pganalyze/libpg_query

## Why this crate exists

pgevolve previously depended on the `pg_query` crate, which is treated as
permanently unmaintained. Two problems made continuing to depend on it
untenable:

**It wrote into the Cargo registry cache.** Its `build.rs` regenerated its own
checked-in `src/protobuf.rs` with `prost-build` on any machine that happened to
have `protoc` on `PATH`, by pointing `OUT_DIR` at its own `src/` directory and
renaming the output over the file. For a crate living in
`~/.cargo/registry/src/`, that mutates a directory Cargo treats as immutable.
This is reproducible, not theoretical: in a container with `protoc` installed,
`src/protobuf.rs` in the extracted 6.1.1 crate carries a fresh mtime while every
sibling file carries the canonical registry timestamp.

**It shipped a large API pgevolve never called.** Roughly 4,500 lines of
generated node-walking machinery — `NodeRef`, `NodeMut`, `nodes()`, `truncate`
— including raw-pointer tree traversal in service of a query-truncation feature.

## What is kept, and what is gone

Kept, because pgevolve uses it:

| Item | Use |
|---|---|
| `parse` | the parser entry point |
| `deparse` | canonical SQL text, which pgevolve's equality checks rest on |
| `parse_plpgsql` | the plpgsql analyzer, for `SETOF`-vs-`void` wrapper choice |
| `protobuf::*` | the generated AST |
| `NodeEnum` | `protobuf::node::Node`, renamed |
| `NodeEnum::deparse` | deparse one node |
| `ParseResult` | `.protobuf` and `.warnings` |

Deleted: `normalize`, `fingerprint`, `scan`, `split_with_parser`,
`split_with_scanner`, `truncate`, `nodes()`, `ParseResult::tables()` and its
alias/CTE/function extraction, `NodeRef`, `NodeMut`, `node_structs`, `Context`,
`LockMode`, `TriggerType`.

`NodeRef` deserves a note. Upstream reached single-node deparsing through
`NodeEnum::to_ref()` → `NodeRef::deparse()` → `NodeRef::to_enum()`: a clone into
a borrowed view and then a clone straight back, to arrive at a five-line
wrapper. That round trip cost ~4,100 lines of generated conversion tables to
serve two call sites, so `NodeEnum::deparse` is called directly instead.

Also dropped: the `itertools` dependency (used only in a deleted file), the dead
`clippy = "0.0.302"` optional build-dep, `fs_extra` (upstream copied the whole
11 MB vendored tree into `OUT_DIR` before compiling, which bought nothing —
`cc` writes its objects there regardless), and `prost-build`/`protoc`.

## Maintainer procedures

Both of the following are **deliberate maintainer actions on a machine with the
tools installed**. Neither is a build step, and neither is a dependency for
anyone consuming this crate — a plain `cargo build` needs only a C compiler.

### Regenerating `src/protobuf.rs`

Needed only when the vendored `pg_query.proto` changes, i.e. after a Postgres
major bump.

```sh
# Requires protoc on PATH.
cd crates/pgevolve-pgquery
protoc --prost_out=/tmp/pgq \
       --proto_path=libpg_query/protobuf \
       libpg_query/protobuf/pg_query.proto
# Re-apply the module header from the top of src/protobuf.rs, then:
cp /tmp/pgq/pg_query.rs src/protobuf.rs
cargo fmt -p pgevolve-pgquery
```

The header at the top of `src/protobuf.rs` carries a justified blanket lint
allow and must survive regeneration — see CLAUDE.md §8.

### Re-extracting the C from PostgreSQL source

This is the bottom of the dependency stack, and it is worth knowing it is
reachable. If libpg_query itself were abandoned, its extraction pipeline is
checked into its repository and can be run against a PostgreSQL release tarball
directly. libpg_query's own `make extract_source` does this:

```sh
# In a libpg_query checkout. Requires Ruby and libclang.
# Makefile sets: PG_VERSION = 18.4, PG_VERSION_NUM = 180004
make tmp/postgres            # downloads and unpacks postgresql-$(PG_VERSION).tar.bz2,
                             # then applies the patches in ./patches/
make extract_source          # runs scripts/extract_source.rb over the unpacked tree
```

`extract_source` then, in order:

1. wipes and recreates `src/postgres/` and `src/postgres/include/`;
2. runs `scripts/extract_source.rb` with `LIBCLANG` pointed at a libclang shared
   library, which walks the Postgres sources and copies out the parser,
   deparser, and their transitive dependencies;
3. overwrites `src/postgres/include/pg_config_os.h` with a Win32-only port shim,
   and `#undef`s `PGDLLIMPORT`/`PGDLLEXPORT` so nothing is marked visible based
   on how Postgres builds itself;
4. rewrites `PG_VERSION_STR` in `pg_config.h` so the string does not vary with
   the build environment;
5. copies `PG_MAJORVERSION`, `PG_VERSION`, and `PG_VERSION_NUM` into
   `pg_query.h` — which is where this crate's `VENDORED_PG_MAJOR` test reads
   them back from.

The companion scripts `extract_headers.rb`, `extract_pg_types.rb`, and
`generate_protobuf_and_funcs.rb` produce the headers, the type tables, and
`protobuf/pg_query.proto` respectively.

> **Not yet run here.** This procedure is recorded from libpg_query's Makefile
> and has not been executed against this vendored tree — it needs the
> PostgreSQL release tarball, and `ftp.postgresql.org` was unreachable from the
> environment where the vendoring was done. Doing so is the outstanding half of
> plan item 3.8. Note that libpg_query's own `scripts/` and `patches/` are *not*
> vendored here (they are maintainer tools, not build inputs); take them from a
> libpg_query checkout of the matching tag.

### Bumping the vendored Postgres major

1. Replace `libpg_query/` wholesale from the new libpg_query release. **Copy
   every root-level `.h`, not just `pg_query.h`** — PG 17 had only the one and
   PG 18 added `postgres_deparse.h` beside it, which every `.c` reaches through
   `pg_query.h`. The `include` list in `Cargo.toml` globs `libpg_query/*.h` for
   this reason; naming root headers individually makes a bump silently
   unpublishable, which is upstream's failure mode.
2. Regenerate `src/protobuf.rs` (above), then diff the message and enum sets
   against the previous version. The 17 → 18 bump added four node types
   (`ReturningClause`, `ReturningExpr`, `ReturningOption`, `ATAlterConstraint`),
   removed `SinglePartitionSpec`, and **renumbered `AlterTableType` wholesale**
   — `AtCheckNotNull` was removed and all 58 later variants shifted down by one,
   moving `AtAttachPartition` from 61 to 60. That is only safe because the parser
   and the generated enum always come from the same major; any code holding a
   hardcoded subtype integer would break silently.
3. Update `VENDORED_PG_MAJOR` in `src/lib.rs`. The
   `vendored_version_constant_matches_the_c_library` test fails if you forget —
   it reads `PG_VERSION_NUM` back out of the compiled C.
4. Run the soak test in `pgevolve-core`
   (`cargo test -p pgevolve-core --lib -- --ignored soak`) against the new
   binding. A recorded-but-unreproduced segfault in PG14/15/16 builds is why
   that test exists.
5. Run `cargo package -p pgevolve-pgquery`. Packaging is a standing
   gate, not a surprise: every path the build touches must be in the `include`
   list in `Cargo.toml`, and upstream's `main` is currently unpublishable for
   exactly this reason.
6. Run the full conformance suite against every supported major.

## License

The Rust code is `MIT OR Apache-2.0`, matching the workspace.

The vendored C is not:

- `libpg_query/src/`, `libpg_query/pg_query.h`, `libpg_query/protobuf/` —
  BSD-3-Clause (libpg_query), over PostgreSQL-licensed sources extracted from
  PostgreSQL itself.
- `libpg_query/vendor/protobuf-c/` — BSD-2-Clause.
- `libpg_query/vendor/xxhash/` — BSD-2-Clause.

The PostgreSQL License is OSI-approved and functionally equivalent to
BSD-2-Clause. It is in `deny.toml`'s allow-list; its previous absence was a
false green, since cargo-deny only ever saw `pg_query.rs`'s `license = "MIT"`
declaration over ~250,000 lines of PostgreSQL-licensed C.
