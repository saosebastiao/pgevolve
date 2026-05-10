# pgevolve

Postgres-specific declarative schema management.

`pgevolve` deploys a directory of `CREATE`-style SQL files as the source of
truth for one or more Postgres schemas, introspects a live database to derive
its current state, and computes ordered, dependency-aware migration plans
that bring the database to the desired state. It refuses to lose data
unless explicitly authorized in a per-plan intent file.

> **Status:** under active development. v0.1 is not yet released.

## Status

See [`docs/superpowers/specs/2026-05-09-pgevolve-design.md`](./docs/superpowers/specs/2026-05-09-pgevolve-design.md)
for the v0.1 design and [`docs/superpowers/plans/`](./docs/superpowers/plans/)
for the implementation plan.

### v0.1 phase progress

| Phase | Title | Status |
|-------|------------------------------|-----------|
| 0     | Workspace                    | done      |
| 1     | IR                           | done      |
| 2     | Source parser                | done      |
| 3     | Catalog reader               | done      |
| 4     | Differ                       | next      |
| 5     | Planner                      | pending   |
| 6     | Rewrites                     | pending   |
| 7     | Plan format                  | pending   |
| 8     | Executor                     | pending   |
| 9     | CLI                          | pending   |
| 10    | Linter                       | pending   |
| 11    | Testkit                      | partial   |
| 12    | Shadow                       | pending   |

### Workspace layout

- `crates/pgevolve-core` — IR, source parser, catalog reader (no I/O, no async).
- `crates/pgevolve` — CLI binary (skeleton only until phase 9).
- `crates/pgevolve-testkit` — internal test infra: `EphemeralPostgres`
  (testcontainers wrapper), `PgCatalogQuerier` (`tokio-postgres` adapter),
  catalog snapshot helpers.
- `xtask` — `cargo xtask bless` regenerates tier-3 catalog goldens.

### Test tiers

- **Tier 1+2** (`cargo test --workspace --lib --tests`) — pure unit and
  fixture-corpus tests; runs without Docker.
- **Tier 3** (`tests/catalog_round_trip.rs`) — applies `source.sql` to an
  ephemeral PG container per major (14/15/16/17), introspects, and asserts
  byte-equal canonical-JSON snapshots under
  `crates/pgevolve-core/tests/fixtures/catalog/`. Set
  `PGEVOLVE_DISABLE_DOCKER_TESTS=1` to skip on hosts without Docker.
  Regenerate goldens with `cargo xtask bless`.

## License

Dual-licensed under MIT or Apache-2.0, at your option.
