# Claude Code — pgevolve project guidance

This file is auto-loaded by Claude Code at the start of every session in this repository. It tells Claude how to work in this codebase.

## Read this first

**Before doing any work in this repository, read [`docs/CONSTITUTION.md`](docs/CONSTITUTION.md).** It defines the binding principles for every decision — licensing, dependency policy, type-system rigor, Postgres support goals, conventions, and security posture. It applies to code, specs, architecture, plans, and tooling. Defer to it when in doubt.

A short summary of what the constitution says — these are not new rules, they are pointers to the actual document, which is authoritative:

- **License:** MIT OR Apache-2.0. No copyleft (GPL/AGPL/LGPL/MPL) or proprietary dependencies. Enforced by `cargo-deny` via `deny.toml`.
- **Dependencies are a liability.** Default is to *not* add a dependency. If no good crate exists, write it ourselves.
- **Make illegal states unrepresentable.** Lean on the type system. Newtypes (`Identifier`, `QualifiedName`) over `String`. Enums over booleans for closed sets.
- **Full Postgres support.** Use the official `pg_query` parser. The conformance suite must cover every feature we claim to support.
- **All actively-maintained Postgres versions.** Currently 14, 15, 16, 17. Drop support cleanly when a version reaches EOL.
- **Rust community conventions.** `cargo fmt`, `cargo clippy` with the workspace lints (pedantic + nursery), `cargo doc` clean. No `unwrap`/`expect` in production code.
- **Readability over cleverness. Correctness over performance.** Small focused files. Explicit over implicit.
- **CI/CD is a hard gate.** Lint + test + clippy + cargo-deny all green before merge.
- **No-blame security disclosures.** Fix and credit; never blame.

## Operating directives for Claude Code

These are not new principles, they're how to apply the constitution to in-session work:

1. **Before adding a dependency**, check `deny.toml` — if the license isn't in the allow-list, do not add the dep. Look for an alternative or write it ourselves.

2. **Before widening a `pub(crate)` to `pub`**, ask whether it's load-bearing for an external consumer. If not, leave it `pub(crate)`.

3. **When modeling a new domain concept**, prefer enums + newtypes over stringly-typed fields. If a field can only be one of N values, it should be an enum.

4. **When writing tests**, prefer property-based or table-driven tests over one-off cases when the input space is finite or naturally generatable.

5. **When the conformance suite or the property tests get slower**, treat that as a regression — investigate before merging.

6. **When in doubt about scope**, ask the user. Do not silently expand a task into adjacent cleanup. Cleanup gets its own PR.

7. **When a Postgres version-specific code path is added**, mark it with the PG version it targets. When that version reaches EOL, the marker tells us what to delete.

8. **Workspace lints are strict** — `clippy::pedantic`, `clippy::nursery`, `-D warnings`. Never use `--no-verify` to skip hooks or `#[allow(clippy::*)]` casually; if a lint is wrong for a specific call site, justify it in a brief comment.

9. **Commits go directly to `main`** for this project (per the user's standing preference). Each commit should be a coherent, testable unit. Run tests + clippy locally before committing; the standing directive is to fix everything (whether bug or test config) until the suite passes, and to run non-deterministic tests at least 10x before stopping.

10. **Co-author trailer.** Every commit Claude makes ends with:
    ```
    Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
    ```

## Project layout pointers

- `crates/pgevolve-core` — library: parser, IR, diff, planner, render, lint.
- `crates/pgevolve` — CLI + executor + shadow-validation; thin wrapper over `pgevolve-core`.
- `crates/pgevolve-core-macros` — proc-macro (`#[derive(DiffMacro)]`); internal.
- `crates/pgevolve-testkit` — generators, mutators, ephemeral PG fixtures.
- `crates/pgevolve-conformance` — Tier-A/B/C fixture suite driving end-to-end coverage.
- `xtask` — bless command, regression capture, etc.
- `docs/CONSTITUTION.md` — **authoritative principles document**.
- `docs/spec/` — living capability catalogue.
- `docs/superpowers/specs/` — design docs, one per sub-spec.
- `docs/superpowers/plans/` — implementation plans, one per sub-spec.

## Skill workflow

For non-trivial work, the established skill chain is:
**brainstorming → writing-plans → subagent-driven-development**.

For one-off changes, skip directly to implementation. The constitution applies either way.
