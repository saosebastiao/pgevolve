# EVENT TRIGGER Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the database-global `EVENT TRIGGER` object kind (CREATE / ALTER ENABLE-DISABLE / ALTER OWNER / DROP / COMMENT), the first v0.4.0 roadmap row.

**Architecture:** Event triggers are database-global (bare name, independently ownable), so they mirror `PUBLICATION` everywhere: a top-level `Catalog::event_triggers` vector, a **lenient** drop policy (live-only event triggers surface via a lint, never auto-dropped), and a lenient `owner`. A new `EventTrigger` IR type threads through canon → diff → plan (ordering + dep-graph + render), plus parser and catalog reader, plus conformance fixtures.

**Tech Stack:** Rust, `pg_query` (libpg_query) for parsing, `pg_catalog` introspection, the in-repo conformance harness.

**Design:** [`docs/superpowers/specs/2026-06-04-event-trigger-design.md`](../specs/2026-06-04-event-trigger-design.md)

**Closest template — copy its shape at every layer:** `PUBLICATION`. Concrete template files are cited per task.

---

## File structure

**New files:**
- `crates/pgevolve-core/src/ir/event_trigger.rs` — IR type + enums
- `crates/pgevolve-core/src/ir/canon/event_triggers.rs` — canon pass
- `crates/pgevolve-core/src/diff/event_triggers.rs` — differ
- `crates/pgevolve-core/src/plan/rewrite/emit/event_trigger.rs` — SQL emitters
- `crates/pgevolve-core/src/parse/builder/event_trigger_stmt.rs` — parse builders
- `crates/pgevolve-core/src/catalog/assemble/event_triggers.rs` — catalog reader
- `crates/pgevolve-core/src/lint/rules/unmanaged_event_trigger.rs` — lint rule
- `crates/pgevolve-conformance/tests/cases/objects/event_triggers/**` — fixtures

**Modified (integration sites, exact edits in tasks):**
- `crates/pgevolve-core/src/ir/mod.rs`, `ir/catalog.rs`, `ir/canon/mod.rs`
- `crates/pgevolve-core/src/diff/change.rs`, `diff/mod.rs`
- `crates/pgevolve-core/src/plan/edges.rs`, `plan/ordering.rs`, `plan/rewrite/mod.rs`, `plan/rewrite/emit/mod.rs`
- `crates/pgevolve-core/src/parse/statement.rs`, `parse/mod.rs`, `parse/builder/mod.rs`
- `crates/pgevolve-core/src/catalog/mod.rs`, `catalog/queries/shared.rs`, `catalog/assemble/mod.rs`
- `crates/pgevolve-core/src/lint/rules/mod.rs`, `lint/universal.rs`

**Conventions:** every commit runs `cargo fmt` first; `cargo clippy --workspace --all-targets` must be clean (`-D warnings`, pedantic+nursery); no `unwrap`/`expect` in non-test code.

---

## Task 1: IR type + enums + `Catalog` field

**Files:**
- Create: `crates/pgevolve-core/src/ir/event_trigger.rs`
- Modify: `crates/pgevolve-core/src/ir/mod.rs` (add `pub mod event_trigger;` next to `pub mod trigger;`)
- Modify: `crates/pgevolve-core/src/ir/catalog.rs` (add field next to `pub publications` at ~line 48; add to any `Catalog::empty()`/`Default` if not derived)

- [ ] **Step 1: Write `event_trigger.rs` with the type, enums, and unit tests**

```rust
//! `EVENT TRIGGER` IR. Database-global (bare name, no schema), independently
//! ownable; modeled like `Publication` (lenient owner, lenient drop in the diff).

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};

/// A `CREATE EVENT TRIGGER` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventTrigger {
    /// Global object name — event triggers are not schema-qualified.
    pub name: Identifier,
    /// The DDL event the trigger fires on.
    pub event: EventTriggerEvent,
    /// `WHEN TAG IN (...)` command-tag filter; empty = no filter.
    /// Canon sorts + dedupes this list.
    pub tag_filter: Vec<String>,
    /// Schema-qualified name of the `EXECUTE FUNCTION` function.
    pub function: QualifiedName,
    /// Fire state (`pg_event_trigger.evtenabled`).
    pub enabled: EventTriggerEnabled,
    /// Lenient owner: `None` = unmanaged (matches `Publication`).
    pub owner: Option<Identifier>,
    /// Optional comment.
    pub comment: Option<String>,
}

/// The DDL event an event trigger fires on (`pg_event_trigger.evtevent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventTriggerEvent {
    /// `ddl_command_start`
    DdlCommandStart,
    /// `ddl_command_end`
    DdlCommandEnd,
    /// `sql_drop`
    SqlDrop,
    /// `table_rewrite`
    TableRewrite,
}

impl EventTriggerEvent {
    /// The SQL keyword used in `ON <event>`.
    #[must_use]
    pub const fn sql(self) -> &'static str {
        match self {
            Self::DdlCommandStart => "ddl_command_start",
            Self::DdlCommandEnd => "ddl_command_end",
            Self::SqlDrop => "sql_drop",
            Self::TableRewrite => "table_rewrite",
        }
    }

    /// Parse from the SQL event name (lower-cased).
    #[must_use]
    pub fn from_sql(s: &str) -> Option<Self> {
        match s {
            "ddl_command_start" => Some(Self::DdlCommandStart),
            "ddl_command_end" => Some(Self::DdlCommandEnd),
            "sql_drop" => Some(Self::SqlDrop),
            "table_rewrite" => Some(Self::TableRewrite),
            _ => None,
        }
    }
}

/// Fire state (`pg_event_trigger.evtenabled`: `O`/`D`/`R`/`A`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventTriggerEnabled {
    /// `O` — fires in origin/local sessions (the default).
    Enabled,
    /// `D` — never fires.
    Disabled,
    /// `R` — fires only when `session_replication_role = replica`.
    Replica,
    /// `A` — fires always (origin and replica).
    Always,
}

impl EventTriggerEnabled {
    /// Decode the single-char `pg_event_trigger.evtenabled` code.
    #[must_use]
    pub const fn from_pg_char(c: char) -> Option<Self> {
        match c {
            'O' => Some(Self::Enabled),
            'D' => Some(Self::Disabled),
            'R' => Some(Self::Replica),
            'A' => Some(Self::Always),
            _ => None,
        }
    }

