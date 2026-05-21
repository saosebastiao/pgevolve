# v0.2 sub-spec #5: Triggers — design

**Status:** Approved 2026-05-20. Implementation plan to follow.

## Goal

First-class management of Postgres triggers: `CREATE TRIGGER` and
`CREATE CONSTRAINT TRIGGER`, via source SQL + catalog reader + differ
+ planner. Triggers diff atomically — any structural change emits
`DROP TRIGGER` + `CREATE TRIGGER`. Internal triggers (PG's
auto-generated RI/FK triggers, with `pg_trigger.tgisinternal = true`)
and extension-owned triggers (`pg_depend.deptype = 'e'`) are excluded
from catalog reads so they never appear as drift.

Validates the v0.2 arch-readiness spec §16: triggers "reuse the
above" — i.e., they reuse the body-canonicalization (`WHEN`
predicates) and dep-graph machinery introduced for views, types, and
functions.

## Non-goals

- `ALTER TRIGGER … RENAME TO …`. Triggers are atomic at the differ
  layer; renaming is DROP+CREATE.
- `ALTER TABLE … { ENABLE | DISABLE } TRIGGER name`. The enabled
  state is not modeled in IR.
- `ALTER TRIGGER … DEPENDS ON EXTENSION`. Extensions own these via
  pg_depend; not user-managed.
- `EVENT TRIGGER`. Per `docs/spec/objects.md`: future, lower priority.
- `CREATE CONSTRAINT TRIGGER … FROM ref_table` clause. Niche;
  rejected with `UnsupportedClause`.
- Reverse dep edges from managed objects to triggers (a trigger
  references its table and its function, not vice versa).

## IR

