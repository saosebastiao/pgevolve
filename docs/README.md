# pgevolve

Postgres-specific declarative schema management.

`pgevolve` treats a directory of `CREATE`-style SQL files as the
source of truth for one or more Postgres schemas, introspects a live
database to derive its current state, and computes ordered,
dependency-aware migration plans that bring the database to the
desired state. It refuses to lose data unless explicitly authorized
in a per-plan intent file.

Current release: **v0.3.9** (Postgres 14–18). See the
[Changelog](./CHANGELOG.md) for per-release detail.

## What to read

- **New?** Start with [Install](./user/installation.md) and
  [Quick start](./user/getting-started.md).
- **Day-to-day user?** [Commands](./user/commands.md),
  [Configuration](./user/configuration.md),
  [Cookbook](./user/cookbook.md), and
  [Troubleshooting](./user/troubleshooting.md).
- **Asking "does pgevolve support X?"** — the
  [Reference](./spec/objects.md) section is the living capability
  catalogue. Every claimed feature has a fixture; status legend at
  the top of each spec file.
- **Curious how it works?** [Architecture](./system/architecture.md)
  explains the parser → IR → diff → planner → executor pipeline.
- **Considering contributing?** Read the
  [Constitution](./CONSTITUTION.md), the [v1.0 charter](./v1.md),
  and any recent [Specs](./superpowers/specs/README.md) +
  [Plans](./superpowers/plans/README.md) to see how features get
  designed and shipped.

## Project links

- Source: [github.com/saosebastiao/pgevolve](https://github.com/saosebastiao/pgevolve)
- Library API: [docs.rs/pgevolve-core](https://docs.rs/pgevolve-core)
- Issues: [GitHub Issues](https://github.com/saosebastiao/pgevolve/issues)

## License

Dual-licensed under MIT or Apache-2.0.
