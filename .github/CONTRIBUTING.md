# Contributing to pgevolve

Thanks for thinking about contributing! pgevolve is a Postgres
declarative schema management tool written in Rust. This document
explains how to get involved.

---

## Welcome

pgevolve is a small, opinionated project. We care about correctness,
type-system rigor, and a tight feedback loop with users. Contributions
of all sizes are welcome — typo fixes, bug reports, feature ideas, and
code — as long as they fit the project's direction.

Before sinking time into a non-trivial change, please read **Before
you start** below: we prefer a short discussion up front to a polished
pull request we then have to ask you to redo.

---

## Before you start

**Open an issue first** for anything beyond a typo or a one-line bug
fix. A two-sentence comment ("I'm thinking of adding X because Y —
does that fit?") saves everyone time. We'd rather catch a scope or
design concern in an issue than in a pull request that has to be
rewritten.

For larger changes, pgevolve uses a lightweight workflow that we
recommend you follow:

1. **Brainstorm** — agree on the shape of the problem in an issue.
2. **Spec** — write the design as a short document under
   [`docs/superpowers/specs/`](../docs/superpowers/specs/) using the
   existing files there as templates. The spec is the contract.
3. **Plan** — decompose the spec into bite-sized tasks under
   [`docs/superpowers/plans/`](../docs/superpowers/plans/).
4. **Implement** — work the plan task-by-task, committing in small
   coherent units.

Steps 2 and 3 sound heavy but they're short documents — usually one
page each. They exist so the design discussion lives in version control
instead of pull-request comment threads. Look at any recent spec/plan
pair for the expected size and tone.

---

## Quick start

You'll need:

- **Rust** ≥ 1.95 (set as `rust-version` in `Cargo.toml`).
- **Docker** — the conformance suite spins up ephemeral Postgres 14, 15,
  16, and 17 containers via testkit fixtures.
- **`cargo-deny`** for the license/advisory audit. Install once with:

  ```sh
  cargo install cargo-deny
  ```

Then:

```sh
git clone https://github.com/saosebastiao/pgevolve.git
cd pgevolve
cargo build --workspace
cargo test --workspace
```

The first `cargo test` pulls Postgres container images and will take
a few minutes. Subsequent runs reuse the images.

---

## The verify gate

Before opening a pull request, run the same gates CI runs. They must
all pass:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
cargo deny check
```

These are non-negotiable: workspace lints are configured to the
pedantic + nursery set with `-D warnings`, and the doc build rejects
broken intra-doc links. If a clippy lint is genuinely wrong for a
specific call site, prefer fixing the call site over `#[allow(...)]`;
when an allow is the right call, justify it in a short comment.

For changes that touch flaky territory (property tests, integration
tests, anything timing-sensitive), run the relevant suite at least
ten times before declaring it green. Non-determinism is a bug, not a
feature.

---

## How decisions get made

The authoritative documents, in order of precedence:

- [**`docs/CONSTITUTION.md`**](../docs/CONSTITUTION.md) — binding
  principles: licensing, dependency policy, type-system rigor,
  Postgres support goals, conventions, security posture. Read this
  first; it overrides everything else in this list.
- [**`docs/v1.md`**](../docs/v1.md) — what pgevolve commits to at v1.0
  (stability surface, quality gates, feature checklist).
- [**`docs/spec/`**](../docs/spec/) — the living capability catalogue:
  which Postgres objects, column types, constraints, etc. pgevolve
  supports today.
- [**`docs/superpowers/specs/`**](../docs/superpowers/specs/) — per-
  feature design documents. Each one captures the reasoning behind a
  specific change. Treat these as ADRs.
- [**`docs/superpowers/plans/`**](../docs/superpowers/plans/) —
  implementation plans matched to specs.

When in doubt, the Constitution wins. If you find a tension between
documents, that's a bug worth opening an issue about.

---

## Filing issues

A few notes on the issue tracker:

- **Bug reports** — please include the pgevolve version, Postgres
  major version, the schema input that triggered the bug, and the
  observed-vs-expected output. A minimal reproducer is the single
  most useful thing you can attach.
- **Feature requests** — describe the use case, not the
  implementation. "I want pgevolve to manage X object kind for
  reason Y" lets us evaluate the request against the v1.0 charter and
  roadmap; a pre-baked implementation sketch is harder to redirect.
- **Questions** — fine to file as issues. They help us see what's
  unclear in the docs.
- **Security concerns** — **do not** file in the public tracker. See
  [`.github/SECURITY.md`](./SECURITY.md) for the private disclosure
  channel.

---

## License

pgevolve is dual-licensed under **MIT OR Apache-2.0**. By submitting a
contribution, you agree it can be released under both licenses.

Dependencies must be permissively licensed (MIT, Apache-2.0, BSD,
ISC, Unicode, Zlib, MPL-2.0 where unavoidable). Copyleft licenses
(GPL, AGPL, LGPL) and proprietary licenses are not allowed and will
be rejected by `cargo deny check`. If a crate you want to add isn't
in the existing `deny.toml` allow-list, please raise it in your
issue before opening the PR.

Adding a dependency at all is a decision worth a sentence of
justification — pgevolve treats dependencies as a liability and
defaults to writing things ourselves rather than pulling in a crate.
Constitution §2 has the full reasoning.

---

## Code of conduct

We expect contributors to treat each other with respect. Disagreements
about technical direction are fine and useful; disagreements that
become personal are not.

A formal Code of Conduct will be added once the project has a
private reporting channel set up. Until then, if you have concerns
about contributor behavior, please raise them via GitHub Issues —
either in the relevant thread if it's a public matter, or by opening
a new issue if a separate report is warranted.

Thanks again for reading this. We're glad you're here.
