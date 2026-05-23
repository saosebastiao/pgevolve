//! Column-level diffing inside an `AlterTable`.
//!
//! Pairs columns by name. Logical column order is intentionally **not**
//! enforced in v0.1 — the IR records it, but reordering would require a table
//! rewrite. If the user wants a specific physical order, they must drop and
//! recreate the table.
//!
//! ## Widening whitelist
//!
//! `is_widening` returns true only for type changes Postgres can perform
//! without rewriting data: `int2 → int4 → int8`, `varchar(N) → varchar(M>N)`,
//! `varchar(*) → text`. Everything else is conservative and tagged
//! [`Destructiveness::RequiresApprovalAndDataLossWarning`].

use std::collections::BTreeMap;

use crate::identifier::Identifier;
use crate::ir::canon::filter_pg_defaults::type_default_storage;
use crate::ir::column::Column;
use crate::ir::column_type::ColumnType;
use crate::ir::table::Table;

use super::destructiveness::Destructiveness;
use super::table_op::{TableOp, TableOpEntry};

/// Diff columns in `target` against `source`, appending entries to `out`.
pub fn diff_columns(target: &Table, source: &Table, out: &mut Vec<TableOpEntry>) {
    let target_map: BTreeMap<&Identifier, &Column> =
        target.columns.iter().map(|c| (&c.name, c)).collect();
    let source_map: BTreeMap<&Identifier, &Column> =
        source.columns.iter().map(|c| (&c.name, c)).collect();

    for (name, source_col) in &source_map {
        if !target_map.contains_key(name) {
            out.push(add_column_entry((*source_col).clone()));
        }
    }

    for (name, target_col) in &target_map {
        match source_map.get(name) {
            None => {
                out.push(TableOpEntry {
                    op: TableOp::DropColumn {
                        name: (*name).clone(),
                        is_populated: false,
                    },
                    destructiveness: Destructiveness::RequiresApprovalAndDataLossWarning {
                        reason: format!("drops column {name}"),
                    },
                });
            }
            Some(source_col) => {
                diff_column(target_col, source_col, out);
            }
        }
    }
}

fn add_column_entry(col: Column) -> TableOpEntry {
    let safe = col.nullable || col.default.is_some();
    let destructiveness = if safe {
        Destructiveness::Safe
    } else {
        Destructiveness::RequiresApproval {
            reason: format!(
                "adds NOT NULL column {} with no default — fails on a non-empty table",
                col.name
            ),
        }
    };
    TableOpEntry {
        op: TableOp::AddColumn(col),
        destructiveness,
    }
}

