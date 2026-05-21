# v0.2 sub-spec #6: Partitioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** First-class management of Postgres declarative partitioning: parents (`PARTITION BY {RANGE,LIST,HASH}`), children (declarative `PARTITION OF` + explicit `ATTACH PARTITION` syntax, both normalized to one IR), sub-partitioning, and all four bound shapes. Partition-bound changes diff as `DETACH PARTITION` + `ATTACH PARTITION` (data-preserving). `PARTITION BY` changes on a parent emit a hard `UnsupportedDiff` error.

**Architecture:** Two new optional fields on the existing `Table` struct (`partition_by: Option<PartitionBy>`, `partition_of: Option<PartitionOf>`). New `TableChange` variants `AttachPartition` and `DetachPartition`. New `StepKind`s `AttachPartition`/`DetachPartition` reusing existing `CreateTable`/`DropTable`. New `emit/partition.rs` family dispatcher (14th).

**Tech Stack:** Same as sub-spec #5 — no new dependencies. `NormalizedExpr` (already exists) handles partition expressions and bound literals through the same canonicalizer used by view bodies, function bodies, trigger WHEN clauses, and default expressions.

**Reference design:** `docs/superpowers/specs/2026-05-21-partitioning-design.md`.

**Reference implementation:** sub-spec #5 (triggers), commits `5a17375..da31833`. Every task in this plan mirrors a sub-spec #5 task; when in doubt, read how the trigger version did it.

---

## File structure

**Created:**
- `crates/pgevolve-core/src/ir/partition.rs` — `PartitionBy`, `PartitionOf`, `PartitionStrategy`, `PartitionColumn`, `PartitionColumnKind`, `PartitionBounds`, `BoundDatum` + tests.
- `crates/pgevolve-core/src/parse/builder/alter_table_attach_partition.rs` — parser for `ALTER TABLE … ATTACH PARTITION`.
- `crates/pgevolve-core/src/catalog/queries/partitioned_tables.rs` — `SELECT_PARTITIONED_TABLES` SQL.
- `crates/pgevolve-core/src/catalog/queries/partitions.rs` — `SELECT_PARTITIONS` SQL.
- `crates/pgevolve-core/src/plan/rewrite/partitions.rs` — SQL emission helpers (`attach_partition`, `detach_partition`, `render_partition_by`, `render_partition_of`).
- `crates/pgevolve-core/src/plan/rewrite/emit/partition.rs` — per-family dispatcher (14th).
- ~14 conformance fixtures under `crates/pgevolve-conformance/tests/cases/objects/partitions/`.

**Modified:**
- `ir/mod.rs` — `pub mod partition;`
- `ir/table.rs` — add `partition_by`, `partition_of` fields with `#[diff(via_debug)]`.
- `parse/builder/create_table_stmt.rs` — parse `PARTITION BY` clause; parse `PARTITION OF parent FOR VALUES` clause (Form 2).
- `parse/builder/mod.rs` — register `alter_table_attach_partition` module.
- `parse/statement.rs` — classify `AlterTableStmt` with `ATTACH PARTITION` sub-command.
- `parse/mod.rs` — dispatch ATTACH PARTITION statement to back-fill an existing Table's `partition_of`.
- `catalog/mod.rs` — `CatalogQuery::PartitionedTables`, `CatalogQuery::Partitions`; wire fetches.
- `catalog/queries/mod.rs` — query mapping.
- `catalog/assemble.rs` — `partitioned_tables: Vec<Row>` and `partitions: Vec<Row>` on `RawRows`; merge into existing `Table` entries.
- `diff/tables.rs` — extend table diff to compare `partition_by`/`partition_of` and emit `AttachPartition`/`DetachPartition`.
- `diff/change.rs` — add `AttachPartition` + `DetachPartition` variants to `TableChange`.
- `plan/edges.rs` — `Table(child) → Table(parent)` edge when `child.partition_of.is_some()`.
- `plan/raw_step.rs` — 2 new `StepKind` variants.
- `plan/ordering.rs` — bucket placement for new `TableChange` variants.
- `plan/plan.rs` — `kind_name` / `parse_kind_name` table updates.
- `plan/rewrite/mod.rs` — route new `TableChange` variants through the dispatcher.
- `plan/rewrite/emit/mod.rs` — `pub(super) mod partition;`
- `plan/rewrite/tables.rs` — extend `create_table` emitter to render `PARTITION BY` / `PARTITION OF`.
- `lint/universal.rs` — new `partition-references-unmanaged-parent` rule.
- `crates/pgevolve/src/commands/diff.rs` — exhaustive `change_kind_name` arms + `print_human` arms.
- `crates/pgevolve-testkit/src/ir_mutator.rs` — set `partition_by: None, partition_of: None` in `Table` literals.
- `README.md`, `CHANGELOG.md`, `docs/spec/objects.md`.

---

## Task 1: Partition IR

**Files:**
- Create: `crates/pgevolve-core/src/ir/partition.rs`
- Modify: `crates/pgevolve-core/src/ir/mod.rs`
- Modify: `crates/pgevolve-core/src/ir/table.rs`
- Modify: `crates/pgevolve-core/src/ir/canon/sort_and_dedupe.rs` (no-op if already iterates by qname)
- Modify: `crates/pgevolve-testkit/src/ir_mutator.rs` (set new fields to None in literals)

- [ ] **Step 1: Create the partition IR module**

Create `crates/pgevolve-core/src/ir/partition.rs`:

```rust
//! Partitioning IR — partition-by clauses on partitioned parents and
//! partition-of declarations on partition children.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PartitionBy {
    pub strategy: PartitionStrategy,
    pub columns: Vec<PartitionColumn>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PartitionStrategy {
    Range,
    List,
    Hash,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PartitionColumn {
    pub kind: PartitionColumnKind,
    pub collation: Option<QualifiedName>,
    pub opclass: Option<QualifiedName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PartitionColumnKind {
    Column(Identifier),
    Expr(NormalizedExpr),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PartitionOf {
    pub parent: QualifiedName,
    pub bounds: PartitionBounds,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PartitionBounds {
    Range { from: Vec<BoundDatum>, to: Vec<BoundDatum> },
    List { values: Vec<BoundDatum> },
    Hash { modulus: u32, remainder: u32 },
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BoundDatum {
    Literal(NormalizedExpr),
    MinValue,
    MaxValue,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_strategy_round_trip() {
        let s = PartitionStrategy::Range;
        let json = serde_json::to_string(&s).unwrap();
        let back: PartitionStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn hash_bounds_round_trip() {
        let b = PartitionBounds::Hash { modulus: 4, remainder: 1 };
        let json = serde_json::to_string(&b).unwrap();
        let back: PartitionBounds = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn default_bounds_round_trip() {
        let b = PartitionBounds::Default;
        let json = serde_json::to_string(&b).unwrap();
        let back: PartitionBounds = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }
}
```

- [ ] **Step 2: Register the module**

Edit `crates/pgevolve-core/src/ir/mod.rs`. Add `pub mod partition;` in alphabetical order with the other `pub mod` lines (between `index` and `procedure` — find the right spot).

- [ ] **Step 3: Add fields to Table IR**

Edit `crates/pgevolve-core/src/ir/table.rs`. Find the `Table` struct. Add two new fields at the end (before `comment`, or wherever the convention places auxiliary fields — look at where the `triggers` field landed on `Catalog` for the pattern):

```rust
    /// `Some` → this table is a partitioned parent.
    #[diff(via_debug)]
    pub partition_by: Option<crate::ir::partition::PartitionBy>,

    /// `Some` → this table is itself a partition.
    #[diff(via_debug)]
    pub partition_of: Option<crate::ir::partition::PartitionOf>,
```

If `Table` already implements `Hash`, the new fields (via `PartitionBy`/`PartitionOf` which derive `Hash`) propagate automatically.

- [ ] **Step 4: Update Table::empty() / Table::new() if applicable**

If there's a `Table::new` or `Table::empty` constructor in `table.rs`, initialize `partition_by: None, partition_of: None`.

- [ ] **Step 5: Sweep Table literal construction sites**

`grep -rn 'Table {' --include='*.rs' crates/` to find every place that constructs a `Table` with named-field syntax. Add `partition_by: None, partition_of: None` to each. Expected sites: `parse/builder/create_table_stmt.rs`, `catalog/assemble.rs`, test fixtures, `ir_mutator.rs`. The compiler will tell you which ones — run `cargo check -p pgevolve-core --lib` and let the errors guide you.

- [ ] **Step 6: Run tests + clippy**

```bash
cargo test -p pgevolve-core --lib ir::partition
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```

Expected: all green. 3 new unit tests pass.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(ir): partition_by / partition_of fields on Table

New ir/partition.rs introduces PartitionBy, PartitionOf,
PartitionStrategy, PartitionColumn, PartitionBounds, BoundDatum.
Table gets two optional fields; partition-hood is a flag, not a new
top-level family.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Source parser — PARTITION BY and PARTITION OF

**Files:**
- Modify: `crates/pgevolve-core/src/parse/builder/create_table_stmt.rs`

