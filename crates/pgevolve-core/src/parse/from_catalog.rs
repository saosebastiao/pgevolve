//! Rebuild IR from server-emitted definition text.
//!
//! Postgres hands back most object definitions as SQL strings —
//! `pg_get_indexdef`, `pg_get_triggerdef`, `pg_get_constraintdef`,
//! `pg_get_function_arguments`, and friends. The catalog reader turns those
//! strings back into IR by re-parsing them, which means the catalog reader used
//! to import the parser directly: nine files under `catalog/assemble/`, each
//! repeating the same wrap-parse-unwrap dance before getting to the two or
//! three fields it actually wanted.
//!
//! That duplication was not free. [`parameter_list`] replaces three near-
//! identical copies of the same forty-line walk that differed only in the
//! wording of their error messages, and the wrap-parse-unwrap prelude appeared
//! nine times with nine slightly different sets of failure cases.
//!
//! Everything here returns pgevolve-owned types. Callers get IR, `Identifier`s,
//! and [`FromCatalogError`]; nothing in this module's public signatures names
//! the parser.

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column_type::ColumnType;
use crate::ir::default_expr::{DefaultExpr, NormalizedExpr};
use crate::ir::function::ArgMode;
use crate::ir::index::Index;
use crate::ir::partition::{PartitionBounds, PartitionBy};
use crate::ir::trigger::Trigger;
use crate::parse::error::{ParseError, SourceLocation};
use pgevolve_pgquery::NodeEnum;

/// Why rebuilding IR from a server-emitted definition failed.
///
/// Every variant carries the definition text that failed. These paths run
/// against output pgevolve did not write and cannot re-request, so a bare
/// "parse failed" leaves nothing to debug from.
#[derive(Debug, thiserror::Error)]
pub enum FromCatalogError {
    /// The parser rejected the (possibly synthesized) SQL outright.
    #[error("parser rejected {kind} {def:?}: {message}")]
    Rejected {
        /// Which catalog accessor produced `def`, e.g. `pg_get_indexdef`.
        kind: &'static str,
        /// The definition text as the server emitted it.
        def: String,
        /// The parser's own message.
        message: String,
    },

    /// The SQL parsed but contained no statement at all.
    #[error("{kind} {def:?} parsed to no statement")]
    NoStatement {
        /// Which catalog accessor produced `def`.
        kind: &'static str,
        /// The definition text as the server emitted it.
        def: String,
    },

    /// The SQL parsed into a statement of the wrong shape.
    #[error("{kind} {def:?} parsed to the wrong statement kind (expected {expected})")]
    WrongStatement {
        /// Which catalog accessor produced `def`.
        kind: &'static str,
        /// The definition text as the server emitted it.
        def: String,
        /// The statement kind the scaffold was built to produce.
        expected: &'static str,
    },

    /// The statement parsed but a clause the scaffold guarantees was absent.
    #[error("{kind} {def:?} parsed but lacked its {missing}")]
    Missing {
        /// Which catalog accessor produced `def`.
        kind: &'static str,
        /// The definition text as the server emitted it.
        def: String,
        /// The clause the scaffold should have produced.
        missing: &'static str,
    },

    /// The statement parsed, but lowering it into IR failed.
    #[error("rebuilding IR from {kind} {def:?} failed: {source}")]
    Build {
        /// Which catalog accessor produced `def`.
        kind: &'static str,
        /// The definition text as the server emitted it.
        def: String,
        /// The lowering failure.
        #[source]
        source: Box<ParseError>,
    },
}

impl FromCatalogError {
    fn build(kind: &'static str, def: &str, source: ParseError) -> Self {
        Self::Build {
            kind,
            def: def.to_string(),
            source: Box::new(source),
        }
    }
}

/// The source location stamped on IR rebuilt from the live catalog.
///
/// Catalog-derived IR has no file behind it; `<catalog>` is the marker every
/// caller used already.
pub fn catalog_location() -> SourceLocation {
    SourceLocation::new(std::path::PathBuf::from("<catalog>"), 1, 1)
}

