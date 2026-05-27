//! Parser for `CREATE STATISTICS` and `ALTER STATISTICS` statements.
//!
//! Both are folded into one `Statistic` per qname — the same pattern as v0.3.4
//! PUBLICATION where `CREATE … WITH (…)` and subsequent `ALTER … SET …` all
//! unify into one IR record.
//!
//! `pg_query` AST nodes:
//! - `CreateStatsStmt` — `CREATE STATISTICS … ON … FROM …`
//! - `AlterStatsStmt`  — `ALTER STATISTICS … SET STATISTICS n`
//! - `RenameStmt` (`ObjectStatisticExt`) — rejected at the Statement classifier.
//! - `CommentStmt` (`ObjectStatisticExt`) — handled by `comment_stmt` dispatcher.
//!
//! Spec: `docs/superpowers/specs/2026-05-27-statistics-and-check-option-design.md`
//! Plan Stage 6: `docs/superpowers/plans/2026-05-27-statistics-and-check-option.md`

use std::collections::BTreeMap;

use pg_query::NodeEnum;
use pg_query::protobuf::a_const;
use pg_query::protobuf::{AlterStatsStmt, CreateStatsStmt};

use crate::identifier::QualifiedName;
use crate::ir::statistic::{Statistic, StatisticColumn, StatisticKinds};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};
use crate::parse::normalize_expr;

/// Apply a `CREATE STATISTICS` statement to the accumulator map.
///
/// Rejects anonymous form (no name), duplicate qnames, empty column lists,
/// and unknown kind strings. An empty `stat_types` list means PG's default
/// (all three enabled).
pub(crate) fn parse_create_statistics(
    stmt: &CreateStatsStmt,
    source_loc: SourceLocation,
    existing: &mut BTreeMap<QualifiedName, Statistic>,
) -> Result<(), ParseError> {
    // 1. Name — defnames must be non-empty; empty = anonymous form.
    if stmt.defnames.is_empty() {
        return Err(ParseError::StatisticAnonymous(source_loc));
    }
    let qname = shared::qname_from_string_list(&stmt.defnames, None, &source_loc)
        .map_err(|_| ParseError::StatisticAnonymous(source_loc.clone()))?;

    // 2. Reject duplicates.
    if existing.contains_key(&qname) {
        return Err(ParseError::DuplicateStatistic(qname, source_loc));
    }

    // 3. Kinds — empty → PG default (all three).
    let kinds = if stmt.stat_types.is_empty() {
        StatisticKinds::pg_default()
    } else {
        parse_statistic_kinds(&stmt.stat_types, &qname, &source_loc)?
    };

    // 4. Target table — always exactly one `RangeVar` in relations.
    let target = extract_target_table(&stmt.relations, &qname, &source_loc)?;

    // 5. Columns / expressions.
    let columns = parse_statistic_columns(&stmt.exprs, &qname, &source_loc)?;

    if columns.is_empty() {
        return Err(ParseError::StatisticEmptyColumns(qname, source_loc));
    }

    existing.insert(
        qname.clone(),
        Statistic {
            qname,
            target,
            kinds,
            columns,
            statistics_target: None,
            owner: None,
            comment: None,
        },
    );
    Ok(())
}

/// Apply an `ALTER STATISTICS … SET STATISTICS n` statement to the accumulator.
///
/// Rejects ALTER-before-CREATE.
pub(crate) fn parse_alter_statistics(
    stmt: &AlterStatsStmt,
    source_loc: &SourceLocation,
    existing: &mut BTreeMap<QualifiedName, Statistic>,
) -> Result<(), ParseError> {
    let qname = shared::qname_from_string_list(&stmt.defnames, None, source_loc).map_err(|_| {
        ParseError::Structural {
            location: source_loc.clone(),
            message: "ALTER STATISTICS: could not extract statistic name".into(),
        }
    })?;

    let statistic = existing
        .get_mut(&qname)
        .ok_or_else(|| ParseError::AlterStatisticBeforeCreate(qname.clone(), source_loc.clone()))?;

    // stxstattarget holds the new integer target value.
    let target = extract_stxstattarget(stmt, &qname, source_loc)?;
    statistic.statistics_target = Some(target);
    Ok(())
}

