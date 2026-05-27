# SUBSCRIPTION Implementation Plan (v0.3.5)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship v0.3.5 — first-class `Subscription` IR object for Postgres logical-replication subscriber state, with `${VAR}` env-var interpolation in CONNECTION strings so source SQL stays secret-free.

**Architecture:** Eleven sequential stages mirroring the v0.3.4 PUBLICATION cadence. Per-field lenient `SubscriptionOptions` (Option<T> everywhere — same pattern as v0.3.3 reloptions); whole-subscription lenient via `unmanaged-subscription` lint. Opaque CONNECTION string; `${VAR}` resolved at apply-time preflight, never persisted. Differ compares connection strings *modulo password* (a tiny libpq tokenizer in the differ). `[fixture] apply = false` flag added to conformance harness so SUBSCRIPTION fixtures can validate parse + diff + plan.sql + lint without a second ephemeral PG. Tier-3 catalog reader tests use `enabled = false, create_slot = false, copy_data = false` to populate `pg_subscription` without network activity.

**Tech Stack:** Rust 1.95+, `pg_query` 6.x, `tokio_postgres`, `serde`, `proptest`. Builds on every v0.3.x pattern (no new cross-cutting concerns) plus one new helper: a small env-var interpolator.

**Source spec:** `docs/superpowers/specs/2026-05-26-subscriptions-design.md`.

---

## Pre-flight

- [ ] **Step 1: Confirm clean baseline**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib
```

All green. v0.3.4 is committed and tagged; `main` is clean.

- [ ] **Step 2: Skim the spec once**

Open `docs/superpowers/specs/2026-05-26-subscriptions-design.md`. Each stage below cites the spec section it implements.

- [ ] **Step 3: Skim the v0.3.4 PUBLICATION plan as the structural template**

`docs/superpowers/plans/2026-05-26-publications.md` (just shipped). SUBSCRIPTION follows the identical cadence; the differences are noted per-stage. The most novel pieces relative to PUBLICATION are: the `${VAR}` interpolator (Stage 4), the diff-modulo-password helper (Stage 7), and the conformance `apply = false` flag (Stage 10).

---

## File structure

```
crates/pgevolve-core/src/
├── ir/
│   ├── subscription.rs              NEW — Stage 1 — Subscription, SubscriptionOptions, StreamingMode, OriginMode
│   ├── catalog.rs                   MODIFY — Stage 2 — add subscriptions field
│   ├── mod.rs                       MODIFY — Stage 1 — re-export subscription
│   └── canon/
│       ├── mod.rs                   MODIFY — Stage 3 — wire subscriptions pass
│       └── subscriptions.rs         NEW — Stage 3 — validate + sort
├── catalog/
│   ├── subscriptions.rs             NEW — Stage 5 — decoder + per-version SQL
│   ├── queries/
│   │   ├── shared.rs                MODIFY — Stage 5 — PG15+/16+/17+ queries
│   │   └── pg14.rs                  MODIFY — Stage 5 — PG14 variants
│   ├── assemble/
│   │   └── subscriptions.rs         NEW — Stage 5 — assembler
│   └── mod.rs                       MODIFY — Stage 5 — wire into read_catalog
├── parse/
│   └── builder/
│       ├── subscription_stmt.rs     NEW — Stage 6 — CREATE/ALTER SUBSCRIPTION + fold
│       └── mod.rs                   MODIFY — Stage 6 — dispatch
├── diff/
│   ├── subscriptions.rs             NEW — Stage 7 — granular diff + connstr-modulo-password
│   ├── change.rs                    MODIFY — Stage 7 — 8 new variants
│   ├── mod.rs                       MODIFY — Stage 7 — call diff_subscriptions
│   └── owner_op.rs                  MODIFY — Stage 7 — OwnerObjectKind::Subscription
├── plan/
│   ├── raw_step.rs                  MODIFY — Stage 8 — 8 new StepKind variants
│   ├── plan.rs                      MODIFY — Stage 8 — extend kind_name / parse_kind_name
│   ├── edges.rs                     MODIFY — Stage 8 — add NodeId::Subscription
│   └── rewrite/
│       ├── subscriptions.rs         NEW — Stage 8 — SQL helpers
│       └── mod.rs                   MODIFY — Stage 8 — dispatch 8 emit arms
└── lint/
    ├── rules/
    │   ├── unmanaged_subscription.rs                              NEW — Stage 9
    │   ├── subscription_references_undeclared_publication.rs      NEW — Stage 9
    │   ├── subscription_feature_requires_pg_version.rs            NEW — Stage 9
    │   ├── subscription_password_in_source.rs                     NEW — Stage 9
    │   └── mod.rs                                                 MODIFY — Stage 9
    └── universal.rs                  MODIFY — Stage 9 — wire 4 rules

crates/pgevolve/src/
├── executor/
│   ├── env_interp.rs                NEW — Stage 4 — ${VAR} interpolator
│   ├── preflight.rs                 MODIFY — Stage 4 — resolve ${VAR} in CONNECTION steps
│   └── error.rs                     MODIFY — Stage 4 — MissingEnvVar variant
└── commands/diff.rs                 MODIFY — Stage 8 — print_human + change_kind_name for 8 variants

crates/pgevolve-conformance/
├── src/fixture.rs                   MODIFY — Stage 10 — [fixture] apply: bool field
├── tests/run.rs                     MODIFY — Stage 10 — honor apply = false
└── tests/cases/objects/subscriptions/  NEW — Stage 10 — 12 fixtures

crates/pgevolve-testkit/src/
└── ir_generator.rs                  MODIFY — Stage 11 — arb_subscription strategies

docs/spec/
├── objects.md                       MODIFY — Stage 11 — SUBSCRIPTION row ✅ Supported
└── subscriptions.md                 NEW — Stage 11 — capability page

CHANGELOG.md                          MODIFY — Stage 11 — [0.3.5] section
Cargo.toml                            MODIFY — Stage 11 — version 0.3.4 → 0.3.5
```

---

## Stage 1 — IR foundation

Pure data types. No behavior beyond derives.

**Files created:** `crates/pgevolve-core/src/ir/subscription.rs`.
**Files modified:** `crates/pgevolve-core/src/ir/mod.rs`.

**Spec ref:** "IR shape".

### Task 1.1: Create the module

- [ ] **Step 1: Write `crates/pgevolve-core/src/ir/subscription.rs`**

```rust
//! Subscription IR — declarative logical-replication subscriber-side metadata.
//!
//! A `Subscription` is a Postgres `CREATE SUBSCRIPTION` object. It lives at
//! the Catalog top level (not schema-qualified) because Postgres treats
//! subscriptions as a per-database global namespace.
//!
//! The `connection` field stores the libpq connection string verbatim,
//! including unresolved `${VAR}` env-var references. Resolution happens at
//! apply-time preflight (`crates/pgevolve/src/executor/env_interp.rs`),
//! never at parse or canon time. The source IR — and therefore plan.sql —
//! never contains resolved secrets.
//!
//! Spec: `docs/superpowers/specs/2026-05-26-subscriptions-design.md`.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;

/// Declarative model of a Postgres `SUBSCRIPTION`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    /// Subscription name (not schema-qualified — subscriptions are global).
    pub name: Identifier,
    /// libpq connection string. May contain `${VAR}` env-var references that
    /// are resolved at apply-time preflight. Stored verbatim through parse,
    /// canon, diff, and plan serialization.
    pub connection: String,
    /// Publications this subscription reads from. Sorted + deduped by canon.
    /// Non-empty (enforced by canon).
    pub publications: Vec<Identifier>,
    /// Per-field lenient WITH options.
    pub options: SubscriptionOptions,
    /// Object owner. `None` = unmanaged (the differ ignores ownership).
    /// `Some(role)` = managed: diff emits `ALTER SUBSCRIPTION ... OWNER TO role`.
    pub owner: Option<Identifier>,
    /// Optional comment.
    pub comment: Option<String>,
}

/// Per-field lenient WITH options for a `Subscription`.
///
/// Every field is `Option<T>`. `None` = unmanaged (pgevolve neither sets
/// nor resets); `Some(value)` = managed (diff emits an ALTER to match).
/// Matches the v0.3.3 reloptions per-field-lenient pattern.
///
/// **CREATE-only fields**: `create_slot` and `copy_data` are PG-CREATE-only
/// (no `ALTER SUBSCRIPTION s SET (create_slot = …)` exists). They flow into
/// the IR from source CREATE statements so users can declare them, but the
/// differ NEVER includes them in `AlterSubscriptionSetOptions` deltas, and
/// the catalog reader ALWAYS returns `None` for them (pg_subscription
/// doesn't store the CREATE-time decision). See `diff::subscriptions::options_delta`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SubscriptionOptions {
    /// Whether the subscription is enabled. PG default: true.
    pub enabled: Option<bool>,
    /// Replication slot name on the publisher. `None` = use subscription name.
    pub slot_name: Option<Identifier>,
    /// Whether `CREATE SUBSCRIPTION` should create the publisher-side slot.
    /// PG default: true.
    pub create_slot: Option<bool>,
    /// Whether to copy existing rows during initial sync. PG default: true.
    pub copy_data: Option<bool>,
    /// `synchronous_commit` GUC value for the subscription's apply worker.
    /// Free-form string (e.g., `"on"`, `"off"`, `"remote_write"`, `"local"`).
    pub synchronous_commit: Option<String>,
    /// Use binary copy / binary replication protocol. PG default: false.
    pub binary: Option<bool>,
    /// Streaming mode for large in-progress transactions.
    pub streaming: Option<StreamingMode>,
    /// Two-phase commit handling. PG 14+; default: false.
    pub two_phase: Option<bool>,
    /// Disable the subscription on apply error. PG 15+; default: false.
    pub disable_on_error: Option<bool>,
    /// Whether the subscription owner must supply a password. PG 16+; default: true.
    pub password_required: Option<bool>,
    /// Run the apply worker as the subscription owner (instead of the table owner).
    /// PG 16+; default: false.
    pub run_as_owner: Option<bool>,
    /// Replication origin handling. PG 16+; default: Any.
    pub origin: Option<OriginMode>,
    /// Whether the subscription survives failover. PG 17+; default: false.
    pub failover: Option<bool>,
}

/// `streaming` mode for in-progress transactions on a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamingMode {
    /// Stream nothing; spool to disk at the subscriber.
    Off,
    /// Stream in-progress transactions to disk on the subscriber.
    On,
    /// Stream in-progress transactions to parallel apply workers. PG 16+.
    Parallel,
}

/// `origin` mode for replication-origin handling on a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OriginMode {
    /// Replicate all changes regardless of origin (default).
    Any,
    /// Replicate only changes from non-replicated sources (avoid loops).
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_default_all_none() {
        let o = SubscriptionOptions::default();
        assert!(o.enabled.is_none());
        assert!(o.slot_name.is_none());
        assert!(o.create_slot.is_none());
        assert!(o.copy_data.is_none());
        assert!(o.synchronous_commit.is_none());
        assert!(o.binary.is_none());
        assert!(o.streaming.is_none());
        assert!(o.two_phase.is_none());
        assert!(o.disable_on_error.is_none());
        assert!(o.password_required.is_none());
        assert!(o.run_as_owner.is_none());
        assert!(o.origin.is_none());
        assert!(o.failover.is_none());
    }

    #[test]
    fn streaming_off_does_not_equal_on() {
        assert_ne!(StreamingMode::Off, StreamingMode::On);
        assert_ne!(StreamingMode::On, StreamingMode::Parallel);
    }

    #[test]
    fn origin_any_does_not_equal_none() {
        assert_ne!(OriginMode::Any, OriginMode::None);
    }
}
```

- [ ] **Step 2: Add to `crates/pgevolve-core/src/ir/mod.rs`**

```rust
pub mod subscription;
```

Alphabetical position within the existing `pub mod` list (between `sequence` and `table`, or wherever alphabetical lands).

- [ ] **Step 3: Build**

```bash
cargo build -p pgevolve-core
```

Expected: clean.

- [ ] **Step 4: Run tests**

```bash
cargo test -p pgevolve-core --lib ir::subscription
```

Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/ir/subscription.rs crates/pgevolve-core/src/ir/mod.rs
git commit -m "$(cat <<'EOF'
feat(ir): Subscription, SubscriptionOptions, StreamingMode, OriginMode

New top-level IR module for SUBSCRIPTION. Pure data types; no behavior
beyond derives. Per-field lenient SubscriptionOptions (Option on every
field) mirrors v0.3.3 reloptions. CONNECTION is opaque String; ${VAR}
env-var references are stored verbatim and resolved only at apply-time
preflight.

Stage 1 of docs/superpowers/plans/2026-05-26-subscriptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 2 — Add `subscriptions` field to Catalog

**Files modified:** `crates/pgevolve-core/src/ir/catalog.rs` (and any hand-rolled `Catalog { … }` struct literals).

### Task 2.1: Backfill the field

- [ ] **Step 1: Add the field to `Catalog`**

In `crates/pgevolve-core/src/ir/catalog.rs`, append to the struct definition (alphabetical position, after `publications`):

```rust
    /// Subscriptions (logical-replication subscriber-side metadata).
    pub subscriptions: Vec<crate::ir::subscription::Subscription>,