/// Parse `sql` and return its single top-level node.
///
/// `def` is the server-emitted text and `kind` its accessor; both exist only so
/// failures name the input rather than the scaffold wrapped around it.
fn single_statement(
    kind: &'static str,
    def: &str,
    sql: &str,
) -> Result<NodeEnum, FromCatalogError> {
    let parsed = pgevolve_pgquery::parse(sql).map_err(|e| FromCatalogError::Rejected {
        kind,
        def: def.to_string(),
        message: e.to_string(),
    })?;
    parsed
        .protobuf
        .stmts
        .into_iter()
        .next()
        .and_then(|raw| raw.stmt)
        .and_then(|n| n.node)
        .ok_or_else(|| FromCatalogError::NoStatement {
            kind,
            def: def.to_string(),
        })
}

/// Wrap `text` in `SELECT (…)` and return the expression node underneath.
///
/// Postgres emits bare expressions for defaults, CHECK bodies, and index
/// predicates. The parser only accepts statements, so the expression has to be
/// scaffolded into one and then dug back out.
fn scalar_expression(kind: &'static str, text: &str) -> Result<NodeEnum, FromCatalogError> {
    let sql = format!("SELECT ({text}) AS __pgevolve_expr__");
    let stmt = single_statement(kind, text, &sql)?;
    let NodeEnum::SelectStmt(select) = stmt else {
        return Err(FromCatalogError::WrongStatement {
            kind,
            def: text.to_string(),
            expected: "SelectStmt",
        });
    };
    let target = select
        .target_list
        .into_iter()
        .next()
        .and_then(|n| n.node)
        .ok_or_else(|| FromCatalogError::Missing {
            kind,
            def: text.to_string(),
            missing: "target list entry",
        })?;
    let NodeEnum::ResTarget(res_target) = target else {
        return Err(FromCatalogError::WrongStatement {
            kind,
            def: text.to_string(),
            expected: "ResTarget",
        });
    };
    res_target
        .val
        .and_then(|n| n.node)
        .ok_or_else(|| FromCatalogError::Missing {
            kind,
            def: text.to_string(),
            missing: "target value",
        })
}

// ---- function/aggregate/cast signatures ----

/// One parameter recovered from a server-emitted argument list.
///
/// `name` stays a `String` rather than an [`Identifier`]: the three callers
/// disagree about whether an unnamed parameter is an error, and that judgement
/// belongs to them.
#[derive(Debug, Clone)]
pub struct CatalogParameter {
    /// Parameter name, or `None` when the server emitted a positional type.
    pub(crate) name: Option<String>,
    /// Argument mode, with `TABLE` columns reported as [`ArgMode::Out`] —
    /// which is what they are in an argument list.
    pub(crate) mode: ArgMode,
    /// Whether this parameter came from a `RETURNS TABLE(…)` clause.
    ///
    /// Separate from `mode` because the mapping to [`ArgMode::Out`] is lossy
    /// and `RETURNS TABLE` callers need the distinction back.
    pub(crate) is_table_column: bool,
    /// The parameter's type, lowered through the same path the source-side
    /// parser uses so both sides of a diff compare equal.
    pub(crate) ty: ColumnType,
    /// Parsed `DEFAULT` expression, if the server emitted one.
    pub(crate) default: Option<NormalizedExpr>,
}

/// Parse a server-emitted argument list into owned parameters.
///
/// Accepts the output of `pg_get_function_arguments` (`"x integer, y text
/// DEFAULT 'a'"`) and `pg_get_function_identity_arguments` (`"integer, text"`)
/// alike — the second is the first with the names and defaults left off.
///
/// An empty or whitespace-only list yields no parameters rather than an error;
/// a zero-argument function is ordinary.
pub fn parameter_list(
    kind: &'static str,
    arg_list: &str,
    location: &SourceLocation,
) -> Result<Vec<CatalogParameter>, FromCatalogError> {
    if arg_list.trim().is_empty() {
        return Ok(Vec::new());
    }
    let wrapper = format!(
        "CREATE FUNCTION pgevolve_temp({arg_list}) RETURNS void LANGUAGE sql AS $$ SELECT NULL $$;"
    );
    parameters_of_create_function(kind, arg_list, &wrapper, location)
}