// Each attribute's diff block is a self-contained guard + push; extracting
// them into sub-functions would scatter the logic across many tiny helpers
// without reducing cognitive load.
#[allow(clippy::too_many_lines)]
fn diff_column(target: &Column, source: &Column, out: &mut Vec<TableOpEntry>) {
    if target.ty != source.ty {
        let destructiveness = if is_widening(&target.ty, &source.ty) {
            Destructiveness::Safe
        } else {
            Destructiveness::RequiresApprovalAndDataLossWarning {
                reason: format!(
                    "narrows or changes type family on column {} ({} -> {})",
                    target.name,
                    target.ty.render_sql(),
                    source.ty.render_sql()
                ),
            }
        };
        out.push(TableOpEntry {
            op: TableOp::AlterColumnType {
                name: target.name.clone(),
                from: target.ty.clone(),
                to: source.ty.clone(),
                using: None,
            },
            destructiveness,
        });
    }

    if target.nullable != source.nullable {
        let destructiveness = if source.nullable {
            // Going to nullable is always safe.
            Destructiveness::Safe
        } else {
            Destructiveness::RequiresApproval {
                reason: format!(
                    "SET NOT NULL on {} may fail if column has NULL values",
                    target.name
                ),
            }
        };
        out.push(TableOpEntry {
            op: TableOp::SetColumnNullable {
                name: target.name.clone(),
                nullable: source.nullable,
            },
            destructiveness,
        });
    }

    if target.default != source.default {
        out.push(TableOpEntry {
            op: TableOp::SetColumnDefault {
                name: target.name.clone(),
                default: source.default.clone(),
            },
            destructiveness: Destructiveness::Safe,
        });
    }

    if target.identity != source.identity {
        out.push(TableOpEntry {
            op: TableOp::SetColumnIdentity {
                name: target.name.clone(),
                identity: source.identity.clone(),
            },
            destructiveness: Destructiveness::RequiresApproval {
                reason: "identity changes can fail or rewrite data".into(),
            },
        });
    }

    if target.generated != source.generated {
        out.push(TableOpEntry {
            op: TableOp::SetColumnGenerated {
                name: target.name.clone(),
                generated: source.generated.clone(),
            },
            destructiveness: Destructiveness::RequiresApproval {
                reason: "generated-column changes can fail or rewrite data".into(),
            },
        });
    }

    if target.comment != source.comment {
        out.push(TableOpEntry {
            op: TableOp::SetColumnComment {
                name: target.name.clone(),
                comment: source.comment.clone(),
            },
            destructiveness: Destructiveness::Safe,
        });
    }

    // Storage: resolve None to the type default on both sides so the
    // emitted op is always explicit. Carries both `from` and `to` so the
    // lint rule can detect downgrades.
    {
        let from = target
            .storage
            .unwrap_or_else(|| type_default_storage(&target.ty));
        let to = source
            .storage
            .unwrap_or_else(|| type_default_storage(&source.ty));
        if from != to {
            out.push(TableOpEntry {
                op: TableOp::SetColumnStorage {
                    name: target.name.clone(),
                    from,
                    to,
                },
                destructiveness: Destructiveness::Safe,
            });
        }
    }

    if target.compression != source.compression {
        out.push(TableOpEntry {
            op: TableOp::SetColumnCompression {
                name: target.name.clone(),
                compression: source.compression,
            },
            destructiveness: Destructiveness::Safe,
        });
    }
}

