//! Partitioning IR — partition-by clauses on partitioned parents and
//! partition-of declarations on partition children.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;

/// Describes the partitioning strategy and key of a partitioned parent table.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PartitionBy {
    /// The partitioning strategy (`RANGE`, `LIST`, or `HASH`).
    pub strategy: PartitionStrategy,
    /// The partition key columns (or expressions).
    pub columns: Vec<PartitionColumn>,
}

/// The partitioning strategy declared by `PARTITION BY <strategy>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionStrategy {
    /// `PARTITION BY RANGE (...)`.
    Range,
    /// `PARTITION BY LIST (...)`.
    List,
    /// `PARTITION BY HASH (...)`.
    Hash,
}

/// A single element of the partition key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PartitionColumn {
    /// The key element — a plain column reference or an arbitrary expression.
    pub kind: PartitionColumnKind,
    /// Optional collation override.
    pub collation: Option<QualifiedName>,
    /// Optional operator class override.
    pub opclass: Option<QualifiedName>,
}

/// Whether the partition key element is a column reference or an expression.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionColumnKind {
    /// A plain column reference.
    Column(Identifier),
    /// A parenthesized expression.
    Expr(NormalizedExpr),
}

/// Declares that this table is itself a partition of a parent table.
///
/// Corresponds to `PARTITION OF <parent> <bounds>` in the DDL.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PartitionOf {
    /// The schema-qualified parent table.
    pub parent: QualifiedName,
    /// The partition bounds clause.
    pub bounds: PartitionBounds,
}

/// The bounds of a partition child table.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PartitionBounds {
    /// `FOR VALUES FROM (...) TO (...)` — range partition.
    Range {
        /// The `FROM` bound datums (one per key column).
        from: Vec<BoundDatum>,
        /// The `TO` bound datums (one per key column).
        to: Vec<BoundDatum>,
    },
    /// `FOR VALUES IN (...)` — list partition.
    List {
        /// The listed bound datums.
        values: Vec<BoundDatum>,
    },
    /// `FOR VALUES WITH (MODULUS m, REMAINDER r)` — hash partition.
    Hash {
        /// Hash modulus.
        modulus: u32,
        /// Hash remainder.
        remainder: u32,
    },
    /// `DEFAULT` — catches rows not matched by any other partition.
    Default,
}

/// A single datum in a partition bound clause.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundDatum {
    /// A concrete literal expression (e.g., `'2024-01-01'`, `42`).
    Literal(NormalizedExpr),
    /// The pseudo-datum `MINVALUE`.
    MinValue,
    /// The pseudo-datum `MAXVALUE`.
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
