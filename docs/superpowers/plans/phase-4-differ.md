# Phase 4 — Differ

**Goal:** Land the `Change` / `TableOp` / `SequenceOp` enums, `Destructiveness` tagging, and a pure-function `diff(target, source) -> ChangeSet` that operates entirely over IR. No SQL strings cross the differ.

**Spec coverage:** §6.3.

**Depends on:** Phase 1 (IR).

**Exit criteria:**

- `pgevolve_core::diff::diff(target: &Catalog, source: &Catalog) -> ChangeSet` returns a `ChangeSet` covering every documented `Change` / `TableOp` / `SequenceOp` variant.
- Each variant carries a `Destructiveness` tag with a human-readable reason for `RequiresApproval` / `RequiresApprovalAndDataLossWarning`.
- `ChangeSet` is *unordered* — ordering is the planner's job (phase 5).
- > 30 unit tests across the variants.
- A small property test: `diff(c, c)` is always empty for any `Catalog c`.

---

## File structure

```
crates/pgevolve-core/src/
└── diff/
    ├── mod.rs                    # public re-exports + diff() entry point
    ├── change.rs                 # Change enum
    ├── table_op.rs               # TableOp enum
    ├── sequence_op.rs            # SequenceOp enum
    ├── destructiveness.rs        # Destructiveness tag + reason helpers
    ├── changeset.rs              # ChangeSet wrapper
    ├── tables.rs                 # diff_tables() pure function
    ├── columns.rs                # diff_columns()
    ├── constraints.rs            # diff_constraints()
    ├── indexes.rs                # diff_indexes()
    ├── sequences.rs              # diff_sequences()
    └── schemas.rs                # diff_schemas()
```

---

### Task 4.1: `Destructiveness` tag

**File:** `crates/pgevolve-core/src/diff/destructiveness.rs`

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "level", rename_all = "snake_case")]
pub enum Destructiveness {
    Safe,
    RequiresApproval { reason: String },
    RequiresApprovalAndDataLossWarning { reason: String },
}

impl Destructiveness {
    pub fn requires_approval(&self) -> bool {
        !matches!(self, Self::Safe)
    }
    pub fn data_loss_risk(&self) -> bool {
        matches!(self, Self::RequiresApprovalAndDataLossWarning { .. })
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Safe => None,
            Self::RequiresApproval { reason } | Self::RequiresApprovalAndDataLossWarning { reason } => Some(reason),
        }
    }
}
```

Tests cover each method.

Commit: `feat(core): Destructiveness tag for diff changes`

---

### Task 4.2: `Change` enum

**File:** `crates/pgevolve-core/src/diff/change.rs`

```rust
pub struct ChangeEntry {
    pub change: Change,
    pub destructiveness: Destructiveness,
}

pub enum Change {
    CreateSchema(Schema),
    DropSchema(QualifiedName),
    CreateTable(Table),
    DropTable { qname: QualifiedName, row_count_estimate: Option<i64> },
    AlterTable { qname: QualifiedName, ops: Vec<TableOpEntry> },

    CreateIndex(Index),
    DropIndex(QualifiedName),
    ReplaceIndex { from: Index, to: Index },  // when a property changed that requires DROP+CREATE

    CreateSequence(Sequence),
    DropSequence(QualifiedName),
    AlterSequence { qname: QualifiedName, ops: Vec<SequenceOpEntry> },
}
```

`row_count_estimate` for `DropTable` is `None` from the differ; the executor populates it just before apply (from `pg_class.reltuples`) when assembling the intent reason. The differ leaves it `None` and includes a placeholder reason; phase 7 (planner) fills the row count from the catalog snapshot.

Tests: serde round-trip; equality of `Change::CreateTable` values.

Commit: `feat(core): Change enum + ChangeEntry`

---

### Task 4.3: `TableOp` enum

**File:** `crates/pgevolve-core/src/diff/table_op.rs`

```rust
pub struct TableOpEntry {
    pub op: TableOp,
    pub destructiveness: Destructiveness,
}

pub enum TableOp {
    AddColumn(Column),
    DropColumn { name: Identifier, is_populated: bool },
    AlterColumnType { name: Identifier, from: ColumnType, to: ColumnType, using: Option<NormalizedExpr> },
    SetColumnNullable { name: Identifier, nullable: bool },
    SetColumnDefault { name: Identifier, default: Option<DefaultExpr> },
    SetColumnIdentity { name: Identifier, identity: Option<Identity> },
    SetColumnGenerated { name: Identifier, generated: Option<Generated> },
    SetColumnComment { name: Identifier, comment: Option<String> },

    AddConstraint(Constraint),
    DropConstraint { name: Identifier },
    SetConstraintComment { name: Identifier, comment: Option<String> },

