//! SQL rendering for statistics operations.
//!
//! Each public function corresponds to one DML kind on a Postgres STATISTICS
//! object. All helpers return a complete SQL statement including the trailing
//! semicolon.

use crate::identifier::QualifiedName;
use crate::ir::statistic::{Statistic, StatisticColumn, StatisticKinds};

/// `CREATE STATISTICS [schema.]name [(kinds)] ON cols FROM [schema.]table;`
///
/// The `(kinds)` clause is omitted when all three kinds are enabled — that is
/// PG's default and omitting it keeps the rendered SQL clean.
#[must_use]
pub fn create_statistic(s: &Statistic) -> String {
    let mut out = format!("CREATE STATISTICS {}", s.qname.render_sql());
    if !is_default_all(s.kinds) {
        out.push_str(&render_kinds(s.kinds));
    }
    out.push_str(" ON ");
    out.push_str(&render_columns(&s.columns));
    out.push_str(" FROM ");
    out.push_str(&s.target.render_sql());
    out.push(';');
    out
}

/// `DROP STATISTICS [schema.]name;`
#[must_use]
pub fn drop_statistic(qname: &QualifiedName) -> String {
    format!("DROP STATISTICS {};", qname.render_sql())
}

/// Two-step replace: `[DROP STATISTICS old, CREATE STATISTICS new]`.
///
/// The caller joins both with a newline when rendering into a single
/// `RawStep` SQL body.
#[must_use]
pub fn replace_statistic(from: &Statistic, to: &Statistic) -> [String; 2] {
    [drop_statistic(&from.qname), create_statistic(to)]
}

/// `ALTER STATISTICS [schema.]name SET STATISTICS n;`
#[must_use]
pub fn alter_statistic_set_target(qname: &QualifiedName, value: i32) -> String {
    format!(
        "ALTER STATISTICS {} SET STATISTICS {value};",
        qname.render_sql()
    )
}

/// `COMMENT ON STATISTICS [schema.]name IS 'text';` or `IS NULL`.
#[must_use]
pub fn comment_on_statistic(qname: &QualifiedName, comment: Option<&str>) -> String {
    let body = comment.map_or_else(|| "NULL".to_owned(), super::sql::sql_string_literal);
    format!("COMMENT ON STATISTICS {} IS {body};", qname.render_sql())
}

/// Returns true iff all three kinds are enabled (PG default — no kinds clause needed).
const fn is_default_all(k: StatisticKinds) -> bool {
    k.ndistinct && k.dependencies && k.mcv
}

/// Renders the `(ndistinct, dependencies, mcv)` kinds clause.
///
/// Panics in debug mode if the bitset is empty (canon enforces non-empty).
fn render_kinds(k: StatisticKinds) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if k.ndistinct {
        parts.push("ndistinct");
    }
    if k.dependencies {
        parts.push("dependencies");
    }
    if k.mcv {
        parts.push("mcv");
    }
    debug_assert!(
        !parts.is_empty(),
        "empty StatisticKinds must be caught by canon"
    );
    format!(" ({})", parts.join(", "))
}