```

- [ ] **Step 2: Initialize in `Catalog::empty()`**

```rust
            subscriptions: Vec::new(),
```

- [ ] **Step 3: Find + backfill hand-rolled literals**

```bash
grep -rln "Catalog {" crates/ | xargs grep -l "schemas:" | head
```

If a literal doesn't use `..Catalog::empty()`, add `subscriptions: Vec::new(),`. The v0.3.4 PUBLICATION Stage 2 had one such site (`ir_mutator.rs`); v0.3.5 will be similar.

- [ ] **Step 4: Build the workspace**

```bash
cargo build --workspace
```

Any "missing field subscriptions" error flags a site missed in step 3.

- [ ] **Step 5: Tests**

```bash
cargo test --workspace --lib
```

Expected: all pass (pure plumbing).

- [ ] **Step 6: Commit**

```bash
git add crates/pgevolve-core/src/
git commit -m "$(cat <<'EOF'
feat(ir): add Catalog::subscriptions

Backfills hand-rolled Catalog struct literals with
subscriptions: Vec::new(). Pure plumbing — no behavior change.

Stage 2 of docs/superpowers/plans/2026-05-26-subscriptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 3 — Canon pass

Validate + sort. Two invariants enforced:
1. `Subscription.publications` is non-empty → else `IrError::EmptySubscriptionPublications`.
2. `Subscription.connection` is non-empty/non-whitespace → else `IrError::EmptyConnection`.
3. Sort + dedupe `publications`.

**Files created:** `crates/pgevolve-core/src/ir/canon/subscriptions.rs`.
**Files modified:** `crates/pgevolve-core/src/ir/canon/mod.rs`, `crates/pgevolve-core/src/ir/mod.rs` (add error variants — `IrError` lives in `ir/mod.rs` per the codebase pattern Stage 3 of v0.3.4 documented).

### Task 3.1: Add error variants

- [ ] **Step 1: Extend `IrError`**

In `crates/pgevolve-core/src/ir/mod.rs` (or wherever `IrError` is defined):

```rust
    /// A `Subscription.publications` was empty.
    #[error("subscription {0:?}: empty publication list (PG requires at least one)")]
    EmptySubscriptionPublications(crate::identifier::Identifier),
    /// A `Subscription.connection` was empty or whitespace-only.
    #[error("subscription {0:?}: empty connection string")]
    EmptyConnection(crate::identifier::Identifier),
```

- [ ] **Step 2: Build**

```bash
cargo build -p pgevolve-core
```

### Task 3.2: Create the canon pass

- [ ] **Step 1: Write `crates/pgevolve-core/src/ir/canon/subscriptions.rs`**

```rust
//! Canon pass for subscriptions. Validates and sorts.
//!
//! Invariants enforced:
//! - `Subscription.publications` is non-empty (PG requires at least one).
//! - `Subscription.connection` is non-empty / non-whitespace.
//!
//! Sorts:
//! - `Subscription.publications` by identifier text; duplicates silently
//!   deduplicated (source-side order is not semantically meaningful).
//! - The subscriptions collection itself is sorted by `sort_and_dedupe`,
//!   not here.

use crate::ir::catalog::Catalog;
use crate::ir::IrError;
use crate::ir::subscription::Subscription;

pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    for s in &mut cat.subscriptions {
        validate_and_sort(s)?;
    }
    Ok(())
}

fn validate_and_sort(s: &mut Subscription) -> Result<(), IrError> {
    if s.connection.trim().is_empty() {
        return Err(IrError::EmptyConnection(s.name.clone()));
    }
    if s.publications.is_empty() {
        return Err(IrError::EmptySubscriptionPublications(s.name.clone()));
    }
    s.publications.sort();
    s.publications.dedup();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::subscription::SubscriptionOptions;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn sub_with(connection: &str, publications: Vec<Identifier>) -> Subscription {
        Subscription {
            name: id("s"),
            connection: connection.into(),
            publications,
            options: SubscriptionOptions::default(),
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn rejects_empty_connection() {
        let mut cat = Catalog::empty();
        cat.subscriptions.push(sub_with("", vec![id("p")]));
        assert!(matches!(run(&mut cat).unwrap_err(), IrError::EmptyConnection(_)));
    }

    #[test]
    fn rejects_whitespace_only_connection() {
        let mut cat = Catalog::empty();
        cat.subscriptions.push(sub_with("   ", vec![id("p")]));
        assert!(matches!(run(&mut cat).unwrap_err(), IrError::EmptyConnection(_)));
    }

    #[test]
    fn rejects_empty_publications() {
        let mut cat = Catalog::empty();
        cat.subscriptions.push(sub_with("host=x", vec![]));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::EmptySubscriptionPublications(_)
        ));
    }

    #[test]
    fn sorts_and_dedupes_publications() {
        let mut cat = Catalog::empty();
        cat.subscriptions.push(sub_with(
            "host=x",
            vec![id("c"), id("a"), id("b"), id("a")],
        ));
        run(&mut cat).unwrap();
        let pubs = &cat.subscriptions[0].publications;
        assert_eq!(pubs.len(), 3);
        assert_eq!(pubs[0].as_str(), "a");
        assert_eq!(pubs[1].as_str(), "b");
        assert_eq!(pubs[2].as_str(), "c");
    }

    #[test]
    fn passes_through_valid_subscription() {
        let mut cat = Catalog::empty();
        cat.subscriptions.push(sub_with("host=x dbname=app", vec![id("p")]));
        assert!(run(&mut cat).is_ok());
    }
}
```

- [ ] **Step 2: Wire into orchestrator**

In `crates/pgevolve-core/src/ir/canon/mod.rs`, add `pub mod subscriptions;` and call `subscriptions::run(cat)?;` after `publications::run(cat)?;` (alphabetical / pipeline order).

- [ ] **Step 3: Build + test**

```bash
cargo test -p pgevolve-core --lib ir::canon::subscriptions
```

Expected: 5 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/ir/canon/ crates/pgevolve-core/src/ir/mod.rs
git commit -m "$(cat <<'EOF'
feat(ir): canon pass for subscriptions

Validates non-empty connection + non-empty publication list.
Sorts + dedupes publications.

Stage 3 of docs/superpowers/plans/2026-05-26-subscriptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 4 — `${VAR}` env-var interpolation

A small interpolator that resolves `${NAME}` references in a string template against a function-supplied env. Lives in the executor (not pgevolve-core) because resolution is apply-time only; the IR keeps the unresolved form.

**Files created:** `crates/pgevolve/src/executor/env_interp.rs`.
**Files modified:** `crates/pgevolve/src/executor/preflight.rs`, `crates/pgevolve/src/executor/error.rs` (or wherever `ApplyError` is defined).

**Spec ref:** "`${VAR}` interpolation".

### Task 4.1: Create the interpolator

- [ ] **Step 1: Write `crates/pgevolve/src/executor/env_interp.rs`**

```rust
//! Apply-time `${VAR}` env-var interpolation.
//!
//! Source IR stores literal `${NAME}` references in SUBSCRIPTION CONNECTION
//! strings. Resolution happens at apply-time preflight: every step's SQL is
//! scanned, every reference is looked up in process env (or a test override),
//! and missing references cause an `ApplyError::MissingEnvVar` before any
//! DB connection is attempted. plan.sql on disk always contains the
//! unresolved form — secrets are never written to disk.
//!
//! Syntax: `${NAME}`. Only matches `[A-Z_][A-Z0-9_]*` between the braces;
//! anything else is left literal (so legitimate `$1`, `${foo}` from a
//! function body etc. don't accidentally trigger).

use std::fmt;

/// Resolve `${VAR}` references in `template` against `env`. Returns the
/// resolved string, or `MissingEnvVar(name)` if any referenced variable
/// is absent.
///
/// `env` is a closure so tests can inject a controlled environment without
/// touching the process's actual env vars.
pub fn resolve<F>(template: &str, env: F) -> Result<String, MissingEnvVar>
where
    F: Fn(&str) -> Option<String>,
{
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            // Find the closing '}'.
            if let Some(end) = bytes[i + 2..].iter().position(|&b| b == b'}') {
                let name_start = i + 2;
                let name_end = name_start + end;
                let name = &template[name_start..name_end];
                if is_valid_var_name(name) {
                    match env(name) {
                        Some(v) => {
                            out.push_str(&v);
                            i = name_end + 1;
                            continue;
                        }
                        None => return Err(MissingEnvVar(name.to_string())),
                    }
                }
                // Invalid name shape → leave literal, advance past '${'.
            }
        }
        // Default: copy one byte.
        out.push(bytes[i] as char);
        i += 1;
    }
    Ok(out)
}

/// Returned when `resolve` encounters a `${VAR}` whose name isn't in the env.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingEnvVar(pub String);

impl fmt::Display for MissingEnvVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "missing environment variable: {}", self.0)
    }
}

impl std::error::Error for MissingEnvVar {}

fn is_valid_var_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap_or(' ');
    if !(first.is_ascii_uppercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Collect every `${VAR}` reference in `template` without resolving. Useful
/// for preflight summary / error messages.
#[must_use]
pub fn references(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = bytes[i + 2..].iter().position(|&b| b == b'}') {
                let name = &template[i + 2..i + 2 + end];
                if is_valid_var_name(name) {
                    out.push(name.to_string());
                    i = i + 2 + end + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + '_ {
        let map: HashMap<_, _> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn resolves_single_var() {
        let r = resolve("host=x password=${PW}", env_from(&[("PW", "secret")])).unwrap();
        assert_eq!(r, "host=x password=secret");
    }

    #[test]
    fn resolves_multiple_vars() {
        let r = resolve(
            "host=${H} user=${U} password=${P}",
            env_from(&[("H", "db.example.com"), ("U", "repl"), ("P", "secret")]),
        )
        .unwrap();
        assert_eq!(r, "host=db.example.com user=repl password=secret");
    }

    #[test]
    fn fails_on_missing_var() {
        let err = resolve("password=${MISSING}", env_from(&[])).unwrap_err();
        assert_eq!(err.0, "MISSING");
    }

    #[test]
    fn template_without_vars_is_identity() {
        let r = resolve("host=x dbname=app", env_from(&[])).unwrap();
        assert_eq!(r, "host=x dbname=app");
    }

    #[test]
    fn invalid_var_shapes_are_literal() {
        // `${lowercase}` doesn't match the [A-Z_][A-Z0-9_]* pattern, so it's
        // left as-is. (PG view bodies may contain unrelated `${…}` text.)
        let r = resolve("foo ${lowercase} bar", env_from(&[])).unwrap();
        assert_eq!(r, "foo ${lowercase} bar");
    }

    #[test]
    fn unclosed_brace_is_literal() {
        let r = resolve("password=${UNCLOSED no end", env_from(&[("UNCLOSED", "x")])).unwrap();
        assert_eq!(r, "password=${UNCLOSED no end");
    }

    #[test]
    fn references_lists_all_vars_in_order() {
        let r = references("host=${H} user=${U} password=${P}");
        assert_eq!(r, vec!["H", "U", "P"]);
    }

    #[test]
    fn references_skips_invalid_names() {
        let r = references("foo ${bad} ${GOOD} ${bad2}");
        assert_eq!(r, vec!["GOOD"]);
    }

    #[test]
    fn underscores_allowed_in_var_names() {
        let r = resolve("password=${MY_VAR_2}", env_from(&[("MY_VAR_2", "x")])).unwrap();
        assert_eq!(r, "password=x");
    }
}
```

- [ ] **Step 2: Build + test**

```bash
cargo test -p pgevolve --lib executor::env_interp
```

Expected: 9 passed.

### Task 4.2: Wire into preflight

- [ ] **Step 1: Add `MissingEnvVar` variant to `ApplyError`**

Locate the `ApplyError` enum (`crates/pgevolve/src/executor/error.rs` or `executor/mod.rs`):

```rust
    /// A plan step's SQL referenced an env var that wasn't set.
    #[error("missing env var ${{{0}}} referenced by step {1}; required for subscription CONNECTION resolution")]
    MissingEnvVar(String, u32),
```

The two fields are the var name and the step number for the error message.

- [ ] **Step 2: Add preflight check**

In `crates/pgevolve/src/executor/preflight.rs`, after the existing preflight checks but before the first DB connection:

```rust
/// Walk every step's SQL; resolve `${VAR}` references against process env.
/// Fail with `MissingEnvVar` for the first unresolved reference.
///
/// Resolution happens *here* (not at plan render time) so plan.sql on disk
/// always stores the unresolved form. The resolved SQL is recomputed at
/// execute time from the same env, ensuring secrets never persist.
pub fn check_env_vars_resolvable(plan: &pgevolve_core::plan::Plan) -> Result<(), ApplyError> {
    use crate::executor::env_interp;
    for group in &plan.groups {
        for step in &group.steps {
            // Only relevant for steps that may contain CONNECTION strings.
            // For now this is just CreateSubscription and AlterSubscriptionConnection;
            // the helper scans every step uniformly — cost is O(SQL bytes).
            let refs = env_interp::references(&step.sql);
            for var in refs {
                if std::env::var(&var).is_err() {
                    return Err(ApplyError::MissingEnvVar(var, step.step_no));
                }
            }
        }
    }
    Ok(())
}
```

Wire the call into the preflight pipeline (find the existing `pub async fn preflight(...)` or equivalent; add `check_env_vars_resolvable(plan)?;` at the top, before any DB I/O).

- [ ] **Step 3: Modify executor's SQL resolution at execute time**

In the per-step execute path (search for where a `RawStep.sql` is sent to PG via `client.execute` or `batch_execute`), wrap the SQL through the interpolator:

```rust
let resolved_sql = env_interp::resolve(&step.sql, |k| std::env::var(k).ok())
    .map_err(|e| ApplyError::MissingEnvVar(e.0, step.step_no))?;
client.batch_execute(&resolved_sql).await?;
```

Preflight should have caught missing vars, but defensive resolution here protects against a race (env mutated between preflight and execute).

- [ ] **Step 4: Add an integration test**

`crates/pgevolve/tests/env_interp_integration.rs`:

```rust
//! Preflight env-var resolution: missing var must error before any DB connection.

use pgevolve_core::plan::{Plan, RawStep, StepKind, TransactionConstraint, TransactionGroup};
// adapt imports to actual crate layout

#[test]
fn preflight_fails_on_missing_env_var() {
    let step = RawStep {
        step_no: 1,
        kind: StepKind::CreateSubscription,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![],
        sql: "CREATE SUBSCRIPTION s CONNECTION 'host=x password=${REPL_PASSWORD_TEST_THAT_DOES_NOT_EXIST}' PUBLICATION p;".into(),
        transactional: TransactionConstraint::InTransaction,
    };
    let plan = build_test_plan(vec![step]);

    // Ensure the env var is NOT set.
    std::env::remove_var("REPL_PASSWORD_TEST_THAT_DOES_NOT_EXIST");

    let err = pgevolve::executor::preflight::check_env_vars_resolvable(&plan).unwrap_err();
    assert!(format!("{err}").contains("REPL_PASSWORD_TEST_THAT_DOES_NOT_EXIST"));
}

#[test]
fn preflight_passes_when_env_var_set() {
    let step = RawStep {
        step_no: 1,
        kind: StepKind::CreateSubscription,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![],
        sql: "CREATE SUBSCRIPTION s CONNECTION 'host=x password=${ENV_SET_FOR_TEST}' PUBLICATION p;".into(),
        transactional: TransactionConstraint::InTransaction,
    };
    let plan = build_test_plan(vec![step]);

    std::env::set_var("ENV_SET_FOR_TEST", "value");
    assert!(pgevolve::executor::preflight::check_env_vars_resolvable(&plan).is_ok());
    std::env::remove_var("ENV_SET_FOR_TEST");
}

fn build_test_plan(steps: Vec<RawStep>) -> Plan {
    // Smallest possible Plan literal. Adapt to actual Plan API.
    // Mirror the pattern in tests/common/mod.rs or other Plan-constructing tests.
    todo!("port from existing test helper")
}
```

The `build_test_plan` helper should mirror whatever the existing test suite uses to construct a `Plan` literal without going through the full parse-diff-rewrite pipeline. Read `crates/pgevolve/tests/common/mod.rs` for the convention.

- [ ] **Step 5: Verify + commit**

```bash
cargo test -p pgevolve --lib executor::env_interp
cargo test -p pgevolve --test env_interp_integration
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Commit:

```bash
git add crates/pgevolve/src/executor/ crates/pgevolve/tests/env_interp_integration.rs
git commit -m "$(cat <<'EOF'
feat(executor): ${VAR} env-var interpolation for SUBSCRIPTION CONNECTION

New env_interp module: resolve("template", env) substitutes ${NAME}
references against a function-supplied env. Variable names match
[A-Z_][A-Z0-9_]* only; invalid shapes left literal so unrelated $-syntax
(e.g., PL/pgSQL $1) doesn't accidentally trigger.

Preflight scans every plan step's SQL and refuses to apply if any
${VAR} reference is unset — fails before any DB connection. plan.sql
on disk always contains the unresolved form; the resolved SQL is
recomputed at execute time from process env, never persisted.

Stage 4 of docs/superpowers/plans/2026-05-26-subscriptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 5 — Catalog reader

Read `pg_subscription` per PG version. Superuser-only; non-super connections get an empty list + a `DriftReport::UnreadableSubscriptions` warning.

**Files created:** `crates/pgevolve-core/src/catalog/subscriptions.rs`, `crates/pgevolve-core/src/catalog/assemble/subscriptions.rs`.
**Files modified:** `crates/pgevolve-core/src/catalog/queries/{shared,pg14}.rs`, `crates/pgevolve-core/src/catalog/mod.rs`.

**Spec ref:** "Catalog reader".

### Task 5.1: Per-version SQL strings

- [ ] **Step 1: Add to `crates/pgevolve-core/src/catalog/queries/shared.rs`**

```rust
/// Subscriptions (PG 15+/16+/17+ surface).
/// pg_subscription is superuser-readable only; non-super connections see
/// empty rows. The wrapping query catches the permission error and surfaces
/// it as a DriftReport entry rather than aborting catalog read.
pub const SUBSCRIPTIONS_QUERY: &str = "\
    SELECT \
        s.oid::bigint AS oid, \
        s.subname::text AS name, \
        coalesce(a.rolname, '') AS owner, \
        s.subenabled AS enabled, \
        s.subconninfo::text AS connection, \
        coalesce(s.subslotname::text, '') AS slot_name, \
        s.subsynccommit::text AS synchronous_commit, \
        s.subpublications::text[] AS publications, \
        s.subbinary AS binary, \
        s.substream::text AS streaming, \
        s.subtwophasestate::text AS two_phase_state, \
        s.subdisableonerr AS disable_on_error, \
        s.subpasswordrequired AS password_required, \
        s.subrunasowner AS run_as_owner, \
        s.suborigin::text AS origin, \
        s.subfailover AS failover, \
        coalesce(d.description, '') AS comment \
    FROM pg_subscription s \
    JOIN pg_authid a ON a.oid = s.subowner \
    LEFT JOIN pg_description d \
        ON d.classoid = 'pg_subscription'::regclass AND d.objoid = s.oid AND d.objsubid = 0 \
    ORDER BY s.subname";
```

- [ ] **Step 2: Override in `crates/pgevolve-core/src/catalog/queries/pg14.rs`**

PG 14 lacks `subdisableonerr` (added PG 15), `subpasswordrequired`/`subrunasowner`/`suborigin` (added PG 16), and `subfailover` (added PG 17). The streaming column `substream` is `bool` in PG 14, becoming `text` (`'f'`/`'t'`/`'p'`) in PG 16+ — normalize to text via `::text` cast for uniform parsing.

```rust
pub const SUBSCRIPTIONS_QUERY_PG14: &str = "\
    SELECT \
        s.oid::bigint AS oid, \
        s.subname::text AS name, \
        coalesce(a.rolname, '') AS owner, \
        s.subenabled AS enabled, \
        s.subconninfo::text AS connection, \
        coalesce(s.subslotname::text, '') AS slot_name, \
        s.subsynccommit::text AS synchronous_commit, \
        s.subpublications::text[] AS publications, \
        s.subbinary AS binary, \
        s.substream::text AS streaming, \
        s.subtwophasestate::text AS two_phase_state, \
        NULL::bool AS disable_on_error, \
        NULL::bool AS password_required, \
        NULL::bool AS run_as_owner, \
        NULL::text AS origin, \
        NULL::bool AS failover, \
        coalesce(d.description, '') AS comment \
    FROM pg_subscription s \
    JOIN pg_authid a ON a.oid = s.subowner \
    LEFT JOIN pg_description d \
        ON d.classoid = 'pg_subscription'::regclass AND d.objoid = s.oid AND d.objsubid = 0 \
    ORDER BY s.subname";
```

Per-version dispatch in `queries/mod.rs`:

```rust
            CatalogQuery::Subscriptions => match major {
                14 => pg14::SUBSCRIPTIONS_QUERY_PG14,
                _ => shared::SUBSCRIPTIONS_QUERY,
            },
```

Plus add `Subscriptions` to the `CatalogQuery` enum.

### Task 5.2: Decoder module

- [ ] **Step 1: Write `crates/pgevolve-core/src/catalog/subscriptions.rs`**

```rust
//! Decode pg_subscription rows into `Subscription` IR.
//!
//! Note: pg_subscription is superuser-readable only. The catalog reader
//! catches permission errors at the query layer (returning empty rows)
//! and surfaces the gap via `DriftReport::UnreadableSubscriptions`.

use crate::catalog::error::CatalogError;
use crate::identifier::Identifier;
use crate::ir::subscription::{
    OriginMode, StreamingMode, Subscription, SubscriptionOptions,
};

// Adapt to the actual codebase row type — Stage 5 of v0.3.4 publications
// found that this codebase uses RawRows / Value rather than tokio_postgres
// Row directly. Read crates/pgevolve-core/src/catalog/publications.rs to
// confirm the API shape before writing decode_subscription_row.

pub fn decode_subscription_row(row: &impl RowLike) -> Result<Subscription, CatalogError> {
    let name_str = row.get_text("name")?;
    let name = Identifier::from_unquoted(&name_str)
        .map_err(|e| CatalogError::InvalidIdentifier(name_str.clone(), e.to_string()))?;

    let owner_str = row.get_text("owner")?;
    let owner = if owner_str.is_empty() {
        None
    } else {
        Some(
            Identifier::from_unquoted(&owner_str)
                .map_err(|e| CatalogError::InvalidIdentifier(owner_str.clone(), e.to_string()))?,
        )
    };

    let slot_str = row.get_text("slot_name")?;
    let slot_name = if slot_str.is_empty() {
        None
    } else {
        Some(
            Identifier::from_unquoted(&slot_str)
                .map_err(|e| CatalogError::InvalidIdentifier(slot_str.clone(), e.to_string()))?,
        )
    };

    let pubs_raw: Vec<String> = row.get_text_array("publications")?;
    let publications = pubs_raw
        .into_iter()
        .map(|p| Identifier::from_unquoted(&p).map_err(|e| {
            CatalogError::InvalidIdentifier(p, e.to_string())
        }))
        .collect::<Result<Vec<_>, _>>()?;

    let comment_str = row.get_text("comment")?;
    let comment = if comment_str.is_empty() { None } else { Some(comment_str) };

    let options = SubscriptionOptions {
        enabled: Some(row.get_bool("enabled")?),
        slot_name,
        // pg_subscription doesn't expose CREATE-time-only flags like create_slot
        // and copy_data — they're not stored in the catalog. Both are always None
        // when reading back from catalog.
        create_slot: None,
        copy_data: None,
        synchronous_commit: {
            let s = row.get_text("synchronous_commit")?;
            if s.is_empty() { None } else { Some(s) }
        },
        binary: Some(row.get_bool("binary")?),
        streaming: Some(decode_streaming(&row.get_text("streaming")?)?),
        two_phase: Some(decode_two_phase(&row.get_text("two_phase_state")?)?),
        disable_on_error: row.get_optional_bool("disable_on_error")?,
        password_required: row.get_optional_bool("password_required")?,
        run_as_owner: row.get_optional_bool("run_as_owner")?,
        origin: row
            .get_optional_text("origin")?
            .map(|s| decode_origin(&s))
            .transpose()?,
        failover: row.get_optional_bool("failover")?,
    };

    Ok(Subscription {
        name,
        connection: row.get_text("connection")?,
        publications,
        options,
        owner,
        comment,
    })
}

fn decode_streaming(s: &str) -> Result<StreamingMode, CatalogError> {
    match s {
        "f" => Ok(StreamingMode::Off),
        "t" => Ok(StreamingMode::On),
        "p" => Ok(StreamingMode::Parallel),
        other => Err(CatalogError::DecodeError(format!(
            "unknown substream value: {other:?}"
        ))),
    }
}

fn decode_two_phase(s: &str) -> Result<bool, CatalogError> {
    match s {
        "d" => Ok(false), // disabled
        "e" => Ok(true),  // enabled
        "p" => Ok(true),  // pending — treat as enabled for diff
        other => Err(CatalogError::DecodeError(format!(
            "unknown subtwophasestate value: {other:?}"
        ))),
    }
}

fn decode_origin(s: &str) -> Result<OriginMode, CatalogError> {
    match s.to_ascii_lowercase().as_str() {
        "any" => Ok(OriginMode::Any),
        "none" => Ok(OriginMode::None),
        other => Err(CatalogError::DecodeError(format!(
            "unknown suborigin value: {other:?}"
        ))),
    }
}

// RowLike: adapt to whatever the codebase exposes (RawRow / Value-typed accessor).
// If the existing pattern (per v0.3.4 publications Stage 5) is `Row.get_text(query, key)`
// taking CatalogQuery as first arg, adjust signatures accordingly.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_off() {
        assert_eq!(decode_streaming("f").unwrap(), StreamingMode::Off);
    }
    #[test]
    fn streaming_on() {
        assert_eq!(decode_streaming("t").unwrap(), StreamingMode::On);
    }
    #[test]
    fn streaming_parallel() {
        assert_eq!(decode_streaming("p").unwrap(), StreamingMode::Parallel);
    }
    #[test]
    fn streaming_unknown_errors() {
        assert!(decode_streaming("x").is_err());
    }
    #[test]
    fn two_phase_disabled() {
        assert!(!decode_two_phase("d").unwrap());
    }
    #[test]
    fn two_phase_enabled() {
        assert!(decode_two_phase("e").unwrap());
    }
    #[test]
    fn two_phase_pending_treated_as_enabled() {
        assert!(decode_two_phase("p").unwrap());
    }
    #[test]
    fn origin_any_case_insensitive() {
        assert_eq!(decode_origin("ANY").unwrap(), OriginMode::Any);
        assert_eq!(decode_origin("any").unwrap(), OriginMode::Any);
    }
    #[test]
    fn origin_none() {
        assert_eq!(decode_origin("none").unwrap(), OriginMode::None);
    }
}
```