    /// The `ALTER EVENT TRIGGER name <clause>` body for this state.
    #[must_use]
    pub const fn alter_clause(self) -> &'static str {
        match self {
            Self::Enabled => "ENABLE",
            Self::Disabled => "DISABLE",
            Self::Replica => "ENABLE REPLICA",
            Self::Always => "ENABLE ALWAYS",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_sql_roundtrips() {
        for e in [
            EventTriggerEvent::DdlCommandStart,
            EventTriggerEvent::DdlCommandEnd,
            EventTriggerEvent::SqlDrop,
            EventTriggerEvent::TableRewrite,
        ] {
            assert_eq!(EventTriggerEvent::from_sql(e.sql()), Some(e));
        }
        assert_eq!(EventTriggerEvent::from_sql("bogus"), None);
    }

    #[test]
    fn enabled_decodes_pg_chars() {
        assert_eq!(EventTriggerEnabled::from_pg_char('O'), Some(EventTriggerEnabled::Enabled));
        assert_eq!(EventTriggerEnabled::from_pg_char('D'), Some(EventTriggerEnabled::Disabled));
        assert_eq!(EventTriggerEnabled::from_pg_char('R'), Some(EventTriggerEnabled::Replica));
        assert_eq!(EventTriggerEnabled::from_pg_char('A'), Some(EventTriggerEnabled::Always));
        assert_eq!(EventTriggerEnabled::from_pg_char('x'), None);
    }

    #[test]
    fn alter_clauses() {
        assert_eq!(EventTriggerEnabled::Disabled.alter_clause(), "DISABLE");
        assert_eq!(EventTriggerEnabled::Replica.alter_clause(), "ENABLE REPLICA");
        assert_eq!(EventTriggerEnabled::Always.alter_clause(), "ENABLE ALWAYS");
        assert_eq!(EventTriggerEnabled::Enabled.alter_clause(), "ENABLE");
    }
}
```

- [ ] **Step 2: Register the module and `Catalog` field**

In `crates/pgevolve-core/src/ir/mod.rs` add (alphabetical-ish, next to `trigger`):
```rust
pub mod event_trigger;
```
In `crates/pgevolve-core/src/ir/catalog.rs`, next to `pub publications: Vec<Publication>,` add:
```rust
    /// Database-global event triggers (lenient drop policy).
    pub event_triggers: Vec<crate::ir::event_trigger::EventTrigger>,
```
If `Catalog::empty()` constructs fields explicitly (not `..Default::default()`), add `event_triggers: Vec::new(),`. Check the struct derives `Default`; if a manual `empty()` lists each field, add it there too.

- [ ] **Step 3: Run tests + clippy**

Run: `cargo test -p pgevolve-core --lib ir::event_trigger` → Expected: 3 passing.
Run: `cargo build -p pgevolve-core` → Expected: compiles (any exhaustive `Catalog { .. }` literal in non-test code that now misses `event_triggers` will error — fix by adding the field; search `grep -rn "Catalog {" crates/pgevolve-core/src` and update struct literals).
Run: `cargo clippy -p pgevolve-core --lib` → Expected: clean.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add -A && git commit -m "feat(ir): EventTrigger type + Catalog::event_triggers"
```

---

## Task 2: Canon pass

**Files:**
- Create: `crates/pgevolve-core/src/ir/canon/event_triggers.rs`
- Modify: `crates/pgevolve-core/src/ir/canon/mod.rs` (add `pub mod event_triggers;` near line 37; call `event_triggers::run(cat)?;` after `publications::run(cat)?;` near line 102)

- [ ] **Step 1: Write the canon pass with tests**

```rust
//! Canon for `Catalog::event_triggers`: sort + dedupe each tag filter, sort the
//! collection by name, reject duplicate names.

use crate::ir::IrError;
use crate::ir::catalog::Catalog;

/// Canonicalize all event triggers in `cat`.
///
/// - Each `tag_filter` is sorted and deduped.
/// - The collection is sorted by `name`; a duplicate name is an [`IrError`].
pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    for et in &mut cat.event_triggers {
        et.tag_filter.sort();
        et.tag_filter.dedup();
    }
    cat.event_triggers.sort_by(|a, b| a.name.cmp(&b.name));
    for w in cat.event_triggers.windows(2) {
        if w[0].name == w[1].name {
            return Err(IrError::DuplicateEventTrigger(w[0].name.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::event_trigger::{EventTrigger, EventTriggerEnabled, EventTriggerEvent};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn et(name: &str, tags: &[&str]) -> EventTrigger {
        EventTrigger {
            name: id(name),
            event: EventTriggerEvent::DdlCommandEnd,
            tag_filter: tags.iter().map(|s| (*s).to_string()).collect(),
            function: QualifiedName::new(id("app"), id("f")),
            enabled: EventTriggerEnabled::Enabled,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn sorts_and_dedupes_tags_and_names() {
        let mut cat = Catalog::empty();
        cat.event_triggers.push(et("zeta", &["b", "a", "a"]));
        cat.event_triggers.push(et("alpha", &[]));
        run(&mut cat).unwrap();
        assert_eq!(cat.event_triggers[0].name.as_str(), "alpha");
        assert_eq!(cat.event_triggers[1].tag_filter, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn rejects_duplicate_names() {
        let mut cat = Catalog::empty();
        cat.event_triggers.push(et("dup", &[]));
        cat.event_triggers.push(et("dup", &[]));
        assert!(matches!(run(&mut cat).unwrap_err(), IrError::DuplicateEventTrigger(_)));
    }
}
```

- [ ] **Step 2: Add the `IrError::DuplicateEventTrigger` variant**

Find the `IrError` enum (`grep -rn "pub enum IrError" crates/pgevolve-core/src/ir/`). Mirror an existing single-`Identifier` variant (e.g. `EmptyPublication(Identifier)` from `publications`). Add:
```rust
    /// Two event triggers share a name.
    #[error("duplicate event trigger: {0}")]
    DuplicateEventTrigger(Identifier),
```
(Match the existing `#[error(...)]`/`thiserror` style in that file exactly.)

- [ ] **Step 3: Wire into the canon orchestrator**

`crates/pgevolve-core/src/ir/canon/mod.rs`: add `pub mod event_triggers;` with the other `pub mod` lines, and after `publications::run(cat)?;`:
```rust
    event_triggers::run(cat)?;
```

- [ ] **Step 4: Test + clippy + commit**

Run: `cargo test -p pgevolve-core --lib ir::canon::event_triggers` → 2 passing.
Run: `cargo clippy -p pgevolve-core --lib` → clean.
```bash
cargo fmt && git add -A && git commit -m "feat(canon): event-trigger canon (sort tags + names, reject dup names)"
```

---

## Task 3: Change enum

**Files:**
- Modify: `crates/pgevolve-core/src/diff/change.rs`

- [ ] **Step 1: Add the `EventTriggerChange` enum**

After the `TriggerChange` enum in `change.rs`, add:
```rust
/// A change to a single event trigger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventTriggerChange {
    /// `CREATE EVENT TRIGGER ...`
    Create(crate::ir::event_trigger::EventTrigger),
    /// `DROP EVENT TRIGGER old; CREATE EVENT TRIGGER new;` — used when
    /// `event`, `tag_filter`, or `function` differ (no in-place ALTER exists).
    Replace {
        /// As it exists in the target (live).
        from: crate::ir::event_trigger::EventTrigger,
        /// As it should exist in the source.
        to: crate::ir::event_trigger::EventTrigger,
    },
    /// `DROP EVENT TRIGGER name;` — destructive.
    Drop {
        /// Event trigger name.
        name: Identifier,
    },
    /// `ALTER EVENT TRIGGER name {ENABLE|DISABLE|ENABLE REPLICA|ENABLE ALWAYS};`
    AlterEnable {
        /// Event trigger name.
        name: Identifier,
        /// Desired fire state.
        enabled: crate::ir::event_trigger::EventTriggerEnabled,
    },
    /// `ALTER EVENT TRIGGER name OWNER TO role;`
    AlterOwner {
        /// Event trigger name.
        name: Identifier,
        /// Desired owner.
        owner: Identifier,
    },
    /// `COMMENT ON EVENT TRIGGER name IS '...'`
    CommentOn {
        /// Event trigger name.
        name: Identifier,
        /// New comment (`None` clears it).
        comment: Option<String>,
    },
}
```

- [ ] **Step 2: Add the `Change::EventTrigger` variant**

In the `Change` enum, next to `Publication(PublicationChange)`:
```rust
    /// A change to an event trigger. See [`EventTriggerChange`].
    EventTrigger(EventTriggerChange),
```
Export it if `change.rs` has a `pub use` list at the top of `diff/mod.rs` (mirror how `PublicationChange` is exported — `grep -n "PublicationChange" crates/pgevolve-core/src/diff/mod.rs`).

- [ ] **Step 3: Build (find exhaustive matches)**

Run: `cargo build -p pgevolve-core` → Expected: compile **errors** at every exhaustive `match … change` lacking the new arm — these are the integration sites for Tasks 5–7. Note them; do not fix yet beyond what those tasks specify. (If `Change` is matched in `destructiveness` or a `Display` impl, add a minimal arm there now mirroring `Publication`'s — see the build errors.)

- [ ] **Step 4: Commit**

```bash
cargo fmt && git add -A && git commit -m "feat(diff): EventTriggerChange + Change::EventTrigger variant"
```

---

## Task 4: Differ (lenient)

**Files:**
- Create: `crates/pgevolve-core/src/diff/event_triggers.rs`
- Modify: `crates/pgevolve-core/src/diff/mod.rs` (add `pub mod event_triggers;`; call it in `diff()` after `subscriptions::diff_subscriptions(...)`)

Template: `crates/pgevolve-core/src/diff/publications.rs` (lenient target-only) and `diff/triggers.rs` (Replace on structural change).

- [ ] **Step 1: Write the differ with tests**

```rust
//! Differ for `Catalog::event_triggers`. Pair by name. Lenient on drop:
//! a live event trigger absent from source is NOT auto-dropped (surfaced by the
//! `unmanaged-event-trigger` lint). `event`/`tag_filter`/`function` change →
//! Replace; `enabled` → AlterEnable; `owner` (lenient) → AlterOwner; comment →
//! CommentOn.

use std::collections::BTreeMap;

use crate::diff::change::{Change, EventTriggerChange};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::ir::catalog::Catalog;
use crate::ir::event_trigger::EventTrigger;

/// Compute event-trigger changes to converge `target` (live) toward `source`.
pub fn diff_event_triggers(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_by: BTreeMap<_, _> =
        target.event_triggers.iter().map(|e| (e.name.clone(), e)).collect();
    let source_by: BTreeMap<_, _> =
        source.event_triggers.iter().map(|e| (e.name.clone(), e)).collect();

    // Source-only → Create. Lenient: target-only emits nothing (lint handles it).
    for (name, s) in &source_by {
        match target_by.get(name) {
            None => out.push(
                Change::EventTrigger(EventTriggerChange::Create((*s).clone())),
                Destructiveness::Safe,
            ),
            Some(t) => emit_modify(t, s, out),
        }
    }
}

fn structural_differs(t: &EventTrigger, s: &EventTrigger) -> bool {
    t.event != s.event || t.tag_filter != s.tag_filter || t.function != s.function
}

fn emit_modify(t: &EventTrigger, s: &EventTrigger, out: &mut ChangeSet) {
    if structural_differs(t, s) {
        // Replace subsumes enable/owner/comment — the recreate carries them.
        out.push(
            Change::EventTrigger(EventTriggerChange::Replace {
                from: t.clone(),
                to: s.clone(),
            }),
            Destructiveness::Safe,
        );
        return;
    }
    if t.enabled != s.enabled {
        out.push(
            Change::EventTrigger(EventTriggerChange::AlterEnable {
                name: s.name.clone(),
                enabled: s.enabled,
            }),
            Destructiveness::Safe,
        );
    }
    // Owner is lenient: only when source declares one and it differs.
    if let Some(src_owner) = &s.owner {
        if t.owner.as_ref() != Some(src_owner) {
            out.push(
                Change::EventTrigger(EventTriggerChange::AlterOwner {
                    name: s.name.clone(),
                    owner: src_owner.clone(),
                }),
                Destructiveness::Safe,
            );
        }
    }
    if t.comment != s.comment {
        out.push(
            Change::EventTrigger(EventTriggerChange::CommentOn {
                name: s.name.clone(),
                comment: s.comment.clone(),
            }),
            Destructiveness::Safe,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::event_trigger::{EventTriggerEnabled, EventTriggerEvent};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn et(name: &str) -> EventTrigger {
        EventTrigger {
            name: id(name),
            event: EventTriggerEvent::DdlCommandEnd,
            tag_filter: vec![],
            function: QualifiedName::new(id("app"), id("f")),
            enabled: EventTriggerEnabled::Enabled,
            owner: None,
            comment: None,
        }
    }
    fn cat(ets: Vec<EventTrigger>) -> Catalog {
        let mut c = Catalog::empty();
        c.event_triggers = ets;
        c
    }

    #[test]
    fn source_only_creates() {
        let mut out = ChangeSet::new();
        diff_event_triggers(&cat(vec![]), &cat(vec![et("e")]), &mut out);
        assert!(matches!(out.entries[0].change, Change::EventTrigger(EventTriggerChange::Create(_))));
        assert_eq!(out.entries.len(), 1);
    }

    #[test]
    fn target_only_is_lenient_no_drop() {
        let mut out = ChangeSet::new();
        diff_event_triggers(&cat(vec![et("e")]), &cat(vec![]), &mut out);
        assert!(out.is_empty(), "live-only event trigger must NOT be auto-dropped");
    }

    #[test]
    fn structural_change_replaces() {
        let mut t = et("e");
        let mut s = et("e");
        s.event = EventTriggerEvent::SqlDrop;
        let mut out = ChangeSet::new();
        diff_event_triggers(&cat(vec![t.clone()]), &cat(vec![s]), &mut out);
        assert!(matches!(out.entries[0].change, Change::EventTrigger(EventTriggerChange::Replace { .. })));
        let _ = &mut t;
    }

    #[test]
    fn enabled_change_alters() {
        let t = et("e");
        let mut s = et("e");
        s.enabled = EventTriggerEnabled::Disabled;
        let mut out = ChangeSet::new();
        diff_event_triggers(&cat(vec![t]), &cat(vec![s]), &mut out);
        assert!(matches!(out.entries[0].change, Change::EventTrigger(EventTriggerChange::AlterEnable { .. })));
    }

    #[test]
    fn owner_lenient_only_when_source_declares() {
        // source owner None → no change even if live has an owner.
        let mut t = et("e");
        t.owner = Some(id("ops"));
        let s = et("e"); // owner None
        let mut out = ChangeSet::new();
        diff_event_triggers(&cat(vec![t]), &cat(vec![s]), &mut out);
        assert!(out.is_empty());
    }
}
```

- [ ] **Step 2: Wire into the diff orchestrator**

`crates/pgevolve-core/src/diff/mod.rs`: add `pub mod event_triggers;` with the other `pub mod` lines, and in `diff()` after the subscriptions call:
```rust
    event_triggers::diff_event_triggers(target, source, &mut out);
```

- [ ] **Step 3: Test + clippy + commit**

Run: `cargo test -p pgevolve-core --lib diff::event_triggers` → 5 passing.
Run: `cargo clippy -p pgevolve-core --lib` → clean.
```bash
cargo fmt && git add -A && git commit -m "feat(diff): lenient event-trigger differ"
```

---

## Task 5: Dep-graph node + edge

**Files:**
- Modify: `crates/pgevolve-core/src/plan/edges.rs`

Template: the `Trigger` node + trigger→function edge at `edges.rs:149-174`, and `NodeId::Publication` at `edges.rs:67`.

- [ ] **Step 1: Add the `NodeId::EventTrigger` variant**

In the `NodeId` enum (after `Publication(Identifier)`):
```rust
    /// A database-global event trigger.
    EventTrigger(Identifier),
```

- [ ] **Step 2: Register nodes + the EventTrigger→Function edge**

In the graph-building function, after the publication/subscription registration loops, add (mirror the trigger→function edge at lines 173-174):
```rust
    // Event triggers: global nodes; edge to the function they execute so the
    // function is created before the event trigger (and dropped after).
    for et in &catalog.event_triggers {
        g.add_node(NodeId::EventTrigger(et.name.clone()));
        if let Some(func) = catalog.functions.iter().find(|f| f.qname == et.function) {
            g.add_edge(
                NodeId::EventTrigger(et.name.clone()),
                NodeId::Function(et.function.clone(), func.arg_types_normalized.clone()),
            );
        }
    }
```
(Confirm the function field name is `arg_types_normalized` by reading the trigger case at `edges.rs:174`; use whatever that line uses.)

- [ ] **Step 3: Build + clippy + commit**

Run: `cargo build -p pgevolve-core` → compiles (NodeId is likely matched exhaustively in Display/ordering — add minimal arms mirroring `Publication` where the compiler points; the ordering arm is Task 6).
Run: `cargo clippy -p pgevolve-core --lib` → clean.
```bash
cargo fmt && git add -A && git commit -m "feat(plan): NodeId::EventTrigger + EventTrigger→Function edge"
```

---

## Task 6: Plan ordering (`change_node`)

**Files:**
- Modify: `crates/pgevolve-core/src/plan/ordering.rs`

- [ ] **Step 1: Map each `EventTriggerChange` to its node**

In `change_node()` (the `match change` near line 446), after the `Publication`/`Subscription` arms, add:
```rust
        Change::EventTrigger(EventTriggerChange::Create(et)) => {
            NodeId::EventTrigger(et.name.clone())
        }
        Change::EventTrigger(EventTriggerChange::Replace { to, .. }) => {
            NodeId::EventTrigger(to.name.clone())
        }
        Change::EventTrigger(
            EventTriggerChange::Drop { name }
            | EventTriggerChange::AlterEnable { name, .. }
            | EventTriggerChange::AlterOwner { name, .. }
            | EventTriggerChange::CommentOn { name, .. },
        ) => NodeId::EventTrigger(name.clone()),
```
Add `EventTriggerChange` to the `use crate::diff::change::{…}` import at the top of the file.

- [ ] **Step 2: Build + commit**

Run: `cargo build -p pgevolve-core` → compiles.
Run: `cargo clippy -p pgevolve-core --lib` → clean.
```bash
cargo fmt && git add -A && git commit -m "feat(plan): order event-trigger changes by EventTrigger node"
```

---

## Task 7: Render / emit

**Files:**
- Create: `crates/pgevolve-core/src/plan/rewrite/emit/event_trigger.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/mod.rs` (add `pub mod event_trigger;`)
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs` (dispatch `Change::EventTrigger`)

Template: `crates/pgevolve-core/src/plan/rewrite/emit/trigger.rs` for the emit signature/shape and how `Replace` returns two steps; check how an existing emitter pushes `RawStep`s (destructive flag, intent) and copy that exactly.

- [ ] **Step 1: Read the trigger emitter to learn the exact `RawStep`/push API**

Run: `sed -n '1,120p' crates/pgevolve-core/src/plan/rewrite/emit/trigger.rs` — note the `emit(...)` signature, how it constructs steps, the destructive/intent handling, and the identifier-rendering helper used (e.g. `Identifier::render_sql`).

- [ ] **Step 2: Write the emitter**

Mirror the trigger emitter's signature exactly. The SQL bodies:
```rust
// CREATE EVENT TRIGGER <name> ON <event> [WHEN TAG IN ('a','b')] EXECUTE FUNCTION <fn>();
fn create_sql(et: &EventTrigger) -> String {
    let mut s = format!(
        "CREATE EVENT TRIGGER {} ON {}",
        et.name.render_sql(),
        et.event.sql(),
    );
    if !et.tag_filter.is_empty() {
        let tags: Vec<String> = et
            .tag_filter
            .iter()
            .map(|t| format!("'{}'", t.replace('\'', "''")))
            .collect();
        s.push_str(&format!(" WHEN TAG IN ({})", tags.join(", ")));
    }
    s.push_str(&format!(" EXECUTE FUNCTION {}();", et.function.render_sql()));
    s
}
// DROP EVENT TRIGGER <name>;
// ALTER EVENT TRIGGER <name> <enabled.alter_clause()>;
// ALTER EVENT TRIGGER <name> OWNER TO <owner>;
// COMMENT ON EVENT TRIGGER <name> IS '<comment>' | IS NULL;
```
Map each `EventTriggerChange`:
- `Create(et)` → one step `create_sql(et)` (Safe). If `et.enabled != Enabled`, append a follow-up `ALTER EVENT TRIGGER name <clause>;` step (CREATE always makes it Enabled). If `et.owner` is `Some`, append `ALTER … OWNER TO`. If `et.comment` is `Some`, append `COMMENT ON …`.
- `Replace { from, to }` → `DROP EVENT TRIGGER from.name;` (destructive) then the full `Create(to)` sequence.
- `Drop { name }` → `DROP EVENT TRIGGER name;` (destructive).
- `AlterEnable { name, enabled }` → `ALTER EVENT TRIGGER name <enabled.alter_clause()>;` (Safe).
- `AlterOwner { name, owner }` → `ALTER EVENT TRIGGER name OWNER TO owner;` (Safe).
- `CommentOn { name, comment }` → `COMMENT ON EVENT TRIGGER name IS '<escaped>';` or `IS NULL;` when `None`.

Use the identifier/comment rendering helpers the trigger emitter uses (e.g. an existing `render_comment` / single-quote escaper — search `grep -rn "fn render_comment\|IS NULL" crates/pgevolve-core/src/plan/rewrite`). Reuse, do not reinvent.

- [ ] **Step 3: Dispatch in `rewrite/mod.rs`**

In the `match entry.change` (near line 113-179), after the `Trigger` arm:
```rust
        Change::EventTrigger(etc) => emit::event_trigger::emit(etc, /* same trailing args as the Trigger arm */),
```
Register `pub mod event_trigger;` in `emit/mod.rs`.

- [ ] **Step 4: Unit-test the SQL strings**

In `event_trigger.rs` tests, assert each form renders exactly, e.g.:
```rust
#[test]
fn renders_create_with_tags() {
    let et = /* DdlCommandStart, tags [CREATE TABLE, ALTER TABLE], fn app.f */;
    assert_eq!(
        super::create_sql(&et),
        "CREATE EVENT TRIGGER guard ON ddl_command_start WHEN TAG IN ('CREATE TABLE', 'ALTER TABLE') EXECUTE FUNCTION app.f();"
    );
}
```
Add tests for drop, alter-enable (each state), alter-owner, comment (set + clear), and replace (two steps, first is the drop).

- [ ] **Step 5: Test + clippy + commit**

Run: `cargo test -p pgevolve-core --lib plan::rewrite::emit::event_trigger` → passing.
Run: `cargo clippy -p pgevolve-core --lib` → clean.
```bash
cargo fmt && git add -A && git commit -m "feat(render): emit event-trigger DDL"
```

---

## Task 8: Parser (statement + builder)

**Files:**
- Create: `crates/pgevolve-core/src/parse/builder/event_trigger_stmt.rs`
- Modify: `crates/pgevolve-core/src/parse/statement.rs` (enum variants + classify arms)
- Modify: `crates/pgevolve-core/src/parse/builder/mod.rs` (`pub mod event_trigger_stmt;`)
- Modify: `crates/pgevolve-core/src/parse/mod.rs` (accumulator + dispatch + flush)

Template: `crates/pgevolve-core/src/parse/builder/publication_stmt.rs` and the publication accumulator wiring in `parse/mod.rs` (~lines 90-92, 137, 691-694).

- [ ] **Step 1: Add `Statement` variants + classify arms**

`parse/statement.rs`: next to the publication variants (lines 58-62, 110-111) add:
```rust
    CreateEventTrigger(protobuf::CreateEventTrigStmt),
    AlterEventTrigger(protobuf::AlterEventTrigStmt),
```
```rust
    NodeEnum::CreateEventTrigStmt(s) => Ok(Self::CreateEventTrigger(*s)),
    NodeEnum::AlterEventTrigStmt(s) => Ok(Self::AlterEventTrigger(*s)),
```
For `DropStmt`/`CommentStmt`, event triggers route through the **existing generic** drop/comment handling: confirm `ObjectType::ObjectEventTrigger` is recognized where publications/triggers are (search the DropStmt and CommentStmt handling for `ObjectPublication`); add an `ObjectEventTrigger` branch that the builder consumes. If drops in source are rejected (publications reject source-side `DROP`), mirror that exact behavior.

- [ ] **Step 2: Inspect the protobuf node shapes**

Run: `grep -rn "CreateEventTrigStmt\|AlterEventTrigStmt" ~/.cargo 2>/dev/null | head` OR print field names from the generated protobuf via a tiny scratch test, OR read an existing usage. Fields needed: `CreateEventTrigStmt { trigname, eventname, whenclause: Vec<Node> (DefElem name="tag", value=list of String), funcname: Vec<Node> }`; `AlterEventTrigStmt { trigname, tgenabled: char }`. Confirm exact names before writing the builder.

- [ ] **Step 3: Write the builder**

```rust
//! Parse `CREATE/ALTER EVENT TRIGGER` into an accumulator keyed by name.

use std::collections::BTreeMap;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::event_trigger::{EventTrigger, EventTriggerEnabled, EventTriggerEvent};
use crate::parse::error::ParseError;
use crate::parse::source_loc::SourceLocation;
// + pg_query protobuf imports

pub fn parse_create_event_trigger(
    stmt: &protobuf::CreateEventTrigStmt,
    loc: SourceLocation,
    acc: &mut BTreeMap<Identifier, EventTrigger>,
) -> Result<(), ParseError> {
    let name = Identifier::from_unquoted(&stmt.trigname).map_err(/* → ParseError */)?;
    let event = EventTriggerEvent::from_sql(&stmt.eventname.to_lowercase())
        .ok_or_else(|| /* ParseError::Structural: unknown event */)?;
    let tag_filter = extract_tag_filter(&stmt.whenclause)?; // DefElem name=="tag" → Vec<String>
    let function = qualified_from_funcname(&stmt.funcname)?; // schema.name (default schema rules)
    if acc.insert(name.clone(), EventTrigger {
        name, event, tag_filter, function,
        enabled: EventTriggerEnabled::Enabled, owner: None, comment: None,
    }).is_some() {
        return Err(/* ParseError: duplicate event trigger in source */);
    }
    Ok(())
}

pub fn parse_alter_event_trigger(
    stmt: &protobuf::AlterEventTrigStmt,
    loc: SourceLocation,
    acc: &mut BTreeMap<Identifier, EventTrigger>,
) -> Result<(), ParseError> {
    let name = Identifier::from_unquoted(&stmt.trigname).map_err(/* */)?;
    let et = acc.get_mut(&name).ok_or_else(|| /* ParseError: ALTER before CREATE */)?;
    // tgenabled is a single char O/D/R/A:
    et.enabled = EventTriggerEnabled::from_pg_char(stmt.tgenabled.chars().next().unwrap_or('O'))
        .ok_or_else(|| /* ParseError: bad enable state */)?;
    Ok(())
}
```
`COMMENT ON EVENT TRIGGER` and `ALTER … OWNER TO` flow through the generic comment/owner builders — confirm where publications handle `COMMENT ON PUBLICATION` and `ALTER PUBLICATION OWNER TO` and route event triggers the same way (the comment/owner setters look up the object by name in the accumulator). Implement `set_event_trigger_comment(acc, name, comment)` and `set_event_trigger_owner(acc, name, owner)` mirroring the publication equivalents.

Write the helper `extract_tag_filter` and `qualified_from_funcname` (reuse the existing funcname→QualifiedName helper triggers use — `grep -rn "funcname" crates/pgevolve-core/src/parse/builder/`).

- [ ] **Step 4: Wire the accumulator + dispatch + flush in `parse/mod.rs`**

Mirror publications: init `let mut event_triggers: BTreeMap<Identifier, EventTrigger> = BTreeMap::new();`, dispatch `Statement::CreateEventTrigger`/`AlterEventTrigger` to the builders, and after all files flush `catalog.event_triggers = event_triggers.into_values().collect();`.

- [ ] **Step 5: Tests**

Add parser tests (mirror `publication_stmt.rs` tests): parse a small SQL string and assert the resulting `EventTrigger` fields. Cover: simple create; create with `WHEN TAG IN (...)`; `ALTER … DISABLE`/`ENABLE REPLICA`; `COMMENT ON`; duplicate-name error; ALTER-before-CREATE error; unknown-event error.

- [ ] **Step 6: Test + clippy + commit**

Run: `cargo test -p pgevolve-core --lib parse::builder::event_trigger_stmt` → passing.
Run: `cargo clippy -p pgevolve-core --lib` → clean.
```bash
cargo fmt && git add -A && git commit -m "feat(parse): CREATE/ALTER/COMMENT EVENT TRIGGER"
```

---

## Task 9: Catalog reader

**Files:**
- Create: `crates/pgevolve-core/src/catalog/assemble/event_triggers.rs`
- Modify: `crates/pgevolve-core/src/catalog/queries/shared.rs` (query constant)
- Modify: `crates/pgevolve-core/src/catalog/mod.rs` (`CatalogQuery::EventTriggers`, `takes_text_array_param` exclusion, fetch call)
- Modify: `crates/pgevolve-core/src/catalog/assemble/mod.rs` (RawRows field + assemble call)
- Modify: `crates/pgevolve-core/src/catalog/assemble/event_triggers.rs` registered in `assemble/mod.rs` module list

Template: `crates/pgevolve-core/src/catalog/assemble/publications.rs` (Row decoding + tests with the `Row::new().with(...)` builder) and the publication query in `queries/shared.rs:282`.

- [ ] **Step 1: Add the query constant**

`queries/shared.rs` (after the publication queries). Event triggers are global, exclude extension-owned via `pg_depend`:
```rust
pub const EVENT_TRIGGERS_QUERY: &str = "\
SELECT
    e.evtname::text                                   AS name,
    e.evtevent::text                                  AS event,
    e.evtenabled::text                                AS enabled,
    e.evttags::text[]                                 AS tags,
    fn_ns.nspname::text                               AS function_schema,
    p.proname::text                                   AS function_name,
    owner.rolname::text                               AS owner,
    obj_description(e.oid, 'pg_event_trigger')::text  AS comment
FROM pg_event_trigger e
JOIN pg_proc p        ON p.oid = e.evtfoid
JOIN pg_namespace fn_ns ON fn_ns.oid = p.pronamespace
JOIN pg_roles owner   ON owner.oid = e.evtowner
WHERE NOT EXISTS (
    SELECT 1 FROM pg_depend d
    WHERE d.classid = 'pg_event_trigger'::regclass
      AND d.objid = e.oid
      AND d.deptype = 'e'
)
ORDER BY e.evtname";
```
(Confirm the project's column-aliasing/`::text` conventions match the publication query; copy its exact casting style. `evttags` is `text[]`, NULL when no filter.)

- [ ] **Step 2: Register `CatalogQuery::EventTriggers`**

`catalog/mod.rs`: add the `EventTriggers` enum variant (near `Publications`, line ~148); in the SQL-mapping match return `EVENT_TRIGGERS_QUERY`; add `Self::EventTriggers` to the `takes_text_array_param` **exclusion** group (global, no schema param, lines 199-211); add the fetch (lines 270-273):
```rust
    let event_triggers_rows = querier.fetch(CatalogQuery::EventTriggers, &[])?;
```

- [ ] **Step 3: RawRows + assemble orchestration**

`catalog/assemble/mod.rs`: add `pub event_triggers: Vec<Row>,` to `RawRows`; populate it from `event_triggers_rows`; add `pub mod event_triggers;`; and the assemble call:
```rust
    catalog.event_triggers = event_triggers::assemble_event_triggers(&raw.event_triggers)?;
```

- [ ] **Step 4: Write the assembler with row-builder tests**

```rust
//! Assemble `pg_event_trigger` rows into `Vec<EventTrigger>`.

use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::event_trigger::{EventTrigger, EventTriggerEnabled, EventTriggerEvent};

const Q: &str = "EVENT_TRIGGERS_QUERY";

pub(super) fn assemble_event_triggers(rows: &[Row]) -> Result<Vec<EventTrigger>, CatalogError> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let name = Identifier::from_unquoted(&row.get_text(Q, "name")?)
            .map_err(/* CatalogError::BadColumnType */)?;
        let event = EventTriggerEvent::from_sql(&row.get_text(Q, "event")?)
            .ok_or_else(/* CatalogError::BadColumnType: unknown event */)?;
        let enabled = EventTriggerEnabled::from_pg_char(
            row.get_text(Q, "enabled")?.chars().next().unwrap_or('?'),
        ).ok_or_else(/* CatalogError::BadColumnType */)?;
        let tag_filter = row.get_text_array_opt(Q, "tags")?.unwrap_or_default(); // NULL → empty
        let function = QualifiedName::new(
            Identifier::from_unquoted(&row.get_text(Q, "function_schema")?).map_err(/* */)?,
            Identifier::from_unquoted(&row.get_text(Q, "function_name")?).map_err(/* */)?,
        );
        let owner = Some(Identifier::from_unquoted(&row.get_text(Q, "owner")?).map_err(/* */)?);
        let comment = row.get_text_opt(Q, "comment")?.filter(|c| !c.is_empty());
        out.push(EventTrigger { name, event, tag_filter, function, enabled, owner, comment });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    // Mirror publications.rs tests: build Row via Row::new().with("name", Value::Text(...))
    // etc., assert the EventTrigger fields. Cover: simple, with tags (Value::TextArray),
    // NULL tags → empty, each enabled char, NULL comment → None.
}
```
Read the actual `Row` accessor names (`get_text`, `get_text_array`, the `_opt`/nullable variants) in `catalog/rows.rs` and `catalog/assemble/publications.rs`; use the exact methods that exist.

- [ ] **Step 5: Test + clippy + commit**

Run: `cargo test -p pgevolve-core --lib catalog::assemble::event_triggers` → passing.
Run: `cargo clippy -p pgevolve-core --lib` → clean.
```bash
cargo fmt && git add -A && git commit -m "feat(catalog): read pg_event_trigger (excludes extension-owned)"
```

---

## Task 10: Lint — `unmanaged-event-trigger`

**Files:**
- Create: `crates/pgevolve-core/src/lint/rules/unmanaged_event_trigger.rs`
- Modify: `crates/pgevolve-core/src/lint/rules/mod.rs` (`pub mod unmanaged_event_trigger;`)
- Modify: `crates/pgevolve-core/src/lint/universal.rs` (call in `run_drift_lints`, ~line 160)

Template: `crates/pgevolve-core/src/lint/rules/unmanaged_publication.rs` (uses the shared `super::check_unmanaged_objects` helper).

- [ ] **Step 1: Write the rule (copy unmanaged_publication.rs, swap the collection/label)**

```rust
//! Warns when the catalog has an event trigger not declared in source.
//!
//! See [`super::check_unmanaged_objects`] for the shared lenient-drift policy.

use crate::ir::catalog::Catalog;
use crate::lint::finding::Finding;

pub const RULE_ID: &str = "unmanaged-event-trigger";

pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    super::check_unmanaged_objects(
        &target.event_triggers,
        &source.event_triggers,
        |e| &e.name,
        RULE_ID,
        "event trigger",
    )
}

