# pgevolve specification

This directory is the living catalogue of what **pgevolve** manages, how it
manages it, and what is (or isn't) on the roadmap. It is the source of
truth for *user-visible capability surface* — the design rationale and
phase-by-phase implementation plan live elsewhere
([design doc](../superpowers/specs/2026-05-09-pgevolve-design.md),
[plans/](../superpowers/plans/)).

Every entry has a one-line description and an implementation status.

## Status legend

| Symbol | Meaning |
|--------|---------|
| ✅ **Implemented** | works in the current `main` and is exercised by tests |
| 🟡 **Partial** | the IR or pipeline handles part of the feature; the rest is documented in the row |
| 📋 **Planned** | committed for an upcoming v0.x; rough timing in the row |
| 🔮 **Future** | on the long-term roadmap, no commitment to a specific version |
| ⛔ **Not planned** | explicitly out of scope for the foreseeable future, usually with a one-line reason |

A status without modifiers refers to the **whole feature**. When a feature
mixes states (e.g., FK constraints are supported as a kind, but the
`MATCH PARTIAL` variant is not), the row breaks out subfeatures.

## Index

| Spec file | Covers |
|---|---|
| [`objects.md`](./objects.md) | Every Postgres object kind pgevolve does or will manage (schemas, tables, indexes, sequences, views, functions, types, etc.) |
| [`column-types.md`](./column-types.md) | Every Postgres column-type family with per-family support status |
| [`constraints.md`](./constraints.md) | Constraint kinds (PK, UNIQUE, FK, CHECK, NOT NULL, EXCLUSION) and their attributes |
| [`indexes.md`](./indexes.md) | Index access methods, options (partial, expression, INCLUDE, opclass, etc.) |
| [`cluster.md`](./cluster.md) | Cluster-level surface: roles (`CREATE ROLE / USER`), role attributes, role membership, `ClusterCatalog`, the `pgevolve cluster …` subcommand family |
| [`grants.md`](./grants.md) | Object-level and column-level `GRANT` / `REVOKE`, per-object `owner`, `ALTER DEFAULT PRIVILEGES`, lenient drift policy, `unmanaged-grant` lint |
| [`policies.md`](./policies.md) | Row-level security: per-table `rls_enabled` / `rls_forced`, embedded `policies: Vec<Policy>`, `unmanaged-policy` lint |
| [`reloptions.md`](./reloptions.md) | Storage parameters / reloptions on tables, indexes, and materialized views; per-AM fillfactor validation; `unmanaged-reloption` lint |
| [`publications.md`](./publications.md) | Logical-replication source-side metadata: all 5 `PUBLICATION` forms, `publish` bitset, `publish_via_partition_root`, 11 step kinds, 4 lint rules, PG-version gating |
| [`subscriptions.md`](./subscriptions.md) | Logical-replication subscriber-side metadata: `SUBSCRIPTION` with per-field lenient options, `${VAR}` env-var interpolation in CONNECTION strings, 8 step kinds, 4 lint rules, PG-version gating |
| [`pipeline.md`](./pipeline.md) | The internal pipeline: parser → IR → diff → planner → rewrite → group → execute |
| [`cli.md`](./cli.md) | CLI command surface, global flags, output formats, exit codes, `pgevolve.toml` schema |
| [`lint-and-layout.md`](./lint-and-layout.md) | Universal lint rules, built-in and custom layout profiles |
| [`testing.md`](./testing.md) | Test tiers 1–7: what each catches, where it lives, how to run it |

## Naming conventions

- **"v0.1"** was the first tagged release (schemas + tables + indexes +
  sequences). v0.1.x patches don't change the spec.
- **"v0.2"** added new object kinds: views/MVs, types, extensions,
  functions/procedures, triggers, declarative partitioning.
- **"v0.3"** added cross-cutting state: cluster roles (v0.3.0), grants
  + ownership (v0.3.1), row-level security (v0.3.2), storage parameters
  (v0.3.3). Entries marked 📋 name the target version when known.
- **"v0.3.4–v0.3.5"** added replication metadata: `PUBLICATION`
  (v0.3.4) and `SUBSCRIPTION` (v0.3.5).
- **"v0.3.6+"** continues v0.3 with PG 18 support (v0.3.6),
  `STATISTICS` + `WITH CHECK OPTION` (v0.3.7), `CREATE COLLATION`
  + `RANGE TYPE` (v0.3.8). See [`roadmap.md`](./roadmap.md) for the
  full per-version plan.
- **"Future"** is anything past the current release with no firm version.

## How to update this directory

When adding or changing a capability:

1. Update the relevant row's status and description.
2. If it's a new row entirely, add it in alphabetical or PG-documentation
   order within its file.
3. Keep descriptions to one to three lines. If a feature needs more
   words, link out to a longer doc in `docs/superpowers/` or to a code
   comment.

The spec is intentionally terse so it can serve as a quick reference; the
implementation plans capture the deeper "how".