### Task 5.3: Assembler module

- [ ] **Step 1: Write `crates/pgevolve-core/src/catalog/assemble/subscriptions.rs`**

```rust
//! Orchestrate pg_subscription read into Vec<Subscription>.
//!
//! pg_subscription is superuser-only. If the query errors with insufficient
//! privilege (PG sqlstate 42501), return an empty Vec and append the
//! `UnreadableSubscriptions` drift report so the operator knows what's hidden.

use crate::catalog::error::CatalogError;
use crate::catalog::queries::CatalogQuery;
use crate::catalog::subscriptions::decode_subscription_row;
use crate::ir::catalog::DriftReport;
use crate::ir::subscription::Subscription;

pub fn assemble_subscriptions(
    rows_result: Result<Vec<RawRow>, CatalogError>,
    drift: &mut DriftReport,
) -> Result<Vec<Subscription>, CatalogError> {
    match rows_result {
        Ok(rows) => rows.iter().map(decode_subscription_row).collect(),
        Err(CatalogError::PgError(e)) if e.sqlstate() == Some("42501") => {
            drift.unreadable_subscriptions = true;
            Ok(Vec::new())
        }
        Err(other) => Err(other),
    }
}
```

`DriftReport::unreadable_subscriptions: bool` — add the field to `DriftReport` in `crates/pgevolve-core/src/ir/catalog.rs` (or wherever `DriftReport` is defined).

If `RawRow` / `CatalogError::PgError` / `sqlstate()` aren't exactly those names, adapt — the v0.3.4 Stage 5 implementer adapted similarly.

### Task 5.4: Wire into `read_catalog`

- [ ] **Step 1: Add the call**

In `crates/pgevolve-core/src/catalog/mod.rs`'s `read_catalog`, after the existing object-kind assemblers and before canonicalize:

```rust
let sub_rows = querier.run(CatalogQuery::Subscriptions);
catalog.subscriptions = crate::catalog::assemble::subscriptions::assemble_subscriptions(
    sub_rows, &mut drift,
)?;
```

### Task 5.5: Docker integration test

- [ ] **Step 1: Create `crates/pgevolve-core/tests/subscription_round_trip.rs`**

```rust
//! Round-trip: CREATE SUBSCRIPTION (with enabled=false to avoid network) → read back → assert equal IR.
//! Requires Docker; skips cleanly when unavailable.

#![cfg(all(test, feature = "testkit"))]

use anyhow::Result;
use pgevolve_core::catalog::{CatalogFilter, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::subscription::StreamingMode;
use pgevolve_testkit::PgCatalogQuerier;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

#[tokio::test(flavor = "multi_thread")]
async fn read_subscription_basic() -> Result<()> {
    if !docker_available() { return Ok(()); }
    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let client = pg.connect().await?;

    // Create a publication first so the subscription has something to reference.
    // The subscription points to localhost but enabled=false + create_slot=false
    // means PG never actually tries to connect.
    client.batch_execute(
        "CREATE SCHEMA app; \
         CREATE TABLE app.t (id bigint PRIMARY KEY); \
         CREATE PUBLICATION p FOR TABLE app.t; \
         CREATE SUBSCRIPTION s \
             CONNECTION 'host=127.0.0.1 dbname=postgres user=postgres' \
             PUBLICATION p \
             WITH (enabled = false, create_slot = false, copy_data = false);"
    ).await?;

    let querier = PgCatalogQuerier::new(client)?;
    let filter = CatalogFilter::new(vec![Identifier::from_unquoted("app").unwrap()], vec![])?;
    let (catalog, _) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter)).await??;

    assert_eq!(catalog.subscriptions.len(), 1);
    let s = &catalog.subscriptions[0];
    assert_eq!(s.name.as_str(), "s");
    assert_eq!(s.publications.len(), 1);
    assert_eq!(s.publications[0].as_str(), "p");
    assert_eq!(s.options.enabled, Some(false));
    assert!(s.connection.contains("host=127.0.0.1"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn non_super_connection_yields_empty_with_drift() -> Result<()> {
    if !docker_available() { return Ok(()); }
    // Create a non-super role, connect as that role, verify subscriptions is
    // empty AND DriftReport::unreadable_subscriptions = true.
    // Adapt to whatever the codebase's connection helper supports.
    Ok(())  // implementation when wiring permits
}
```

### Task 5.6: Verify + commit

```bash
cargo build -p pgevolve-core
cargo test -p pgevolve-core --lib catalog::subscriptions
cargo test -p pgevolve-core --test subscription_round_trip
cargo clippy -p pgevolve-core --all-targets -- -D warnings
cargo fmt --all -- --check
```

Commit:

```
feat(catalog): read subscriptions from pg_subscription

Per-PG variants: PG 14 strips PG15+/16+/17+ columns (disable_on_error,
password_required, run_as_owner, origin, failover). PG 14's `substream`
is bool; PG 16+ becomes text — uniform text decoding handles both via
::text cast.

pg_subscription is superuser-only. Non-super connections get an empty
subscriptions: Vec<_> plus DriftReport::unreadable_subscriptions = true
so operators know what's hidden.

subtwophasestate 'd'/'e'/'p' → two_phase Option<bool>: pending ('p') is
treated as Some(true) (matches the eventual state).

Stage 5 of docs/superpowers/plans/2026-05-26-subscriptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Stage 6 — Source parser

Parse `CREATE SUBSCRIPTION` and `ALTER SUBSCRIPTION` into IR. Fold inline `WITH` + subsequent `ALTER` ops into one canonical record per name. Reject operational verbs (REFRESH, SKIP, standalone ENABLE/DISABLE, RENAME).

**Files created:** `crates/pgevolve-core/src/parse/builder/subscription_stmt.rs`.
**Files modified:** `crates/pgevolve-core/src/parse/builder/mod.rs`, `crates/pgevolve-core/src/parse/error.rs`.

**Spec ref:** "Source surface".

### Task 6.1: Add ParseError variants

- [ ] **Step 1: Extend `ParseError`**

```rust
    DuplicateSubscription(Identifier, SourceLocation),
    SubscriptionEmptyConnection(Identifier, SourceLocation),
    SubscriptionEmptyPublications(Identifier, SourceLocation),
    SubscriptionOptionMalformed(Identifier, SourceLocation),
    UnknownSubscriptionOption(String, Identifier, SourceLocation),
    UnknownStreamingMode(String, Identifier, SourceLocation),
    UnknownOriginMode(String, Identifier, SourceLocation),
    SubscriptionRefreshNotSupported(Identifier, SourceLocation),
    SubscriptionSkipNotSupported(Identifier, SourceLocation),
    SubscriptionStandaloneEnableDisableNotSupported(Identifier, SourceLocation),
    SubscriptionRenameNotSupported(Identifier, SourceLocation),
    AlterSubscriptionBeforeCreate(Identifier, SourceLocation),
```

Mirror existing variant style (thiserror with formatted messages).

### Task 6.2: Write the parser module

- [ ] **Step 1: Create `crates/pgevolve-core/src/parse/builder/subscription_stmt.rs`**

Two pub fns:

```rust
pub fn parse_create_subscription(
    stmt: &CreateSubscriptionStmt,
    source_loc: SourceLocation,
    existing: &mut BTreeMap<Identifier, Subscription>,
) -> Result<(), ParseError>

pub fn parse_alter_subscription(
    stmt: &AlterSubscriptionStmt,
    source_loc: SourceLocation,
    existing: &mut BTreeMap<Identifier, Subscription>,
) -> Result<(), ParseError>
```

Key behaviors mirror v0.3.4 PUBLICATION's `publication_stmt.rs`:

1. **CREATE**: parse `subname`, `conninfo` (a string node), `publication` (list of string nodes), `options` (list of DefElems). Build a `Subscription`, insert. Reject duplicates. CONNECTION string stored verbatim — no `${VAR}` resolution.

2. **ALTER**: pg_query exposes `AlterSubscriptionStmt.kind` (an enum: `AlterSubscriptionRefresh`, `AlterSubscriptionAddPublication`, `AlterSubscriptionDropPublication`, `AlterSubscriptionSetPublication`, `AlterSubscriptionConnection`, `AlterSubscriptionEnabled`, `AlterSubscriptionSkip`, `AlterSubscriptionOptions`). Map each to either:
   - Fold into existing IR (ADD/DROP/SET PUBLICATION, CONNECTION, OPTIONS).
   - Reject with the appropriate ParseError (REFRESH, SKIP, standalone ENABLE/DISABLE).

3. **`publish` ... no, that was PUBLICATION**. For SUBSCRIPTION, the `WITH (...)` options are parsed by name:
   - `enabled` (bool)
   - `slot_name` (identifier)
   - `create_slot` (bool)
   - `copy_data` (bool)
   - `synchronous_commit` (text)
   - `binary` (bool)
   - `streaming` (text: `off`/`on`/`parallel`)
   - `two_phase` (bool)
   - `disable_on_error` (bool)
   - `password_required` (bool)
   - `run_as_owner` (bool)
   - `origin` (text: `any`/`none`)
   - `failover` (bool)

Unknown option names → `UnknownSubscriptionOption`.

4. **RENAME**: encoded as `RenameStmt` (not `AlterSubscriptionStmt`). Add a rejection arm in the existing `RenameStmt` dispatcher.

Test list (12+ tests):

- CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p — minimal
- CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p, q — multi-pub
- CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p WITH (enabled=false, binary=true, streaming=parallel)
- CREATE SUBSCRIPTION s CONNECTION 'host=x password=${PWD}' PUBLICATION p — connstr stored verbatim
- ALTER SUBSCRIPTION s ADD PUBLICATION q (folded with prior CREATE)
- ALTER SUBSCRIPTION s DROP PUBLICATION q
- ALTER SUBSCRIPTION s SET (binary = true)
- ALTER SUBSCRIPTION s CONNECTION 'host=y'
- ALTER SUBSCRIPTION s REFRESH PUBLICATION → ParseError::SubscriptionRefreshNotSupported
- ALTER SUBSCRIPTION s SKIP (lsn = '0/0') → ParseError::SubscriptionSkipNotSupported
- ALTER SUBSCRIPTION s ENABLE → ParseError::SubscriptionStandaloneEnableDisableNotSupported
- ALTER SUBSCRIPTION s RENAME TO t → ParseError::SubscriptionRenameNotSupported
- CREATE SUBSCRIPTION s CONNECTION '' PUBLICATION p → ParseError::SubscriptionEmptyConnection
- CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION — no pubs → ParseError::SubscriptionEmptyPublications
- CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p WITH (streaming = bogus) → ParseError::UnknownStreamingMode

### Task 6.3: Wire dispatch + state

- [ ] **Step 1: Add `pub mod subscription_stmt;` and dispatch arms in `parse/builder/mod.rs`**

```rust
            node::Node::CreateSubscriptionStmt(s) => {
                subscription_stmt::parse_create_subscription(s, loc, &mut subscriptions)?;
            }
            node::Node::AlterSubscriptionStmt(s) => {
                subscription_stmt::parse_alter_subscription(s, loc, &mut subscriptions)?;
            }