#[cfg(test)]
mod tests {
    // Mirror unmanaged_publication.rs tests: empty silent; in both silent;
    // only-in-target fires one Finding with RULE_ID.
}
```

- [ ] **Step 2: Register + invoke**

`lint/rules/mod.rs`: `pub mod unmanaged_event_trigger;`.
`lint/universal.rs` in `run_drift_lints`, after the unmanaged_publication call:
```rust
    out.extend(rules::unmanaged_event_trigger::check(source, target));
```

- [ ] **Step 3: Test + clippy + commit**

Run: `cargo test -p pgevolve-core --lib lint::rules::unmanaged_event_trigger` → passing.
Run: `cargo clippy -p pgevolve-core --lib` → clean.
```bash
cargo fmt && git add -A && git commit -m "feat(lint): unmanaged-event-trigger rule"
```

---

## Task 11: Conformance fixtures

**Files (all new):** under `crates/pgevolve-conformance/tests/cases/objects/event_triggers/`

Layout per case: `fixture.toml`, `before.sql`, `after.sql`, `expected/plan.sql`, `expected/dep-graph.dot`. Fixtures are auto-discovered (no code change). Template: `crates/pgevolve-conformance/tests/cases/objects/publications/for-all-tables/`.

Every fixture's SQL must declare the function the event trigger uses:
```sql
CREATE SCHEMA app;
CREATE FUNCTION app.audit() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN END $$;
```

- [ ] **Step 1: Read a real fixture set to copy structure exactly**

Run: `for f in fixture.toml before.sql after.sql expected/plan.sql expected/dep-graph.dot; do echo "== $f =="; cat crates/pgevolve-conformance/tests/cases/objects/publications/for-all-tables/$f; done`
Note the `[pg]` min/max convention (use `min = 14`, `max = 18` — event triggers exist since PG 9.3; the project floor is 14), the `[expect.plan]` keys, and the `expected/plan.sql` header format.

- [ ] **Step 2: Author the fixtures**

Create these case dirs (each: `fixture.toml` + `before.sql` + `after.sql`; generate `expected/` via the bless step):
- `create-simple/` — before: schema+function; after: `+ CREATE EVENT TRIGGER et_audit ON ddl_command_end EXECUTE FUNCTION app.audit();`
- `create-with-tag-filter/` — after adds `WHEN TAG IN ('CREATE TABLE', 'ALTER TABLE')`.
- `enable-disable/` — before has the event trigger ENABLED; after `ALTER EVENT TRIGGER et_audit DISABLE;` (so the diff emits AlterEnable).
- `replica-always/` — after `ALTER EVENT TRIGGER et_audit ENABLE REPLICA;` (and a sibling case for `ENABLE ALWAYS`).
- `replace-on-event-change/` — before fires on `ddl_command_start`; after on `sql_drop` (diff → Replace = drop+create).
- `drop/` — before has the event trigger; after removes it from source. **Lenient:** the plan should be empty (no auto-drop) and the `unmanaged-event-trigger` lint fires. Encode that in `fixture.toml` (`[expect.plan] steps = 0` + a `[[expect.lint]]` entry — copy the publications `lint/` fixture's shape).
- `comment-on/` — after adds `COMMENT ON EVENT TRIGGER et_audit IS 'audits DDL';`.
- `scenarios/extension-event-trigger-ignored/` — before.sql installs an extension that creates an event trigger (or simulate via a row the reader must skip); after = same source; expect empty plan + no lint. If no suitable real extension exists in CI images, instead add a **reader unit test** in Task 9 asserting `deptype='e'` rows are skipped, and drop this fixture. Decide based on what extensions the conformance PG images provide (`grep -rn "CREATE EXTENSION" crates/pgevolve-conformance/tests/cases | head`).

- [ ] **Step 3: Bless expected output**

Run: `cargo run -p xtask -- bless objects/event_triggers` (confirm the exact bless subcommand: `grep -rn "bless" xtask/src | head`). Inspect every generated `expected/plan.sql` and `expected/dep-graph.dot` by hand — the dep-graph must show the `EventTrigger → Function` edge for create cases, and `plan.sql` must match the rendered DDL from Task 7.

- [ ] **Step 4: Run conformance + commit**

Run: `cargo test -p pgevolve-conformance` → all event_triggers cases pass (plus no regressions).
```bash
cargo fmt && git add -A && git commit -m "test(conformance): EVENT TRIGGER fixtures"
```

---

## Task 12: Spec/objects catalogue + roadmap + end-to-end verification

**Files:**
- Modify: `docs/spec/objects.md` (flip `EVENT TRIGGER` 📋 → ✅ Supported)
- Modify: `docs/spec/roadmap.md` (move the EVENT TRIGGER row to "Shipped"; update the plan link to this dated plan)
- Modify: `CHANGELOG.md` (`[Unreleased] → Added`: EVENT TRIGGER)
- Modify: `docs/superpowers/plans/_skeleton/event-trigger.md` (delete — promoted to this dated plan)

- [ ] **Step 1: Full workspace gate**

Run: `cargo test --workspace` → all pass.
Run: `cargo clippy --workspace --all-targets` → clean.
Run: `cargo fmt --check` → clean.
Run: `cargo deny check` → ok.

- [ ] **Step 2: Property-test round-trip on a real PG (event triggers exercised)**

Add an event trigger to the testkit IR generator so the soak exercises it (the generator currently emits no event triggers). Mirror the publication generator: a `arb_event_triggers` strategy drawing `name` from a small pool, `event` from the 4 variants, optional `tag_filter`, `function` referencing a generated function that `RETURNS event_trigger`, `enabled` from the 4 states. **Prerequisite:** the function generator must be able to emit a `RETURNS event_trigger` no-arg function for the event trigger to reference (add that shape). Wire `arb_event_triggers` into `arbitrary_catalog` and the mutator's cascade (drop the event trigger when its function is dropped — mirror the trigger cascade). If this is too large for this task, split it into a follow-up plan and note it — but at minimum add **one** hand-written end-to-end test in `crates/pgevolve/tests/` that applies a catalog with an event trigger to an ephemeral PG and asserts `round_trip` convergence.

Run the hand-written test: `cargo test -p pgevolve --test <name> -- --ignored` (Docker required) → passes.

- [ ] **Step 3: Update docs + remove skeleton + commit**

```bash
git rm docs/superpowers/plans/_skeleton/event-trigger.md
cargo fmt && git add -A
git commit -m "feat(event-trigger): mark shipped — objects.md, roadmap, CHANGELOG"
```

---

## Self-review notes (coverage vs spec)

- §1 IR → Task 1. §2 parser → Task 8. §3 reader (extension exclusion) → Task 9. §4 canon → Task 2. §5 diff (lenient, Replace/AlterEnable/AlterOwner/CommentOn) → Tasks 3-4. §6 render + dep-graph → Tasks 5-7. §7 owner lenient → Task 4 (`emit_modify`). §8 lint → Task 10. §9 fixtures → Task 11. §10 non-goals: RENAME rejected (Task 8 routes only Create/Alter-enable/owner/comment; no rename builder), no return-type IR check (Task 11 relies on PG's apply-time check).
- **Known follow-up:** generator/mutator coverage for the soak (Task 12 Step 2) may warrant its own small plan; the minimum bar is one hand-written end-to-end test.
