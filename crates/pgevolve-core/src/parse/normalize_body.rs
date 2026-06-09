//! Statement-scope body canonicalization.
//!
//! Counterpart to [`crate::ir::default_expr::NormalizedExpr`].
//! Where `NormalizedExpr` canonicalizes one expression, `NormalizedBody`
//! canonicalizes a statement-shaped body — a view's `SELECT`, a function
//! body, an expression-index predicate at full-statement scope.
//!
//! Canonicalization rules (per arch spec Decision 10):
//!
//! - Whitespace collapses; one space between tokens; newlines stripped.
//! - Keywords lowercased (via `pg_query`'s deparser, which already lowercases
//!   most keywords; see `normalize_expr` for additional belt-and-suspenders
//!   lowercasing if needed in v0.2).
//! - Redundant parens folded (`pg_query`'s deparser removes them on round-trip).
//! - Identifiers preserved verbatim (qualification, quoting).
//!
//! For v0.1 this module is unused; v0.2 view/function sub-specs are
//! its first consumers.

use serde::{Deserialize, Serialize};

/// A canonicalized statement-scope body.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NormalizedBody {
    canonical_text: String,
    canonical_hash: [u8; 32],
}

/// Error parsing a body.
#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    /// `pg_query` rejected the SQL.
    #[error("pg_query rejected body: {0}")]
    Parse(String),
}

impl NormalizedBody {
    /// Sentinel for source-parse provisional records.
    ///
    /// T4's AST canonicalization pass overwrites this immediately after the
    /// source IR is assembled. Never serialized to plan output.
    pub const fn empty() -> Self {
        Self {
            canonical_text: String::new(),
            canonical_hash: [0u8; 32],
        }
    }

    /// Build a `NormalizedBody` from a pre-computed canonical text string.
    ///
    /// Used by the PL/pgSQL and SQL body parsers in `parse::builder::plpgsql`
    /// which produce their own canonical form (whitespace-collapsed text or
    /// `pg_query::normalize` output) and need to inject it directly.
    ///
    /// Callers are responsible for ensuring `canonical_text` is in the
    /// pgevolve canonical form (whitespace collapsed, keywords lowercased).
    pub fn from_raw_canonical(canonical_text: String) -> Self {
        let canonical_hash = hash_canonical(&canonical_text);
        Self {
            canonical_text,
            canonical_hash,
        }
    }

    /// Canonicalize a body given its raw SQL text.
    ///
    /// The body may be any complete SQL statement (`SELECT`, `CREATE VIEW`,
    /// etc.). Invalid SQL returns [`BodyError::Parse`]. If the deparser
    /// unexpectedly fails on a successfully-parsed tree, the original SQL is
    /// used as the canonical form (silent graceful degradation).
    pub fn from_sql(sql: &str) -> Result<Self, BodyError> {
        let parsed = pg_query::parse(sql).map_err(|e| BodyError::Parse(e.to_string()))?;
        // Strip redundant table-qualifier prefixes from column references in
        // single-table SELECTs (e.g., `SELECT users.id FROM app.users` →
        // `SELECT id FROM app.users`). PG14's `pg_get_viewdef` keeps the
        // qualifier even when unambiguous, while PG17 strips it; canonicalize
        // to the unqualified form so source and catalog texts match.
        let mut protobuf = parsed.protobuf;
        strip_redundant_qualifiers(&mut protobuf);
        let deparsed = pg_query::deparse(&protobuf).unwrap_or_default();
        let source = if deparsed.is_empty() { sql } else { &deparsed };
        let canonical_text = collapse_whitespace(source);
        let canonical_hash = hash_canonical(&canonical_text);
        Ok(Self {
            canonical_text,
            canonical_hash,
        })
    }

    /// The canonical text. Two bodies are equivalent iff their canonical
    /// texts are byte-equal.
    pub fn canonical_text(&self) -> &str {
        &self.canonical_text
    }

    /// BLAKE3 hash of the canonical text. Domain-separated with
    /// `pgevolve-normalized-body-v1\n` to avoid collisions with
    /// [`crate::plan::plan::PlanId`] hashes (`pgevolve-plan-id-v1\n`).
    ///
    /// Not `const fn`: `NormalizedBody` is only constructed at runtime (via
    /// `pg_query`), so `const` would signal intent the type cannot fulfill.
    #[allow(clippy::missing_const_for_fn)]
    pub fn canonical_hash(&self) -> &[u8; 32] {
        &self.canonical_hash
    }
}