/// Parse the column list of a `RETURNS TABLE(…)` clause.
///
/// Only the `TABLE` parameters come back; a `RETURNS TABLE` scaffold has no
/// others, but filtering keeps the contract explicit.
pub fn returns_table_columns(
    kind: &'static str,
    inner: &str,
    location: &SourceLocation,
) -> Result<Vec<CatalogParameter>, FromCatalogError> {
    let wrapper = format!(
        "CREATE FUNCTION pgevolve_temp() RETURNS TABLE({inner}) LANGUAGE sql AS $$ SELECT NULL $$;"
    );
    let params = parameters_of_create_function(kind, inner, &wrapper, location)?;
    Ok(params.into_iter().filter(|p| p.is_table_column).collect())
}

/// Shared walk over a synthesized `CREATE FUNCTION`'s parameter list.
fn parameters_of_create_function(
    kind: &'static str,
    def: &str,
    wrapper: &str,
    location: &SourceLocation,
) -> Result<Vec<CatalogParameter>, FromCatalogError> {
    use pgevolve_pgquery::protobuf::FunctionParameterMode as PgMode;

    let stmt = single_statement(kind, def, wrapper)?;
    let NodeEnum::CreateFunctionStmt(stmt) = stmt else {
        return Err(FromCatalogError::WrongStatement {
            kind,
            def: def.to_string(),
            expected: "CreateFunctionStmt",
        });
    };

    let mut out = Vec::with_capacity(stmt.parameters.len());
    for param_node in &stmt.parameters {
        let Some(NodeEnum::FunctionParameter(param)) = param_node.node.as_ref() else {
            continue;
        };
        let raw_mode = PgMode::try_from(param.mode).unwrap_or(PgMode::Undefined);
        let mode = match raw_mode {
            PgMode::FuncParamIn | PgMode::FuncParamDefault | PgMode::Undefined => ArgMode::In,
            PgMode::FuncParamOut | PgMode::FuncParamTable => ArgMode::Out,
            PgMode::FuncParamInout => ArgMode::InOut,
            PgMode::FuncParamVariadic => ArgMode::Variadic,
        };
        let Some(type_name) = param.arg_type.as_ref() else {
            continue;
        };
        let ty = super::builder::shared::type_name_to_column_type(type_name, location)
            .map_err(|e| FromCatalogError::build(kind, def, e))?;

        // Errors are swallowed rather than propagated: a parameter default the
        // normalizer cannot represent is recorded as "no default", which is how
        // this path has always behaved.
        let default = param
            .defexpr
            .as_ref()
            .and_then(|d| d.node.as_ref())
            .and_then(|node| super::normalize_expr::from_pg_node(node, Some(&ty), location).ok());

        out.push(CatalogParameter {
            name: (!param.name.is_empty()).then(|| param.name.clone()),
            mode,
            is_table_column: raw_mode == PgMode::FuncParamTable,
            ty,
            default,
        });
    }
    Ok(out)
}

// ---- partitioning ----

/// Rebuild a `PARTITION BY` clause from `pg_get_partkeydef` output.
pub fn partition_by(def: &str, location: &SourceLocation) -> Result<PartitionBy, FromCatalogError> {
    const KIND: &str = "pg_get_partkeydef";

    let synthetic = format!(
        "CREATE TABLE _pgevolve_synth () PARTITION BY {};",
        def.trim()
    );
    let stmt = single_statement(KIND, def, &synthetic)?;
    let NodeEnum::CreateStmt(create) = stmt else {
        return Err(FromCatalogError::WrongStatement {
            kind: KIND,
            def: def.to_string(),
            expected: "CreateStmt",
        });
    };
    let spec = create
        .partspec
        .as_ref()
        .ok_or_else(|| FromCatalogError::Missing {
            kind: KIND,
            def: def.to_string(),
            missing: "partition spec",
        })?;
    super::builder::create_stmt::build_partition_by(spec, location)
        .map_err(|e| FromCatalogError::build(KIND, def, e))
}