```

Thread `mut subscriptions: BTreeMap<Identifier, Subscription>` through the parser state and copy `subscriptions.into_values().collect()` into `catalog.subscriptions` at the end.

- [ ] **Step 2: Add RENAME rejection arm**

Find the existing `RenameStmt` dispatcher (similar arm for PUBLICATION was added in v0.3.4 Stage 6). Add:

```rust
ObjectType::ObjectSubscription => {
    return Err(ParseError::SubscriptionRenameNotSupported(
        Identifier::from_unquoted(&rename.subname)?,
        loc,
    ));
}
```

### Task 6.4: Verify + commit

```bash
cargo test -p pgevolve-core --lib parse::builder::subscription_stmt
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

```
feat(parse): CREATE SUBSCRIPTION and ALTER SUBSCRIPTION

Folds CREATE … CONNECTION … PUBLICATION … WITH (…) and subsequent
ALTER ADD/DROP/SET PUBLICATION / CONNECTION / SET (…) into one
canonical Subscription per name. CONNECTION string stored verbatim
including ${VAR} placeholders — resolution happens only at apply.

Rejects: REFRESH PUBLICATION, SKIP, standalone ENABLE/DISABLE, RENAME.

Stage 6 of docs/superpowers/plans/2026-05-26-subscriptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Stage 7 — Differ

Granular per-subscription diff with 8 new Change variants. Connection strings compared **modulo password** via a small libpq tokenizer.

**Files created:** `crates/pgevolve-core/src/diff/subscriptions.rs`.
**Files modified:** `crates/pgevolve-core/src/diff/change.rs`, `crates/pgevolve-core/src/diff/mod.rs`, `crates/pgevolve-core/src/diff/owner_op.rs`.

**Spec ref:** "Differ".

### Task 7.1: Add Change variants

- [ ] **Step 1: Extend `Change` enum in `change.rs`**

```rust
    /// `CREATE SUBSCRIPTION ...`
    CreateSubscription(crate::ir::subscription::Subscription),
    /// `DROP SUBSCRIPTION ...` — destructive.
    DropSubscription { name: crate::identifier::Identifier },
    /// `ALTER SUBSCRIPTION s CONNECTION '...'`
    AlterSubscriptionConnection {
        name: crate::identifier::Identifier,
        new_connection: String,
    },
    /// `ALTER SUBSCRIPTION s ADD PUBLICATION p`
    AlterSubscriptionAddPublication {
        name: crate::identifier::Identifier,
        publication: crate::identifier::Identifier,
    },
    /// `ALTER SUBSCRIPTION s DROP PUBLICATION p`
    AlterSubscriptionDropPublication {
        name: crate::identifier::Identifier,
        publication: crate::identifier::Identifier,
    },
    /// Reserved: pgevolve never emits this (granular ADD/DROP only), but
    /// parser accepts source `ALTER SUBSCRIPTION s SET PUBLICATION …` for
    /// normalizing into the IR's publications field. The variant exists so
    /// kind_name / parse_kind_name round-trip every legal StepKind name.
    AlterSubscriptionSetPublication {
        name: crate::identifier::Identifier,
        publications: Vec<crate::identifier::Identifier>,
    },
    /// `ALTER SUBSCRIPTION s SET (option = value, ...)` — sparse-delta.
    AlterSubscriptionSetOptions {
        name: crate::identifier::Identifier,
        options: crate::ir::subscription::SubscriptionOptions,  // sparse: only changed fields set
    },
    /// `COMMENT ON SUBSCRIPTION s IS '...'`
    CommentOnSubscription {
        name: crate::identifier::Identifier,
        comment: Option<String>,
    },
```

### Task 7.2: Add OwnerObjectKind::Subscription

```rust
    Subscription,
```

Plus the Display arm:

```rust
            OwnerObjectKind::Subscription => write!(f, "SUBSCRIPTION"),
```

### Task 7.3: Implement the differ

- [ ] **Step 1: Write `crates/pgevolve-core/src/diff/subscriptions.rs`**

```rust
//! Differ for subscriptions. Pair by name; per-subscription granular diff.
//!
//! CONNECTION strings compare *modulo password*: a tiny libpq-style
//! tokenizer strips `password=…` from both sides before text-compare.
//! All other connstr keys participate in diff normally.

use std::collections::BTreeMap;

use crate::diff::change::{Change, ChangeSet};
use crate::diff::destructiveness::Destructiveness;
use crate::diff::owner_op::{AlterObjectOwner, OwnerObjectKind};
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::subscription::{Subscription, SubscriptionOptions};

pub fn diff_subscriptions(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<&Identifier, &Subscription> =
        target.subscriptions.iter().map(|s| (&s.name, s)).collect();
    let source_map: BTreeMap<&Identifier, &Subscription> =
        source.subscriptions.iter().map(|s| (&s.name, s)).collect();

    // Creates.
    for (name, src) in &source_map {
        if !target_map.contains_key(name) {
            out.push(
                Change::CreateSubscription((*src).clone()),
                Destructiveness::Safe,
            );
        }
    }

    // Drops: lenient — no auto-drop. Surfaces via unmanaged-subscription lint.
    // (No code in this loop.)

    // Modifies.
    for (name, src) in &source_map {
        let Some(tgt) = target_map.get(name) else { continue; };
        diff_one(tgt, src, out);
    }
}

fn diff_one(target: &Subscription, source: &Subscription, out: &mut ChangeSet) {
    // CONNECTION (modulo password).
    if connection_differs_ignoring_password(&target.connection, &source.connection) {
        out.push(
            Change::AlterSubscriptionConnection {
                name: source.name.clone(),
                new_connection: source.connection.clone(),
            },
            Destructiveness::Safe,
        );
    }

    // Publications: granular ADD/DROP.
    let t_pubs: std::collections::BTreeSet<_> = target.publications.iter().collect();
    let s_pubs: std::collections::BTreeSet<_> = source.publications.iter().collect();
    for added in s_pubs.difference(&t_pubs) {
        out.push(
            Change::AlterSubscriptionAddPublication {
                name: source.name.clone(),
                publication: (*added).clone(),
            },
            Destructiveness::Safe,
        );
    }
    for dropped in t_pubs.difference(&s_pubs) {
        out.push(
            Change::AlterSubscriptionDropPublication {
                name: source.name.clone(),
                publication: (*dropped).clone(),
            },
            Destructiveness::Safe,
        );
    }

    // Options: sparse delta.
    let opts_delta = options_delta(&target.options, &source.options);
    if !options_delta_is_empty(&opts_delta) {
        out.push(
            Change::AlterSubscriptionSetOptions {
                name: source.name.clone(),
                options: opts_delta,
            },
            Destructiveness::Safe,
        );
    }

    // Owner (v0.3.1 lenient).
    if let Some(s_owner) = &source.owner
        && target.owner.as_ref() != Some(s_owner)
    {
        let from = target.owner.clone().unwrap_or_else(|| {
            Identifier::from_unquoted("__unknown_owner__").expect("literal valid")
        });
        out.push(
            Change::AlterObjectOwner(AlterObjectOwner {
                kind: OwnerObjectKind::Subscription,
                qname: QualifiedName::new(
                    Identifier::from_unquoted("__cluster__").expect("literal valid"),
                    source.name.clone(),
                ),
                signature: String::new(),
                from,
                to: s_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Comment.
    if target.comment != source.comment {
        out.push(
            Change::CommentOnSubscription {
                name: source.name.clone(),
                comment: source.comment.clone(),
            },
            Destructiveness::Safe,
        );
    }
}

/// Compare two libpq connection strings ignoring the `password` key.
///
/// Tokenizes by `key=value` pairs separated by whitespace. Values may be
/// single-quoted with backslash-escaping for embedded quotes/backslashes
/// (libpq's documented syntax).
///
/// `${VAR}` placeholders are compared literally — a change in the env-var
/// name DOES trigger a diff (legitimate config change; operator should
/// approve via plan review).
fn connection_differs_ignoring_password(a: &str, b: &str) -> bool {
    tokenize_dropping_password(a) != tokenize_dropping_password(b)
}

fn tokenize_dropping_password(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        // Skip whitespace.
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        if chars.peek().is_none() {
            break;
        }
        // Read key.
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' {
                chars.next();
                break;
            }
            key.push(c);
            chars.next();
        }
        // Read value: either quoted or unquoted.
        let mut value = String::new();
        if chars.peek() == Some(&'\'') {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(esc) = chars.next() {
                            value.push(esc);
                        }
                    }
                    '\'' => break,
                    other => value.push(other),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                value.push(c);
                chars.next();
            }
        }
        if key.eq_ignore_ascii_case("password") {
            continue;
        }
        out.push((key, value));
    }
    out.sort();
    out
}

fn options_delta(target: &SubscriptionOptions, source: &SubscriptionOptions) -> SubscriptionOptions {
    macro_rules! delta_field {
        ($field:ident) => {{
            if source.$field.is_some() && target.$field != source.$field {
                source.$field.clone()
            } else {
                None
            }
        }};
    }
    // create_slot and copy_data are PG-CREATE-only — no ALTER SET variant.
    // They never appear in a delta; emitting them would produce SQL PG rejects.
    SubscriptionOptions {
        enabled:            delta_field!(enabled),
        slot_name:          delta_field!(slot_name),
        create_slot:        None,    // CREATE-only; intentionally never diffed.
        copy_data:          None,    // CREATE-only; intentionally never diffed.
        synchronous_commit: delta_field!(synchronous_commit),
        binary:             delta_field!(binary),
        streaming:          delta_field!(streaming),
        two_phase:          delta_field!(two_phase),
        disable_on_error:   delta_field!(disable_on_error),
        password_required:  delta_field!(password_required),
        run_as_owner:       delta_field!(run_as_owner),
        origin:             delta_field!(origin),
        failover:           delta_field!(failover),
    }
}

fn options_delta_is_empty(d: &SubscriptionOptions) -> bool {
    d.enabled.is_none()
        && d.slot_name.is_none()
        && d.create_slot.is_none()
        && d.copy_data.is_none()
        && d.synchronous_commit.is_none()
        && d.binary.is_none()
        && d.streaming.is_none()
        && d.two_phase.is_none()
        && d.disable_on_error.is_none()
        && d.password_required.is_none()
        && d.run_as_owner.is_none()
        && d.origin.is_none()
        && d.failover.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    // 12+ unit tests covering:
    // - identical subscriptions → empty diff
    // - source has it, target doesn't → CreateSubscription
    // - target has it, source doesn't → NO change (lenient)
    // - connection differs in non-password key → AlterSubscriptionConnection
    // - connection differs ONLY in password → no change
    // - publication added → AlterSubscriptionAddPublication
    // - publication removed → AlterSubscriptionDropPublication
    // - binary changed → AlterSubscriptionSetOptions with only binary set
    // - multiple options changed → single AlterSubscriptionSetOptions with all changed fields
    // - source enabled=None, catalog enabled=true → no diff (lenient)
    // - owner change → AlterObjectOwner
    // - comment change → CommentOnSubscription

    #[test]
    fn tokenize_drops_password() {
        let a = tokenize_dropping_password("host=x user=u password=secret dbname=app");
        let b = tokenize_dropping_password("host=x user=u password=different dbname=app");
        assert_eq!(a, b);
    }

    #[test]
    fn tokenize_preserves_other_keys() {
        let a = tokenize_dropping_password("host=x user=u password=p");
        let b = tokenize_dropping_password("host=y user=u password=p");
        assert_ne!(a, b);
    }

    #[test]
    fn tokenize_handles_quoted_values() {
        let a = tokenize_dropping_password("host='db.example.com' password=p");
        assert_eq!(a, vec![("host".to_string(), "db.example.com".to_string())]);
    }

    #[test]
    fn tokenize_handles_escapes_in_quoted_values() {
        let a = tokenize_dropping_password(r"host='db\'.com' password=p");
        assert_eq!(a, vec![("host".to_string(), "db'.com".to_string())]);
    }

    #[test]
    fn tokenize_case_insensitive_password_key() {
        let a = tokenize_dropping_password("host=x PASSWORD=secret");
        let b = tokenize_dropping_password("host=x Password=other");
        assert_eq!(a, b);
    }
}
```

### Task 7.4: Wire into top-level diff + stub arms

- [ ] **Step 1: Top-level diff call**

In `crates/pgevolve-core/src/diff/mod.rs`'s top-level `diff` function:

```rust
crate::diff::subscriptions::diff_subscriptions(target, source, &mut changes);
```

After existing per-object-kind diff calls. Add `pub mod subscriptions;`.

- [ ] **Step 2: 8 stub arms in 4 Change consumers**

Same pattern as v0.3.4 PUBLICATION Stage 7. Add a combined no-op arm in:
- `plan/rewrite/mod.rs` emit dispatcher (Stage 8 fills in)
- `plan/ordering.rs::partition` (`CreateSubscription` → creates; `DropSubscription` → drops; other 6 → modifies)
- `plan/ordering.rs::change_node` (return placeholder `NodeId::Schema("__sub_placeholder__")` until Stage 8 wires `NodeId::Subscription`)
- `commands/diff.rs::print_human` + `change_kind_name` (placeholder strings)

### Task 7.5: Verify + commit

```bash
cargo test -p pgevolve-core --lib diff::subscriptions
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