    SetTableComment { comment: Option<String> },
}
```

> `is_populated` is filled by the differ from the catalog snapshot — `true` if the column exists on a table that has rows. The differ leaves `false` if it can't determine; the planner fills it from `pg_class.reltuples`.

Tests: serde round-trip.

Commit: `feat(core): TableOp enum`

---

### Task 4.4: `SequenceOp` enum

**File:** `crates/pgevolve-core/src/diff/sequence_op.rs`

```rust
pub struct SequenceOpEntry {
    pub op: SequenceOp,
    pub destructiveness: Destructiveness,
}

pub enum SequenceOp {
    SetIncrement(i64),
    SetMinValue(Option<i64>),
    SetMaxValue(Option<i64>),
    SetCache(i64),
    SetCycle(bool),
    SetDataType(ColumnType),
    SetOwnedBy(Option<SequenceOwner>),
    // We intentionally do NOT support "set start" because Postgres requires `RESTART`,
    // which has different semantics. v0.1 emits a recreate for start changes.
}
```

Commit: `feat(core): SequenceOp enum`

---

### Task 4.5: `ChangeSet` wrapper

**File:** `crates/pgevolve-core/src/diff/changeset.rs`

```rust
pub struct ChangeSet {
    pub entries: Vec<ChangeEntry>,
}