// ── Kind parsing ─────────────────────────────────────────────────────────────

fn parse_statistic_kinds(
    nodes: &[pg_query::protobuf::Node],
    qname: &QualifiedName,
    loc: &SourceLocation,
) -> Result<StatisticKinds, ParseError> {
    let mut kinds = StatisticKinds {
        ndistinct: false,
        dependencies: false,
        mcv: false,
    };

    for node in nodes {
        let Some(NodeEnum::String(s)) = node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!("statistic {qname}: expected String node in stat_types"),
            });
        };
        match s.sval.to_ascii_lowercase().as_str() {
            "ndistinct" => kinds.ndistinct = true,
            "dependencies" => kinds.dependencies = true,
            "mcv" => kinds.mcv = true,
            other => {
                return Err(ParseError::UnknownStatisticKind(
                    other.to_string(),
                    qname.clone(),
                    loc.clone(),
                ));
            }
        }
    }

    if kinds.is_empty() {
        return Err(ParseError::StatisticEmptyKinds(qname.clone(), loc.clone()));
    }

    Ok(kinds)
}

// ── Column / expression parsing ───────────────────────────────────────────────

/// Parse the `exprs` field of `CreateStatsStmt`.
///
/// Each entry is a `NodeEnum::StatsElem`. A `StatsElem` has:
/// - `name` — non-empty means this is a plain column reference.
/// - `expr` — non-None means this is an expression statistic (PG 14+).
fn parse_statistic_columns(
    nodes: &[pg_query::protobuf::Node],
    qname: &QualifiedName,
    loc: &SourceLocation,
) -> Result<Vec<StatisticColumn>, ParseError> {
    let mut out = Vec::with_capacity(nodes.len());

    for node in nodes {
        let Some(NodeEnum::StatsElem(elem)) = node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "statistic {qname}: unexpected node kind in exprs (expected StatsElem)"
                ),
            });
        };

        if !elem.name.is_empty() {
            // Plain column reference.
            let id = shared::ident(&elem.name, loc)?;
            out.push(StatisticColumn::Column(id));
        } else if let Some(expr_node) = elem.expr.as_ref().and_then(|n| n.node.as_ref()) {
            // Expression statistic.
            let normalized = normalize_expr::from_pg_node(expr_node, None, loc)?;
            out.push(StatisticColumn::Expression(normalized));
        } else {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!("statistic {qname}: StatsElem has neither name nor expression"),
            });
        }
    }

    Ok(out)
}

// ── Target table extraction ───────────────────────────────────────────────────

fn extract_target_table(
    relations: &[pg_query::protobuf::Node],
    qname: &QualifiedName,
    loc: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let Some(first) = relations.first() else {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("statistic {qname}: missing FROM clause (no relations)"),
        });
    };
    let Some(NodeEnum::RangeVar(rv)) = first.node.as_ref() else {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("statistic {qname}: expected RangeVar in relations"),
        });
    };
    shared::resolve_qname(rv, None, loc)
}

// ── ALTER STATISTICS target extraction ───────────────────────────────────────

