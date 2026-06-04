---
status: design
target: v0.4.0
sub_spec: event-trigger
---

# `EVENT TRIGGER` — design

Adds the `EVENT TRIGGER` object kind, the first v0.4.0 roadmap row
([`roadmap.md`](../../spec/roadmap.md) active matrix). Event triggers fire
on DDL events (`ddl_command_start`, `ddl_command_end`, `sql_drop`,
`table_rewrite`) and run a user function; they are used by audit tooling,
schema-protection tooling, and some extensions.

Unlike plain triggers — which are schema-scoped to a managed table and are
fully managed (auto-dropped when absent from source) — event triggers are
**database-global** (a bare name, no schema qualifier) and independently
ownable. They therefore follow the conventions pgevolve already uses for its
other global objects, `PUBLICATION` and `SUBSCRIPTION`: a top-level
`Catalog` vector, a **lenient** drop policy (an event trigger present in the
live database but absent from source is never auto-dropped — it surfaces via
a lint), and a lenient `owner`.

Scope confirmed during brainstorming: full object surface —
`CREATE EVENT TRIGGER` (with `ON <event>` and `WHEN TAG IN (...)` filter),
`ALTER EVENT TRIGGER … ENABLE | DISABLE | ENABLE REPLICA | ENABLE ALWAYS`,
`DROP EVENT TRIGGER`, `COMMENT ON EVENT TRIGGER`, and a lenient `owner`.
`ALTER EVENT TRIGGER … RENAME TO` is **out** — rename is drop+create,
matching the established plain-trigger policy.

---

## §1. IR

New module `crates/pgevolve-core/src/ir/event_trigger.rs`:

```rust
pub struct EventTrigger {
    /// Global object name — event triggers are not schema-qualified.
    pub name: Identifier,
    /// The DDL event the trigger fires on.
    pub event: EventTriggerEvent,
    /// `WHEN TAG IN (...)` command-tag filter. Empty = no filter.
    /// Canonicalized: sorted + deduped. Valid on all four events
    /// (verified against PG 16 — `table_rewrite` accepts a TAG filter too).
    pub tag_filter: Vec<String>,
    /// Schema-qualified name of the `EXECUTE FUNCTION` function.
    pub function: QualifiedName,
    /// Fire state (`pg_event_trigger.evtenabled`: O/D/R/A).
    pub enabled: EventTriggerEnabled,
    /// Lenient owner — `None` means unmanaged (v0.3.1 lenient pattern,
    /// matching `Publication`/`Subscription`).
    pub owner: Option<Identifier>,
    pub comment: Option<String>,
}

#[derive(…, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventTriggerEvent {
    DdlCommandStart,
    DdlCommandEnd,
    SqlDrop,
    TableRewrite,
}

#[derive(…, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventTriggerEnabled {
    /// `O` — fires in origin/local sessions (the default).
    Enabled,
    /// `D` — never fires.
    Disabled,
    /// `R` — fires only in replica (session_replication_role = replica).
    Replica,
    /// `A` — fires always, both origin and replica.
    Always,
}
```

`Catalog` gains `pub event_triggers: Vec<EventTrigger>` (added to the struct,
`Default`, and any exhaustive constructors/tests).

Closed-set fields are enums, not strings — `event` and `enabled` can only be
one of N documented values. `name` is `Identifier`, not `QualifiedName`,
because event triggers have no schema. Newtypes throughout, per the
constitution.

## §2. Parser

`crates/pgevolve-core/src/parse/builder/event_trigger_stmt.rs`, dispatched
from the statement router on the relevant `pg_query` AST nodes:

- `CreateEventTrigStmt` → a new `EventTrigger` (event from `eventname`,
  function from `funcname`, tag filter from the `whenclause` `DefElem` named
  `tag`, `enabled` defaults to `Enabled`).
- `AlterEventTrigStmt` → mutate the matching trigger's `enabled` from
  `tgenabled` (`O`/`D`/`R`/`A`). `ALTER … OWNER TO` sets `owner`.
- `DropStmt` with `removeType = OBJECT_EVENT_TRIGGER` → remove by name.
- `CommentStmt` with `objtype = OBJECT_EVENT_TRIGGER` → set `comment`.

`RENAME` is rejected with the same "rename is drop+create" parse error plain
triggers use.

## §3. Catalog reader

`crates/pgevolve-core/src/catalog/assemble/event_triggers.rs`, fed by a query
over `pg_event_trigger` joined to `pg_proc` for the function name:

- `evtname` → `name`; `evtevent` → `event`; `evtenabled` → `enabled`;
  `evttags` (text[], NULL → empty) → `tag_filter`; `evtfoid` → `function`
  (schema-qualified via `pg_proc`/`pg_namespace`); `evtowner` → `owner`;
  comment via `obj_description(oid, 'pg_event_trigger')`.
