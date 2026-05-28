# pgevolve Project Constitution

This document records the guiding principles that govern every decision made in this project — code, architecture, specifications, plans, and tooling. It binds all contributors, human and AI alike. To amend it, open a PR with a clear rationale and obtain at least one maintainer approval.

---

## 1. Purpose

pgevolve is a Postgres-specific declarative schema management tool. Its job is to be correct, safe, and complete — not clever. This constitution exists so that day-to-day decisions (what crate to reach for, how to model a domain concept, when to ship) are made consistently against a shared set of values rather than re-litigated case by case.

---

## 2. Licensing

Everything published under this project is dual-licensed as **MIT OR Apache-2.0**. This is stated in `[workspace.package].license` in `Cargo.toml` and must be the declared license for every crate in the workspace.

We do not accept dependencies with copyleft licenses (GPL, AGPL, LGPL, EUPL, or similar). We do not accept proprietary dependencies that would restrict redistribution or compel relicensing. This is enforced at PR time by `cargo-deny` via `deny.toml` (added in CLEAN-7). A failing `cargo deny check licenses` is a hard block on merge — no exceptions without an explicit maintainer decision recorded in the PR.

---

## 3. Dependencies Are a Liability

Every dependency we accept is a vector for bugs, supply-chain attacks, license violations, and unmaintained code. The convenience benefit must be weighed honestly against that risk.

Before adding a dependency, ask: Is it actively maintained? Does it have a meaningful user base? Has it been audited, or does it have a strong track record? Is its license compatible with MIT OR Apache-2.0? Could we reasonably implement the functionality ourselves in a bounded amount of effort? If a crate fails more than one of these tests, the default answer is to not add it. If no suitable crate exists, we write the code ourselves rather than accepting a poor dependency.

`cargo-deny` runs in CI against the `deny.toml` config. It checks licenses, detects known-vulnerable versions via the RustSec advisory database, and flags duplicate major versions of key crates. A clean `cargo deny check` is required for merge.

---

## 4. Make Illegal States Unrepresentable

Rust's type system is one of the most powerful tools we have for eliminating entire classes of bugs before a test is ever written. We use it aggressively.

Closed sets of values are always Rust enums, never strings or integers. Optional fields are always `Option<T>`, never sentinel values. Raw strings are not used where a validated type exists — `Identifier` and `QualifiedName` (in `crates/pgevolve-core/src/identifier.rs`) are the canonical examples: you cannot accidentally pass an unvalidated, un-case-folded string where a Postgres identifier is expected. Where an operation has sequential phases, typestate patterns make it impossible to invoke a later-phase function before an earlier-phase has completed. The goal is not to be exhaustive about which patterns to apply — it is to internalize the discipline of asking, for every type boundary: "can the caller construct an invalid value here, and if so, why?"

This approach reduces the testing footprint without sacrificing safety. A property that the type system enforces does not need a test. Fast compiler feedback replaces a slow test-run-and-fix loop.

---

## 5. Full Postgres Support

pgevolve's goal is to support the complete superset of Postgres features that any real application might use. "Most common features" is not an acceptable scope boundary. If Postgres supports it and an application can use it, pgevolve must eventually support it.

We use the official Postgres parser made available by the `pg_query` crate (`pg_query = "6"` in `[workspace.dependencies]`). Parsing is not reimplemented. The `pgevolve-conformance` crate provides the authoritative conformance suite — it tests the full planning and application pipeline against all supported Postgres versions. Sub-spec implementation work is scoped to what Postgres provides, not what is convenient to implement. Gaps in conformance are tracked as issues, not quietly tolerated.

---

## 6. Postgres Version Support

We support every Postgres version that the Postgres community actively maintains. The currently supported versions are **14, 15, 16, 17, and 18**. The conformance suite runs against all five.

When a version reaches end of life per the [Postgres versioning policy](https://www.postgresql.org/support/versioning/), we drop it from the support matrix and remove any code that existed solely for compatibility with that version. This is a feature, not a chore: EOL drops pay down maintenance debt. Postgres 14 reaches EOL in **November 2026** and will be dropped at that time.

---

## 7. Rust Conventions

We follow Rust community best practices without exception.

The workspace `[workspace.lints]` section in `Cargo.toml` is the canonical lint configuration. `clippy::all`, `clippy::pedantic`, and `clippy::nursery` are enabled as warnings; `unsafe_code` and `missing_docs` at the workspace level enforce documentation and memory-safety discipline. Production code contains no `unwrap()` or `expect()` calls — use explicit error propagation with typed errors (`thiserror` for library errors, `anyhow` for binary entry points). `Box<dyn Error>` is not used as a return type. `cargo fmt` and `cargo clippy` must be clean before merge. `cargo doc` must build without warnings.

`unwrap()` and `expect()` are permitted in tests and `xtask` tooling where a panic is an acceptable test failure signal.

---

## 8. Readability, Correctness, Then Performance

The codebase should be easy to read by someone who is not its author. This means: names that describe what a thing is, not how it is implemented; small modules with one clear responsibility; explicit over implicit; no premature abstraction.

Correctness comes before performance. We do not optimize code that has not been measured to be a bottleneck. The domain model — Postgres schemas, the IR, the planner, the change-set representation — is the load-bearing abstraction of this project. Its clarity is not traded away for runtime efficiency. When performance work is warranted, it is done with profiler evidence and reviewed as carefully as any other change.

The simplest architecture capable of solving the full problem is the right architecture. Scope changes that would require materially more complex architecture are worth pushing back on — complexity is a cost that compounds.

---

## 9. CI/CD and Release Discipline

Every PR must pass the full CI suite before it can merge: `cargo fmt --check`, `cargo clippy`, `cargo test`, `cargo doc`, and `cargo deny check`. No exceptions. A green CI run is a necessary (not sufficient) condition for merge.

Releases follow [Semantic Versioning](https://semver.org/). `CHANGELOG.md` at the repository root is the authoritative release-notes source; every release entry must be present before tagging. Release tags are signed. The `[workspace.package].version` field and the `CHANGELOG.md` entry must agree at the time of tagging.

---

## 10. Security

We write secure code by default. We do not defer security concerns for later; they are addressed when the code is written.

When a vulnerability is reported — whether by an external researcher, a user, or a contributor — the response is: acknowledge, investigate, fix, and disclose. The person who reported it is doing the project a favor and is treated as such. Blame is not part of the process. The expected response timeline is: acknowledge within **7 days**, provide a fix or mitigation within **30 days** for medium-severity issues, and faster for criticals at the maintainer's discretion. Security issues are disclosed publicly after a fix is available, with a CVE filed where applicable.

A `SECURITY.md` documenting the reporting process is a TODO for the repository root.

---

## Relationship to the v1.0 charter

This constitution states the **always-binding principles** for the
project — they apply to every commit, v0.x and v1.x alike. The
[v1.0 charter](./v1.md) is the parallel document that states what
pgevolve v1.0 **specifically** commits to (stable surface, quality
gate, PG-version policy, post-1.0 parking lot). At the 1.0 cut, the
charter's stability + cadence sections will be merged into this
constitution and the rest of the charter retired.