/// Rebuild a partition's bounds from `pg_get_expr(relpartbound, …)` output.
pub fn partition_bounds(
    def: &str,
    location: &SourceLocation,
) -> Result<PartitionBounds, FromCatalogError> {
    const KIND: &str = "pg_get_expr(relpartbound)";

    let synthetic = format!(
        "ALTER TABLE _pgevolve_synth ATTACH PARTITION _pgevolve_synth_child {};",
        def.trim()
    );
    let stmt = single_statement(KIND, def, &synthetic)?;
    let NodeEnum::AlterTableStmt(alter) = stmt else {
        return Err(FromCatalogError::WrongStatement {
            kind: KIND,
            def: def.to_string(),
            expected: "AlterTableStmt",
        });
    };
    // ALTER TABLE … ATTACH PARTITION lowers to
    // AlterTableStmt → AlterTableCmd → PartitionCmd → PartitionBoundSpec.
    let missing = |what: &'static str| FromCatalogError::Missing {
        kind: KIND,
        def: def.to_string(),
        missing: what,
    };
    let spec = alter
        .cmds
        .into_iter()
        .next()
        .and_then(|n| n.node)
        .and_then(|n| match n {
            NodeEnum::AlterTableCmd(cmd) => cmd.def,
            _ => None,
        })
        .and_then(|n| n.node)
        .and_then(|n| match n {
            NodeEnum::PartitionCmd(part_cmd) => part_cmd.bound,
            _ => None,
        })
        .ok_or_else(|| missing("partition bound spec"))?;
    super::builder::create_stmt::build_partition_bounds(&spec, location)
        .map_err(|e| FromCatalogError::build(KIND, def, e))
}

// ---- indexes and triggers ----

/// Rebuild [`Index`] IR from `pg_get_indexdef` output.
pub fn index(def: &str, location: &SourceLocation) -> Result<Index, FromCatalogError> {
    const KIND: &str = "pg_get_indexdef";

    let stmt = single_statement(KIND, def, def)?;
    let NodeEnum::IndexStmt(index_stmt) = stmt else {
        return Err(FromCatalogError::WrongStatement {
            kind: KIND,
            def: def.to_string(),
            expected: "IndexStmt",
        });
    };
    super::builder::index_stmt::build_index(&index_stmt, None, location)
        .map_err(|e| FromCatalogError::build(KIND, def, e))
}

/// Rebuild [`Trigger`] IR from `pg_get_triggerdef` output.
pub fn trigger(def: &str, location: &SourceLocation) -> Result<Trigger, FromCatalogError> {
    const KIND: &str = "pg_get_triggerdef";

    let stmt = single_statement(KIND, def, def)?;
    let NodeEnum::CreateTrigStmt(trig) = stmt else {
        return Err(FromCatalogError::WrongStatement {
            kind: KIND,
            def: def.to_string(),
            expected: "CreateTrigStmt",
        });
    };
    super::builder::create_trigger_stmt::build_trigger(&trig, location)
        .map_err(|e| FromCatalogError::build(KIND, def, e))
}

// ---- constraints and expressions ----

/// Extract the referenced-column list from a `pg_get_constraintdef` FK body.
///
/// `pk_attrs` holds the columns on the *referenced* side; `fk_attrs` holds the
/// local ones.
///
/// Returns `None` when the body does not parse or names no columns. The caller
/// treats that as an error — this signature stays `Option` because "no columns"
/// and "unparseable" call for the same handling and the caller has the
/// constraint name needed to say so usefully.
pub fn fk_referenced_columns(def: &str) -> Option<Vec<Identifier>> {
    let synthetic =
        format!("CREATE TABLE _pgevolve_synth (_pgevolve_dummy int, CONSTRAINT _c {def});");
    let stmt = single_statement("pg_get_constraintdef", def, &synthetic).ok()?;
    let NodeEnum::CreateStmt(create) = stmt else {
        return None;
    };
    let constraint = create.table_elts.into_iter().find_map(|n| match n.node {
        Some(NodeEnum::Constraint(c)) => Some(c),
        _ => None,
    })?;
    let columns: Vec<Identifier> = constraint
        .pk_attrs
        .into_iter()
        .filter_map(|n| match n.node {
            Some(NodeEnum::String(s)) => Identifier::from_unquoted(&s.sval).ok(),
            _ => None,
        })
        .collect();
    (!columns.is_empty()).then_some(columns)
}