- **Exclude extension-owned event triggers** (`pg_depend.deptype = 'e'`),
  mirroring how publications/subscriptions exclude extension-owned rows — an
  audit extension's event triggers are not pgevolve's to manage.

Event triggers are database-global, so the read is **not** parameterized by
the managed-schema filter (same as publications/subscriptions).

## §4. Canon

`crates/pgevolve-core/src/ir/canon/event_triggers.rs`, run from
`canonicalize()`:

- Sort `tag_filter` and dedupe within each event trigger.
- Sort `Catalog::event_triggers` by `name` (via `sort_and_dedupe`, rejecting
  duplicate names).
- No owner/enabled normalization — `owner: None` is the lenient sentinel,
  handled in the diff, not canon.

## §5. Diff

`crates/pgevolve-core/src/diff/event_triggers.rs`, paired by `name`:

- **Source-only** (in source, not in target) → `Create`.
- **Target-only** (in target/live, not in source) → **no auto-drop**
  (lenient). Surfaces via the `unmanaged-event-trigger` lint (§8). This is
  the publication/subscription pattern, not the plain-trigger pattern.
- **Both present:**
  - `event`, `tag_filter`, or `function` differ → `Replace` (DROP + CREATE;
    there is no in-place ALTER for these). `Safe` — event triggers carry no
    data.
  - `enabled` differs → `AlterEnable` (cheap `ALTER EVENT TRIGGER … ENABLE/…`;
    no recreate).
  - `owner` differs **and source declares one** (`Some`) → `AlterOwner`
    (lenient: source `None` emits nothing).
  - `comment` differs → `CommentOn`.

Change variants live on a new `EventTriggerChange` enum
(`Create`/`Replace`/`Drop`/`AlterEnable`/`AlterOwner`/`CommentOn`),
following `TriggerChange`/`PublicationChange`.

## §6. Render + dependency graph

- Render the five forms: `CREATE EVENT TRIGGER name ON event
  [WHEN TAG IN ('…', …)] EXECUTE FUNCTION fn();`,
  `ALTER EVENT TRIGGER name {ENABLE | DISABLE | ENABLE REPLICA |
  ENABLE ALWAYS};`, `ALTER EVENT TRIGGER name OWNER TO role;`,
  `DROP EVENT TRIGGER name;`, `COMMENT ON EVENT TRIGGER name IS '…';`.
- Dependency graph: a new `NodeId::EventTrigger(name)` with an edge to its
  function node, so the function is created before the event trigger and
  dropped after it. No schema edge (event triggers are global), but the
  function it references is schema-scoped, so the edge is `EventTrigger →
  Function`.

## §7. Owner observation (lenient)

Consistent with `RevokeWithOwnerObservation`/publication owner handling:
`owner` only produces an `AlterOwner` when source declares a non-`None`
owner that differs from live. A source that omits owner leaves the live
owner untouched.

## §8. Lint

`unmanaged-event-trigger` rule (Stage 9), mirroring
`unmanaged-publication`: when the live database has an event trigger absent
from source, emit an informational lint rather than a destructive drop, so
the user is told about it without pgevolve silently removing another tool's
event trigger.

## §9. Conformance fixtures

Under `objects/event_triggers/`:

- `create-simple` — `ON ddl_command_end EXECUTE FUNCTION f()`.
- `create-with-tag-filter` — `WHEN TAG IN ('CREATE TABLE','ALTER TABLE')`.
- `enable-disable` — `ALTER … DISABLE` then re-enable.
- `replica-always` — `ENABLE REPLICA` and `ENABLE ALWAYS`.
- `drop` — drop an event trigger.
- `comment-on` — set and change a comment.
- `scenarios/extension-event-trigger-ignored` — an extension-owned event
  trigger is not read into the managed catalog and never diffed/dropped.

Each fixture exercises the function dependency (a managed `f() RETURNS
event_trigger` exists in the source).

## §10. Out of scope / non-goals

- `ALTER EVENT TRIGGER … RENAME TO` — drop+create (trigger policy).
- Auto-dropping unmanaged event triggers — lenient by design (§5, §8).
- Validating the function body — only that the referenced function exists in
  source (the closed-world reference check already covers this). The function
  must return `event_trigger`; PG enforces this at `CREATE EVENT TRIGGER`
  time, so a bad reference fails at apply, surfaced by the existing apply
  error path. (Open question from the skeleton: whether to add an IR-level
  return-type check — deferred; PG's own check is sufficient for v0.4.0.)
