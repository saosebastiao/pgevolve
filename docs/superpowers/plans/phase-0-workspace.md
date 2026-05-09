# Phase 0 — Workspace and tooling foundation

**Goal:** Land a clean Cargo workspace with three crate skeletons (`pgevolve-core`, `pgevolve`, `pgevolve-testkit`), shared lints, formatting, basic CI, and project metadata. Nothing functional yet — just the scaffolding everything else builds on.

**Spec coverage:** §4.1 (crate layout).

**Exit criteria:**

- `cargo build --workspace` succeeds.
- `cargo test --workspace` runs (and passes — there will be a single trivial test in each crate).
- `cargo clippy --all-targets --all-features -- -D warnings` passes.
- `cargo fmt --all -- --check` passes.
- GitHub Actions CI workflow runs build + test + clippy + fmt on push.
- Repo has README + LICENSE (MIT/Apache-2.0 dual).

---

### Task 0.1: Create the workspace `Cargo.toml`

**Files:**
- Create: `Cargo.toml`

- [ ] **Step 1: Write the workspace manifest**

`Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = [
  "crates/pgevolve-core",
  "crates/pgevolve",
  "crates/pgevolve-testkit",
]

[workspace.package]
version      = "0.1.0-dev"
edition      = "2021"
rust-version = "1.85"
license      = "MIT OR Apache-2.0"
repository   = "https://github.com/saosebastiao/pgevolve"
authors      = ["Daniel Toone"]

[workspace.lints.rust]
missing_docs        = "warn"
unsafe_code         = "forbid"
rust_2018_idioms    = { level = "warn", priority = -1 }

[workspace.lints.clippy]
all                 = { level = "warn", priority = -1 }
pedantic            = { level = "warn", priority = -1 }
nursery             = { level = "warn", priority = -1 }
# Allow these pedantic lints that fight idiomatic code:
module_name_repetitions = "allow"
must_use_candidate      = "allow"
missing_errors_doc      = "allow"
missing_panics_doc      = "allow"

[workspace.dependencies]
# Parser
pg_query = "6"

# Errors / utilities
thiserror = "1"
anyhow    = "1"

# Serialization
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
toml        = "0.8"

# Hashing / IDs
blake3 = "1"
uuid   = { version = "1", features = ["v4", "serde"] }

# Logging / tracing
tracing             = "0.1"
tracing-subscriber  = { version = "0.3", features = ["env-filter", "fmt"] }

# Time
time = { version = "0.3", features = ["serde", "formatting", "parsing", "macros"] }

# CLI
clap = { version = "4", features = ["derive", "env"] }

# Postgres driver (binary only)
tokio          = { version = "1", features = ["macros", "rt-multi-thread", "sync", "signal"] }
tokio-postgres = { version = "0.7", features = ["with-uuid-1", "with-time-0_3"] }

# Testing
proptest         = "1"
testcontainers   = "0.22"
insta            = { version = "1", features = ["yaml", "filters"] }
pretty_assertions = "1"

[profile.dev]
opt-level = 0

[profile.release]
opt-level = 3
lto       = "thin"
strip     = "symbols"

[profile.test]
opt-level = 1
```

- [ ] **Step 2: Commit**

```bash
git add Cargo.toml
git commit -m "chore: initialize cargo workspace manifest

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 0.2: Pin the toolchain and configure cargo

**Files:**
- Create: `rust-toolchain.toml`
- Create: `.cargo/config.toml`
- Create: `rustfmt.toml`
- Create: `.gitignore`

- [ ] **Step 1: Pin toolchain**

`rust-toolchain.toml`:

```toml
[toolchain]
channel    = "1.85"
components = ["rustfmt", "clippy", "rust-src"]
profile    = "minimal"
```

- [ ] **Step 2: Cargo config**

`.cargo/config.toml`:

```toml
[build]
# Use a single workspace target dir so all crates share artifacts.
# (Default already does this for workspaces; explicit for clarity.)
target-dir = "target"

