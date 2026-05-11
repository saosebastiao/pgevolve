# System documentation

How pgevolve is built. Aimed at users who want to understand the
internals well enough to reason about edge cases.

| Topic | Read when… |
|---|---|
| [Architecture](./architecture.md) | You want the big picture: crates, data flow, key invariants. |
| [IR](./ir.md) | You want to understand the in-memory representation pgevolve diffs against. |
| [Planner](./planner.md) | You want to know how pgevolve orders changes and rewrites destructive ops into online-safe ones. |
| [Executor](./executor.md) | You want to understand the apply loop: locks, audit, rollback, recovery. |

See also:

- [`docs/user/`](../user/) — operating pgevolve as a tool.
- [`docs/spec/`](../spec/) — the capability catalogue.
- [`docs/superpowers/specs/2026-05-09-pgevolve-design.md`](../superpowers/specs/2026-05-09-pgevolve-design.md)
  — the original design doc.
- [`docs/superpowers/plans/`](../superpowers/plans/) — phase-by-phase
  implementation plans.