```
feat(diff): subscriptions — 8 granular Change variants

Pair by name; per-subscription granular diff. CONNECTION strings
compare modulo password via a small libpq-style tokenizer (key=value
pairs with quoted-value + escape handling). Publication list diffs
emit per-pub AlterSubscriptionAddPublication / DropPublication.
Options diff is sparse-delta — only changed fields flow into the
single AlterSubscriptionSetOptions.

Owner uses v0.3.1 lenient pattern. Target-only subscriptions do
NOT emit DropSubscription (lenient). 8 stub emit arms in 4 Change
consumers let the workspace compile.

Stage 7 of docs/superpowers/plans/2026-05-26-subscriptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Stage 8 — Render + emit + 8 StepKinds + NodeId

Fill in SQL helpers, StepKind variants, real emit, NodeId. Mirror v0.3.4 Stage 8 verbatim where shapes are identical.

**Files created:** `crates/pgevolve-core/src/plan/rewrite/subscriptions.rs`.
**Files modified:** `crates/pgevolve-core/src/plan/raw_step.rs`, `crates/pgevolve-core/src/plan/plan.rs`, `crates/pgevolve-core/src/plan/rewrite/mod.rs`, `crates/pgevolve-core/src/plan/edges.rs`, `crates/pgevolve/src/commands/diff.rs`.

### Task 8.1: 8 StepKind variants + kind_name

- [ ] **Step 1: Extend `StepKind` in `raw_step.rs`**

```rust
    CreateSubscription,
    DropSubscription,
    AlterSubscriptionConnection,
    AlterSubscriptionAddPublication,
    AlterSubscriptionDropPublication,
    AlterSubscriptionSetPublication,
    AlterSubscriptionSetOptions,
    CommentOnSubscription,
```

Extend the round-trip serialization test (every variant must appear).

- [ ] **Step 2: `kind_name` / `parse_kind_name` in `plan.rs`**

```
CreateSubscription              <-> "create_subscription"
DropSubscription                <-> "drop_subscription"
AlterSubscriptionConnection     <-> "alter_subscription_connection"
AlterSubscriptionAddPublication <-> "alter_subscription_add_publication"
AlterSubscriptionDropPublication <-> "alter_subscription_drop_publication"
AlterSubscriptionSetPublication <-> "alter_subscription_set_publication"
AlterSubscriptionSetOptions     <-> "alter_subscription_set_options"
CommentOnSubscription           <-> "comment_on_subscription"
```

### Task 8.2: SQL helpers

- [ ] **Step 1: Create `crates/pgevolve-core/src/plan/rewrite/subscriptions.rs`**

```rust
//! SQL rendering for SUBSCRIPTION operations.

use crate::identifier::Identifier;
use crate::ir::subscription::{
    OriginMode, StreamingMode, Subscription, SubscriptionOptions,
};

/// `CREATE SUBSCRIPTION s CONNECTION '...' PUBLICATION ... WITH (...);`
#[must_use]
pub fn create_subscription(s: &Subscription) -> String {
    let mut out = format!("CREATE SUBSCRIPTION {} ", s.name.render_sql());
    out.push_str(&format!("CONNECTION '{}' ", escape_sql_literal(&s.connection)));
    out.push_str("PUBLICATION ");
    let pubs: Vec<String> = s.publications.iter().map(|p| p.render_sql()).collect();
    out.push_str(&pubs.join(", "));
    let with = render_with_options(&s.options);
    if !with.is_empty() {
        out.push(' ');
        out.push_str(&with);
    }
    out.push(';');
    out
}

#[must_use]
pub fn drop_subscription(name: &Identifier) -> String {
    format!("DROP SUBSCRIPTION {};", name.render_sql())
}

#[must_use]
pub fn alter_subscription_connection(name: &Identifier, new_connection: &str) -> String {
    format!(
        "ALTER SUBSCRIPTION {} CONNECTION '{}';",
        name.render_sql(),
        escape_sql_literal(new_connection),
    )
}

#[must_use]
pub fn alter_subscription_add_publication(name: &Identifier, publication: &Identifier) -> String {
    format!(
        "ALTER SUBSCRIPTION {} ADD PUBLICATION {};",
        name.render_sql(),
        publication.render_sql(),
    )
}

#[must_use]
pub fn alter_subscription_drop_publication(name: &Identifier, publication: &Identifier) -> String {
    format!(
        "ALTER SUBSCRIPTION {} DROP PUBLICATION {};",
        name.render_sql(),
        publication.render_sql(),
    )
}

#[must_use]
pub fn alter_subscription_set_publication(name: &Identifier, publications: &[Identifier]) -> String {
    let pubs: Vec<String> = publications.iter().map(|p| p.render_sql()).collect();
    format!(
        "ALTER SUBSCRIPTION {} SET PUBLICATION {};",
        name.render_sql(),
        pubs.join(", "),
    )
}

#[must_use]
pub fn alter_subscription_set_options(name: &Identifier, opts: &SubscriptionOptions) -> String {
    // ALTER SUBSCRIPTION SET (...) excludes create_slot and copy_data which
    // are CREATE-only PG options. The differ's options_delta also skips
    // them, so this is a defense-in-depth filter.
    let body = render_options_body_for_alter(opts);
    format!("ALTER SUBSCRIPTION {} SET ({});", name.render_sql(), body)
}

#[must_use]
pub fn comment_on_subscription(name: &Identifier, comment: Option<&str>) -> String {
    let body = comment.map_or_else(
        || "NULL".to_string(),
        |c| format!("'{}'", c.replace('\'', "''")),
    );
    format!("COMMENT ON SUBSCRIPTION {} IS {};", name.render_sql(), body)
}

// ---- private helpers ----

fn render_with_options(opts: &SubscriptionOptions) -> String {
    let body = render_options_body_for_create(opts);
    if body.is_empty() {
        String::new()
    } else {
        format!("WITH ({body})")
    }
}

/// Render all WITH options (used by CREATE SUBSCRIPTION). Includes
/// CREATE-only `create_slot` and `copy_data`.
fn render_options_body_for_create(opts: &SubscriptionOptions) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = opts.enabled            { parts.push(format!("enabled = {v}")); }
    if let Some(ref v) = opts.slot_name      { parts.push(format!("slot_name = {}", v.render_sql())); }
    if let Some(v) = opts.create_slot        { parts.push(format!("create_slot = {v}")); }
    if let Some(v) = opts.copy_data          { parts.push(format!("copy_data = {v}")); }
    push_alterable_options(opts, &mut parts);
    parts.join(", ")
}

/// Render only ALTER-able WITH options (used by ALTER SUBSCRIPTION SET).
/// Omits CREATE-only `create_slot` and `copy_data`.
fn render_options_body_for_alter(opts: &SubscriptionOptions) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = opts.enabled            { parts.push(format!("enabled = {v}")); }
    if let Some(ref v) = opts.slot_name      { parts.push(format!("slot_name = {}", v.render_sql())); }
    push_alterable_options(opts, &mut parts);
    parts.join(", ")
}

/// Shared option-rendering for the post-slot_name fields (all alterable).
fn push_alterable_options(opts: &SubscriptionOptions, parts: &mut Vec<String>) {
    if let Some(ref v) = opts.synchronous_commit {
        parts.push(format!("synchronous_commit = '{}'", v.replace('\'', "''")));
    }
    if let Some(v) = opts.binary             { parts.push(format!("binary = {v}")); }
    if let Some(ref v) = opts.streaming      { parts.push(format!("streaming = {}", streaming_keyword(*v))); }
    if let Some(v) = opts.two_phase          { parts.push(format!("two_phase = {v}")); }
    if let Some(v) = opts.disable_on_error   { parts.push(format!("disable_on_error = {v}")); }
    if let Some(v) = opts.password_required  { parts.push(format!("password_required = {v}")); }
    if let Some(v) = opts.run_as_owner       { parts.push(format!("run_as_owner = {v}")); }
    if let Some(ref v) = opts.origin         { parts.push(format!("origin = {}", origin_keyword(*v))); }
    if let Some(v) = opts.failover           { parts.push(format!("failover = {v}")); }
}

const fn streaming_keyword(m: StreamingMode) -> &'static str {
    match m {
        StreamingMode::Off => "off",
        StreamingMode::On => "on",
        StreamingMode::Parallel => "parallel",
    }
}

const fn origin_keyword(m: OriginMode) -> &'static str {
    match m {
        OriginMode::Any => "any",
        OriginMode::None => "none",
    }
}