[net]
git-fetch-with-cli = true
```

- [ ] **Step 3: rustfmt config**

`rustfmt.toml`:

```toml
edition           = "2021"
max_width         = 100
use_field_init_shorthand = true
use_try_shorthand        = true
imports_granularity      = "Module"
group_imports            = "StdExternalCrate"
reorder_imports          = true
```

- [ ] **Step 4: .gitignore**

`.gitignore`:

```
/target
**/*.rs.bk
.DS_Store
*.swp
*.swo
.idea/
.vscode/
```

- [ ] **Step 5: Commit**

```bash
git add rust-toolchain.toml .cargo .gitignore rustfmt.toml
git commit -m "chore: pin rust toolchain and configure cargo / rustfmt

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 0.3: Create `pgevolve-core` crate skeleton

**Files:**
- Create: `crates/pgevolve-core/Cargo.toml`
- Create: `crates/pgevolve-core/src/lib.rs`

- [ ] **Step 1: Crate manifest**

`crates/pgevolve-core/Cargo.toml`:

```toml
[package]
name         = "pgevolve-core"
description  = "Postgres declarative schema management — core library (parser, IR, diff, planner)"
version      = { workspace = true }
edition      = { workspace = true }
rust-version = { workspace = true }
license      = { workspace = true }
repository   = { workspace = true }
authors      = { workspace = true }

[lints]
workspace = true

[dependencies]
pg_query   = { workspace = true }
thiserror  = { workspace = true }
serde      = { workspace = true }
toml       = { workspace = true }
blake3     = { workspace = true }
tracing    = { workspace = true }
time       = { workspace = true }

[dev-dependencies]
pretty_assertions = { workspace = true }
proptest          = { workspace = true }
insta             = { workspace = true }
```

- [ ] **Step 2: Stub library**

`crates/pgevolve-core/src/lib.rs`:

```rust
//! `pgevolve-core` — the declarative-schema-management engine.
//!
//! This crate is I/O-free: it accepts source SQL bytes and a [`CatalogQuerier`]
//! implementation from callers, and returns IR, diffs, and plans as data.
//! See the workspace `docs/superpowers/specs/` for the design.
//!
//! [`CatalogQuerier`]: https://example.invalid/  // wired in phase 3
#![warn(missing_docs)]
#![forbid(unsafe_code)]

/// Crate version, exposed for embedding in plan manifests.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_nonempty() {
        assert!(!VERSION.is_empty());
    }
}
```

- [ ] **Step 3: Verify build**

Run: `cargo build -p pgevolve-core`
Expected: succeeds with no warnings.

- [ ] **Step 4: Verify test**

Run: `cargo test -p pgevolve-core`
Expected: 1 passing test.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core
git commit -m "feat(core): skeleton crate with version constant

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 0.4: Create `pgevolve` binary crate skeleton

**Files:**
- Create: `crates/pgevolve/Cargo.toml`
- Create: `crates/pgevolve/src/main.rs`

- [ ] **Step 1: Crate manifest**

`crates/pgevolve/Cargo.toml`:

```toml
[package]
name         = "pgevolve"
description  = "Postgres declarative schema management CLI"
version      = { workspace = true }
edition      = { workspace = true }
rust-version = { workspace = true }
license      = { workspace = true }
repository   = { workspace = true }
authors      = { workspace = true }

[lints]
workspace = true

[[bin]]
name = "pgevolve"
path = "src/main.rs"

[dependencies]
pgevolve-core = { path = "../pgevolve-core" }

clap                = { workspace = true }
anyhow              = { workspace = true }
serde               = { workspace = true }
serde_json          = { workspace = true }
toml                = { workspace = true }
tracing             = { workspace = true }
tracing-subscriber  = { workspace = true }
tokio               = { workspace = true }
tokio-postgres      = { workspace = true }
uuid                = { workspace = true }
time                = { workspace = true }

[dev-dependencies]
pretty_assertions = { workspace = true }
```

- [ ] **Step 2: Stub binary**

`crates/pgevolve/src/main.rs`:

```rust
//! `pgevolve` CLI entry point.

fn main() -> anyhow::Result<()> {
    println!("pgevolve {} (skeleton)", pgevolve_core::VERSION);
    Ok(())
}
```

- [ ] **Step 3: Verify**

Run: `cargo run -p pgevolve --quiet`
Expected: prints `pgevolve 0.1.0-dev (skeleton)`.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve
git commit -m "feat(cli): skeleton binary that prints version

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 0.5: Create `pgevolve-testkit` crate skeleton

**Files:**
- Create: `crates/pgevolve-testkit/Cargo.toml`
- Create: `crates/pgevolve-testkit/src/lib.rs`

- [ ] **Step 1: Crate manifest**

`crates/pgevolve-testkit/Cargo.toml`:

```toml
[package]
name         = "pgevolve-testkit"
description  = "Test infrastructure for pgevolve — ephemeral Postgres, generators, harnesses"
version      = { workspace = true }
edition      = { workspace = true }
rust-version = { workspace = true }
license      = { workspace = true }
repository   = { workspace = true }
authors      = { workspace = true }
publish      = false  # internal use only; revisit before public release

[lints]
workspace = true

[dependencies]
pgevolve-core = { path = "../pgevolve-core" }

anyhow         = { workspace = true }
proptest       = { workspace = true }
testcontainers = { workspace = true }
tokio          = { workspace = true }
tokio-postgres = { workspace = true }
tracing        = { workspace = true }
uuid           = { workspace = true }

[dev-dependencies]
pretty_assertions = { workspace = true }
```

- [ ] **Step 2: Stub library**

`crates/pgevolve-testkit/src/lib.rs`:

```rust
//! `pgevolve-testkit` — internal test infrastructure for the pgevolve workspace.
//!
//! Consumed only as a `dev-dependency`. Provides ephemeral Postgres
//! provisioning, IR generators, equivalence asserters, and end-to-end
//! harnesses for property and chaos testing.
#![warn(missing_docs)]
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {}
}
```

- [ ] **Step 3: Verify**

Run: `cargo build -p pgevolve-testkit`
Expected: succeeds.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-testkit
git commit -m "feat(testkit): skeleton crate

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 0.6: Verify lints + format are clean across the workspace

- [ ] **Step 1: Run clippy**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: no warnings.

- [ ] **Step 2: Run fmt check**

Run: `cargo fmt --all -- --check`
Expected: no changes needed.

If either fails, fix and re-run before proceeding.

---

### Task 0.7: Add LICENSE files

**Files:**
- Create: `LICENSE-MIT`
- Create: `LICENSE-APACHE`

- [ ] **Step 1: Add MIT license**

Use the standard MIT license text with `2026 Daniel Toone` copyright. Pull from https://opensource.org/license/mit (or copy from any well-known Rust crate).

- [ ] **Step 2: Add Apache 2.0 license**

Use the standard Apache-2.0 license text. Pull from https://www.apache.org/licenses/LICENSE-2.0.txt.

- [ ] **Step 3: Commit**

```bash
git add LICENSE-MIT LICENSE-APACHE
git commit -m "chore: add dual MIT/Apache-2.0 license files

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 0.8: Add README

**Files:**
- Create: `README.md`

- [ ] **Step 1: Write README**

`README.md`:

```markdown
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
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add project README

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 0.9: Add CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Write workflow**

`.github/workflows/ci.yml`:

```yaml
name: ci

on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: -D warnings

jobs:
  fmt:
    name: rustfmt
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: 1.85
          components: rustfmt
      - run: cargo fmt --all -- --check

  clippy:
    name: clippy
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: 1.85
          components: clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo clippy --workspace --all-targets --all-features -- -D warnings

  test:
    name: test (unit + tier-2)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: 1.85
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --workspace --lib --tests
        env:
          # Tier-3+ tests require ephemeral PG containers; gate them via
          # this env var so CI can opt in once the harness exists (phase 3).
          PGEVOLVE_DISABLE_DOCKER_TESTS: "1"

  # Placeholder for phase 6 onward; uncomment when the harness exists.
  # pg-matrix:
  #   name: pg matrix (tier 3-6)
  #   needs: [test]
  #   runs-on: ubuntu-latest
  #   strategy:
  #     fail-fast: false
  #     matrix:
  #       pg: ["14", "15", "16", "17"]
  #   steps:
  #     - uses: actions/checkout@v4
  #     - uses: dtolnay/rust-toolchain@stable
  #       with: { toolchain: 1.85 }
  #     - uses: Swatinem/rust-cache@v2
  #     - run: cargo test --workspace --features pg-tests
  #       env:
  #         PGEVOLVE_TEST_PG_VERSION: ${{ matrix.pg }}
```

- [ ] **Step 2: Commit and push**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add fmt / clippy / test workflow

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
git push origin main
```

- [ ] **Step 3: Verify**

Open the GitHub Actions tab on the repo and confirm the `ci` workflow runs all three jobs and they pass on the latest commit.

---

### Task 0.10: Phase 0 self-review

- [ ] **Step 1: Run the exit-criteria gauntlet**

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

All four must succeed.

- [ ] **Step 2: Verify CI is green on `main`.**

GitHub → Actions → most recent run on `main` → all jobs green.

- [ ] **Step 3: Commit (no-op if nothing changed)** and proceed to phase 1.

Phase 0 complete.