Flat struct in `pgevolve_core::ir::trigger`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, DiffMacro)]
pub struct Trigger {
    /// Schema-qualified trigger name. Schema mirrors the owning
    /// table's schema; name is the trigger identifier.
    pub qname: QualifiedName,
    /// Owning table (or view, for INSTEAD OF triggers).
    pub table: QualifiedName,
    /// `BEFORE`, `AFTER`, or `INSTEAD OF`.
    #[diff(via_debug)]
    pub timing: TriggerTiming,
    /// One or more events; order is the source-declaration order.
    #[diff(via_debug)]
    pub events: Vec<TriggerEvent>,
    /// `FOR EACH ROW` or `FOR EACH STATEMENT`.
    #[diff(via_debug)]
    pub level: TriggerLevel,
    /// Optional `WHEN (condition)` predicate; canonicalized via
    /// [`NormalizedExpr`].
    #[diff(via_debug)]
    pub when_clause: Option<NormalizedExpr>,
    /// Statement-level transition tables (`REFERENCING NEW TABLE AS …`).
    #[diff(via_debug)]
    pub transition_tables: Vec<TransitionTable>,
    /// Qualified name of the trigger function.
    pub function_qname: QualifiedName,
    /// Literal string arguments passed to the trigger function.
    #[diff(via_debug)]
    pub function_args: Vec<String>,
    /// `true` for `CREATE CONSTRAINT TRIGGER`. Constraint triggers
    /// always run AFTER, per-row, and support deferral.
    pub is_constraint: bool,
    /// Deferrability (`NotDeferrable` for normal triggers).
    /// Reused from `crate::ir::constraint::Deferrable`.
    #[diff(via_debug)]
    pub deferrable: Deferrable,
    /// Optional `COMMENT ON TRIGGER` text.
    #[diff(via_debug)]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerTiming {
    Before,
    After,
    InsteadOf,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerEvent {
    Insert,
    Update {
        /// Optional column list for `UPDATE OF col1, col2`. Empty
        /// vector = unrestricted `UPDATE`.
        columns: Vec<Identifier>,
    },
    Delete,
    Truncate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerLevel {
    Row,
    Statement,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransitionTable {
    pub name: Identifier,
    pub kind: TransitionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    NewTable,
    OldTable,
}
```

Lives in `Catalog::triggers: Vec<Trigger>`. Identity = `qname`. Canon
`sort_and_dedupe` sorts by `qname`, rejects duplicates with
`IrError::InvalidIdentifier("duplicate trigger: …")`.

## Source parser

New whitelist entry: `CREATE TRIGGER` and `CREATE CONSTRAINT TRIGGER`.
Builder at `parse/builder/create_trigger_stmt.rs`.

Accepts:

```sql
CREATE [ CONSTRAINT ] TRIGGER name { BEFORE | AFTER | INSTEAD OF }
    { event [ OR ... ] }
    ON table_name
    [ NOT DEFERRABLE | DEFERRABLE [ INITIALLY IMMEDIATE | INITIALLY DEFERRED ] ]
    [ REFERENCING { { OLD | NEW } TABLE [ AS ] transition_relation_name } [ ... ] ]
    [ FOR [ EACH ] { ROW | STATEMENT } ]
    [ WHEN ( condition ) ]
    EXECUTE { FUNCTION | PROCEDURE } function_name ( arguments )
```

Where `event ::= INSERT | UPDATE [ OF column [, ...] ] | DELETE | TRUNCATE`.

- `EXECUTE PROCEDURE` (deprecated PG syntax) accepted as a synonym for
  `EXECUTE FUNCTION`.
- `WHEN (condition)` canonicalizes via `NormalizedExpr::from_sql`
  (same path as CHECK constraints).
- Function args parsed as literal strings; preserved verbatim.
- `FROM referenced_table_name` (constraint-trigger only) rejected with
  `UnsupportedClause`.
- `ALTER TRIGGER` and `DROP TRIGGER` in source files rejected at
  statement classification (source = desired state).

Constraint-trigger validation: when `CONSTRAINT` is present, the
parser additionally requires `AFTER` timing and `FOR EACH ROW` level;
violations raise a structural error. PG itself enforces these
constraints; pgevolve catches them at parse time for clearer errors.

## Catalog reader

New `CatalogQuery::Triggers` variant. SQL at `catalog/queries/triggers.rs`:

```sql
SELECT
    t.tgname::text                            AS name,
    n.nspname::text                           AS table_schema,
    c.relname::text                           AS table_name,
    pg_catalog.pg_get_triggerdef(t.oid, true) AS triggerdef,
    d.description                             AS comment
FROM pg_catalog.pg_trigger t
JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_description d
    ON d.objoid = t.oid
   AND d.classoid = 'pg_catalog.pg_trigger'::regclass
WHERE NOT t.tgisinternal
  AND n.nspname = ANY($1::text[])
  AND NOT EXISTS (
      SELECT 1 FROM pg_catalog.pg_depend dep
      WHERE dep.classid = 'pg_catalog.pg_trigger'::regclass
        AND dep.objid = t.oid
        AND dep.deptype = 'e'
  )
ORDER BY n.nspname, c.relname, t.tgname;
```

Assembler reads `triggerdef` (canonical PG output), feeds it back
through the same `create_trigger_stmt::build_trigger` builder used for
source, then attaches the comment. This piggybacks on the source
parser's correctness — the source builder is the single source of
truth for parsing trigger syntax.

Filters:
- `NOT tgisinternal` excludes PG's auto-generated FK/RI triggers and
  similar internal helpers.
- `pg_depend deptype='e'` excludes extension-owned triggers (e.g.,
  triggers installed by `timescaledb` or `pg_partman`).
- `n.nspname = ANY($1)` scopes to managed schemas (matches every
  other v0.2 catalog query).

## Differ

`pgevolve_core::diff::triggers` exposes `diff_triggers(target, source, &mut ChangeSet)`.

```rust
pub enum TriggerChange {
    Create(Trigger),
    Drop { qname: QualifiedName, table: QualifiedName },
    Replace(Trigger),                          // DROP + CREATE for any structural change
    CommentOn {
        qname: QualifiedName,
        table: QualifiedName,
        comment: Option<String>,
    },
}
```

Pair-by-qname. For each pair:

1. **In source but not catalog** → `Create(source)`.
2. **In catalog but not source** → `Drop { qname, table }`. Non-destructive
   (triggers carry no data).
3. **In both with comment difference only** → `CommentOn { … }`.
4. **In both with any other difference** → `Replace(source)`.

All variants are `Destructiveness::Safe`. Triggers are pure derived
behavior; rebuilding them loses nothing.

`Change::Trigger(TriggerChange)` enum variant added to
`diff::change::Change`.

## Planner

Three new `StepKind` variants:

| Variant | Destructive | Transactional | SQL emitted |
|---|---|---|---|
| `CreateTrigger` | no | InTransaction | `CREATE [CONSTRAINT] TRIGGER name [BEFORE \| AFTER \| INSTEAD OF] event ON table … EXECUTE FUNCTION fn(args);` |
| `DropTrigger` | no | InTransaction | `DROP TRIGGER name ON table;` |
| `CommentOnTrigger` | no | InTransaction | `COMMENT ON TRIGGER name ON table IS '…';` / `IS NULL` |

`Replace` is rendered as a `DropTrigger` step followed by a
`CreateTrigger` step in the same group. No separate `ReplaceTrigger`
step kind — matches the existing pattern for view replacement.

### Dep graph

New `NodeId::Trigger(QualifiedName)`. Edges:

- `Trigger → Table(table)` — the trigger's target.
- `Trigger → Function(function_qname, NormalizedArgTypes::empty())` —
  the trigger executes the function. Trigger functions are
  argument-less by SQL convention (they accept implicit `NEW`/`OLD`
  records, not declared arguments at the trigger registration site;
  the `function_args` field carries literal string args that are
  passed to the function but are not part of the function's signature).

Schema dependency comes transitively via `Trigger → Table → Schema`
and `Trigger → Function → Schema`.

### Rewrite

New SQL emission helpers in `plan/rewrite/triggers.rs`:

- `create_trigger(t: &Trigger) -> String`
- `drop_trigger(qname: &QualifiedName, table: &QualifiedName) -> String`
- `comment_on_trigger(qname: &QualifiedName, table: &QualifiedName, comment: Option<&str>) -> String`

New per-family dispatcher at `plan/rewrite/emit/trigger.rs` — 13th
`emit/` family file. Routes the four `TriggerChange` variants.

The `emit_change` dispatcher in `plan/rewrite/mod.rs` gains one new
arm:

```rust
Change::Trigger(tc) => emit::trigger::emit(tc, destructive, destructive_reason, out),
```

## Lints

Two new rules in `lint/universal.rs`:

### `trigger-references-unmanaged-table` (Error)

`CREATE TRIGGER … ON tbl` but `tbl` isn't in the source catalog
(neither `tables`, `views`, nor `materialized_views`). The planner
can't guarantee creation order, and PG will fail apply with a
confusing message. Suggested fix: declare the target, or drop the
trigger.

### `trigger-references-unmanaged-function` (Error)

`EXECUTE FUNCTION fn(...)` but `fn` isn't in the source catalog
(`functions`). Same reasoning.

## Testing

### ~12 conformance fixtures

Under `crates/pgevolve-conformance/tests/cases/`:

- `objects/triggers/create-simple` — minimal `BEFORE INSERT FOR EACH ROW EXECUTE FUNCTION` on a table.
- `objects/triggers/create-after-statement` — `AFTER INSERT FOR EACH STATEMENT`.
- `objects/triggers/create-instead-of-on-view` — `INSTEAD OF UPDATE ON view` requires a view in before.sql.
- `objects/triggers/create-multi-event` — `BEFORE INSERT OR UPDATE OR DELETE` (events list).
- `objects/triggers/create-update-of-columns` — `BEFORE UPDATE OF col1, col2`.
- `objects/triggers/create-with-when-clause` — `WHEN (NEW.id > 0)` predicate; verifies `NormalizedExpr` canonicalization equality.
- `objects/triggers/create-transition-tables` — statement-level trigger with `REFERENCING NEW TABLE AS new_rows`.
- `objects/triggers/create-constraint-trigger` — `CREATE CONSTRAINT TRIGGER … DEFERRABLE INITIALLY DEFERRED`.
- `objects/triggers/drop-simple` — drop trigger.
- `objects/triggers/replace-timing-change` — `BEFORE` → `AFTER` requires Replace (DROP+CREATE).
- `objects/triggers/comment-on` — set / change / clear comment.
- `scenarios/extension-installs-trigger-ignored` — `CREATE EXTENSION timescaledb` (or any extension installing triggers) — verify the catalog reader skips extension-owned triggers via the `deptype='e'` filter.

### Lint fixtures

- `objects/triggers/lint-references-unmanaged-table` — trigger on a table not in source.
- `objects/triggers/lint-references-unmanaged-function` — trigger calling a function not in source.

### Unit tests

Co-located with each new module:

- `ir/trigger.rs` — canonical_eq, diff per-field.
- `parse/builder/create_trigger_stmt.rs` — every clause combination + the `CONSTRAINT TRIGGER` validation rule + rejection of `FROM ref_table`.
- `diff/triggers.rs` — every `TriggerChange` variant path.
- `plan/rewrite/triggers.rs` — every SQL emission helper.
- `plan/rewrite/emit/trigger.rs` — every dispatch path.
- `lint/universal.rs` — both new rules with positive + negative fixtures.

### Property tests

None added in this sub-spec — triggers' atomicity at the differ layer
keeps the round-trip property obvious enough that fixtures cover it.

## Files

### Created

- `crates/pgevolve-core/src/ir/trigger.rs`
- `crates/pgevolve-core/src/parse/builder/create_trigger_stmt.rs`
- `crates/pgevolve-core/src/catalog/queries/triggers.rs`
- `crates/pgevolve-core/src/diff/triggers.rs`
- `crates/pgevolve-core/src/plan/rewrite/triggers.rs`
- `crates/pgevolve-core/src/plan/rewrite/emit/trigger.rs`
- ~12 conformance fixtures under `objects/triggers/` + 1 scenario fixture.

### Modified

- `crates/pgevolve-core/src/ir/mod.rs` — `pub mod trigger;`.
- `crates/pgevolve-core/src/ir/catalog.rs` — `pub triggers: Vec<Trigger>` field.
- `crates/pgevolve-core/src/ir/canon/sort_and_dedupe.rs` — sort + dedupe pass for triggers (sorted by `qname`).
- `crates/pgevolve-core/src/parse/builder/mod.rs` — register new builder.
- `crates/pgevolve-core/src/parse/statement.rs` — `Statement::CreateTrigger` variant; reject `AlterTriggerStmt`.
- `crates/pgevolve-core/src/parse/mod.rs` — dispatch arm.
- `crates/pgevolve-core/src/catalog/mod.rs` — `CatalogQuery::Triggers` variant; wire into `read_catalog`.
- `crates/pgevolve-core/src/catalog/queries/mod.rs` — register query mapping.
- `crates/pgevolve-core/src/catalog/assemble.rs` — `triggers: Vec<Row>` on `RawRows`; `build_triggers` helper.
- `crates/pgevolve-core/src/diff/change.rs` — `TriggerChange` enum + `Change::Trigger(_)` variant.
- `crates/pgevolve-core/src/diff/mod.rs` — wire `diff_triggers` into `diff()`; re-export `TriggerChange`.
- `crates/pgevolve-core/src/plan/edges.rs` — `NodeId::Trigger(QualifiedName)` + edges in `build_create_graph`.
- `crates/pgevolve-core/src/plan/raw_step.rs` — 3 new step kinds.
- `crates/pgevolve-core/src/plan/ordering.rs` — bucket placement for triggers + `change_node` mapping.
- `crates/pgevolve-core/src/plan/plan.rs` — `kind_name` / `parse_kind_name` table for the new step kinds.
- `crates/pgevolve-core/src/plan/error.rs` — `render_node` arm for `NodeId::Trigger`.
- `crates/pgevolve-core/src/plan/rewrite/mod.rs` — new `Change::Trigger(tc)` arm.
- `crates/pgevolve-core/src/plan/rewrite/emit/mod.rs` — `pub(super) mod trigger;`.
- `crates/pgevolve-core/src/lint/universal.rs` — two new rules + docstring entries + tests.
- `crates/pgevolve/src/commands/diff.rs` — `change_kind_name` exhaustive match.
- `crates/pgevolve/src/commands/graph.rs` — `NodeId` exhaustive match for rendering.
- `crates/pgevolve-conformance/src/assertions/dep_graph.rs` — `NodeId` exhaustive match if any.
- `crates/pgevolve-testkit/src/ir_mutator.rs` — `Catalog` literals need `triggers: vec![]`.
- `README.md`, `CHANGELOG.md`, `docs/spec/objects.md`.

## Open questions

None — design is closed.
