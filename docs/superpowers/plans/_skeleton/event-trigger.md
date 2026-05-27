---
status: skeleton
target_version: v0.4.0
sub_spec: event-trigger
---

# `EVENT TRIGGER` — implementation plan (skeleton)

## Problem
Event triggers fire on DDL events (`ddl_command_start`, `ddl_command_end`,
`table_rewrite`, `sql_drop`). pgevolve has plain triggers but not event
triggers. Used by audit tooling, schema-protection tooling, and some
extensions.

## Scope
- In: `CREATE EVENT TRIGGER`, `ALTER EVENT TRIGGER ... ENABLE/DISABLE`,
  `DROP EVENT TRIGGER`, `COMMENT ON EVENT TRIGGER`. `WHEN TAG IN (...)`
  filter.
- Out: `ALTER EVENT TRIGGER ... RENAME TO` (consistent with plain trigger
  policy — rename is drop+create).

## IR sketch
TBD — new top-level `Catalog::event_triggers: Vec<EventTrigger>`,
analogous to but separate from `Trigger`. Fields: `name`, `event`
(`DdlCommandStart` | `DdlCommandEnd` | `TableRewrite` | `SqlDrop`),
`tag_filter: Vec<String>`, `function_name`, `enabled` (`Always` |
`Replica` | `Disabled` | `Enabled`), `comment`.

## Catalog reader notes
TBD — `pg_event_trigger` joined with `pg_proc` for the function name.
Exclude extension-owned entries (`pg_depend.deptype = 'e'`).

## Conformance fixtures
TBD — `objects/event_triggers/create-simple`, `create-with-tag-filter`,
`enable-disable`, `drop`, `comment-on`,
`scenarios/extension-event-trigger-ignored`.

## Open questions
- Event trigger functions return `event_trigger`; ensure the function-IR
  validates this return type.

## Dependencies on other roadmap items
None.
