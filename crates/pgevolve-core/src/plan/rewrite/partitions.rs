//! SQL emission for partition operations.

#![allow(dead_code)]

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
    use crate::ir::partition::{PartitionColumn, PartitionColumnKind};

    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(
            Identifier::from_unquoted(s).unwrap(),
            Identifier::from_unquoted(n).unwrap(),
        )
    }

    fn lit(s: &str) -> crate::ir::default_expr::NormalizedExpr {
        crate::ir::default_expr::NormalizedExpr::from_text(s)
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
        assert_eq!(render_for_values(&PartitionBounds::Default), "DEFAULT");
    }

    #[test]
    fn hash_renders() {
        assert_eq!(
            render_for_values(&PartitionBounds::Hash { modulus: 4, remainder: 1 }),
            "FOR VALUES WITH (MODULUS 4, REMAINDER 1)"
        );
    }

    #[test]
    fn minvalue_maxvalue_render() {
        let b = PartitionBounds::Range {
            from: vec![BoundDatum::MinValue],
            to: vec![BoundDatum::MaxValue],
        };
        assert_eq!(
            render_for_values(&b),
            "FOR VALUES FROM (MINVALUE) TO (MAXVALUE)"
        );
    }

    #[test]
    fn partition_by_list_column_renders() {
        let pb = PartitionBy {
            strategy: PartitionStrategy::List,
            columns: vec![PartitionColumn {
                kind: PartitionColumnKind::Column(Identifier::from_unquoted("region").unwrap()),
                collation: None,
                opclass: None,
            }],
        };
        assert_eq!(render_partition_by(&pb), "PARTITION BY LIST (region)");
    }
}