/// Walk the parse tree and strip redundant table qualifiers from column
/// references. For each `SelectStmt`, collect the names usable for column
/// qualification from its `FROM` clause (the alias if present, else the last
/// segment of the relation name). When the FROM clause yields exactly one
/// such name, every `ColumnRef` of the form `[that_name, col]` is rewritten
/// to `[col]`.
///
/// Limitations:
/// - Only applies to single-relation FROM clauses. Joins (a multi-relation
///   FROM) keep their qualifiers (PG may still need them for disambiguation):
///   such a scope yields no unique name, so nothing local is stripped.
/// - Each nested `SelectStmt` is canonicalized in its OWN scope. We recurse
///   into set-op children, `RangeSubselect` subqueries, `JoinExpr` arms,
///   `WITH`/CTE queries (`CommonTableExpr.ctequery`), and `SubLink`
///   subselects found in the target list / `WHERE` / `HAVING`. Every nested
///   scope computes its own single-FROM qualifier from its own `FROM` clause
///   and may only strip that local name. A correlated subquery's reference to
///   an OUTER relation uses a different qualifier (the outer name), which does
///   not match the inner scope's local name and is therefore preserved — so
///   recursion can only ever remove a qualifier that is genuinely redundant
///   within the scope it appears in (never collapse two distinct bodies).
fn strip_redundant_qualifiers(root: &mut pg_query::protobuf::ParseResult) {
    use pg_query::NodeEnum;
    for stmt in &mut root.stmts {
        let Some(node) = stmt.stmt.as_mut().and_then(|n| n.node.as_mut()) else {
            continue;
        };
        if let NodeEnum::SelectStmt(sel) = node {
            strip_qualifiers_in_select(sel);
        }
    }
}

fn strip_qualifiers_in_select(sel: &mut pg_query::protobuf::SelectStmt) {
    // Recurse into every nested scope FIRST (each in its own scope), then
    // strip the local scope. This mirrors the larg/rarg ordering: a child
    // scope's qualifiers are resolved against the child's own FROM clause,
    // never against this scope's name.

    // Set-op children.
    if let Some(larg) = sel.larg.as_mut() {
        strip_qualifiers_in_select(larg);
    }
    if let Some(rarg) = sel.rarg.as_mut() {
        strip_qualifiers_in_select(rarg);
    }

    // WITH / CTE queries: each CTE body is its own scope.
    if let Some(with) = sel.with_clause.as_mut() {
        for cte in &mut with.ctes {
            recurse_cte(cte);
        }
    }

    // FROM clause: subqueries (`RangeSubselect`) and join arms (`JoinExpr`).
    for n in &mut sel.from_clause {
        recurse_from_node(n);
    }

    // SubLinks embedded in expression positions: their subselect is a scope.
    for n in &mut sel.target_list {
        recurse_subselects_in_node(n);
    }
    if let Some(w) = sel.where_clause.as_mut() {
        recurse_subselects_in_node(w);
    }
    if let Some(h) = sel.having_clause.as_mut() {
        recurse_subselects_in_node(h);
    }

    let from_names = collect_from_qualifiers(&sel.from_clause);
    let Some(unique_name) = unique_from_qualifier(&from_names) else {
        return;
    };

    // Walk every column ref in target list, WHERE, GROUP BY, HAVING, ORDER BY.
    for n in &mut sel.target_list {
        strip_qualifier_in_node(n, &unique_name);
    }
    if let Some(w) = sel.where_clause.as_mut() {
        strip_qualifier_in_node(w, &unique_name);
    }
    for n in &mut sel.group_clause {
        strip_qualifier_in_node(n, &unique_name);
    }
    if let Some(h) = sel.having_clause.as_mut() {
        strip_qualifier_in_node(h, &unique_name);
    }
    for n in &mut sel.sort_clause {
        strip_qualifier_in_node(n, &unique_name);
    }
}

/// If `node` wraps a `SelectStmt`, canonicalize it in its own scope.
fn recurse_select_node(node: &mut pg_query::protobuf::Node) {
    use pg_query::NodeEnum;
    if let Some(NodeEnum::SelectStmt(sel)) = node.node.as_mut() {
        strip_qualifiers_in_select(sel);
    }
}

