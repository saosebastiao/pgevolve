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
| [`pipeline.md`](./pipeline.md) | The internal pipeline: parser → IR → diff → planner → rewrite → group → execute |
| [`cli.md`](./cli.md) | CLI command surface, global flags, output formats, exit codes, `pgevolve.toml` schema |
| [`lint-and-layout.md`](./lint-and-layout.md) | Universal lint rules, built-in and custom layout profiles |
| [`testing.md`](./testing.md) | Test tiers 1–7: what each catches, where it lives, how to run it |

## Naming conventions

- **"v0.1"** in this directory is the first tagged release once every phase
  plan is implemented. It may include minor patch releases (v0.1.x) for
  bug fixes that don't change the spec.
- **"v0.2", "v0.3", …** refer to upcoming minor versions; entries marked
  📋 will name the target version when known.
- **"Future"** is anything past v0.2 with no firm version.

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