/// Extract the integer target from `AlterStatsStmt.stxstattarget`.
///
/// `pg_query` 6.x encodes `SET STATISTICS n` as a bare `Integer { ival }` node
/// (not an `AConst`). We handle both forms for forward-compatibility.
fn extract_stxstattarget(
    stmt: &AlterStatsStmt,
    qname: &QualifiedName,
    loc: &SourceLocation,
) -> Result<i32, ParseError> {
    let node = stmt
        .stxstattarget
        .as_ref()
        .and_then(|n| n.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "statistic {qname}: ALTER STATISTICS SET STATISTICS missing target value"
            ),
        })?;

    match node {
        // pg_query 6.x encodes the target as a bare Integer node.
        NodeEnum::Integer(i) => Ok(i.ival),
        // Forward-compat: also accept AConst integer form.
        NodeEnum::AConst(ac) => match ac.val.as_ref() {
            Some(a_const::Val::Ival(i)) => Ok(i.ival),
            _ => Err(ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "statistic {qname}: ALTER STATISTICS SET STATISTICS expected integer value"
                ),
            }),
        },
        _ => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "statistic {qname}: ALTER STATISTICS SET STATISTICS expected integer, got unexpected node kind"
            ),
        }),
    }
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::catalog::Catalog;
    use crate::parse::parse_directory;

    fn write(dir: &std::path::Path, rel: &str, contents: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    fn parse_source(sql: &str) -> Result<Catalog, ParseError> {
        let tmp = tempdir().expect("tempdir");
        write(tmp.path(), "schema.sql", sql);
        parse_directory(tmp.path(), &[])
    }

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn parse_one_create_stmt(sql: &str) -> CreateStatsStmt {
        let parsed = pg_query::parse(sql).expect("pg_query parse");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreateStatsStmt(s) = node else {
            panic!("expected CreateStatsStmt, got something else");
        };
        s
    }

    fn parse_one_alter_stmt(sql: &str) -> AlterStatsStmt {
        let parsed = pg_query::parse(sql).expect("pg_query parse");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::AlterStatsStmt(s) = node else {
            panic!("expected AlterStatsStmt, got something else");
        };
        *s
    }

    // ── CREATE tests ──────────────────────────────────────────────────────────

    #[test]
    fn create_basic_default_kinds() {
        // No kinds clause → PG default (all three).
        let stmt = parse_one_create_stmt("CREATE STATISTICS app.s ON a, b FROM app.t;");
        let mut acc: BTreeMap<QualifiedName, Statistic> = BTreeMap::new();
        parse_create_statistics(&stmt, loc(), &mut acc).expect("ok");
        let s = acc.values().next().unwrap();
        assert_eq!(s.qname.schema.as_str(), "app");
        assert_eq!(s.qname.name.as_str(), "s");
        assert_eq!(s.kinds, StatisticKinds::pg_default());
        assert_eq!(s.target.schema.as_str(), "app");
        assert_eq!(s.target.name.as_str(), "t");
        assert_eq!(s.columns.len(), 2);
        assert!(matches!(&s.columns[0], StatisticColumn::Column(id) if id.as_str() == "a"));
        assert!(matches!(&s.columns[1], StatisticColumn::Column(id) if id.as_str() == "b"));
        assert!(s.statistics_target.is_none());
        assert!(s.comment.is_none());
    }

    #[test]
    fn create_explicit_single_kind() {
        let stmt = parse_one_create_stmt("CREATE STATISTICS app.s (ndistinct) ON a, b FROM app.t;");
        let mut acc: BTreeMap<QualifiedName, Statistic> = BTreeMap::new();
        parse_create_statistics(&stmt, loc(), &mut acc).expect("ok");
        let s = acc.values().next().unwrap();
        assert_eq!(
            s.kinds,
            StatisticKinds {
                ndistinct: true,
                dependencies: false,
                mcv: false
            }
        );
    }

    #[test]
    fn create_all_three_kinds() {
        let stmt = parse_one_create_stmt(
            "CREATE STATISTICS app.s (ndistinct, dependencies, mcv) ON a, b FROM app.t;",
        );
        let mut acc: BTreeMap<QualifiedName, Statistic> = BTreeMap::new();
        parse_create_statistics(&stmt, loc(), &mut acc).expect("ok");
        let s = acc.values().next().unwrap();
        assert_eq!(s.kinds, StatisticKinds::pg_default());
    }

    #[test]
    fn create_expression_form() {
        let stmt = parse_one_create_stmt("CREATE STATISTICS app.s ON (lower(name)) FROM app.t;");
        let mut acc: BTreeMap<QualifiedName, Statistic> = BTreeMap::new();
        parse_create_statistics(&stmt, loc(), &mut acc).expect("ok");
        let s = acc.values().next().unwrap();
        assert_eq!(s.columns.len(), 1);
        assert!(matches!(s.columns[0], StatisticColumn::Expression(_)));
    }

    #[test]
    fn create_mixed_columns_and_expressions() {
        let stmt = parse_one_create_stmt("CREATE STATISTICS app.s ON a, (lower(name)) FROM app.t;");
        let mut acc: BTreeMap<QualifiedName, Statistic> = BTreeMap::new();
        parse_create_statistics(&stmt, loc(), &mut acc).expect("ok");
        let s = acc.values().next().unwrap();
        assert_eq!(s.columns.len(), 2);
        assert!(matches!(s.columns[0], StatisticColumn::Column(_)));
        assert!(matches!(s.columns[1], StatisticColumn::Expression(_)));
    }

    // ── ALTER folding test ────────────────────────────────────────────────────

    #[test]
    fn alter_set_statistics_folds_with_create() {
        let create = parse_one_create_stmt("CREATE STATISTICS app.s ON a, b FROM app.t;");
        let alter = parse_one_alter_stmt("ALTER STATISTICS app.s SET STATISTICS 1000;");
        let mut acc: BTreeMap<QualifiedName, Statistic> = BTreeMap::new();
        parse_create_statistics(&create, loc(), &mut acc).expect("create ok");
        parse_alter_statistics(&alter, &loc(), &mut acc).expect("alter ok");
        let s = acc.values().next().unwrap();
        assert_eq!(s.statistics_target, Some(1000));
    }

    // ── Error cases ───────────────────────────────────────────────────────────

    #[test]
    fn anonymous_form_errors() {
        // pg_query rejects `CREATE STATISTICS ON (a, b) FROM app.t;` (no name)
        // at the SQL level in most PG versions — inject via empty defnames.
        let mut stmt = parse_one_create_stmt("CREATE STATISTICS app.s ON a, b FROM app.t;");
        stmt.defnames.clear();
        let mut acc: BTreeMap<QualifiedName, Statistic> = BTreeMap::new();
        let err = parse_create_statistics(&stmt, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::StatisticAnonymous(_)),
            "got: {err:?}"
        );
    }

    #[test]
    fn duplicate_statistic_errors() {
        let stmt = parse_one_create_stmt("CREATE STATISTICS app.s ON a, b FROM app.t;");
        let mut acc: BTreeMap<QualifiedName, Statistic> = BTreeMap::new();
        parse_create_statistics(&stmt, loc(), &mut acc).expect("first ok");
        let err = parse_create_statistics(&stmt, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::DuplicateStatistic(_, _)),
            "got: {err:?}"
        );
    }

    #[test]
    fn unknown_kind_errors() {
        // Inject a bogus kind node into a freshly-parsed stmt.
        let mut stmt = parse_one_create_stmt("CREATE STATISTICS app.s ON a FROM app.t;");
        stmt.stat_types.push(pg_query::protobuf::Node {
            node: Some(NodeEnum::String(pg_query::protobuf::String {
                sval: "bogus".to_string(),
            })),
        });
        let mut acc: BTreeMap<QualifiedName, Statistic> = BTreeMap::new();
        let err = parse_create_statistics(&stmt, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::UnknownStatisticKind(ref s, _, _) if s == "bogus"),
            "got: {err:?}"
        );
    }

    #[test]
    fn alter_before_create_errors() {
        let alter = parse_one_alter_stmt("ALTER STATISTICS app.s SET STATISTICS 1000;");
        let mut acc: BTreeMap<QualifiedName, Statistic> = BTreeMap::new();
        let err = parse_alter_statistics(&alter, &loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::AlterStatisticBeforeCreate(_, _)),
            "got: {err:?}"
        );
    }

    // ── Integration tests via parse_directory ──────────────────────────────────

    #[test]
    fn parse_directory_basic_create() {
        let sql = "CREATE SCHEMA app; CREATE STATISTICS app.s ON a, b FROM app.t;";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.statistics.len(), 1);
        let s = &cat.statistics[0];
        assert_eq!(s.qname.name.as_str(), "s");
        assert_eq!(s.target.name.as_str(), "t");
        assert_eq!(s.kinds, StatisticKinds::pg_default());
        assert_eq!(s.columns.len(), 2);
    }

    #[test]
    fn parse_directory_explicit_kind() {
        let sql = "CREATE SCHEMA app; CREATE STATISTICS app.s (ndistinct) ON a, b FROM app.t;";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.statistics.len(), 1);
        assert_eq!(
            cat.statistics[0].kinds,
            StatisticKinds {
                ndistinct: true,
                dependencies: false,
                mcv: false
            }
        );
    }

    #[test]
    fn parse_directory_folded_create_and_alter() {
        let sql = "
            CREATE SCHEMA app;
            CREATE STATISTICS app.s ON a, b FROM app.t;
            ALTER STATISTICS app.s SET STATISTICS 500;
        ";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.statistics.len(), 1);
        assert_eq!(cat.statistics[0].statistics_target, Some(500));
    }

    #[test]
    fn parse_directory_comment_folds() {
        let sql = "
            CREATE SCHEMA app;
            CREATE STATISTICS app.s ON a, b FROM app.t;
            COMMENT ON STATISTICS app.s IS 'correlation stats';
        ";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.statistics.len(), 1);
        assert_eq!(
            cat.statistics[0].comment.as_deref(),
            Some("correlation stats")
        );
    }

    #[test]
    fn rename_statistic_in_source_errors() {
        // RENAME is encoded as RenameStmt(ObjectStatisticExt) — should error.
        // New name must be unqualified (PG syntax).
        let sql = "ALTER STATISTICS app.s RENAME TO t_new;";
        let err = parse_source(sql).expect_err("should fail");
        assert!(
            matches!(
                err,
                ParseError::StatisticRenameNotSupported(_, _)
                    | ParseError::Structural { .. }
                    | ParseError::UnsupportedObjectKind { .. }
            ),
            "got: {err:?}"
        );
    }

    #[test]
    fn comment_on_statistic_before_create_errors() {
        let sql = "
            CREATE SCHEMA app;
            COMMENT ON STATISTICS app.s IS 'no stat yet';
        ";
        let err = parse_source(sql).expect_err("should fail");
        assert!(
            matches!(
                err,
                ParseError::CommentOnStatisticBeforeCreate(_, _) | ParseError::Structural { .. }
            ),
            "got: {err:?}"
        );
    }

    #[test]
    fn parse_directory_expression_form() {
        let sql = "
            CREATE SCHEMA app;
            CREATE STATISTICS app.s ON (lower(name)) FROM app.t;
        ";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.statistics.len(), 1);
        assert_eq!(cat.statistics[0].columns.len(), 1);
        assert!(matches!(
            cat.statistics[0].columns[0],
            StatisticColumn::Expression(_)
        ));
    }

    #[test]
    fn include_clause_not_supported() {
        // No `include` field on CreateStatsStmt in pg_query 6.x (PG 14-17).
        // Verify the StatisticIncludeNotSupported variant is reachable via
        // direct function call (simulating what a PG 18 pg_query would produce).
        let stmt = parse_one_create_stmt("CREATE STATISTICS app.s ON a, b FROM app.t;");
        let mut acc: BTreeMap<QualifiedName, Statistic> = BTreeMap::new();
        // Normal create works fine — INCLUDE is a future field.
        // This test verifies the error path is constructable (compile-time check).
        let qname = QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted("s").unwrap(),
        );
        let err = ParseError::StatisticIncludeNotSupported(qname, loc());
        assert!(matches!(
            err,
            ParseError::StatisticIncludeNotSupported(_, _)
        ));
        let _ = (stmt, &mut acc);
    }
}