/// Postgres can perform these type changes in place without rewriting data.
///
/// `char(N)` is intentionally absent from the whitelist: char→varchar/text
/// changes semantics (pg right-pads char), and char(N)→char(M) is treated
/// like other length changes — conservative for v0.1.
//
// Each arm is left as a separate case to keep the whitelist readable; the
// extra `match_same_arms` lint hurts readability here without giving us
// anything in return.
#[allow(clippy::match_same_arms)]
fn is_widening(from: &ColumnType, to: &ColumnType) -> bool {
    use ColumnType::{BigInt, Integer, SmallInt, Text, Varchar};
    match (from, to) {
        // Integer family: small → larger only.
        (SmallInt, Integer | BigInt) => true,
        (Integer, BigInt) => true,
        // varchar(N) → varchar(M) when M > N.
        (Varchar { len: Some(n) }, Varchar { len: Some(m) }) => m > n,
        // varchar(N) → unbounded varchar.
        (Varchar { len: Some(_) }, Varchar { len: None }) => true,
        // varchar(N) → text (text is unbounded).
        (Varchar { len: Some(_) }, Text) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::QualifiedName;
    use crate::ir::column::{Compression, StorageKind};
    use crate::ir::default_expr::{DefaultExpr, LiteralValue};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(name: &str) -> QualifiedName {
        QualifiedName::new(id("app"), id(name))
    }

    fn col(name: &str, ty: ColumnType, nullable: bool) -> Column {
        Column {
            name: id(name),
            ty,
            nullable,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        }
    }

    fn tbl(cols: Vec<Column>) -> Table {
        Table {
            qname: qn("users"),
            columns: cols,
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        }
    }

    fn diff_one(target: &Table, source: &Table) -> Vec<TableOpEntry> {
        let mut out = Vec::new();
        diff_columns(target, source, &mut out);
        out
    }

    // ---- adds ----

    #[test]
    fn add_nullable_column_is_safe() {
        let target = tbl(vec![col("id", ColumnType::BigInt, false)]);
        let source = tbl(vec![
            col("id", ColumnType::BigInt, false),
            col("email", ColumnType::Text, true),
        ]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0].op, TableOp::AddColumn(_)));
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn add_not_null_column_with_default_is_safe() {
        let target = tbl(vec![col("id", ColumnType::BigInt, false)]);
        let mut new_col = col("created", ColumnType::BigInt, false);
        new_col.default = Some(DefaultExpr::Literal(LiteralValue::Integer(0)));
        let source = tbl(vec![col("id", ColumnType::BigInt, false), new_col]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn add_not_null_column_without_default_requires_approval() {
        let target = tbl(vec![col("id", ColumnType::BigInt, false)]);
        let source = tbl(vec![
            col("id", ColumnType::BigInt, false),
            col("email", ColumnType::Text, false),
        ]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(ops[0].destructiveness.requires_approval());
        assert!(!ops[0].destructiveness.data_loss_risk());
    }

    // ---- drops ----

    #[test]
    fn drop_column_is_data_loss_warning() {
        let target = tbl(vec![
            col("id", ColumnType::BigInt, false),
            col("email", ColumnType::Text, true),
        ]);
        let source = tbl(vec![col("id", ColumnType::BigInt, false)]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0].op, TableOp::DropColumn { .. }));
        assert!(ops[0].destructiveness.data_loss_risk());
    }

    // ---- type changes / widening ----

    #[test]
    fn integer_to_bigint_is_widening_safe() {
        let target = tbl(vec![col("c", ColumnType::Integer, true)]);
        let source = tbl(vec![col("c", ColumnType::BigInt, true)]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0].op, TableOp::AlterColumnType { .. }));
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn smallint_to_bigint_is_widening_safe() {
        let target = tbl(vec![col("c", ColumnType::SmallInt, true)]);
        let source = tbl(vec![col("c", ColumnType::BigInt, true)]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn bigint_to_integer_is_data_loss() {
        let target = tbl(vec![col("c", ColumnType::BigInt, true)]);
        let source = tbl(vec![col("c", ColumnType::Integer, true)]);
        let ops = diff_one(&target, &source);
        assert!(ops[0].destructiveness.data_loss_risk());
    }

    #[test]
    fn varchar_widening_is_safe() {
        let target = tbl(vec![col("c", ColumnType::Varchar { len: Some(50) }, true)]);
        let source = tbl(vec![col("c", ColumnType::Varchar { len: Some(100) }, true)]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn varchar_to_unbounded_is_safe() {
        let target = tbl(vec![col("c", ColumnType::Varchar { len: Some(50) }, true)]);
        let source = tbl(vec![col("c", ColumnType::Varchar { len: None }, true)]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn varchar_to_text_is_safe() {
        let target = tbl(vec![col("c", ColumnType::Varchar { len: Some(50) }, true)]);
        let source = tbl(vec![col("c", ColumnType::Text, true)]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn varchar_narrowing_is_data_loss() {
        let target = tbl(vec![col("c", ColumnType::Varchar { len: Some(100) }, true)]);
        let source = tbl(vec![col("c", ColumnType::Varchar { len: Some(50) }, true)]);
        let ops = diff_one(&target, &source);
        assert!(ops[0].destructiveness.data_loss_risk());
    }

    #[test]
    fn cross_family_change_is_data_loss() {
        let target = tbl(vec![col("c", ColumnType::Text, true)]);
        let source = tbl(vec![col("c", ColumnType::Integer, true)]);
        let ops = diff_one(&target, &source);
        assert!(ops[0].destructiveness.data_loss_risk());
    }

    // ---- nullability ----

    #[test]
    fn drop_not_null_is_safe() {
        let target = tbl(vec![col("c", ColumnType::Text, false)]);
        let source = tbl(vec![col("c", ColumnType::Text, true)]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops.len(), 1);
        match &ops[0].op {
            TableOp::SetColumnNullable { nullable, .. } => assert!(*nullable),
            other => panic!("got {other:?}"),
        }
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn set_not_null_requires_approval() {
        let target = tbl(vec![col("c", ColumnType::Text, true)]);
        let source = tbl(vec![col("c", ColumnType::Text, false)]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(ops[0].destructiveness.requires_approval());
        assert!(!ops[0].destructiveness.data_loss_risk());
    }

    // ---- defaults ----

    #[test]
    fn set_default_is_safe() {
        let target = tbl(vec![col("c", ColumnType::Integer, true)]);
        let mut to = col("c", ColumnType::Integer, true);
        to.default = Some(DefaultExpr::Literal(LiteralValue::Integer(0)));
        let source = tbl(vec![to]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0].op, TableOp::SetColumnDefault { .. }));
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn drop_default_is_safe() {
        let mut from = col("c", ColumnType::Integer, true);
        from.default = Some(DefaultExpr::Literal(LiteralValue::Integer(0)));
        let target = tbl(vec![from]);
        let source = tbl(vec![col("c", ColumnType::Integer, true)]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops.len(), 1);
        match &ops[0].op {
            TableOp::SetColumnDefault { default, .. } => assert!(default.is_none()),
            other => panic!("got {other:?}"),
        }
    }

    // ---- identity / generated ----

    #[test]
    fn identity_change_requires_approval() {
        use crate::ir::column::{Identity, IdentityKind, SequenceOptions};
        let target = tbl(vec![col("c", ColumnType::BigInt, false)]);
        let mut to = col("c", ColumnType::BigInt, false);
        to.identity = Some(Identity {
            kind: IdentityKind::ByDefault,
            sequence: SequenceOptions {
                start: 1,
                increment: 1,
                min_value: None,
                max_value: None,
                cache: 1,
                cycle: false,
            },
        });
        let source = tbl(vec![to]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(ops[0].destructiveness.requires_approval());
    }

    // ---- comment ----

    #[test]
    fn column_comment_change_is_safe() {
        let target = tbl(vec![col("c", ColumnType::Integer, true)]);
        let mut to = col("c", ColumnType::Integer, true);
        to.comment = Some("the c column".into());
        let source = tbl(vec![to]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0].op, TableOp::SetColumnComment { .. }));
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    // ---- equality / order ----

    #[test]
    fn equal_columns_emit_nothing() {
        let target = tbl(vec![col("id", ColumnType::BigInt, false)]);
        let source = tbl(vec![col("id", ColumnType::BigInt, false)]);
        let ops = diff_one(&target, &source);
        assert!(ops.is_empty());
    }

    #[test]
    fn column_reorder_is_ignored_in_v0_1() {
        let target = tbl(vec![
            col("a", ColumnType::Integer, true),
            col("b", ColumnType::Integer, true),
        ]);
        let source = tbl(vec![
            col("b", ColumnType::Integer, true),
            col("a", ColumnType::Integer, true),
        ]);
        let ops = diff_one(&target, &source);
        assert!(ops.is_empty(), "v0.1 ignores logical column order");
    }

    // ---- storage ----

    #[test]
    fn storage_change_emits_safe_op() {
        let mut from_col = col("doc", ColumnType::Text, true);
        from_col.storage = Some(StorageKind::Extended); // text default → canon would strip
        let mut to_col = col("doc", ColumnType::Text, true);
        to_col.storage = Some(StorageKind::External);
        let target = tbl(vec![from_col]);
        let source = tbl(vec![to_col]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(matches!(
            ops[0].op,
            TableOp::SetColumnStorage {
                from: StorageKind::Extended,
                to: StorageKind::External,
                ..
            }
        ));
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn storage_none_vs_type_default_is_noop() {
        let from_col = col("doc", ColumnType::Text, true);
        let mut to_col = col("doc", ColumnType::Text, true);
        to_col.storage = Some(StorageKind::Extended); // text default
        let target = tbl(vec![from_col]);
        let source = tbl(vec![to_col]);
        let ops = diff_one(&target, &source);
        assert!(
            ops.is_empty(),
            "None and Some(type_default) must collapse to the same effective storage"
        );
    }

    // ---- compression ----

    #[test]
    fn compression_change_emits_safe_op() {
        let mut from_col = col("blob", ColumnType::Bytea, true);
        from_col.compression = Some(Compression::Pglz);
        let mut to_col = col("blob", ColumnType::Bytea, true);
        to_col.compression = Some(Compression::Lz4);
        let target = tbl(vec![from_col]);
        let source = tbl(vec![to_col]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(matches!(
            ops[0].op,
            TableOp::SetColumnCompression {
                compression: Some(Compression::Lz4),
                ..
            }
        ));
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn compression_to_cluster_default_emits_none() {
        let mut from_col = col("blob", ColumnType::Bytea, true);
        from_col.compression = Some(Compression::Lz4);
        let to_col = col("blob", ColumnType::Bytea, true);
        let target = tbl(vec![from_col]);
        let source = tbl(vec![to_col]);
        let ops = diff_one(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(matches!(
            ops[0].op,
            TableOp::SetColumnCompression {
                compression: None,
                ..
            }
        ));
    }
}
