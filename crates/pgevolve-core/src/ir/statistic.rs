//! Statistic IR — declarative model for Postgres CREATE STATISTICS.
//!
//! pgevolve manages `pg_statistic_ext` objects with explicit names. Source
//! must declare the name (`CREATE STATISTICS app.s ON (...) FROM app.t`);
//! anonymous form `CREATE STATISTICS ON (...) FROM app.t` is rejected at
//! parse time, mirroring the no-anonymous-indexes policy.
//!
//! Spec: `docs/superpowers/specs/2026-05-27-statistics-and-check-option-design.md`.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::difference::Difference;
use crate::ir::eq::{Equiv, field_difference};

/// Declarative model of a Postgres `CREATE STATISTICS` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statistic {
    /// Schema-qualified statistic name (explicit names required).
    pub qname: QualifiedName,
    /// The target table whose columns are correlated.
    pub target: QualifiedName,
    /// Which kinds are enabled. At least one must be true (canon enforces).
    pub kinds: StatisticKinds,
    /// Column / expression list. Sorted by canon; deduped.
    pub columns: Vec<StatisticColumn>,
    /// `ALTER STATISTICS s SET STATISTICS n` — analyze target.
    /// `None` = unmanaged / use PG default (-1).
    pub statistics_target: Option<i32>,
    /// Object owner. `None` = unmanaged (v0.3.1 lenient pattern).
    pub owner: Option<Identifier>,
    /// Optional `COMMENT ON STATISTICS`.
    pub comment: Option<String>,
}

impl Equiv for Statistic {
    fn differences(&self, other: &Self) -> Vec<Difference> {
        // Field-completeness guard: the compiler errors if a field is added
        // without being handled below. Bindings are unused (read via `self`).
        let Self {
            qname: _,
            target: _,
            kinds: _,
            columns: _,
            statistics_target: _,
            owner: _,
            comment: _,
        } = self;
        let mut out = Vec::new();
        out.extend(field_difference("qname", &self.qname, &other.qname));
        out.extend(field_difference("target", &self.target, &other.target));
        out.extend(field_difference(
            "kinds",
            &format!("{:?}", self.kinds),
            &format!("{:?}", other.kinds),
        ));
        out.extend(field_difference(
            "columns",
            &format!("{:?}", self.columns),
            &format!("{:?}", other.columns),
        ));
        out.extend(field_difference(
            "statistics_target",
            &format!("{:?}", self.statistics_target),
            &format!("{:?}", other.statistics_target),
        ));
        out.extend(field_difference(
            "owner",
            &format!("{:?}", self.owner),
            &format!("{:?}", other.owner),
        ));
        out.extend(field_difference(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out
    }
}

/// Which `kinds` flags are enabled on a `CREATE STATISTICS` object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StatisticKinds {
    /// `ndistinct` — multi-column n-distinct counts.
    pub ndistinct: bool,
    /// `dependencies` — functional dependencies between columns.
    pub dependencies: bool,
    /// `mcv` — most-common-value lists per column combination.
    pub mcv: bool,
}

impl StatisticKinds {
    /// PG's default when no kinds clause is given: all three enabled.
    #[must_use]
    pub const fn pg_default() -> Self {
        Self {
            ndistinct: true,
            dependencies: true,
            mcv: true,
        }
    }

    /// True iff at least one kind is enabled. An empty bitset is illegal
    /// at the IR level (canon rejects).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        !self.ndistinct && !self.dependencies && !self.mcv
    }
}

/// A single entry in the statistic's column list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatisticColumn {
    /// Plain `column_name` reference.
    Column(Identifier),
    /// Expression statistic (PG 14+): `(lower(name))`. Canonicalized via
    /// `NormalizedExpr` (same canon as CHECK / USING / WITH CHECK).
    Expression(NormalizedExpr),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_default_is_all_true() {
        let k = StatisticKinds::pg_default();
        assert!(k.ndistinct && k.dependencies && k.mcv);
        assert!(!k.is_empty());
    }

    #[test]
    fn kinds_empty_when_all_false() {
        let k = StatisticKinds {
            ndistinct: false,
            dependencies: false,
            mcv: false,
        };
        assert!(k.is_empty());
    }

    #[test]
    fn column_form_does_not_equal_expression_form() {
        let c = StatisticColumn::Column(Identifier::from_unquoted("a").unwrap());
        let e = StatisticColumn::Expression(NormalizedExpr::from_text("lower(a)"));
        assert_ne!(c, e);
    }
}