Forms 1 and 2 from the spec — `CREATE TABLE ... PARTITION BY ...` (parent) and `CREATE TABLE child PARTITION OF parent FOR VALUES ...` (declarative child). Both live on the same `CreateStmt` AST node.

- [ ] **Step 1: Read the existing builder**

Open `crates/pgevolve-core/src/parse/builder/create_table_stmt.rs`. Identify:
- Where `CreateStmt` is processed end-to-end (the entry function — likely `build_table`).
- How columns and constraints are extracted from `CreateStmt.tableElts`.
- Where the resulting `Table { ... }` struct is constructed.

Look at how the recently-landed trigger builder structured its work — same general shape applies.

- [ ] **Step 2: Add PARTITION BY parsing**

In `create_table_stmt.rs`, before returning the `Table` struct, check `CreateStmt.partspec`. The pg_query AST node is `PartitionSpec` with:
- `strategy: String` — `"r"` (range), `"l"` (list), `"h"` (hash) in pg_query v6.
- `part_params: Vec<Node>` — each is a `PartitionElem` containing either a column reference or an expression, plus optional collation and opclass.

Add this helper at the top of the file:

```rust
fn build_partition_by(
    spec: &pg_query::protobuf::PartitionSpec,
) -> Result<crate::ir::partition::PartitionBy, crate::parse::ParseError> {
    use crate::ir::partition::{PartitionBy, PartitionColumn, PartitionColumnKind, PartitionStrategy};

    let strategy = match spec.strategy.as_str() {
        "r" | "RANGE" | "range" => PartitionStrategy::Range,
        "l" | "LIST" | "list" => PartitionStrategy::List,
        "h" | "HASH" | "hash" => PartitionStrategy::Hash,
        other => {
            return Err(crate::parse::ParseError::Structural {
                message: format!("unknown partition strategy {other:?}"),
            });
        }
    };

    let mut columns = Vec::new();
    for part_elem_node in &spec.part_params {
        let elem = match &part_elem_node.node {
            Some(pg_query::NodeEnum::PartitionElem(e)) => e,
            _ => {
                return Err(crate::parse::ParseError::Structural {
                    message: "PARTITION BY entry was not a PartitionElem".into(),
                });
            }
        };

        let kind = if !elem.name.is_empty() {
            PartitionColumnKind::Column(
                crate::identifier::Identifier::from_unquoted(&elem.name).map_err(|e| {
                    crate::parse::ParseError::Structural {
                        message: format!("invalid partition column name: {e}"),
                    }
                })?,
            )
        } else if let Some(expr_node) = elem.expr.as_ref() {
            PartitionColumnKind::Expr(crate::ir::default_expr::normalize_expr::from_pg_node(
                expr_node,
            )?)
        } else {
            return Err(crate::parse::ParseError::Structural {
                message: "PartitionElem had neither name nor expr".into(),
            });
        };

        // collation: elem.collation is a list of Nodes representing a qualified name.
        let collation = qualified_name_from_node_list(&elem.collation)?;
        let opclass = qualified_name_from_node_list(&elem.opclass)?;

        columns.push(PartitionColumn { kind, collation, opclass });
    }

    if columns.is_empty() {
        return Err(crate::parse::ParseError::Structural {
            message: "PARTITION BY had no columns".into(),
        });
    }
    if matches!(strategy, PartitionStrategy::Hash) && columns.len() != 1 {
        return Err(crate::parse::ParseError::Structural {
            message: "HASH partition strategy supports exactly one column".into(),
        });
    }

    Ok(PartitionBy { strategy, columns })
}

fn qualified_name_from_node_list(
    nodes: &[pg_query::protobuf::Node],
) -> Result<Option<crate::identifier::QualifiedName>, crate::parse::ParseError> {
    if nodes.is_empty() {
        return Ok(None);
    }
    let mut parts: Vec<String> = Vec::new();
    for node in nodes {
        match &node.node {
            Some(pg_query::NodeEnum::String(s)) => parts.push(s.sval.clone()),
            _ => {
                return Err(crate::parse::ParseError::Structural {
                    message: "expected String node in qualified name list".into(),
                });
            }
        }
    }
    let (schema, name) = match parts.len() {
        1 => ("public".to_string(), parts.remove(0)),
        2 => {
            let n = parts.pop().unwrap();
            let s = parts.pop().unwrap();
            (s, n)
        }
        _ => {
            return Err(crate::parse::ParseError::Structural {
                message: "qualified name list had unexpected length".into(),
            });
        }
    };
    Ok(Some(crate::identifier::QualifiedName::new(
        crate::identifier::Identifier::from_unquoted(&schema).map_err(|e| {
            crate::parse::ParseError::Structural {
                message: format!("bad schema {schema}: {e}"),
            }
        })?,
        crate::identifier::Identifier::from_unquoted(&name).map_err(|e| {
            crate::parse::ParseError::Structural {
                message: format!("bad name {name}: {e}"),
            }
        })?,
    )))
}
```

Then call `build_partition_by` at the right spot in the main `build_table` function and set `partition_by` on the constructed Table.

- [ ] **Step 3: Add PARTITION OF parsing (Form 2)**

In the same builder, the `CreateStmt` AST also has:
- `inh_relations: Vec<Node>` — non-empty when `PARTITION OF parent` or `INHERITS (parent)` is present. For partitioning we expect `partbound` to also be set.
- `partbound: Option<PartitionBoundSpec>` — bounds when this CREATE creates a declarative partition.

If `stmt.partbound.is_some()`:
1. Extract parent from `stmt.inh_relations[0]` (must be exactly one for partitions).
2. Call a new `build_partition_bounds(&stmt.partbound)` helper.
3. Set `partition_of: Some(PartitionOf { parent, bounds })`.

Add the helper:

```rust
fn build_partition_bounds(
    spec: &pg_query::protobuf::PartitionBoundSpec,
) -> Result<crate::ir::partition::PartitionBounds, crate::parse::ParseError> {
    use crate::ir::partition::{BoundDatum, PartitionBounds};

    if spec.is_default {
        return Ok(PartitionBounds::Default);
    }

    match spec.strategy.as_str() {
        "h" | "HASH" | "hash" => {
            if spec.modulus < 1 {
                return Err(crate::parse::ParseError::Structural {
                    message: "HASH partition modulus must be >= 1".into(),
                });
            }
            if spec.remainder < 0 || spec.remainder >= spec.modulus {
                return Err(crate::parse::ParseError::Structural {
                    message: "HASH partition remainder out of range".into(),
                });
            }
            Ok(PartitionBounds::Hash {
                modulus: u32::try_from(spec.modulus).map_err(|_| {
                    crate::parse::ParseError::Structural {
                        message: "modulus did not fit in u32".into(),
                    }
                })?,
                remainder: u32::try_from(spec.remainder).map_err(|_| {
                    crate::parse::ParseError::Structural {
                        message: "remainder did not fit in u32".into(),
                    }
                })?,
            })
        }
        "l" | "LIST" | "list" => {
            let values = spec.listdatums.iter().map(build_bound_datum).collect::<Result<Vec<_>, _>>()?;
            Ok(PartitionBounds::List { values })
        }
        "r" | "RANGE" | "range" => {
            let from = spec.lowerdatums.iter().map(build_bound_datum).collect::<Result<Vec<_>, _>>()?;
            let to = spec.upperdatums.iter().map(build_bound_datum).collect::<Result<Vec<_>, _>>()?;
            Ok(PartitionBounds::Range { from, to })
        }
        other => Err(crate::parse::ParseError::Structural {
            message: format!("unknown partition bound strategy {other:?}"),
        }),
    }
}

fn build_bound_datum(
    node: &pg_query::protobuf::Node,
) -> Result<crate::ir::partition::BoundDatum, crate::parse::ParseError> {
    use crate::ir::partition::BoundDatum;

    let pbs = match &node.node {
        Some(pg_query::NodeEnum::PartitionRangeDatum(d)) => d,
        _ => {
            // Treat as a literal expression node directly.
            let expr = crate::ir::default_expr::normalize_expr::from_pg_node(node)?;
            return Ok(BoundDatum::Literal(expr));
        }
    };

    // PartitionRangeDatumKind: PARTITION_RANGE_DATUM_VALUE=0, MINVALUE=1, MAXVALUE=2 (verify enum encoding in pg_query v6 — adjust if it uses string tags).
    match pbs.kind {
        0 => {
            let v = pbs.value.as_ref().ok_or_else(|| crate::parse::ParseError::Structural {
                message: "PartitionRangeDatum kind=value but value is None".into(),
            })?;
            let expr = crate::ir::default_expr::normalize_expr::from_pg_node(v)?;
            Ok(BoundDatum::Literal(expr))
        }
        1 => Ok(BoundDatum::MinValue),
        2 => Ok(BoundDatum::MaxValue),
        other => Err(crate::parse::ParseError::Structural {
            message: format!("unknown PartitionRangeDatumKind {other}"),
        }),
    }
}
```

If `pg_query` v6 encodes `PartitionRangeDatumKind` as a string rather than int, adjust the match arms accordingly (look at how other pg_query enums are used in this codebase — search for `.kind` usage in `parse/builder/`).

