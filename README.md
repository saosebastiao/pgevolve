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

## License

Dual-licensed under MIT or Apache-2.0, at your option.