/// Recurse into a CTE's query (`CommonTableExpr.ctequery`), a fresh scope.
fn recurse_cte(cte: &mut pg_query::protobuf::Node) {
    use pg_query::NodeEnum;
    if let Some(NodeEnum::CommonTableExpr(c)) = cte.node.as_mut()
        && let Some(q) = c.ctequery.as_mut()
    {
        recurse_select_node(q);
    }
}

/// Recurse into a FROM-clause entry: a `RangeSubselect` subquery is its own
/// scope; a `JoinExpr`'s arms may themselves be subselects/joins/range-vars.
/// Plain `RangeVar`s are left for `collect_from_qualifiers` (the local scope).
fn recurse_from_node(n: &mut pg_query::protobuf::Node) {
    use pg_query::NodeEnum;
    match n.node.as_mut() {
        Some(NodeEnum::RangeSubselect(rs)) => {
            if let Some(q) = rs.subquery.as_mut() {
                recurse_select_node(q);
            }
        }
        Some(NodeEnum::JoinExpr(je)) => {
            if let Some(l) = je.larg.as_mut() {
                recurse_from_node(l);
            }
            if let Some(r) = je.rarg.as_mut() {
                recurse_from_node(r);
            }
        }
        _ => {}
    }
}

/// Walk an expression node looking for `SubLink`s; recurse into each
/// `SubLink.subselect` in its own scope. Mirrors the child-walking shape of
/// [`strip_qualifier_in_node`] so every expression position that can hold a
/// subquery is reached. No outer-scope name is passed down.
fn recurse_subselects_in_node(n: &mut pg_query::protobuf::Node) {
    use pg_query::NodeEnum;
    let Some(node) = n.node.as_mut() else { return };
    match node {
        NodeEnum::SubLink(sl) => {
            if let Some(t) = sl.testexpr.as_mut() {
                recurse_subselects_in_node(t);
            }
            if let Some(s) = sl.subselect.as_mut() {
                recurse_select_node(s);
            }
        }
        NodeEnum::ResTarget(rt) => {
            if let Some(v) = rt.val.as_mut() {
                recurse_subselects_in_node(v);
            }
        }
        NodeEnum::AExpr(e) => {
            if let Some(l) = e.lexpr.as_mut() {
                recurse_subselects_in_node(l);
            }
            if let Some(r) = e.rexpr.as_mut() {
                recurse_subselects_in_node(r);
            }
        }
        NodeEnum::BoolExpr(e) => {
            for a in &mut e.args {
                recurse_subselects_in_node(a);
            }
        }
        NodeEnum::FuncCall(fc) => {
            for a in &mut fc.args {
                recurse_subselects_in_node(a);
            }
            if let Some(filt) = fc.agg_filter.as_mut() {
                recurse_subselects_in_node(filt);
            }
            for a in &mut fc.agg_order {
                recurse_subselects_in_node(a);
            }
        }
        NodeEnum::CoalesceExpr(c) => {
            for a in &mut c.args {
                recurse_subselects_in_node(a);
            }
        }
        NodeEnum::CaseExpr(c) => {
            if let Some(arg) = c.arg.as_mut() {
                recurse_subselects_in_node(arg);
            }
            for w in &mut c.args {
                recurse_subselects_in_node(w);
            }
            if let Some(d) = c.defresult.as_mut() {
                recurse_subselects_in_node(d);
            }
        }
        NodeEnum::CaseWhen(w) => {
            if let Some(e) = w.expr.as_mut() {
                recurse_subselects_in_node(e);
            }
            if let Some(r) = w.result.as_mut() {
                recurse_subselects_in_node(r);
            }
        }
        NodeEnum::TypeCast(tc) => {
            if let Some(arg) = tc.arg.as_mut() {
                recurse_subselects_in_node(arg);
            }
        }
        NodeEnum::SortBy(sb) => {
            if let Some(arg) = sb.node.as_mut() {
                recurse_subselects_in_node(arg);
            }
        }
        NodeEnum::List(l) => {
            for item in &mut l.items {
                recurse_subselects_in_node(item);
            }
        }
        NodeEnum::NullTest(nt) => {
            if let Some(arg) = nt.arg.as_mut() {
                recurse_subselects_in_node(arg);
            }
        }
        NodeEnum::BooleanTest(bt) => {
            if let Some(arg) = bt.arg.as_mut() {
                recurse_subselects_in_node(arg);
            }
        }
        _ => {}
    }
}