/// Renders the column list for a `CREATE STATISTICS … ON …` clause.
///
/// Column entries are rendered as bare identifiers; expression entries are
/// wrapped in parentheses per PG syntax.
fn render_columns(cols: &[StatisticColumn]) -> String {
    let parts: Vec<String> = cols
        .iter()
        .map(|c| match c {
            StatisticColumn::Column(id) => id.render_sql(),
            StatisticColumn::Expression(expr) => format!("({})", expr.canonical_text),
        })
        .collect();
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::default_expr::NormalizedExpr;
    use crate::ir::statistic::StatisticKinds;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn stat_all_kinds(cols: Vec<StatisticColumn>) -> Statistic {
        Statistic {
            qname: qn("app", "s"),
            target: qn("app", "t"),
            kinds: StatisticKinds::pg_default(),
            columns: cols,
            statistics_target: None,
            owner: None,
            comment: None,
        }
    }

    fn stat_with_kinds(kinds: StatisticKinds, cols: Vec<StatisticColumn>) -> Statistic {
        Statistic {
            qname: qn("app", "s"),
            target: qn("app", "t"),
            kinds,
            columns: cols,
            statistics_target: None,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn create_statistic_omits_kinds_clause_when_all_enabled() {
        let s = stat_all_kinds(vec![
            StatisticColumn::Column(id("a")),
            StatisticColumn::Column(id("b")),
        ]);
        let sql = create_statistic(&s);
        // No parenthesized kinds clause — just ON cols FROM table.
        assert!(!sql.contains('('), "unexpected kinds clause in: {sql}");
        assert_eq!(sql, "CREATE STATISTICS app.s ON a, b FROM app.t;");
    }

    #[test]
    fn create_statistic_includes_kinds_clause_when_partial() {
        let s = stat_with_kinds(
            StatisticKinds {
                ndistinct: true,
                dependencies: false,
                mcv: false,
            },
            vec![
                StatisticColumn::Column(id("a")),
                StatisticColumn::Column(id("b")),
            ],
        );
        let sql = create_statistic(&s);
        assert!(
            sql.contains("(ndistinct)"),
            "expected kinds clause in: {sql}"
        );
        assert_eq!(
            sql,
            "CREATE STATISTICS app.s (ndistinct) ON a, b FROM app.t;"
        );
    }

    #[test]
    fn create_statistic_renders_expression_columns_in_parens() {
        let s = stat_all_kinds(vec![
            StatisticColumn::Column(id("a")),
            StatisticColumn::Expression(NormalizedExpr::from_text("lower(name)")),
        ]);
        let sql = create_statistic(&s);
        assert!(
            sql.contains("(lower(name))"),
            "expected expr in parens: {sql}"
        );
        assert_eq!(
            sql,
            "CREATE STATISTICS app.s ON a, (lower(name)) FROM app.t;"
        );
    }

    #[test]
    fn drop_statistic_renders_correctly() {
        let sql = drop_statistic(&qn("app", "s"));
        assert_eq!(sql, "DROP STATISTICS app.s;");
    }

    #[test]
    fn replace_statistic_returns_drop_then_create() {
        let from = stat_all_kinds(vec![StatisticColumn::Column(id("a"))]);
        let to = stat_with_kinds(
            StatisticKinds {
                ndistinct: true,
                dependencies: false,
                mcv: false,
            },
            vec![
                StatisticColumn::Column(id("a")),
                StatisticColumn::Column(id("b")),
            ],
        );
        let [drop_sql, create_sql] = replace_statistic(&from, &to);
        assert_eq!(drop_sql, "DROP STATISTICS app.s;");
        assert_eq!(
            create_sql,
            "CREATE STATISTICS app.s (ndistinct) ON a, b FROM app.t;"
        );
    }

    #[test]
    fn alter_statistic_set_target_renders_correctly() {
        let sql = alter_statistic_set_target(&qn("app", "s"), 100);
        assert_eq!(sql, "ALTER STATISTICS app.s SET STATISTICS 100;");
    }

    #[test]
    fn comment_on_statistic_with_text() {
        let sql = comment_on_statistic(&qn("app", "s"), Some("my comment"));
        assert_eq!(sql, "COMMENT ON STATISTICS app.s IS 'my comment';");
    }

    #[test]
    fn comment_on_statistic_null_clears() {
        let sql = comment_on_statistic(&qn("app", "s"), None);
        assert_eq!(sql, "COMMENT ON STATISTICS app.s IS NULL;");
    }

    #[test]
    fn comment_on_statistic_escapes_single_quotes() {
        let sql = comment_on_statistic(&qn("app", "s"), Some("it's a stat"));
        assert_eq!(sql, "COMMENT ON STATISTICS app.s IS 'it''s a stat';");
    }

    #[test]
    fn render_kinds_two_enabled() {
        let k = StatisticKinds {
            ndistinct: true,
            dependencies: true,
            mcv: false,
        };
        assert!(!is_default_all(k));
        let s = render_kinds(k);
        assert_eq!(s, " (ndistinct, dependencies)");
    }

    #[test]
    fn is_default_all_true_when_all_enabled() {
        assert!(is_default_all(StatisticKinds::pg_default()));
    }

    #[test]
    fn is_default_all_false_when_any_disabled() {
        assert!(!is_default_all(StatisticKinds {
            ndistinct: true,
            dependencies: true,
            mcv: false
        }));
        assert!(!is_default_all(StatisticKinds {
            ndistinct: false,
            dependencies: true,
            mcv: true
        }));
    }
}