fn escape_sql_literal(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;
    // 8+ unit tests covering:
    // - create_subscription minimal (no WITH)
    // - create_subscription with all options
    // - create_subscription with ${VAR} in connection
    // - alter_subscription_set_options with single field
    // - alter_subscription_set_options with multiple fields
    // - alter_subscription_add_publication
    // - drop_subscription
    // - comment_on_subscription with NULL
    // - streaming_keyword round-trip
    // - origin_keyword round-trip
}
```

### Task 8.3: Replace Stage 7 stub arms with real emit

- [ ] **Step 1: Update `plan/rewrite/mod.rs`**

Replace the combined Stage 7 stub with 8 real emit arms:

```rust
        Change::CreateSubscription(s) => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateSubscription,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::create_subscription(s),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(c) = &s.comment {
                raw_steps.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnSubscription,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![],
                    sql: subscriptions::comment_on_subscription(&s.name, Some(c)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        Change::DropSubscription { name } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::DropSubscription,
                destructive: true,
                destructive_reason: destructive_reason.clone(),
                intent_id: None,
                targets: vec![],
                sql: subscriptions::drop_subscription(name),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::AlterSubscriptionConnection { name, new_connection } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterSubscriptionConnection,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::alter_subscription_connection(name, new_connection),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::AlterSubscriptionAddPublication { name, publication } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterSubscriptionAddPublication,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::alter_subscription_add_publication(name, publication),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::AlterSubscriptionDropPublication { name, publication } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterSubscriptionDropPublication,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::alter_subscription_drop_publication(name, publication),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::AlterSubscriptionSetPublication { name, publications } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterSubscriptionSetPublication,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::alter_subscription_set_publication(name, publications),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::AlterSubscriptionSetOptions { name, options } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterSubscriptionSetOptions,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::alter_subscription_set_options(name, options),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::CommentOnSubscription { name, comment } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnSubscription,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: subscriptions::comment_on_subscription(name, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
```

Add `pub mod subscriptions;` to `plan/rewrite/mod.rs`.

### Task 8.4: NodeId::Subscription + planner tier rule

- [ ] **Step 1: Extend `NodeId` in `plan/edges.rs`**

```rust
    Subscription(Identifier),
```

Update any `impl NodeId` (Display, etc.) and replace Stage 7's `change_node` placeholder.

- [ ] **Step 2: Tier rule — subscriptions create last, drop first**

Find the planner's ordering tier rules (search for where partition / view orderings apply). Add a rule:

```rust
// Subscriptions create after every other object and drop before every other object.
// They cross-reference publications in a different cluster; the local dep graph
// has no anchor. The tier rule minimizes the window where a referenced object
// might be missing.
```

The mechanism varies by codebase — could be a sort-key tie-breaker, an explicit second-pass append, or a dedicated tier enum. Read `plan/ordering.rs` for the existing convention; add the rule in the lowest-disruption place.

### Task 8.5: Update CLI display

- [ ] **Step 1: Update `commands/diff.rs::print_human` + `change_kind_name`**

```rust
        Change::CreateSubscription(s)
            => format!("+ CREATE SUBSCRIPTION {}", s.name),
        Change::DropSubscription { name }
            => format!("- DROP SUBSCRIPTION {name}"),
        Change::AlterSubscriptionConnection { name, .. }
            => format!("~ ALTER SUBSCRIPTION {name} CONNECTION '...'"),
        Change::AlterSubscriptionAddPublication { name, publication }
            => format!("~ ALTER SUBSCRIPTION {name} ADD PUBLICATION {publication}"),
        Change::AlterSubscriptionDropPublication { name, publication }
            => format!("~ ALTER SUBSCRIPTION {name} DROP PUBLICATION {publication}"),
        Change::AlterSubscriptionSetPublication { name, publications }
            => format!("~ ALTER SUBSCRIPTION {name} SET PUBLICATION ({} items)", publications.len()),
        Change::AlterSubscriptionSetOptions { name, .. }
            => format!("~ ALTER SUBSCRIPTION {name} SET (...)"),
        Change::CommentOnSubscription { name, .. }
            => format!("~ COMMENT ON SUBSCRIPTION {name}"),
```

`change_kind_name`:

```rust
        Change::CreateSubscription(_)              => "create_subscription",
        Change::DropSubscription { .. }            => "drop_subscription",
        Change::AlterSubscriptionConnection { .. } => "alter_subscription_connection",
        Change::AlterSubscriptionAddPublication { .. } => "alter_subscription_add_publication",
        Change::AlterSubscriptionDropPublication { .. } => "alter_subscription_drop_publication",
        Change::AlterSubscriptionSetPublication { .. } => "alter_subscription_set_publication",
        Change::AlterSubscriptionSetOptions { .. } => "alter_subscription_set_options",
        Change::CommentOnSubscription { .. }       => "comment_on_subscription",
```

### Task 8.6: Verify + commit

```bash
cargo test -p pgevolve-core --lib plan::rewrite::subscriptions
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

```
feat(plan): subscriptions render + emit + 8 new StepKinds

plan::rewrite::subscriptions renders CREATE/DROP/ALTER SUBSCRIPTION
SQL. 8 new StepKind variants. NodeId::Subscription added; planner
tier rule schedules subscriptions create-last, drop-first (no local
dep edges; cross-cluster references).

Stage 8 of docs/superpowers/plans/2026-05-26-subscriptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Stage 9 — Lint rules (4)

Four rules: three drift/correctness + one hard-error parse-time secret check.

**Files created:**
- `crates/pgevolve-core/src/lint/rules/unmanaged_subscription.rs`
- `crates/pgevolve-core/src/lint/rules/subscription_references_undeclared_publication.rs`
- `crates/pgevolve-core/src/lint/rules/subscription_feature_requires_pg_version.rs`
- `crates/pgevolve-core/src/lint/rules/subscription_password_in_source.rs`

**Files modified:** `crates/pgevolve-core/src/lint/rules/mod.rs`, `crates/pgevolve-core/src/lint/universal.rs`.

### Task 9.1: `unmanaged-subscription` (Warning, waivable)

```rust
//! `unmanaged-subscription` (Warning) — catalog has a subscription source doesn't.

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "unmanaged-subscription";

pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    let source_names: std::collections::BTreeSet<_> =
        source.subscriptions.iter().map(|s| &s.name).collect();
    target
        .subscriptions
        .iter()
        .filter(|s| !source_names.contains(&s.name))
        .map(|s| Finding {
            rule: RULE_ID,
            severity: Severity::Warning,
            message: format!("catalog has subscription {} not declared in source", s.name),
            location: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // 4 tests:
    //   - empty + empty → silent
    //   - source has s, target has s → silent
    //   - source has s, target has s + t → fires for t
    //   - source has s, target has t (different name) → fires for t
}
```

### Task 9.2: `subscription-references-undeclared-publication` (Warning, waivable)

```rust
//! `subscription-references-undeclared-publication` (Warning) — source
//! subscription's PUBLICATION list references a name with no matching
//! Publication in source. Cross-cluster, but catches local-source typos.

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "subscription-references-undeclared-publication";

pub fn check(source: &Catalog) -> Vec<Finding> {
    let pub_names: std::collections::BTreeSet<_> =
        source.publications.iter().map(|p| &p.name).collect();
    source
        .subscriptions
        .iter()
        .flat_map(|s| {
            s.publications
                .iter()
                .filter(|p| !pub_names.contains(p))
                .map(move |p| Finding {
                    rule: RULE_ID,
                    severity: Severity::Warning,
                    message: format!(
                        "subscription {} references undeclared publication {}",
                        s.name, p,
                    ),
                    location: None,
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // 4 tests covering each branch.
}
```

### Task 9.3: `subscription-feature-requires-pg-version` (Error, NOT waivable)

```rust
//! `subscription-feature-requires-pg-version` (Error, not waivable) —
//! source uses a PG-version-gated subscription option below the project's
//! declared min_pg_version.

use crate::ir::catalog::Catalog;
use crate::ir::subscription::StreamingMode;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "subscription-feature-requires-pg-version";

pub fn check(source: &Catalog, min_pg_version: u32) -> Vec<Finding> {
    let mut findings = Vec::new();
    for s in &source.subscriptions {
        if matches!(s.options.streaming, Some(StreamingMode::Parallel)) && min_pg_version < 16 {
            findings.push(fire(s.name.as_str(), "streaming = parallel", 16));
        }
        if s.options.two_phase.is_some() && min_pg_version < 14 {
            // PG 14 introduced two_phase; pgevolve's min is 14 so this never fires
            // in practice. Kept for future-proofing.
        }
        if s.options.disable_on_error.is_some() && min_pg_version < 15 {
            findings.push(fire(s.name.as_str(), "disable_on_error", 15));
        }
        if s.options.password_required.is_some() && min_pg_version < 16 {
            findings.push(fire(s.name.as_str(), "password_required", 16));
        }
        if s.options.run_as_owner.is_some() && min_pg_version < 16 {
            findings.push(fire(s.name.as_str(), "run_as_owner", 16));
        }
        if s.options.origin.is_some() && min_pg_version < 16 {
            findings.push(fire(s.name.as_str(), "origin", 16));
        }
        if s.options.failover.is_some() && min_pg_version < 17 {
            findings.push(fire(s.name.as_str(), "failover", 17));
        }
    }
    findings
}

fn fire(sub_name: &str, feature: &str, required: u32) -> Finding {
    Finding {
        rule: RULE_ID,
        severity: Severity::Error,
        message: format!(
            "subscription {sub_name}: option `{feature}` requires PG {required}+; \
             raise [managed].min_pg_version to {required} or remove the option",
        ),
        location: None,
    }
}

#[cfg(test)]
mod tests {
    // 6+ tests, one per gated feature.
}
```

### Task 9.4: `subscription-password-in-source` (Error, NOT waivable)

```rust
//! `subscription-password-in-source` (Error) — source CONNECTION contains
//! a `password=` value that isn't a `${VAR}` env-var reference. Catches
//! accidental plaintext credential commits at parse/lint time.

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "subscription-password-in-source";

pub fn check(source: &Catalog) -> Vec<Finding> {
    let mut findings = Vec::new();
    for s in &source.subscriptions {
        if let Some(value) = extract_password_value(&s.connection) {
            if !is_env_var_ref(&value) {
                findings.push(Finding {
                    rule: RULE_ID,
                    severity: Severity::Error,
                    message: format!(
                        "subscription {} CONNECTION contains plaintext password; \
                         use ${{ENV_VAR}} reference instead",
                        s.name,
                    ),
                    location: None,
                });
            }
        }
    }
    findings
}

/// Find a `password=…` value in a libpq connstr. Returns None if absent.
/// Case-insensitive on the key; handles quoted and unquoted values.
fn extract_password_value(connstr: &str) -> Option<String> {
    let mut chars = connstr.chars().peekable();
    loop {
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() { chars.next(); } else { break; }
        }
        if chars.peek().is_none() { break None; }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' { chars.next(); break; }
            key.push(c);
            chars.next();
        }
        let mut value = String::new();
        if chars.peek() == Some(&'\'') {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => { if let Some(esc) = chars.next() { value.push(esc); } }
                    '\'' => break,
                    other => value.push(other),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() { break; }
                value.push(c);
                chars.next();
            }
        }
        if key.eq_ignore_ascii_case("password") {
            return Some(value);
        }
    }
}

fn is_env_var_ref(value: &str) -> bool {
    // Must be exactly `${NAME}` where NAME matches [A-Z_][A-Z0-9_]*.
    // No surrounding text allowed (so `xx${VAR}` doesn't satisfy — that'd
    // be a partial concatenation we don't want to encourage).
    if !value.starts_with("${") || !value.ends_with('}') { return false; }
    let inner = &value[2..value.len() - 1];
    if inner.is_empty() { return false; }
    let mut chars = inner.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_uppercase() || first == '_') { return false; }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_password_fires() {
        // Build a Catalog with one Subscription whose connection is `host=x password=secret`.
        // Assert exactly one finding with severity Error.
    }
    #[test]
    fn env_var_password_silent() {
        // CONNECTION 'host=x password=${PWD}' → no findings.
    }
    #[test]
    fn quoted_plaintext_fires() {
        // CONNECTION "host=x password='hunter2'" → fires.
    }
    #[test]
    fn no_password_silent() {
        // CONNECTION 'host=x dbname=app user=u' → no findings.
    }
    #[test]
    fn case_insensitive_password_key() {
        // CONNECTION 'host=x PASSWORD=plain' → fires.
    }
}
```

### Task 9.5: Register + wire

- [ ] **Step 1: Add modules**

```rust
pub mod unmanaged_subscription;
pub mod subscription_references_undeclared_publication;
pub mod subscription_feature_requires_pg_version;
pub mod subscription_password_in_source;
```

- [ ] **Step 2: Wire into dispatchers**

`unmanaged-subscription` → `run_drift_lints` (alongside other unmanaged-* rules — v0.3.4 added this dispatcher entry point).

The other three are source-only → wire into the source-only dispatcher. `subscription-feature-requires-pg-version` takes `min_pg_version` — thread through, mirror how v0.3.4 wired `publication-feature-requires-pg-version`.

### Task 9.6: Verify + commit

```bash
cargo test -p pgevolve-core --lib lint::rules::unmanaged_subscription
cargo test -p pgevolve-core --lib lint::rules::subscription_
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

```
feat(lint): 4 subscription rules

  - unmanaged-subscription (Warning, waivable)
  - subscription-references-undeclared-publication (Warning, waivable)
  - subscription-feature-requires-pg-version (Error, not waivable)
  - subscription-password-in-source (Error, not waivable) — catches
    plaintext password= in CONNECTION at parse/lint time; source must
    use ${ENV_VAR} interpolation.

Stage 9 of docs/superpowers/plans/2026-05-26-subscriptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Stage 10 — Conformance fixtures + `apply = false` flag

Subscriptions can't apply end-to-end against the single-PG conformance harness. New `[fixture] apply: bool` flag in `fixture.toml`; harness skips the apply step when `apply = false`.

**Files modified:** `crates/pgevolve-conformance/src/fixture.rs`, `crates/pgevolve-conformance/tests/run.rs`.
**Files created:** `crates/pgevolve-conformance/tests/cases/objects/subscriptions/<12 fixtures>/`.

### Task 10.1: Add `apply` field to fixture loader

- [ ] **Step 1: Extend the `Fixture` struct in `crates/pgevolve-conformance/src/fixture.rs`**

Find the existing `pub struct Fixture { ... }` (or wherever `fixture.toml` is parsed). Add:

```rust
    /// Whether the apply layer (Layer 7) runs against the fixture. Default
    /// true. Set to false for fixtures that can't apply end-to-end (e.g.,
    /// SUBSCRIPTION fixtures pointing at a publisher that doesn't exist).
    /// Parse + diff + plan.sql + lint layers always run.
    #[serde(default = "default_apply")]
    pub apply: bool,
```

```rust
const fn default_apply() -> bool {
    true
}
```

Add a unit test for the parser:

```rust
#[test]
fn apply_defaults_to_true() {
    let cfg: Fixture = toml::from_str(
        r#"
[meta]
title = "t"
authoring = "objects"
spec_refs = []
[pg]
min = 14
max = 17
"#,
    ).unwrap();
    assert!(cfg.apply);
}

#[test]
fn apply_can_be_false() {
    let cfg: Fixture = toml::from_str(
        r#"
[meta]
title = "t"
authoring = "objects"
spec_refs = []
[pg]
min = 14
max = 17
[fixture]
apply = false
"#,
    ).unwrap();
    assert!(!cfg.apply);
}
```

- [ ] **Step 2: Honor in the runner**

In `crates/pgevolve-conformance/tests/run.rs`, find where the apply layer runs (search for `apply_diff` or similar). Wrap in a check:

```rust
if fixture.apply {
    // existing apply layer code
} else {
    tracing::info!(fixture = %fixture.name(), "skipping apply (apply=false)");
}
```

Make sure the diff / plan.sql / lint layers still run when `apply = false`.

### Task 10.2: Create 12 fixtures

Under `crates/pgevolve-conformance/tests/cases/objects/subscriptions/`. Each fixture has `before.sql`, `after.sql`, `fixture.toml`, `expected/` (bless populates).

All fixtures set `[fixture] apply = false`.

**1. `for-publication-list/`** (min PG 14):

```sql
-- before.sql
CREATE SCHEMA app;

-- after.sql
CREATE SCHEMA app;
CREATE PUBLICATION p FOR ALL TABLES;
CREATE SUBSCRIPTION s
    CONNECTION 'host=replica.example.com dbname=app user=repl password=${REPL_PWD}'
    PUBLICATION p;
```

`fixture.toml`:
```toml
[meta]
title = "CREATE SUBSCRIPTION basic"
authoring = "objects"
spec_refs = ["objects.subscription"]
[pg]
min = 14
max = 17
[fixture]
apply = false
[expect.plan]
steps = 2  # CREATE PUBLICATION + CREATE SUBSCRIPTION
```

**2. `with-binary-and-streaming/`** (min PG 14): `WITH (binary = true, streaming = on)`.

**3. `with-streaming-parallel/`** (min PG 16): `WITH (streaming = parallel)`.

**4. `with-two-phase/`** (min PG 14): `WITH (two_phase = true)`.

**5. `with-disable-on-error/`** (min PG 15): `WITH (disable_on_error = true)`.

**6. `with-password-required-origin-runasowner/`** (min PG 16): `WITH (password_required = true, origin = none, run_as_owner = true)`.

**7. `with-failover/`** (min PG 17): `WITH (failover = true)`.

**8. `alter-add-publication/`** (min PG 14):
- before: publication + subscription with PUBLICATION p1
- after: same + PUBLICATION p1, p2 (publication p2 added)
- Expected: 1 AlterSubscriptionAddPublication step (+ maybe a CreatePublication for p2)

**9. `alter-drop-publication/`** (min PG 14): inverse of #8.

**10. `alter-connection-change/`** (min PG 14): connection env-var changed from `${PWD_A}` to `${PWD_B}`. Expected: 1 AlterSubscriptionConnection.

**11. `lint/unmanaged-subscription/`** (min PG 14):
- before.sql seeds catalog state with a subscription
- after.sql doesn't declare it
- Expected: 0 plan steps + advisory `unmanaged-subscription`. Since the lint fires via `run_drift_lints` (real-catalog path) and conformance pipeline may not exercise that — confirm by reading how v0.3.4's `objects/publications/lint/unmanaged-publication/` is set up.
- `fixture.toml` likely uses `golden = false`, `minimality = false`.

**12. `lint/password-in-source/`** (min PG 14):
- before.sql: empty schema
- after.sql:
  ```sql
  CREATE SUBSCRIPTION s
      CONNECTION 'host=x dbname=app user=repl password=hunter2'
      PUBLICATION p;
  ```
- Expected: lint fires `subscription-password-in-source` Error. Plan should fail / not produce steps depending on harness behavior. Lookup how v0.3.4 handled the equivalent for `publication-feature-requires-pg-version` lint (also Error severity).

### Task 10.3: Bless + verify

```bash
cargo xtask bless --conformance
cargo test -p pgevolve-conformance
```

Spot-check 3-4 fixtures' blessed `expected/plan.sql`. Especially:
- `for-publication-list/`: contains `CREATE SUBSCRIPTION s CONNECTION 'host=...' PUBLICATION p;` — note `${REPL_PWD}` is preserved (unresolved).
- `alter-add-publication/`: single ALTER SUBSCRIPTION ADD PUBLICATION step.
- `lint/password-in-source/`: lint fires; no plan emitted OR plan has 0 steps depending on harness.

### Task 10.4: Commit

```bash
git add crates/pgevolve-conformance/
git commit -m "$(cat <<'EOF'
test(conformance): [fixture] apply flag + 12 subscription fixtures

New optional [fixture] apply: bool field defaults to true; fixtures
set it to false to skip the Layer-7 apply step. Needed for
SUBSCRIPTION fixtures because subscriptions can't apply end-to-end
against the single-PG conformance harness (no real publisher to
connect to).

12 fixtures cover the full PG14-17 surface: basic CREATE, every
PG-version-gated option, ALTER add/drop publication, CONNECTION
change, and the two Error-severity lints (unmanaged-subscription
+ password-in-source). plan.sql goldens preserve ${VAR} placeholders
verbatim.

Stage 10 of docs/superpowers/plans/2026-05-26-subscriptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 11 — Proptest + docs + v0.3.5 release

### Task 11.1: Proptest extensions

- [ ] **Step 1: Extend `crates/pgevolve-testkit/src/ir_generator.rs`**

```rust
fn arb_streaming_mode() -> impl Strategy<Value = StreamingMode> {
    prop_oneof![
        Just(StreamingMode::Off),
        Just(StreamingMode::On),
        Just(StreamingMode::Parallel),
    ]
}

fn arb_origin_mode() -> impl Strategy<Value = OriginMode> {
    prop_oneof![Just(OriginMode::Any), Just(OriginMode::None)]
}

fn arb_subscription_options() -> impl Strategy<Value = SubscriptionOptions> {
    (
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))],     // enabled
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))],     // binary
        prop_oneof![Just(None), arb_streaming_mode().prop_map(Some)],     // streaming
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))],     // two_phase
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))],     // disable_on_error
        prop_oneof![Just(None), arb_origin_mode().prop_map(Some)],        // origin
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))],     // failover
    )
        .prop_map(|(enabled, binary, streaming, two_phase, disable_on_error, origin, failover)| {
            SubscriptionOptions {
                enabled,
                slot_name: None,
                create_slot: None,
                copy_data: None,
                synchronous_commit: None,
                binary,
                streaming,
                two_phase,
                disable_on_error,
                password_required: None,
                run_as_owner: None,
                origin,
                failover,
            }
        })
}

pub fn arb_subscription(
    publication_pool: Vec<Identifier>,
) -> impl Strategy<Value = Subscription> {
    // Pick 1-3 pubs from the pool; fall back to a synthetic name if pool empty
    // (so the strategy works even before publications are generated).
    let pubs_strategy = if publication_pool.is_empty() {
        Just(vec![Identifier::from_unquoted("p").unwrap()]).boxed()
    } else {
        proptest::sample::subsequence(publication_pool, 1..=3.min(publication_pool.len()))
            .prop_map(|v| { let mut v = v; v.sort(); v.dedup(); v })
            .boxed()
    };
    (
        identifier_strategy("sub"),
        pubs_strategy,
        arb_subscription_options(),
    )
        .prop_map(|(name, publications, options)| Subscription {
            name,
            // Synthetic connection string with a ${VAR} placeholder for password.
            // Strategy doesn't need to vary connection text — every generated
            // subscription uses a benign placeholder that lints clean.
            connection: format!("host=replica.example.com dbname=app user=repl password=${{TEST_PWD}}"),
            publications,
            options,
            owner: None,
            comment: None,
        })
}
```

Plumb into `arbitrary_catalog`: generate 0–1 subscriptions per catalog by drawing the publication pool from the catalog's existing publications.

- [ ] **Step 2: Run 10× per constitution §9**

```bash
for i in 1 2 3 4 5 6 7 8 9 10; do
    echo "=== Run $i ==="
    PROPTEST_CASES=512 cargo test --workspace --release 2>&1 | tail -3
done
```

All 10 green.

- [ ] **Step 3: Commit**

```
test(proptest): subscriptions in arbitrary_catalog

arb_subscription / arb_subscription_options / arb_streaming_mode /
arb_origin_mode draw publication names from the catalog's actual
contents so generated subscriptions always reference real publications.
Connection string uses a fixed ${TEST_PWD} placeholder (lints clean).

10× per §9; all green.

Stage 11.1 of docs/superpowers/plans/2026-05-26-subscriptions.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

### Task 11.2: Docs

- [ ] **Step 1: Update `docs/spec/objects.md`** — find the SUBSCRIPTION row in "Replication and federation". Replace:

```markdown
| `SUBSCRIPTION` | 🔮 Future | Logical replication consumer; connection strings introduce secrets-management questions. |
```

with:

```markdown
| `SUBSCRIPTION` | ✅ Supported | Logical-replication subscriber-side metadata. Per-field lenient WITH options (enabled, slot_name, binary, streaming, two_phase, disable_on_error PG15+, password_required PG16+, run_as_owner PG16+, origin PG16+, failover PG17+). CONNECTION supports `${VAR}` env-var interpolation resolved at apply preflight; plan.sql stores unresolved placeholders. Lenient drift via unmanaged-subscription; hard-error on plaintext password in source. change_kinds: [create, drop, alter_connection, alter_add_publication, alter_drop_publication, alter_set_options, comment_on] |
```

- [ ] **Step 2: Create `docs/spec/subscriptions.md`** — capability page modeled on `docs/spec/publications.md`. Cover:

- Source surface (the 3 example CREATE forms)
- `${VAR}` interpolation semantics (resolved at preflight, plan.sql preserves placeholders)
- Per-field lenient options (table of each option + PG version + PG default)
- Diff-modulo-password behavior
- Lints (4)
- Operational-verb rejection (REFRESH/SKIP/standalone ENABLE/DISABLE)
- pg_subscription superuser restriction
- Out of scope (RENAME; subscription statistics)

- [ ] **Step 3: Add `subscriptions.md` to `docs/spec/README.md`** index table.

- [ ] **Step 4: Add a cookbook recipe** at `docs/user/cookbook.md` — "Set up logical replication" with a worked example showing publication + subscription + the `${REPL_PWD}` workflow.

- [ ] **Step 5: CHANGELOG** — add `[0.3.5] — 2026-05-26` section above `[0.3.4]`:

```markdown
## [0.3.5] — 2026-05-26

### Added

- **SUBSCRIPTION as a first-class IR object.** Per-field lenient
  `SubscriptionOptions` (enabled, binary, streaming Off/On/Parallel,
  two_phase, disable_on_error PG15+, password_required + run_as_owner
  + origin PG16+, failover PG17+). Opaque CONNECTION string with
  `${VAR}` env-var interpolation.
- **Apply-time `${VAR}` resolution.** Source IR and plan.sql store
  unresolved `${VAR}` placeholders. Preflight scans every step's SQL,
  resolves against process env, fails before any DB connection if a
  reference is unset. Secrets never persist to disk.
- **8 new StepKind variants** for subscription operations.
- **4 lint rules**: `unmanaged-subscription` (Warning),
  `subscription-references-undeclared-publication` (Warning),
  `subscription-feature-requires-pg-version` (Error, not waivable),
  `subscription-password-in-source` (Error, not waivable) —
  catches plaintext password= at parse time.
- **`[fixture] apply` flag** in the conformance harness so fixtures
  with cross-cluster side-effects (subscriptions) can validate
  parse/diff/plan/lint without applying.
- **12 conformance fixtures** under `objects/subscriptions/`.

### Closes

Second item from the post-v0.3.3 agreed roadmap (next:
CREATE VIEW WITH CHECK OPTION).
```

### Task 11.3: Version bump

```bash
# Root Cargo.toml [workspace.package].version = "0.3.5"
cargo build --workspace

v=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
echo "version: $v"
grep -q "^## \[$v\] — " CHANGELOG.md && echo OK || echo MISMATCH
```

### Task 11.4: §9 verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo doc --workspace --no-deps 2>&1 | grep -cE "^warning"  # expect 0
```

### Task 11.5: Re-bless conformance + tier-3

**Critical**: the version bump shifts every plan ID in the conformance corpus and (because Catalog gained a new `subscriptions` field in Stage 2) every tier-3 catalog snapshot. v0.3.4 forgot this step in the original release commit and required a follow-up push to clear CI. Don't repeat:

```bash
cargo xtask bless --conformance      # re-bless plan.sql goldens
cargo xtask bless                    # re-bless tier-3 catalog snapshots (requires Docker)
cargo test -p pgevolve-conformance
cargo test -p pgevolve-core --test catalog_round_trip --test functions_round_trip --test types_round_trip --test dump_round_trip --test publication_round_trip --test subscription_round_trip
```

### Task 11.6: Release commit

```bash
git add docs/spec/objects.md docs/spec/subscriptions.md docs/spec/README.md docs/user/cookbook.md CHANGELOG.md Cargo.toml Cargo.lock crates/*/Cargo.toml crates/pgevolve-conformance/tests/cases/ crates/pgevolve-core/tests/fixtures/catalog/
git commit -m "$(cat <<'EOF'
release: v0.3.5 — SUBSCRIPTION

First-class declarative model for Postgres SUBSCRIPTION. Source
SQL stays secret-free via ${VAR} env-var interpolation in CONNECTION
strings; resolution happens at apply-time preflight, never persisted
to plan.sql.

Per-field lenient WITH options (enabled, binary, streaming, two_phase,
PG15+ disable_on_error, PG16+ password_required/run_as_owner/origin,
PG17+ failover). 8 new StepKind variants for granular ALTER
SUBSCRIPTION operations. 4 new lint rules including a hard-error
subscription-password-in-source that catches plaintext credential
commits at parse time.

New [fixture] apply: bool harness flag lets SUBSCRIPTION fixtures
validate parse/diff/plan/lint without applying (subscriptions can't
apply end-to-end against the single-PG conformance harness).
12 conformance fixtures + re-blessed plan IDs / tier-3 catalog
snapshots for the v0.3.5 version bump.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 11.7: STOP

Do NOT `git tag`, `git push`, or close GH issues. The user handles those.

---

## Done.

After Stage 11, v0.3.5 is committed locally and ready for tagging.

Next plan target: **CREATE VIEW WITH CHECK OPTION** (smaller scope; incremental on existing view machinery) or **AGGREGATES** (heavier; per agreed roadmap reordering).