/// Normalize a bare SQL expression emitted by `pg_get_expr`.
pub fn expression(
    kind: &'static str,
    text: &str,
    location: &SourceLocation,
) -> Result<NormalizedExpr, FromCatalogError> {
    let node = scalar_expression(kind, text)?;
    super::normalize_expr::from_pg_node(&node, None, location)
        .map_err(|e| FromCatalogError::build(kind, text, e))
}

/// Rebuild a column `DEFAULT` from `pg_get_expr(adbin, …)` output.
///
/// `target_type` lets the lowering strip the redundant casts Postgres adds when
/// it deparses a stored default, so a catalog-side default compares equal to
/// the same default written by hand.
pub fn default_expr(
    text: &str,
    target_type: &ColumnType,
    location: &SourceLocation,
) -> Result<DefaultExpr, FromCatalogError> {
    const KIND: &str = "pg_get_expr(adbin)";

    let node = scalar_expression(KIND, text)?;
    super::builder::shared::build_default_expr(&node, Some(target_type), None, location)
        .map_err(|e| FromCatalogError::build(KIND, text, e))
}

// ---- view bodies ----

/// A view body that the parser rejected outright.
///
/// Distinct from [`FromCatalogError`] because the caller maps it to a different
/// catalog error, and because the distinction matters: extraction *within* a
/// parsed body is deliberately best-effort, but a body that does not parse
/// yields an empty edge list that is indistinguishable from "depends on
/// nothing" — and the planner would then order the view before the relations it
/// selects from.
#[derive(Debug, thiserror::Error)]
#[error("view body did not parse")]
pub struct UnparseableViewBody;

/// Collect dependency edges from a view body on the catalog side.
///
/// Any schema-qualified relation reference becomes an edge. Unqualified
/// references are skipped: on the catalog side every name the server emits is
/// already schema-qualified, so an unqualified one is a CTE or an alias.
pub fn view_dep_edges(
    body_text: &str,
    view_qname: &QualifiedName,
) -> Result<Vec<crate::plan::edges::DepEdge>, UnparseableViewBody> {
    let parsed = pgevolve_pgquery::parse(body_text).map_err(|_| UnparseableViewBody)?;
    let mut deps = Vec::new();
    for raw_stmt in &parsed.protobuf.stmts {
        if let Some(node) = &raw_stmt.stmt {
            walk_node_for_deps(node, view_qname, &mut deps);
        }
    }
    deps.sort();
    deps.dedup();
    Ok(deps)
}

/// Walk a single AST node, collecting schema-qualified relation references.
fn walk_node_for_deps(
    node: &pgevolve_pgquery::protobuf::Node,
    view_qname: &QualifiedName,
    deps: &mut Vec<crate::plan::edges::DepEdge>,
) {
    use crate::plan::edges::{DepEdge, DepSource, NodeId};
    use pgevolve_pgquery::NodeEnum as N;

    let Some(inner) = &node.node else { return };
    match inner {
        N::SelectStmt(sel) => {
            for from in &sel.from_clause {
                walk_node_for_deps(from, view_qname, deps);
            }
            if let Some(wc) = &sel.where_clause {
                walk_node_for_deps(wc, view_qname, deps);
            }
            if let Some(larg) = &sel.larg {
                let n = pgevolve_pgquery::protobuf::Node {
                    node: Some(N::SelectStmt(Box::new(larg.as_ref().clone()))),
                };
                walk_node_for_deps(&n, view_qname, deps);
            }
            if let Some(rarg) = &sel.rarg {
                let n = pgevolve_pgquery::protobuf::Node {
                    node: Some(N::SelectStmt(Box::new(rarg.as_ref().clone()))),
                };
                walk_node_for_deps(&n, view_qname, deps);
            }
            if let Some(with) = &sel.with_clause {
                for cte in &with.ctes {
                    walk_node_for_deps(cte, view_qname, deps);
                }
            }
        }
        N::RangeVar(rv) if !rv.schemaname.is_empty() && !rv.relname.is_empty() => {
            if let (Ok(s), Ok(n)) = (
                Identifier::from_unquoted(&rv.schemaname)
                    .or_else(|_| Identifier::from_quoted(&rv.schemaname)),
                Identifier::from_unquoted(&rv.relname)
                    .or_else(|_| Identifier::from_quoted(&rv.relname)),
            ) {
                deps.push(DepEdge {
                    from: NodeId::Table(view_qname.clone()),
                    to: NodeId::Table(QualifiedName::new(s, n)),
                    source: DepSource::AstExtracted,
                });
            }
        }
        N::JoinExpr(j) => {
            if let Some(l) = &j.larg {
                walk_node_for_deps(l, view_qname, deps);
            }
            if let Some(r) = &j.rarg {
                walk_node_for_deps(r, view_qname, deps);
            }
        }
        N::RangeSubselect(sub) => {
            if let Some(sq) = &sub.subquery {
                walk_node_for_deps(sq, view_qname, deps);
            }
        }
        N::CommonTableExpr(cte) => {
            if let Some(q) = &cte.ctequery {
                walk_node_for_deps(q, view_qname, deps);
            }
        }
        _ => {}
    }
}