fn collect_from_qualifiers(from: &[pg_query::protobuf::Node]) -> Vec<String> {
    use pg_query::NodeEnum;
    let mut names = Vec::new();
    for n in from {
        let Some(node) = n.node.as_ref() else {
            continue;
        };
        if let NodeEnum::RangeVar(rv) = node {
            let qual = rv
                .alias
                .as_ref()
                .map_or_else(|| rv.relname.clone(), |a| a.aliasname.clone());
            if !qual.is_empty() {
                names.push(qual);
            }
        }
    }
    names
}

fn unique_from_qualifier(names: &[String]) -> Option<String> {
    if names.len() == 1 {
        Some(names[0].clone())
    } else {
        None
    }
}

fn strip_qualifier_in_node(n: &mut pg_query::protobuf::Node, qualifier: &str) {
    use pg_query::NodeEnum;
    let Some(node) = n.node.as_mut() else { return };
    match node {
        NodeEnum::ColumnRef(cref) => {
            if cref.fields.len() == 2
                && let Some(first) = cref.fields.first()
                && let Some(NodeEnum::String(s)) = first.node.as_ref()
                && s.sval == qualifier
            {
                cref.fields.remove(0);
            }
        }
        NodeEnum::ResTarget(rt) => {
            if let Some(v) = rt.val.as_mut() {
                strip_qualifier_in_node(v, qualifier);
            }
        }
        NodeEnum::AExpr(e) => {
            if let Some(l) = e.lexpr.as_mut() {
                strip_qualifier_in_node(l, qualifier);
            }
            if let Some(r) = e.rexpr.as_mut() {
                strip_qualifier_in_node(r, qualifier);
            }
        }
        NodeEnum::BoolExpr(e) => {
            for a in &mut e.args {
                strip_qualifier_in_node(a, qualifier);
            }
        }
        NodeEnum::FuncCall(fc) => {
            for a in &mut fc.args {
                strip_qualifier_in_node(a, qualifier);
            }
            if let Some(filt) = fc.agg_filter.as_mut() {
                strip_qualifier_in_node(filt, qualifier);
            }
            for a in &mut fc.agg_order {
                strip_qualifier_in_node(a, qualifier);
            }
        }
        NodeEnum::CoalesceExpr(c) => {
            for a in &mut c.args {
                strip_qualifier_in_node(a, qualifier);
            }
        }
        NodeEnum::CaseExpr(c) => {
            if let Some(arg) = c.arg.as_mut() {
                strip_qualifier_in_node(arg, qualifier);
            }
            for w in &mut c.args {
                strip_qualifier_in_node(w, qualifier);
            }
            if let Some(d) = c.defresult.as_mut() {
                strip_qualifier_in_node(d, qualifier);
            }
        }
        NodeEnum::CaseWhen(w) => {
            if let Some(e) = w.expr.as_mut() {
                strip_qualifier_in_node(e, qualifier);
            }
            if let Some(r) = w.result.as_mut() {
                strip_qualifier_in_node(r, qualifier);
            }
        }
        NodeEnum::TypeCast(tc) => {
            if let Some(arg) = tc.arg.as_mut() {
                strip_qualifier_in_node(arg, qualifier);
            }
        }
        NodeEnum::SortBy(sb) => {
            if let Some(arg) = sb.node.as_mut() {
                strip_qualifier_in_node(arg, qualifier);
            }
        }
        NodeEnum::List(l) => {
            for item in &mut l.items {
                strip_qualifier_in_node(item, qualifier);
            }
        }
        NodeEnum::SubLink(sl) => {
            if let Some(t) = sl.testexpr.as_mut() {
                strip_qualifier_in_node(t, qualifier);
            }
        }
        NodeEnum::NullTest(nt) => {
            if let Some(arg) = nt.arg.as_mut() {
                strip_qualifier_in_node(arg, qualifier);
            }
        }
        NodeEnum::BooleanTest(bt) => {
            if let Some(arg) = bt.arg.as_mut() {
                strip_qualifier_in_node(arg, qualifier);
            }
        }
        _ => {}
    }
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn hash_canonical(text: &str) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(b"pgevolve-normalized-body-v1\n");
    h.update(text.as_bytes());
    *h.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_qualifiers_equates_pg14_and_source_form() {
        let pg14 = "SELECT products.category, count(*) AS cnt, avg(products.price) AS avg_price FROM app.products GROUP BY products.category";
        let src = "SELECT category, count(*) AS cnt, avg(price) AS avg_price FROM app.products GROUP BY category";
        let a = NormalizedBody::from_sql(pg14).unwrap();
        let b = NormalizedBody::from_sql(src).unwrap();
        assert_eq!(
            a.canonical_text(),
            b.canonical_text(),
            "left: {} right: {}",
            a.canonical_text(),
            b.canonical_text()
        );
    }

    /// A qualified column inside a `RangeSubselect` subquery is stripped in
    /// the inner scope (its single FROM is `app.t`, alias-less, so `t`).
    #[test]
    fn subquery_inner_scope_strip() {
        let qualified =
            NormalizedBody::from_sql("SELECT s.id FROM (SELECT t.id FROM app.t) s").unwrap();
        let unqualified =
            NormalizedBody::from_sql("SELECT s.id FROM (SELECT id FROM app.t) s").unwrap();
        assert_eq!(
            qualified.canonical_text(),
            unqualified.canonical_text(),
            "inner t.id should strip to id: {} vs {}",
            qualified.canonical_text(),
            unqualified.canonical_text()
        );
    }

    /// A qualified column inside a CTE body is stripped in the CTE's scope.
    #[test]
    fn cte_inner_scope_strip() {
        let qualified =
            NormalizedBody::from_sql("WITH c AS (SELECT u.x FROM app.u) SELECT * FROM c").unwrap();
        let unqualified =
            NormalizedBody::from_sql("WITH c AS (SELECT x FROM app.u) SELECT * FROM c").unwrap();
        assert_eq!(
            qualified.canonical_text(),
            unqualified.canonical_text(),
            "inner u.x should strip to x: {} vs {}",
            qualified.canonical_text(),
            unqualified.canonical_text()
        );
    }

    /// THE false-negative guard. In a correlated `EXISTS` subquery the inner
    /// scope's single FROM is `app.u`, so `u.fk` strips to `fk`, but the
    /// outer reference `t.pk` (qualifier `t` != inner `u`) MUST survive.
    /// Stripping it would collapse semantically-different bodies.
    #[test]
    fn correlated_subquery_outer_qualifier_preserved() {
        let body = NormalizedBody::from_sql(
            "SELECT a FROM app.t WHERE EXISTS (SELECT 1 FROM app.u WHERE u.fk = t.pk)",
        )
        .unwrap();
        // Outer correlated qualifier `t` preserved on the column it qualifies.
        assert!(
            body.canonical_text().contains("t.pk"),
            "correlated outer ref t.pk must be preserved, got: {}",
            body.canonical_text()
        );
        // Inner local qualifier `u` was stripped (u.fk -> fk).
        assert!(
            !body.canonical_text().contains("u.fk"),
            "inner u.fk should have stripped to fk, got: {}",
            body.canonical_text()
        );

        // And we did not over-collapse: changing the correlation predicate
        // produces a DIFFERENT canonical hash.
        let variant = NormalizedBody::from_sql(
            "SELECT a FROM app.t WHERE EXISTS (SELECT 1 FROM app.u WHERE u.fk = t.other)",
        )
        .unwrap();
        assert_ne!(
            body.canonical_hash(),
            variant.canonical_hash(),
            "distinct correlation predicates must not collapse: {} vs {}",
            body.canonical_text(),
            variant.canonical_text()
        );
    }

    /// Distinct top-level bodies still hash differently.
    #[test]
    fn distinct_bodies_differ() {
        let a = NormalizedBody::from_sql("SELECT a FROM app.t").unwrap();
        let b = NormalizedBody::from_sql("SELECT b FROM app.t").unwrap();
        assert_ne!(a.canonical_hash(), b.canonical_hash());
    }

    /// A multi-relation top-level FROM (a join) yields no unique local name,
    /// so its qualifiers are preserved — unchanged behavior.
    #[test]
    fn multi_table_top_level_from_unchanged() {
        let body =
            NormalizedBody::from_sql("SELECT t.a, u.b FROM app.t JOIN app.u ON t.k = u.k").unwrap();
        assert!(
            body.canonical_text().contains("t.a"),
            "join scope must keep t.a, got: {}",
            body.canonical_text()
        );
        assert!(
            body.canonical_text().contains("u.b"),
            "join scope must keep u.b, got: {}",
            body.canonical_text()
        );
    }
}