For Form 2 with sub-partitioning, the same `CreateStmt` may have BOTH `partbound` AND `partspec` populated. The order of helpers above handles this — we call both unconditionally.

- [ ] **Step 4: Unit tests**

Append to `create_table_stmt.rs` (or wherever the file's `#[cfg(test)] mod tests` lives):

```rust
    #[test]
    fn parses_partition_by_list() {
        let sql = "-- @pgevolve schema=app\n\
                   CREATE TABLE orders (id bigint NOT NULL, region text NOT NULL)\n\
                   PARTITION BY LIST (region);";
        let parsed = pg_query::parse(sql).unwrap();
        let stmt = match &parsed.protobuf.stmts[0].stmt.as_ref().unwrap().node {
            Some(pg_query::NodeEnum::CreateStmt(s)) => s.clone(),
            other => panic!("expected CreateStmt, got {other:?}"),
        };
        let t = build_table(&stmt, /* directives */ &Default::default()).unwrap();
        let pb = t.partition_by.expect("partition_by");
        assert!(matches!(pb.strategy, crate::ir::partition::PartitionStrategy::List));
        assert_eq!(pb.columns.len(), 1);
    }

    #[test]
    fn parses_partition_of_range() {
        let sql = "-- @pgevolve schema=app\n\
                   CREATE TABLE orders_2024 PARTITION OF orders\n\
                   FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');";
        let parsed = pg_query::parse(sql).unwrap();
        let stmt = match &parsed.protobuf.stmts[0].stmt.as_ref().unwrap().node {
            Some(pg_query::NodeEnum::CreateStmt(s)) => s.clone(),
            other => panic!("expected CreateStmt, got {other:?}"),
        };
        let t = build_table(&stmt, /* directives */ &Default::default()).unwrap();
        let po = t.partition_of.expect("partition_of");
        assert_eq!(po.parent.name.as_str(), "orders");
        assert!(matches!(po.bounds, crate::ir::partition::PartitionBounds::Range { .. }));
    }

    #[test]
    fn rejects_hash_with_two_columns() {
        let sql = "-- @pgevolve schema=app\n\
                   CREATE TABLE t (a int, b int) PARTITION BY HASH (a, b);";
        let parsed = pg_query::parse(sql).unwrap();
        let stmt = match &parsed.protobuf.stmts[0].stmt.as_ref().unwrap().node {
            Some(pg_query::NodeEnum::CreateStmt(s)) => s.clone(),
            other => panic!("expected CreateStmt, got {other:?}"),
        };
        let err = build_table(&stmt, /* directives */ &Default::default()).unwrap_err();
        assert!(matches!(err, crate::parse::ParseError::Structural { .. }));
    }
```

Adjust `build_table`'s second argument to match its real signature (whatever the directives type is).

- [ ] **Step 5: Run tests + clippy**

```bash
cargo test -p pgevolve-core --lib parse::builder::create_table_stmt
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```

Expected: all green, 3 new tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(parse): PARTITION BY + PARTITION OF clauses on CREATE TABLE

Extends the existing CreateStmt builder. PARTITION BY {RANGE,LIST,HASH}
populates Table.partition_by; PARTITION OF parent FOR VALUES ...
populates Table.partition_of. HASH validated as single-column.
PartitionRangeDatum handles MINVALUE/MAXVALUE sentinels.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Source parser — ALTER TABLE ... ATTACH PARTITION

**Files:**
- Create: `crates/pgevolve-core/src/parse/builder/alter_table_attach_partition.rs`
- Modify: `crates/pgevolve-core/src/parse/builder/mod.rs`
- Modify: `crates/pgevolve-core/src/parse/statement.rs`
- Modify: `crates/pgevolve-core/src/parse/mod.rs`

Form 3 — the second statement (`ALTER TABLE parent ATTACH PARTITION child FOR VALUES ...`) back-fills `partition_of` on an already-parsed child Table.

- [ ] **Step 1: Create the builder**

Create `crates/pgevolve-core/src/parse/builder/alter_table_attach_partition.rs`:

```rust
//! `ALTER TABLE parent ATTACH PARTITION child FOR VALUES ...` — back-fills
//! `partition_of` on an already-parsed child Table.

use pg_query::protobuf::AlterTableStmt;

use crate::identifier::QualifiedName;
use crate::ir::partition::PartitionOf;
use crate::parse::ParseError;

pub struct AttachPartition {
    pub parent: QualifiedName,
    pub child: QualifiedName,
    pub partition_of: PartitionOf,
}

pub fn build_attach_partition(stmt: &AlterTableStmt) -> Result<AttachPartition, ParseError> {
    let parent_rangevar = stmt.relation.as_ref().ok_or_else(|| ParseError::Structural {
        message: "ALTER TABLE ATTACH PARTITION missing relation".into(),
    })?;
    let parent = qualified_name_from_rangevar(parent_rangevar)?;

    if stmt.cmds.len() != 1 {
        return Err(ParseError::Structural {
            message: "ALTER TABLE with ATTACH PARTITION must be the only sub-command".into(),
        });
    }
    let cmd_node = &stmt.cmds[0];
    let cmd = match &cmd_node.node {
        Some(pg_query::NodeEnum::AlterTableCmd(c)) => c,
        _ => {
            return Err(ParseError::Structural {
                message: "expected AlterTableCmd".into(),
            });
        }
    };

    // AT_AttachPartition in pg_query v6 — check the AlterTableType enum value;
    // it's usually represented as an integer in the .subtype field.
    // Use the named constant if pg_query v6 exposes it; otherwise the integer.
    use pg_query::protobuf::AlterTableType;
    if cmd.subtype() != AlterTableType::AtAttachPartition {
        return Err(ParseError::Structural {
            message: "only ATTACH PARTITION sub-command is supported on ALTER TABLE".into(),
        });
    }

    let part_cmd_node = cmd.def.as_ref().ok_or_else(|| ParseError::Structural {
        message: "ATTACH PARTITION missing partition cmd".into(),
    })?;
    let part_cmd = match &part_cmd_node.node {
        Some(pg_query::NodeEnum::PartitionCmd(p)) => p,
        _ => {
            return Err(ParseError::Structural {
                message: "expected PartitionCmd on ATTACH PARTITION".into(),
            });
        }
    };

    if part_cmd.concurrent {
        return Err(ParseError::Structural {
            message: "ATTACH PARTITION ... CONCURRENTLY is not supported".into(),
        });
    }

    let child_rangevar = part_cmd.name.as_ref().ok_or_else(|| ParseError::Structural {
        message: "ATTACH PARTITION missing child name".into(),
    })?;
    let child = qualified_name_from_rangevar(child_rangevar)?;

    let bound_spec = part_cmd.bound.as_ref().ok_or_else(|| ParseError::Structural {
        message: "ATTACH PARTITION missing FOR VALUES bounds".into(),
    })?;
    let bounds = crate::parse::builder::create_table_stmt::build_partition_bounds(bound_spec)?;

    Ok(AttachPartition {
        parent: parent.clone(),
        child,
        partition_of: PartitionOf { parent, bounds },
    })
}

fn qualified_name_from_rangevar(rv: &pg_query::protobuf::RangeVar) -> Result<QualifiedName, ParseError> {
    let schema = if rv.schemaname.is_empty() {
        return Err(ParseError::UnqualifiedName {
            name: rv.relname.clone(),
        });
    } else {
        &rv.schemaname
    };
    Ok(QualifiedName::new(
        crate::identifier::Identifier::from_unquoted(schema).map_err(|e| ParseError::Structural {
            message: format!("bad schema {schema}: {e}"),
        })?,
        crate::identifier::Identifier::from_unquoted(&rv.relname).map_err(|e| {
            ParseError::Structural {
                message: format!("bad name {}: {e}", rv.relname),
            }
        })?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one(sql: &str) -> AlterTableStmt {
        let parsed = pg_query::parse(sql).unwrap();
        match &parsed.protobuf.stmts[0].stmt.as_ref().unwrap().node {
            Some(pg_query::NodeEnum::AlterTableStmt(s)) => s.clone(),
            other => panic!("expected AlterTableStmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_attach_partition_range() {
        let stmt = parse_one(
            "ALTER TABLE app.orders ATTACH PARTITION app.orders_2024 \
             FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');",
        );
        let r = build_attach_partition(&stmt).unwrap();
        assert_eq!(r.parent.name.as_str(), "orders");
        assert_eq!(r.child.name.as_str(), "orders_2024");
    }

    #[test]
    fn rejects_concurrently() {
        let stmt = parse_one(
            "ALTER TABLE app.orders ATTACH PARTITION app.orders_2024 \
             FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');",
        );
        // Manually flip concurrent to true to simulate the input.
        let mut s = stmt;
        match s.cmds[0].node.as_mut().unwrap() {
            pg_query::NodeEnum::AlterTableCmd(c) => match c.def.as_mut().unwrap().node.as_mut().unwrap() {
                pg_query::NodeEnum::PartitionCmd(p) => p.concurrent = true,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
        let err = build_attach_partition(&s).unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }
}
```

If `build_partition_bounds` from Task 2 ends up `pub(crate)` vs `pub(super)` the call path here may need to be adjusted — make it `pub(crate)` so this builder can use it.

- [ ] **Step 2: Register the module**

Edit `crates/pgevolve-core/src/parse/builder/mod.rs`. Add `pub(crate) mod alter_table_attach_partition;` in alphabetical order.

- [ ] **Step 3: Classify the statement**

Edit `crates/pgevolve-core/src/parse/statement.rs`. Find the `Statement` enum and the existing `AlterTableStmt` handling (currently a fallthrough to error). Add a new variant:

```rust
    AlterTableAttachPartition(pg_query::protobuf::AlterTableStmt),
```

And in the classifier:

```rust
            Some(NodeEnum::AlterTableStmt(s)) => {
                // Check if it's an ATTACH PARTITION.
                if is_attach_partition(s) {
                    Statement::AlterTableAttachPartition(s.clone())
                } else {
                    // Existing behavior — reject as UnsupportedObjectKind / Structural.
                    return Err(/* existing error */);
                }
            }
```

Add the helper:

```rust
fn is_attach_partition(stmt: &pg_query::protobuf::AlterTableStmt) -> bool {
    use pg_query::protobuf::AlterTableType;
    stmt.cmds.len() == 1
        && matches!(
            stmt.cmds[0].node,
            Some(pg_query::NodeEnum::AlterTableCmd(ref c)) if c.subtype() == AlterTableType::AtAttachPartition,
        )
}
```

- [ ] **Step 4: Dispatch in parse/mod.rs**

Edit `crates/pgevolve-core/src/parse/mod.rs`. In the statement dispatch loop, add:

```rust
            Statement::AlterTableAttachPartition(stmt) => {
                let attach = crate::parse::builder::alter_table_attach_partition::build_attach_partition(&stmt)?;
                // Find the child Table in catalog.tables and back-fill its partition_of.
                let child_table = catalog
                    .tables
                    .iter_mut()
                    .find(|t| t.qname == attach.child)
                    .ok_or_else(|| ParseError::Structural {
                        message: format!(
                            "ATTACH PARTITION {child} must follow its CREATE TABLE statement",
                            child = attach.child
                        ),
                    })?;
                if child_table.partition_of.is_some() {
                    return Err(ParseError::Structural {
                        message: format!(
                            "table {} already declared as a partition",
                            attach.child
                        ),
                    });
                }
                child_table.partition_of = Some(attach.partition_of);
            }
```

- [ ] **Step 5: Run tests + clippy**

```bash
cargo test -p pgevolve-core --lib parse::builder::alter_table_attach_partition
cargo test -p pgevolve-core --lib parse
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```

Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(parse): ALTER TABLE ... ATTACH PARTITION back-fills partition_of

New alter_table_attach_partition builder. The statement dispatcher
finds the already-parsed child Table and back-fills its partition_of
field. Concurrent ATTACH is rejected.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Catalog reader — partitioned parents + partitions

**Files:**
- Create: `crates/pgevolve-core/src/catalog/queries/partitioned_tables.rs`
- Create: `crates/pgevolve-core/src/catalog/queries/partitions.rs`
- Modify: `crates/pgevolve-core/src/catalog/queries/mod.rs`
- Modify: `crates/pgevolve-core/src/catalog/mod.rs`
- Modify: `crates/pgevolve-core/src/catalog/assemble.rs`

- [ ] **Step 1: SQL for partitioned parents**

Create `crates/pgevolve-core/src/catalog/queries/partitioned_tables.rs`:

```rust
//! Reads partitioned-table parents (pg_class.relkind = 'p').

pub const SELECT_PARTITIONED_TABLES: &str = r#"
SELECT
    n.nspname        AS schema_name,
    c.relname        AS table_name,
    pg_get_partkeydef(c.oid) AS partkey_def
FROM pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind = 'p'
  AND n.nspname = ANY($1::text[])
  AND NOT EXISTS (
      SELECT 1 FROM pg_depend d
      WHERE d.objid = c.oid AND d.deptype = 'e'
  );
"#;

#[derive(Debug, Clone)]
pub struct Row {
    pub schema_name: String,
    pub table_name: String,
    pub partkey_def: String, // e.g. "RANGE (placed)"
}
```

- [ ] **Step 2: SQL for partitions**

Create `crates/pgevolve-core/src/catalog/queries/partitions.rs`:

```rust
//! Reads child partitions (pg_class.relispartition = true).

pub const SELECT_PARTITIONS: &str = r#"
SELECT
    n.nspname          AS schema_name,
    c.relname          AS table_name,
    parent_n.nspname   AS parent_schema,
    parent_c.relname   AS parent_name,
    pg_get_expr(c.relpartbound, c.oid) AS partbound_def
FROM pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
JOIN pg_inherits i ON i.inhrelid = c.oid
JOIN pg_class parent_c ON parent_c.oid = i.inhparent
JOIN pg_namespace parent_n ON parent_n.oid = parent_c.relnamespace
WHERE c.relispartition = true
  AND n.nspname = ANY($1::text[])
  AND NOT EXISTS (
      SELECT 1 FROM pg_depend d
      WHERE d.objid = c.oid AND d.deptype = 'e'
  );
"#;

#[derive(Debug, Clone)]
pub struct Row {
    pub schema_name: String,
    pub table_name: String,
    pub parent_schema: String,
    pub parent_name: String,
    pub partbound_def: String, // e.g. "FOR VALUES FROM ('2024-01-01') TO ('2025-01-01')"
}
```

- [ ] **Step 3: Register query modules**

Edit `crates/pgevolve-core/src/catalog/queries/mod.rs`. Add `pub mod partitioned_tables;` and `pub mod partitions;` in alphabetical order.

- [ ] **Step 4: Add CatalogQuery variants**

Edit `crates/pgevolve-core/src/catalog/mod.rs`. In the `CatalogQuery` enum, add:

```rust
    PartitionedTables,
    Partitions,
```

Wire them into the query dispatch — find where `CatalogQuery::Triggers` was added in TRG3 and mirror it exactly.

- [ ] **Step 5: Re-parse partition definitions in assemble.rs**

Edit `crates/pgevolve-core/src/catalog/assemble.rs`. Add two new fields to `RawRows`:

```rust
    pub partitioned_tables: Vec<crate::catalog::queries::partitioned_tables::Row>,
    pub partitions: Vec<crate::catalog::queries::partitions::Row>,
```

After all tables are built and pushed into the catalog, add a merge pass:

```rust
fn merge_partition_metadata(
    catalog: &mut crate::ir::catalog::Catalog,
    partitioned: &[crate::catalog::queries::partitioned_tables::Row],
    partitions: &[crate::catalog::queries::partitions::Row],
) -> Result<(), crate::catalog::AssembleError> {
    use crate::identifier::{Identifier, QualifiedName};

    for row in partitioned {
        let qname = QualifiedName::new(
            Identifier::from_unquoted(&row.schema_name)?,
            Identifier::from_unquoted(&row.table_name)?,
        );
        let table = catalog
            .tables
            .iter_mut()
            .find(|t| t.qname == qname)
            .ok_or_else(|| crate::catalog::AssembleError::Structural(format!(
                "partitioned-table row references missing table {qname}"
            )))?;
        // Re-parse partkey_def by wrapping it back into a CREATE TABLE
        // and pulling out the partition_by.
        let synthetic = format!(
            "CREATE TABLE _pgevolve_synth () PARTITION BY {};",
            row.partkey_def,
        );
        let parsed = pg_query::parse(&synthetic).map_err(|e| {
            crate::catalog::AssembleError::Structural(format!(
                "could not re-parse partkey_def {:?}: {e}",
                row.partkey_def
            ))
        })?;
        let stmt = match &parsed.protobuf.stmts[0].stmt.as_ref().unwrap().node {
            Some(pg_query::NodeEnum::CreateStmt(s)) => s,
            _ => return Err(crate::catalog::AssembleError::Structural("expected CreateStmt".into())),
        };
        let spec = stmt.partspec.as_ref().ok_or_else(|| {
            crate::catalog::AssembleError::Structural("synth lost partspec".into())
        })?;
        let partition_by = crate::parse::builder::create_table_stmt::build_partition_by(spec)
            .map_err(|e| crate::catalog::AssembleError::Structural(format!("{e}")))?;
        table.partition_by = Some(partition_by);
    }

    for row in partitions {
        let qname = QualifiedName::new(
            Identifier::from_unquoted(&row.schema_name)?,
            Identifier::from_unquoted(&row.table_name)?,
        );
        let parent = QualifiedName::new(
            Identifier::from_unquoted(&row.parent_schema)?,
            Identifier::from_unquoted(&row.parent_name)?,
        );
        let table = catalog
            .tables
            .iter_mut()
            .find(|t| t.qname == qname)
            .ok_or_else(|| crate::catalog::AssembleError::Structural(format!(
                "partition row references missing table {qname}"
            )))?;
        let synthetic = format!(
            "ALTER TABLE _pgevolve_synth ATTACH PARTITION _pgevolve_synth_child {};",
            row.partbound_def,
        );
        let parsed = pg_query::parse(&synthetic).map_err(|e| {
            crate::catalog::AssembleError::Structural(format!(
                "could not re-parse partbound_def {:?}: {e}",
                row.partbound_def
            ))
        })?;
        let stmt = match &parsed.protobuf.stmts[0].stmt.as_ref().unwrap().node {
            Some(pg_query::NodeEnum::AlterTableStmt(s)) => s,
            _ => return Err(crate::catalog::AssembleError::Structural("expected AlterTableStmt".into())),
        };
        let cmd = match &stmt.cmds[0].node {
            Some(pg_query::NodeEnum::AlterTableCmd(c)) => c,
            _ => return Err(crate::catalog::AssembleError::Structural("expected AlterTableCmd".into())),
        };
        let part_cmd = match &cmd.def.as_ref().unwrap().node {
            Some(pg_query::NodeEnum::PartitionCmd(p)) => p,
            _ => return Err(crate::catalog::AssembleError::Structural("expected PartitionCmd".into())),
        };
        let bounds = crate::parse::builder::create_table_stmt::build_partition_bounds(
            part_cmd.bound.as_ref().ok_or_else(|| crate::catalog::AssembleError::Structural("no bounds".into()))?
        )
        .map_err(|e| crate::catalog::AssembleError::Structural(format!("{e}")))?;
        table.partition_of = Some(crate::ir::partition::PartitionOf { parent, bounds });
    }

    Ok(())
}
```

Call `merge_partition_metadata(&mut catalog, &raw.partitioned_tables, &raw.partitions)?;` at the end of the assembly function, after all tables are pushed.

For this to compile, `build_partition_by` and `build_partition_bounds` from Task 2 must be `pub(crate)`. Adjust their visibility.

- [ ] **Step 6: Run tests + clippy**

```bash
cargo test -p pgevolve-core --lib catalog
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```

Expected: all green. Catalog integration tests will exercise this in Task 12 (conformance).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(catalog): read pg_partitioned_table + relispartition into Table IR

Two new catalog queries (partitioned_tables, partitions) merge into
the existing Table entries via a post-pass that re-parses the
PG-emitted partkey_def / partbound_def back through the source
parser. Single-source-of-truth pattern from triggers + indexes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Differ — partition_by/partition_of diff matrix

**Files:**
- Modify: `crates/pgevolve-core/src/diff/tables.rs`
- Modify: `crates/pgevolve-core/src/diff/change.rs`

- [ ] **Step 1: Add new TableChange variants**

Edit `crates/pgevolve-core/src/diff/change.rs`. Find the `TableChange` enum. Add:

```rust
    AttachPartition {
        parent: QualifiedName,
        child: QualifiedName,
        bounds: crate::ir::partition::PartitionBounds,
    },
    DetachPartition {
        parent: QualifiedName,
        child: QualifiedName,
    },
```

Both are `Destructiveness::Safe` (no data loss).

- [ ] **Step 2: Add a new ParseError-style variant for unsupported diff**

Check `diff/change.rs` and the differ's error type. If there's already a way to surface "this diff is unsupported, fail at plan time", use it. Otherwise add a new variant or use an existing one (probably there's a `DiffError::Unsupported { reason }` or similar — pattern-match the existing approach).

If diff currently can't fail (returns just `ChangeSet`), we need to add the error path. Look at how other irrecoverable diffs surface — search for existing `Err(` returns in `crates/pgevolve-core/src/diff/`. If none, the partition diff needs to push a synthetic "error" change that the planner converts to a hard failure. Simplest path: add a `Change::Error(String)` variant that the planner rejects at plan time.

Decision (locked in): use the existing lint-at-plan mechanism that triggers/extensions already use. Add a synthetic lint-at-plan finding with code `partition-by-change-unsupported`.

- [ ] **Step 3: Extend diff_tables**

Edit `crates/pgevolve-core/src/diff/tables.rs`. Find the pair-by-qname loop. After the existing column/constraint diff, add:

```rust
    // ---- partition_by diff ----
    match (&source.partition_by, &target.partition_by) {
        (None, None) => {}
        (Some(s), Some(t)) if s == t => {}
        (Some(_), Some(_)) => {
            // Different. Hard fail via the lint-at-plan path.
            changes.push_lint(LintAtPlan {
                code: "partition-by-change-unsupported".into(),
                message: format!(
                    "cannot change PARTITION BY clause on {} in-place; manual migration required",
                    source.qname
                ),
                location: Some(source.qname.to_string()),
            });
        }
        (Some(_), None) => {
            changes.push_lint(LintAtPlan {
                code: "partition-by-add-unsupported".into(),
                message: format!(
                    "cannot turn {} into a partitioned parent in-place; manual migration required",
                    source.qname
                ),
                location: Some(source.qname.to_string()),
            });
        }
        (None, Some(_)) => {
            changes.push_lint(LintAtPlan {
                code: "partition-by-remove-unsupported".into(),
                message: format!(
                    "cannot un-partition {} in-place; manual migration required",
                    source.qname
                ),
                location: Some(source.qname.to_string()),
            });
        }
    }

    // ---- partition_of diff ----
    match (&source.partition_of, &target.partition_of) {
        (None, None) => {}
        (Some(s), Some(t)) if s == t => {}
        (Some(s), None) => {
            // Source says it's a partition; catalog says it's standalone.
            // Emit ATTACH.
            changes.push(Change::Table(TableChange::AttachPartition {
                parent: s.parent.clone(),
                child: source.qname.clone(),
                bounds: s.bounds.clone(),
            }));
        }
        (None, Some(t)) => {
            // Source declares it standalone; catalog has it as a partition.
            // Emit DETACH.
            changes.push(Change::Table(TableChange::DetachPartition {
                parent: t.parent.clone(),
                child: source.qname.clone(),
            }));
        }
        (Some(s), Some(t)) if s.parent != t.parent => {
            // Re-parented. Detach from old, attach to new.
            changes.push(Change::Table(TableChange::DetachPartition {
                parent: t.parent.clone(),
                child: source.qname.clone(),
            }));
            changes.push(Change::Table(TableChange::AttachPartition {
                parent: s.parent.clone(),
                child: source.qname.clone(),
                bounds: s.bounds.clone(),
            }));
        }
        (Some(s), Some(t)) => {
            // Same parent, bounds differ.
            changes.push(Change::Table(TableChange::DetachPartition {
                parent: t.parent.clone(),
                child: source.qname.clone(),
            }));
            changes.push(Change::Table(TableChange::AttachPartition {
                parent: s.parent.clone(),
                child: source.qname.clone(),
                bounds: s.bounds.clone(),
            }));
        }
    }
```

Match the actual `ChangeSet` API — the method names `push` and `push_lint` are illustrative; copy whatever the trigger/extension diffs use.

- [ ] **Step 4: Unit tests**

Add to the existing `diff/tables.rs` test module:

```rust
    #[test]
    fn detects_attach_partition_when_source_declares_it() {
        // source says partition; catalog says standalone → AttachPartition
        let mut s = sample_table();
        s.partition_of = Some(po("app", "parent", PartitionBounds::List { values: vec![BoundDatum::Literal(litexpr("'us'"))] }));
        let t = sample_table();
        let mut cs = ChangeSet::default();
        diff_tables(&[s], &[t], &mut cs);
        assert!(matches!(
            cs.changes().first().unwrap(),
            Change::Table(TableChange::AttachPartition { .. })
        ));
    }

    #[test]
    fn detects_detach_partition_when_source_drops_declaration() {
        let s = sample_table();
        let mut t = sample_table();
        t.partition_of = Some(po("app", "parent", PartitionBounds::Default));
        let mut cs = ChangeSet::default();
        diff_tables(&[s], &[t], &mut cs);
        assert!(matches!(
            cs.changes().first().unwrap(),
            Change::Table(TableChange::DetachPartition { .. })
        ));
    }

    #[test]
    fn bounds_change_emits_detach_then_attach() {
        let mut s = sample_table();
        s.partition_of = Some(po("app", "parent", PartitionBounds::Range { from: vec![BoundDatum::MinValue], to: vec![BoundDatum::Literal(litexpr("10"))] }));
        let mut t = sample_table();
        t.partition_of = Some(po("app", "parent", PartitionBounds::Range { from: vec![BoundDatum::MinValue], to: vec![BoundDatum::Literal(litexpr("20"))] }));
        let mut cs = ChangeSet::default();
        diff_tables(&[s], &[t], &mut cs);
        let changes = cs.changes();
        assert!(matches!(changes[0], Change::Table(TableChange::DetachPartition { .. })));
        assert!(matches!(changes[1], Change::Table(TableChange::AttachPartition { .. })));
    }

    #[test]
    fn parent_partition_by_change_produces_lint_at_plan() {
        // source: parent partitioned by LIST; catalog: parent partitioned by RANGE
        let mut s = sample_table();
        s.partition_by = Some(pb_list("region"));
        let mut t = sample_table();
        t.partition_by = Some(pb_range("placed"));
        let mut cs = ChangeSet::default();
        diff_tables(&[s], &[t], &mut cs);
        // No changes — only a lint-at-plan finding.
        assert!(cs.lints().iter().any(|l| l.code == "partition-by-change-unsupported"));
    }
```

Adapt `sample_table`, `po`, `pb_list`, `pb_range`, `litexpr` to the test helpers that already exist in the file — the convention is `mk_*` or `test_*` in the trigger/extension test modules.

- [ ] **Step 5: Run tests + clippy**

```bash
cargo test -p pgevolve-core --lib diff::tables
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```

Expected: 4 new tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(diff): partition_by + partition_of diff matrix

Two new TableChange variants (AttachPartition, DetachPartition). Bound
changes emit detach+attach (data-preserving). Parent partition-by
changes surface as lint-at-plan findings (no in-place rekey).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Plan edges — child partition → parent

**Files:**
- Modify: `crates/pgevolve-core/src/plan/edges.rs`

- [ ] **Step 1: Add the edge**

Edit `crates/pgevolve-core/src/plan/edges.rs`. Find `build_create_graph` — the function that walks the catalog and emits dep edges between `NodeId`s.

In the loop over `catalog.tables`, add (after the existing column-type / FK edges):

```rust
    if let Some(po) = &table.partition_of {
        // The partition depends on its parent existing first.
        graph.add_edge(
            NodeId::Table(table.qname.clone()),
            NodeId::Table(po.parent.clone()),
        );
    }
```

- [ ] **Step 2: Test**

Add a unit test in the existing edges test module:

```rust
    #[test]
    fn partition_edges_to_parent() {
        let mut t_parent = sample_table_with_qname("app", "parent");
        t_parent.partition_by = Some(pb_list("region"));
        let mut t_child = sample_table_with_qname("app", "child");
        t_child.partition_of = Some(po("app", "parent", PartitionBounds::Default));
        let catalog = Catalog { tables: vec![t_parent, t_child], ..Catalog::empty() };
        let graph = build_create_graph(&catalog);
        assert!(graph.has_edge(
            &NodeId::Table(qn("app", "child")),
            &NodeId::Table(qn("app", "parent")),
        ));
    }
```

- [ ] **Step 3: Run tests + clippy**

```bash
cargo test -p pgevolve-core --lib plan::edges
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/edges.rs
git commit -m "$(cat <<'EOF'
feat(plan): partition child → parent edge in create graph

Adds an edge from each partition's NodeId::Table to its parent's
NodeId::Table so the topological sort emits parent before children.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: StepKind variants + ordering buckets

**Files:**
- Modify: `crates/pgevolve-core/src/plan/raw_step.rs`
- Modify: `crates/pgevolve-core/src/plan/plan.rs`
- Modify: `crates/pgevolve-core/src/plan/ordering.rs`

- [ ] **Step 1: Add StepKind variants**

Edit `crates/pgevolve-core/src/plan/raw_step.rs`. After the trigger step kinds, add:

```rust
    // --- v0.2 partition step kinds ---
    /// `ALTER TABLE parent ATTACH PARTITION child FOR VALUES ...`.
    AttachPartition,
    /// `ALTER TABLE parent DETACH PARTITION child`.
    DetachPartition,
```

If `raw_step.rs` has a serde round-trip test that enumerates every variant, add the two new ones there.

- [ ] **Step 2: kind_name / parse_kind_name tables**

Edit `crates/pgevolve-core/src/plan/plan.rs`. Find `kind_name` (or wherever StepKind → str lives). Add:

```rust
        StepKind::AttachPartition => "attach_partition",
        StepKind::DetachPartition => "detach_partition",
```

And the inverse in `parse_kind_name`:

```rust
        "attach_partition" => Some(StepKind::AttachPartition),
        "detach_partition" => Some(StepKind::DetachPartition),
```

- [ ] **Step 3: Ordering buckets**

Edit `crates/pgevolve-core/src/plan/ordering.rs`. Find the `partition()` function that routes `Change` variants into `creates` / `modifies` / `drops` buckets. Find the existing `Change::Table(_)` arm. The new partition sub-variants are MODIFIES (they don't create or drop the table itself, they only attach/detach):

```rust
        Change::Table(tc) => match tc {
            // ... existing variants stay where they are ...
            TableChange::AttachPartition { .. } | TableChange::DetachPartition { .. } => {
                modifies.push(entry);
            }
        },
```

Order within modifies: DETACH must come before ATTACH if both are emitted for the same child. Since they're already pushed in order by the differ (Task 5 step 3), the slice order is preserved through the bucket.

- [ ] **Step 4: Run tests + clippy**

```bash
cargo test -p pgevolve-core --lib plan::raw_step plan::ordering plan::plan
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(plan): AttachPartition + DetachPartition step kinds

Wires the two new TableChange variants through the ordering buckets
and the kind_name registry. Both route to the modifies bucket;
slice-order from the differ ensures DETACH precedes ATTACH when both
fire on the same child.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: SQL emission helpers

**Files:**
- Create: `crates/pgevolve-core/src/plan/rewrite/partitions.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/tables.rs`

- [ ] **Step 1: Write the helpers**

Create `crates/pgevolve-core/src/plan/rewrite/partitions.rs`:

```rust
//! SQL emission for partition operations.

use crate::identifier::QualifiedName;
use crate::ir::partition::{
    BoundDatum, PartitionBounds, PartitionBy, PartitionColumnKind, PartitionOf, PartitionStrategy,
};

pub(crate) fn attach_partition(
    parent: &QualifiedName,
    child: &QualifiedName,
    bounds: &PartitionBounds,
) -> String {
    format!(
        "ALTER TABLE {} ATTACH PARTITION {} {};",
        parent.render_sql(),
        child.render_sql(),
        render_for_values(bounds),
    )
}

pub(crate) fn detach_partition(parent: &QualifiedName, child: &QualifiedName) -> String {
    format!(
        "ALTER TABLE {} DETACH PARTITION {};",
        parent.render_sql(),
        child.render_sql(),
    )
}

pub(crate) fn render_partition_by(pb: &PartitionBy) -> String {
    let mut out = String::from("PARTITION BY ");
    out.push_str(match pb.strategy {
        PartitionStrategy::Range => "RANGE",
        PartitionStrategy::List => "LIST",
        PartitionStrategy::Hash => "HASH",
    });
    out.push_str(" (");
    let cols: Vec<String> = pb.columns.iter().map(render_partition_column).collect();
    out.push_str(&cols.join(", "));
    out.push(')');
    out
}

pub(crate) fn render_partition_of(po: &PartitionOf) -> String {
    format!(
        "PARTITION OF {} {}",
        po.parent.render_sql(),
        render_for_values(&po.bounds),
    )
}

fn render_partition_column(col: &crate::ir::partition::PartitionColumn) -> String {
    let mut s = match &col.kind {
        PartitionColumnKind::Column(name) => name.as_str().to_string(),
        PartitionColumnKind::Expr(e) => format!("({})", e.canonical_text),
    };
    if let Some(coll) = &col.collation {
        s.push_str(" COLLATE ");
        s.push_str(&coll.render_sql());
    }
    if let Some(op) = &col.opclass {
        s.push(' ');
        s.push_str(&op.render_sql());
    }
    s
}

pub(crate) fn render_for_values(bounds: &PartitionBounds) -> String {
    match bounds {
        PartitionBounds::Default => "DEFAULT".to_string(),
        PartitionBounds::Hash { modulus, remainder } => {
            format!("FOR VALUES WITH (MODULUS {modulus}, REMAINDER {remainder})")
        }
        PartitionBounds::List { values } => {
            let parts: Vec<String> = values.iter().map(render_bound_datum).collect();
            format!("FOR VALUES IN ({})", parts.join(", "))
        }
        PartitionBounds::Range { from, to } => {
            let f: Vec<String> = from.iter().map(render_bound_datum).collect();
            let t: Vec<String> = to.iter().map(render_bound_datum).collect();
            format!("FOR VALUES FROM ({}) TO ({})", f.join(", "), t.join(", "))
        }
    }
}

fn render_bound_datum(d: &BoundDatum) -> String {
    match d {
        BoundDatum::Literal(expr) => expr.canonical_text.clone(),
        BoundDatum::MinValue => "MINVALUE".to_string(),
        BoundDatum::MaxValue => "MAXVALUE".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};

    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(
            Identifier::from_unquoted(s).unwrap(),
            Identifier::from_unquoted(n).unwrap(),
        )
    }

    fn lit(s: &str) -> crate::ir::default_expr::NormalizedExpr {
        crate::ir::default_expr::NormalizedExpr {
            canonical_text: s.to_string(),
            canonical_hash: [0u8; 32],
        }
    }

    #[test]
    fn attach_range_renders() {
        let parent = qn("app", "orders");
        let child = qn("app", "orders_2024");
        let bounds = PartitionBounds::Range {
            from: vec![BoundDatum::Literal(lit("'2024-01-01'"))],
            to: vec![BoundDatum::Literal(lit("'2025-01-01'"))],
        };
        let s = attach_partition(&parent, &child, &bounds);
        assert_eq!(
            s,
            "ALTER TABLE app.orders ATTACH PARTITION app.orders_2024 FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');"
        );
    }

    #[test]
    fn detach_renders() {
        assert_eq!(
            detach_partition(&qn("app", "orders"), &qn("app", "orders_2024")),
            "ALTER TABLE app.orders DETACH PARTITION app.orders_2024;"
        );
    }

    #[test]
    fn list_default_renders() {
        let s = render_for_values(&PartitionBounds::Default);
        assert_eq!(s, "DEFAULT");
    }

    #[test]
    fn hash_renders() {
        let s = render_for_values(&PartitionBounds::Hash { modulus: 4, remainder: 1 });
        assert_eq!(s, "FOR VALUES WITH (MODULUS 4, REMAINDER 1)");
    }

    #[test]
    fn minvalue_maxvalue_render() {
        let b = PartitionBounds::Range {
            from: vec![BoundDatum::MinValue],
            to: vec![BoundDatum::MaxValue],
        };
        assert_eq!(render_for_values(&b), "FOR VALUES FROM (MINVALUE) TO (MAXVALUE)");
    }

    #[test]
    fn partition_by_list_column_renders() {
        let pb = PartitionBy {
            strategy: PartitionStrategy::List,
            columns: vec![crate::ir::partition::PartitionColumn {
                kind: PartitionColumnKind::Column(Identifier::from_unquoted("region").unwrap()),
                collation: None,
                opclass: None,
            }],
        };
        assert_eq!(render_partition_by(&pb), "PARTITION BY LIST (region)");
    }
}
```

- [ ] **Step 2: Register the module**

Edit `crates/pgevolve-core/src/plan/rewrite/mod.rs`. Add `pub(crate) mod partitions;` in alphabetical order with the other rewrite helpers.

- [ ] **Step 3: Extend create_table emitter**

Edit `crates/pgevolve-core/src/plan/rewrite/tables.rs`. Find the `create_table` function that renders a `CREATE TABLE ...` statement.

After the column/constraint list closes (the `)`) and before the trailing `;`, append:

```rust
    // Form 2: declarative partition.
    if let Some(po) = &table.partition_of {
        sql.push(' ');
        sql.push_str(&crate::plan::rewrite::partitions::render_partition_of(po));
    }
    // Partitioned parent or sub-partition.
    if let Some(pb) = &table.partition_by {
        sql.push(' ');
        sql.push_str(&crate::plan::rewrite::partitions::render_partition_by(pb));
    }
```

If `table.partition_of.is_some()`, the existing code that renders `(col1 type, col2 type, ...)` must skip the column list — partitions inherit columns from the parent. Check the existing emitter; either:
- Add a guard: `if table.partition_of.is_none() { /* emit columns */ }`, OR
- Emit an empty `()` if the columns are empty when `partition_of.is_some()`.

PG syntax for declarative partitions: `CREATE TABLE child PARTITION OF parent FOR VALUES ...;` — no column list, no parens. Adjust the emitter accordingly.

- [ ] **Step 4: Run tests + clippy**

```bash
cargo test -p pgevolve-core --lib plan::rewrite
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```

Expected: 6 new partition tests pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(rewrite): SQL emission helpers for partitions

attach_partition / detach_partition / render_partition_by /
render_partition_of / render_for_values. Extends create_table to
emit PARTITION OF / PARTITION BY clauses; partitions inherit columns
from parent so the column list is suppressed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: emit/partition.rs dispatcher (14th family file)

**Files:**
- Create: `crates/pgevolve-core/src/plan/rewrite/emit/partition.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/mod.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/table.rs` (route new sub-variants)

- [ ] **Step 1: Inspect the existing dispatch shape**

Read `crates/pgevolve-core/src/plan/rewrite/emit/table.rs` (existing). It handles `TableChange` variants — Create, Drop, AlterColumn, etc. Add the two new arms.

Decision: rather than a separate 14th dispatcher file, **extend the existing `emit/table.rs`** with two new `TableChange` arms. Partitions are still tables — they ride the same family dispatcher. The "14th family file" the spec mentioned was a fallback; cleaner to extend the existing one.

In `emit/table.rs`, find the match on `TableChange`. Add:

```rust
        TableChange::AttachPartition { parent, child, bounds } => {
            out.push(RawStep {
                kind: StepKind::AttachPartition,
                sql: crate::plan::rewrite::partitions::attach_partition(parent, child, bounds),
                destructive: false,
                destructive_reason: None,
                targets: vec![Target::Table(child.clone())],
            });
        }
        TableChange::DetachPartition { parent, child } => {
            out.push(RawStep {
                kind: StepKind::DetachPartition,
                sql: crate::plan::rewrite::partitions::detach_partition(parent, child),
                destructive: false,
                destructive_reason: None,
                targets: vec![Target::Table(child.clone())],
            });
        }
```

Match the exact `RawStep` field shape that the existing arms use — copy the convention.

- [ ] **Step 2: Tests**

Add to the test module in `emit/table.rs`:

```rust
    #[test]
    fn attach_emits_alter_table() {
        let change = TableChange::AttachPartition {
            parent: qn("app", "orders"),
            child: qn("app", "orders_2024"),
            bounds: PartitionBounds::Default,
        };
        let mut out = Vec::new();
        emit_table(&change, false, None, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::AttachPartition);
        assert!(out[0].sql.contains("ATTACH PARTITION"));
    }

    #[test]
    fn detach_emits_alter_table() {
        let change = TableChange::DetachPartition {
            parent: qn("app", "orders"),
            child: qn("app", "orders_2024"),
        };
        let mut out = Vec::new();
        emit_table(&change, false, None, &mut out);
        assert_eq!(out[0].kind, StepKind::DetachPartition);
        assert!(out[0].sql.contains("DETACH PARTITION"));
    }
```

- [ ] **Step 3: Run tests + clippy**

```bash
cargo test -p pgevolve-core --lib plan::rewrite::emit::table
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(rewrite): emit AttachPartition / DetachPartition steps

Extends emit/table.rs (partitions are tables — they ride the same
family dispatcher). Both step kinds are non-destructive: PG validates
bounds at attach time and detach preserves data.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Lint — partition-references-unmanaged-parent

**Files:**
- Modify: `crates/pgevolve-core/src/lint/universal.rs`

- [ ] **Step 1: Add the lint rule**

Edit `crates/pgevolve-core/src/lint/universal.rs`. Model on the
`trigger_references_unmanaged_table_rule` from TRG9 (commit
`8d7cd57`). Add:

```rust
fn partition_references_unmanaged_parent_rule(
    catalog: &Catalog,
    findings: &mut Vec<Finding>,
) {
    for t in &catalog.tables {
        let Some(po) = &t.partition_of else { continue };
        let found = catalog.tables.iter().any(|other| other.qname == po.parent);
        if !found {
            findings.push(Finding {
                code: "partition-references-unmanaged-parent".into(),
                severity: LintSeverity::Error,
                message: format!(
                    "partition `{}` references parent `{}`, which is not declared in this project's managed schema",
                    t.qname, po.parent
                ),
                location: Some(t.qname.to_string()),
            });
        }
    }
}
```

Register it in `check_universal` alongside the trigger lint rules.

- [ ] **Step 2: Tests**

Add three tests in the same file:

```rust
    #[test]
    fn partition_with_managed_parent_no_finding() {
        let mut parent = mk_table("app", "orders");
        parent.partition_by = Some(pb_list("region"));
        let mut child = mk_table("app", "orders_us");
        child.partition_of = Some(po("app", "orders", PartitionBounds::Default));
        let catalog = Catalog { tables: vec![parent, child], ..Catalog::empty() };
        let mut findings = Vec::new();
        partition_references_unmanaged_parent_rule(&catalog, &mut findings);
        assert!(findings.is_empty());
    }

    #[test]
    fn partition_with_unmanaged_parent_fires() {
        let mut child = mk_table("app", "orders_us");
        child.partition_of = Some(po("external", "orders", PartitionBounds::Default));
        let catalog = Catalog { tables: vec![child], ..Catalog::empty() };
        let mut findings = Vec::new();
        partition_references_unmanaged_parent_rule(&catalog, &mut findings);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, "partition-references-unmanaged-parent");
    }

    #[test]
    fn non_partition_tables_ignored() {
        let t = mk_table("app", "regular");
        let catalog = Catalog { tables: vec![t], ..Catalog::empty() };
        let mut findings = Vec::new();
        partition_references_unmanaged_parent_rule(&catalog, &mut findings);
        assert!(findings.is_empty());
    }
```

Use the same test helpers (`mk_table`, `pb_list`, `po`, etc.) as in TRG9's tests — read those for the exact names.

- [ ] **Step 3: Run tests + clippy**

```bash
cargo test -p pgevolve-core --lib lint::universal
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/lint/universal.rs
git commit -m "$(cat <<'EOF'
feat(lint): partition-references-unmanaged-parent

Error-severity universal lint: a partition's PARTITION OF target
must be in the managed catalog. Matches the same-shape lints for
triggers and function bodies.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Exhaustive-match fixups across the workspace

**Files:**
- Modify: `crates/pgevolve/src/commands/diff.rs` (change_kind_name, print_human)
- Modify: any other crate that exhaustively matches `TableChange` or `StepKind`

- [ ] **Step 1: Run workspace clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tee /tmp/clippy.out
```

Note every warning about missing `AttachPartition` / `DetachPartition` arms. Most likely sites:
- `crates/pgevolve/src/commands/diff.rs` — `change_kind_name`, `print_human`
- `crates/pgevolve-core/src/plan/rewrite/mod.rs` — already routed via Task 9
- `crates/pgevolve/src/commands/graph.rs` — if it switches on `StepKind`

- [ ] **Step 2: Upgrade diff.rs**

In `change_kind_name`:

```rust
        TableChange::AttachPartition { .. } => "AttachPartition",
        TableChange::DetachPartition { .. } => "DetachPartition",
```

In `print_human`:

```rust
        TableChange::AttachPartition { parent, child, .. } => {
            println!("  attach partition {child} to {parent}");
        }
        TableChange::DetachPartition { parent, child } => {
            println!("  detach partition {child} from {parent}");
        }
```

Match the exact voice the other existing arms use (with or without bullets — copy the convention).

- [ ] **Step 3: Verify**

```bash
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
chore(partitions): workspace exhaustive-match fixups

Diff CLI rendering for AttachPartition / DetachPartition plus any
remaining workspace match sites that needed teaching about the new
TableChange variants.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Conformance fixtures

**Files:**
- Create: ~14 fixtures under `crates/pgevolve-conformance/tests/cases/objects/partitions/`

- [ ] **Step 1: Read the established fixture shape**

Look at `crates/pgevolve-conformance/tests/cases/objects/triggers/create-row-trigger-simple/` for the gold-standard layout: `before.sql`, `after.sql`, `fixture.toml`, `expected/plan.sql`, `expected/dep-graph.dot`.

- [ ] **Step 2: Write the 14 fixtures**

Create these directories under `crates/pgevolve-conformance/tests/cases/objects/partitions/`:

1. **`create-range-parent-and-two-partitions/`** — parent + 2 RANGE partitions in after.sql. Steps = 3 (1 parent + 2 children).

2. **`create-list-parent/`** — LIST strategy, single partition. Steps = 2.

3. **`create-hash-parent-and-partitions/`** — HASH with MODULUS 4 + 4 partitions (MOD 4 REM 0/1/2/3). Steps = 5.

4. **`create-default-partition/`** — LIST parent + 1 regular + 1 DEFAULT partition. Steps = 3.

5. **`add-partition/`** — before: parent + 1 partition. after: same + 1 new partition. Steps = 1 (just the new CREATE TABLE PARTITION OF).

6. **`drop-partition/`** — before: parent + 2 partitions. after: parent + 1 partition. Steps = 1 (drop the removed partition; intent-gated if drop-requires-intent is on).

7. **`replace-bounds/`** — partition bounds change FROM '2024-01-01' TO '2025-01-01' → FROM '2024-01-01' TO '2026-01-01'. Steps = 2 (detach + attach).

8. **`attach-existing-standalone/`** — before.sql has a regular table; after.sql declares it `PARTITION OF parent`. Steps = 1 (attach).

9. **`detach-to-standalone/`** — symmetric reverse. Steps = 1 (detach).

10. **`subpartitioned/`** — parent → child (itself PARTITION BY) → grandchildren. Steps = depends on shape (≥3).

11. **`lint-unmanaged-parent/`** — partition declares `PARTITION OF external.t`, parent not in source. Expects the lint to fire with `expect.apply.succeeds = false`.

12. **`reject-rekey/`** — source changes a parent's strategy from RANGE to LIST. Expects `partition-by-change-unsupported` lint-at-plan finding. Failure-mode fixture under `failure/lint-at-plan/`.

13. **`reject-partition-to-nonpartitioned/`** — source removes PARTITION BY from a parent. Expects `partition-by-remove-unsupported` lint-at-plan finding.

14. **`attach-form-vs-declarative-form-equivalent/`** — before.sql uses `CREATE TABLE child (...); ALTER TABLE parent ATTACH PARTITION child ...;` (Form 3). after.sql uses `CREATE TABLE child PARTITION OF parent ...` (Form 2). End-state identical → empty plan.

For each, write `before.sql`, `after.sql`, and `fixture.toml`. The `fixture.toml` minimum:

```toml
[meta]
title     = "Partitioning — <case description>"
authoring = "objects"
spec_refs = ["partitioning.<topic>"]

[pg]
min = 14
max = 17

[expect.plan]
steps = N
```

Reference: how `objects/triggers/comment-on/fixture.toml` is shaped.

- [ ] **Step 3: Bless expected outputs**

```bash
cargo xtask bless --conformance
```

This generates `expected/plan.sql` and `expected/dep-graph.dot` for each fixture by running the planner.

- [ ] **Step 4: Run the conformance suite**

```bash
cargo test -p pgevolve-conformance
```

Expected: all green. Iterate on SQL syntax / fixture.toml until passing.

- [ ] **Step 5: Run full workspace tests + clippy + fmt**

```bash
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
test(conformance): 14 partitioning fixtures + bless

Covers create (range/list/hash/default), add, drop, bounds-replace,
attach-existing, detach-to-standalone, sub-partitioned, both reject
paths (rekey, un-partition), lint-unmanaged-parent, and Form 2 vs
Form 3 equivalence.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Docs (README, CHANGELOG, objects spec)

**Files:**
- Modify: `README.md`
- Modify: `CHANGELOG.md`
- Modify: `docs/spec/objects.md`
- Possibly: `docs/spec/pipeline.md` (if it lists object-kind coverage)

- [ ] **Step 1: Read the existing docs**

Identify the v0.2 sub-spec rows in README and CHANGELOG. The triggers entry (commit `4402bb3`) is the freshest precedent — match its voice.

- [ ] **Step 2: Update README**

Find the v0.2 status block. Change the partitioning row from "📋 Planned" to "✅ Landed `<commit-hash>`". Add a `### v0.2 partitioning` summary section with the same shape as the triggers section.

- [ ] **Step 3: Update CHANGELOG**

Add four sections under the v0.2.0 entry, mirroring the triggers format:
- `Added — IR (partitioning)` — partition_by, partition_of fields.
- `Added — pipeline (partitioning)` — three syntactic forms unified, AttachPartition/DetachPartition steps, dep edges.
- `Added — lint rules (partitioning)` — partition-references-unmanaged-parent.
- `Added — tests (partitioning)` — 14 conformance fixtures.

- [ ] **Step 4: Update docs/spec/objects.md**

Add a full Partitioning section after Triggers. Mirror the Triggers section's structure: IR shape, parser support, catalog reader, differ behavior, planner steps, dep edges, lint rules, out-of-scope.

- [ ] **Step 5: Update docs/spec/pipeline.md if applicable**

If the pipeline doc lists object-kind coverage, mark partitioning as Implemented.

- [ ] **Step 6: Verify**

`grep -ni "partition" README.md CHANGELOG.md docs/spec/*.md` and sanity-check the references make sense.

- [ ] **Step 7: Commit**

```bash
git add README.md CHANGELOG.md docs/spec/
git commit -m "$(cat <<'EOF'
docs: partitioning — README, CHANGELOG, objects spec

Marks v0.2 sub-spec #6 as landed and documents the partitioning IR,
parser, catalog reader, differ, planner, edges, lints, and
out-of-scope items.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Workspace verification + push

- [ ] **Step 1: Full lib tests**

```bash
cargo test --workspace --lib
```

Expected: all green.

- [ ] **Step 2: Full workspace tests (integration + golden)**

```bash
cargo test --workspace --all-targets
```

Expected: all green. If catalog goldens fail because of the new `partition_by`/`partition_of` fields on Table being added, run:

```bash
cargo xtask bless
git add crates/pgevolve-core/tests/fixtures/catalog/
git commit -m "test(catalog): re-bless Tier-3 catalog goldens for partition fields"
```

- [ ] **Step 3: Clippy + fmt**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

- [ ] **Step 4: Property tests (Tier-5)**

```bash
cargo test --test property_tests -- --ignored
```

Run 3 times consecutively to catch flakes:

```bash
for i in 1 2 3; do cargo test --test property_tests -- --ignored 2>&1 | tail -2; done
```

- [ ] **Step 5: Conformance suite**

```bash
cargo test -p pgevolve-conformance
```

- [ ] **Step 6: Push**

```bash
git push origin main
```

---

## Self-review checklist

- [ ] Every spec section has a task: IR (T1), parser forms 1+2+3 (T2+T3), catalog (T4), differ (T5), edges (T6), step kinds + ordering (T7), SQL emitter (T8), per-family dispatch (T9), lint (T10), exhaustive-match (T11), fixtures (T12), docs (T13), verify (T14). ✅
- [ ] No "TBD" or "implement later" placeholders. ✅
- [ ] Type and function names consistent across tasks: `partition_by`, `partition_of`, `PartitionBy`, `PartitionOf`, `PartitionBounds`, `BoundDatum`, `AttachPartition`, `DetachPartition`. ✅
- [ ] Tasks ordered by dependency: IR → parser → catalog → diff → plan → emit → lint → polish → fixtures → docs → verify. ✅
- [ ] Each task ends with tests + commit. ✅