// ---- row-filter expressions ----

/// Collect unqualified column names referenced by a SQL expression.
///
/// Used by the publication row-filter lint. Only bare single-field references
/// come back: a row filter is scoped to one table, so a qualified reference
/// cannot name a column the lint is checking.
///
/// Returns `None` when the expression does not parse, which the lint treats as
/// "nothing to say about this filter".
pub fn unqualified_column_refs(expr_text: &str) -> Option<Vec<String>> {
    let sql = format!("SELECT * FROM _t WHERE {expr_text}");
    let parsed = pgevolve_pgquery::parse(&sql).ok()?;
    let mut names = Vec::new();
    for stmt in &parsed.protobuf.stmts {
        let Some(node) = &stmt.stmt else { continue };
        collect_column_refs(node, &mut names);
    }
    Some(names)
}

/// Recursively walk a node, collecting unqualified column references.
fn collect_column_refs(node: &pgevolve_pgquery::protobuf::Node, out: &mut Vec<String>) {
    use pgevolve_pgquery::NodeEnum as N;

    let Some(inner) = &node.node else { return };
    match inner {
        // Only single-field refs; `schema.table.col` cannot name a row-filter column.
        N::ColumnRef(cref) if cref.fields.len() == 1 => {
            if let Some(field) = cref.fields.first()
                && let Some(N::String(s)) = &field.node
                && !s.sval.is_empty()
            {
                out.push(s.sval.clone());
            }
        }
        N::SelectStmt(sel) => {
            if let Some(w) = &sel.where_clause {
                collect_column_refs(w, out);
            }
            for t in &sel.target_list {
                collect_column_refs(t, out);
            }
        }
        N::BoolExpr(b) => {
            for arg in &b.args {
                collect_column_refs(arg, out);
            }
        }
        N::AExpr(a) => {
            if let Some(l) = &a.lexpr {
                collect_column_refs(l, out);
            }
            if let Some(r) = &a.rexpr {
                collect_column_refs(r, out);
            }
        }
        N::SubLink(sl) => {
            if let Some(t) = &sl.testexpr {
                collect_column_refs(t, out);
            }
            if let Some(q) = &sl.subselect {
                collect_column_refs(q, out);
            }
        }
        N::FuncCall(fc) => {
            for arg in &fc.args {
                collect_column_refs(arg, out);
            }
        }
        N::NullTest(nt) => {
            if let Some(a) = &nt.arg {
                collect_column_refs(a, out);
            }
        }
        N::BooleanTest(bt) => {
            if let Some(a) = &bt.arg {
                collect_column_refs(a, out);
            }
        }
        N::CaseExpr(ce) => {
            if let Some(a) = &ce.arg {
                collect_column_refs(a, out);
            }
            for w in &ce.args {
                collect_column_refs(w, out);
            }
            if let Some(d) = &ce.defresult {
                collect_column_refs(d, out);
            }
        }
        N::CaseWhen(cw) => {
            if let Some(e) = &cw.expr {
                collect_column_refs(e, out);
            }
            if let Some(r) = &cw.result {
                collect_column_refs(r, out);
            }
        }
        N::ResTarget(rt) => {
            if let Some(v) = &rt.val {
                collect_column_refs(v, out);
            }
        }
        N::TypeCast(tc) => {
            if let Some(a) = &tc.arg {
                collect_column_refs(a, out);
            }
        }
        _ => {}
    }
}