impl ChangeSet {
    pub fn new() -> Self { Self { entries: Vec::new() } }
    pub fn push(&mut self, change: Change, destructiveness: Destructiveness) {
        self.entries.push(ChangeEntry { change, destructiveness });
    }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn iter(&self) -> impl Iterator<Item = &ChangeEntry> { self.entries.iter() }
    pub fn extend(&mut self, other: ChangeSet) { self.entries.extend(other.entries); }
}
```

Tests: `ChangeSet::new().is_empty()`; `push` then `len` == 1.

Commit: `feat(core): ChangeSet wrapper`

---

### Task 4.6: Top-level `diff()` entry point

**File:** `crates/pgevolve-core/src/diff/mod.rs`

```rust
pub fn diff(target: &Catalog, source: &Catalog) -> ChangeSet {
    let mut out = ChangeSet::new();
    diff_schemas(target, source, &mut out);
    diff_tables(target, source, &mut out);
    diff_indexes(target, source, &mut out);
    diff_sequences(target, source, &mut out);
    out
}
```

Stub everything else for compile.

Test: `diff(&c, &c).is_empty() == true` for several hand-built catalogs.

Commit: `feat(core): diff() entry point with empty stubs`

---

### Task 4.7: `diff_schemas`

**File:** `crates/pgevolve-core/src/diff/schemas.rs`

Pair schemas by qname (here, just by `name`). Three cases:

- In source, not in target → `CreateSchema(Schema)`, `Destructiveness::Safe`.
- In target, not in source → `DropSchema(qname)`, `Destructiveness::RequiresApproval { reason: "drops schema" }`.
- In both: if `Diff::diff()` non-empty → currently only `comment` can differ → emit `Change::AlterTable`-style? No, we don't have an `AlterSchema`. Add it:

Add a `Change::AlterSchema { qname, comment: Option<String> }` variant. Only the `comment` differs in v0.1.

Tests cover each case.

Commit: `feat(core): diff schemas (create / drop / alter comment)`

---

### Task 4.8: `diff_tables`

**File:** `crates/pgevolve-core/src/diff/tables.rs`

Pair tables by qname. For each pair, dispatch to `diff_columns` and `diff_constraints` and produce a single `Change::AlterTable { qname, ops }`. New tables → `Change::CreateTable(Table)`. Removed tables → `Change::DropTable`.

For `DropTable`, set `row_count_estimate: None` for now; planner fills it.

Destructiveness rules:

- `CreateTable` → `Safe`.
- `DropTable` → `RequiresApprovalAndDataLossWarning { reason: format!("drops table {qname}") }`.
- `AlterTable` itself is `Safe` — destructiveness lives on each `TableOpEntry`.

Tests: add table, drop table, retain table with column-only change.

Commit: `feat(core): diff tables — create/drop/alter dispatch`

---

### Task 4.9: `diff_columns`

**File:** `crates/pgevolve-core/src/diff/columns.rs`

Walk paired columns by `name`:

- Add: `AddColumn(Column)`. Destructiveness:
  - `Safe` if `column.nullable == true` OR has a `DEFAULT`.
  - `RequiresApproval` if NOT NULL with no default — Postgres rejects this on a non-empty table; the user must explicitly approve.
- Drop: `DropColumn { name, is_populated: false }`. Destructiveness: `RequiresApprovalAndDataLossWarning { reason: format!("drops column {name}") }`.
- Type change: `AlterColumnType`. Destructiveness:
  - `Safe` if widening (`int4 → int8`, `varchar(N) → varchar(M>N)`, `varchar(N) → text`). Whitelist these widenings.
  - `RequiresApprovalAndDataLossWarning` for narrowings, type-family changes (e.g., `text → integer`), or unknown.
- Nullable change: `SetColumnNullable { name, nullable: true }` → `Safe`. `nullable: false` → `RequiresApproval { reason: "may fail if column has NULL values" }`.
- Default change: `SetColumnDefault` → `Safe`.
- Identity / generated change: `SetColumnIdentity` / `SetColumnGenerated`. Destructiveness: `RequiresApproval { reason: "identity/generated changes can fail or rewrite data" }`.

Note column-reorder: v0.1 ignores logical column order in the diff. The IR records order, but reordering would require rewriting the table. We document this as a v0.1 limitation: "column order in source is not enforced; if you want a specific physical order, reorder the source and DROP+CREATE the table."

Tests: each case + the widening whitelist.

Commit: `feat(core): diff columns with destructiveness tags`

---

### Task 4.10: `diff_constraints`

**File:** `crates/pgevolve-core/src/diff/constraints.rs`

Pair by `qname`. For added constraints on existing tables:

- `AddConstraint(PK)` / `AddConstraint(Unique)` → `Safe` (will fail at apply if data violates, but that's expected).
- `AddConstraint(ForeignKey)` → `Safe` (the rewrite pass converts to NOT VALID + VALIDATE).
- `AddConstraint(Check)` → `Safe`.

Drops:
- `DropConstraint(name)` → `RequiresApproval { reason: format!("drops {kind} constraint {name}") }` for PK and FK; `Safe` for CHECK and UNIQUE drops (no data loss). v0.1 default is conservative: all constraint drops require approval. Document and review later.

Constraint definition changes (rare in practice — most constraint changes are drop+add):
- If FK changes ON DELETE/ON UPDATE/columns → emit `DropConstraint` + `AddConstraint` pair.
- If PK columns change → same.
- If CHECK expression changes → same.
- This means there's no `AlterConstraint` op; everything is drop+add. Simpler and matches Postgres's actual ALTER capabilities (you can't ALTER most constraint properties anyway).

Tests cover each variant.

Commit: `feat(core): diff constraints (add/drop, definition changes via drop+add)`

---

### Task 4.11: `diff_indexes`

**File:** `crates/pgevolve-core/src/diff/indexes.rs`

Pair by qname.

- Add: `CreateIndex(Index)` → `Safe`. (The rewrite pass converts non-unique adds on existing tables to `CONCURRENTLY`.)
- Drop: `DropIndex(qname)` → `RequiresApproval { reason: "drops index" }`.
- Property changes: indexes are mostly DROP+CREATE. Postgres doesn't support `ALTER INDEX SET FILLFACTOR` for column changes; column list, expression, predicate, opclass, sort/nulls order all require recreate. Use `Change::ReplaceIndex { from, to }` → `RequiresApproval`. Comment-only changes use `Change::ReplaceIndex` too unless we add a separate `SetIndexComment` op (worth doing for clarity — add it as a TableOp-like).

Tests cover each.

Commit: `feat(core): diff indexes`

---

### Task 4.12: `diff_sequences`

**File:** `crates/pgevolve-core/src/diff/sequences.rs`

Pair by qname.

- Add: `CreateSequence(Sequence)` → `Safe`.
- Drop: `DropSequence(qname)` → `RequiresApproval { reason: "drops sequence (current value lost)" }`.
- Alter: emit `SequenceOp` for each differing field. Destructiveness:
  - `SetIncrement` / `SetMinValue` / `SetMaxValue` / `SetCache` / `SetCycle` → `Safe`.
  - `SetDataType` → `RequiresApproval { reason: "may overflow current value" }`.
  - `SetOwnedBy` → `Safe` (ownership changes don't affect data).

Sequences owned by columns: skip diffs entirely — the column's diff drives it. Track ownership in `assemble_catalog`.

Tests cover each.

Commit: `feat(core): diff sequences`

---

### Task 4.13: Property test — empty diff against self

Add a property test (using `proptest`) that constructs random IR via the testkit's `IRGenerator` (phase 11 — for now, hand-author a few non-trivial catalogs):

```rust
#[test]
fn diff_against_self_is_empty() {
    let catalogs = vec![
        catalog_empty(),
        catalog_with_one_table(),
        catalog_with_indexes_and_fks(),
        // ...
    ];
    for c in &catalogs {
        assert!(diff(c, c).is_empty());
    }
}
```

When phase 11 lands `IRGenerator`, replace this with a real proptest.

Commit: `test(core): diff(c, c).is_empty for hand-built catalogs`

---

### Task 4.14: Phase 4 self-review

- Spec §6.3 walkthrough: every `Change` / `TableOp` / `SequenceOp` listed has a test.
- `Destructiveness` reason text reads cleanly in human prose.
- `cargo test -p pgevolve-core` passes; clippy clean.

Phase 4 complete.
