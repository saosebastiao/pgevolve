//! PL/pgSQL body parsing — dep extraction, COMMIT/ROLLBACK detection,
//! `-- @pgevolve dep:` directive scanning.
//!
//! Entry point: [`parse_routine_body`].

use serde_json::Value;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::function::FunctionLanguage;
use crate::parse::error::{ParseError, SourceLocation};
use crate::parse::normalize_body::NormalizedBody;
use crate::plan::edges::{DepEdge, DepSource, NodeId};

/// Parse a routine body and produce its [`NormalizedBody`], extracted
/// [`DepEdge`]s, and the `commits_in_body` flag.
///
/// `commits_in_body` is only meaningful for procedures; it is always `false`
/// for SQL-language bodies (SQL functions cannot issue COMMIT/ROLLBACK).
///
/// `is_set_returning` controls which wrapper `RETURNS` clause the PL/pgSQL
/// parser uses: `RETURNS SETOF record` for set-returning functions (those
/// declared with `RETURNS SETOF …` or `RETURNS TABLE(…)`), `RETURNS void`
/// otherwise. The `pg_query` plpgsql analyzer validates `RETURN QUERY`/`RETURN
/// NEXT` against the declared set-returning-ness of the wrapper function, so
/// using the wrong clause causes it to reject legal bodies with "cannot use
/// RETURN QUERY in a non-SETOF function".  The SQL-body path ignores the flag.
pub fn parse_routine_body(
    body_text: &str,
    language: FunctionLanguage,
    is_set_returning: bool,
    routine_qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<(NormalizedBody, Vec<DepEdge>, bool), ParseError> {
    match language {
        FunctionLanguage::Sql => {
            let (body, deps) = parse_sql_body(body_text, routine_qname, location)?;
            Ok((body, deps, false))
        }
        FunctionLanguage::PlPgSql => {
            parse_plpgsql_body(body_text, is_set_returning, routine_qname, location)
        }
    }
}

// ---------------------------------------------------------------------------
// PL/pgSQL path
// ---------------------------------------------------------------------------

fn parse_plpgsql_body(
    body_text: &str,
    is_set_returning: bool,
    routine_qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<(NormalizedBody, Vec<DepEdge>, bool), ParseError> {
    // Wrap the body in a synthetic CREATE FUNCTION so pg_query::parse_plpgsql
    // can parse it.  Use a dollar-quote tag unlikely to collide with body
    // content.
    //
    // The RETURNS clause matters: the pg_query plpgsql analyzer validates
    // RETURN QUERY / RETURN NEXT against the declared set-returning-ness of
    // the wrapper function.  Using `RETURNS void` for a SETOF/TABLE body
    // causes the analyzer to reject the body with "cannot use RETURN QUERY in
    // a non-SETOF function".  We therefore use `RETURNS SETOF record` when the
    // original function is set-returning.
    let returns_clause = if is_set_returning {
        "RETURNS SETOF record"
    } else {
        "RETURNS void"
    };
    let wrapper = format!(
        "CREATE FUNCTION pgevolve_temp() {returns_clause} LANGUAGE plpgsql \
         AS $pgevolve_outer${body_text}$pgevolve_outer$;"
    );
    let json = pg_query::parse_plpgsql(&wrapper).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("function {routine_qname}: PL/pgSQL parse error — {e}"),
    })?;

    let mut walker = PlpgsqlWalker {
        routine_qname: routine_qname.clone(),
        location: location.clone(),
        dependencies: Vec::new(),
        commits_in_body: false,
    };
    walker.walk_root(&json);

    // Scan body text for `-- @pgevolve dep:` directives.
    let directive_edges = scan_dep_directives(body_text, routine_qname, location)?;
    walker.dependencies.extend(directive_edges);

    // Stable dedup.
    walker.dependencies.sort();
    walker.dependencies.dedup();

    let canonical_text = canonicalize_plpgsql_text(body_text);
    let body = NormalizedBody::from_raw_canonical(canonical_text);
    Ok((body, walker.dependencies, walker.commits_in_body))
}

// ---------------------------------------------------------------------------
// SQL path
// ---------------------------------------------------------------------------

fn parse_sql_body(
    body_text: &str,
    routine_qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<(NormalizedBody, Vec<DepEdge>), ParseError> {
    let parsed = pg_query::parse(body_text).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("function {routine_qname}: SQL body parse error — {e}"),
    })?;

    let mut deps: Vec<DepEdge> = Vec::new();
    for stmt in &parsed.protobuf.stmts {
        if let Some(node) = stmt.stmt.as_ref().and_then(|n| n.node.as_ref()) {
            walk_sql_node_for_deps(node, routine_qname, &mut deps);
        }
    }

    deps.sort();
    deps.dedup();

    // Use pg_query parse → deparse to get a byte-stable canonical form. NOTE:
    // pg_query::normalize is the WRONG tool here — it replaces literal
    // constants with positional placeholders (`$1`, `$2`, …) for query-log
    // aggregation. When such a normalized body is later embedded inside a
    // CREATE FUNCTION definition, PG interprets `$1` as the first function
    // argument and the apply fails with `[42P02] there is no parameter $1`.
    // parse.deparse() round-trips through the AST without parameter
    // substitution, preserving the original literals.
    let canonical_text = parsed
        .deparse()
        .unwrap_or_else(|_| collapse_whitespace(body_text));
    let body = NormalizedBody::from_raw_canonical(canonical_text);
    Ok((body, deps))
}

// ---------------------------------------------------------------------------
// PL/pgSQL JSON walker
// ---------------------------------------------------------------------------

struct PlpgsqlWalker {
    routine_qname: QualifiedName,
    #[allow(dead_code)] // retained for future error-reporting (e.g., T11 lint sites)
    location: SourceLocation,
    dependencies: Vec<DepEdge>,
    commits_in_body: bool,
}

impl PlpgsqlWalker {
    fn walk_root(&mut self, json: &Value) {
        // pg_query::parse_plpgsql returns a JSON array, one element per
        // function/procedure body.
        if let Some(arr) = json.as_array() {
            for item in arr {
                if let Some(action) = item.get("PLpgSQL_function").and_then(|f| f.get("action")) {
                    self.walk(action);
                }
            }
        }
    }

    fn walk(&mut self, node: &Value) {
        match node {
            Value::Object(map) => {
                for (key, value) in map {
                    match key.as_str() {
                        // -------------------------------------------------- //
                        // Transaction control — set the flag regardless of
                        // nesting depth (inside IF, loops, etc.).
                        // -------------------------------------------------- //
                        "PLpgSQL_stmt_commit" | "PLpgSQL_stmt_rollback" => {
                            self.commits_in_body = true;
                        }

                        // -------------------------------------------------- //
                        // Static embedded SQL — re-parse and walk for deps.
                        // -------------------------------------------------- //
                        "PLpgSQL_stmt_execsql" => {
                            // pg_query emits sqlstmt as:
                            //   { "PLpgSQL_expr": { "query": "<sql text>" } }
                            if let Some(query) = value
                                .get("sqlstmt")
                                .and_then(|s| s.get("PLpgSQL_expr"))
                                .and_then(|e| e.get("query"))
                                .and_then(|q| q.as_str())
                            {
                                self.extract_embedded_sql_deps(query);
                            }
                        }

                        // Dynamic SQL (PLpgSQL_stmt_dynexecute, PLpgSQL_stmt_dynfors):
                        // Opaque to static analysis. The pl-pgsql-dynamic-sql lint
                        // (T11) checks body text for EXECUTE sites and requires at
                        // least one @pgevolve dep: directive. Fall through to default.
                        _ => {}
                    }
                    // Recurse into all values (handles IF, LOOP, CASE, etc.).
                    self.walk(value);
                }
            }
            Value::Array(arr) => {
                for v in arr {
                    self.walk(v);
                }
            }
            _ => {}
        }
    }

    fn extract_embedded_sql_deps(&mut self, sql: &str) {
        let Ok(parsed) = pg_query::parse(sql) else {
            return;
        };
        for stmt in &parsed.protobuf.stmts {
            if let Some(node) = stmt.stmt.as_ref().and_then(|n| n.node.as_ref()) {
                walk_sql_node_for_deps(node, &self.routine_qname, &mut self.dependencies);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SQL AST walker — relation-ref extraction
// ---------------------------------------------------------------------------

/// Walk a `pg_query::NodeEnum` tree for relation references (`RangeVar`) and
/// emit `DepEdge` entries for each schema-qualified reference found.
///
/// Mirrors the `walk_node` logic in `parse/ast_canon.rs` that extracts
/// `body_dependencies` for views, but without the `KnownObjects` catalog
/// check: function bodies may reference relations that do not yet exist in the
/// source catalog (e.g., catalog tables, external schemas). Validation is
/// deferred to the T6 AST resolution pass.
fn walk_sql_node_for_deps(
    node: &pg_query::NodeEnum,
    from_qname: &QualifiedName,
    deps: &mut Vec<DepEdge>,
) {
    use pg_query::NodeEnum as N;

    match node {
        // SELECT: walk FROM, WHERE, UNION branches, CTEs.
        N::SelectStmt(sel) => {
            for from in &sel.from_clause {
                if let Some(n) = from.node.as_ref() {
                    walk_sql_node_for_deps(n, from_qname, deps);
                }
            }
            if let Some(wc) = sel.where_clause.as_ref().and_then(|n| n.node.as_ref()) {
                walk_sql_node_for_deps(wc, from_qname, deps);
            }
            if let Some(larg) = &sel.larg {
                let node = pg_query::protobuf::Node {
                    node: Some(N::SelectStmt(Box::new(*larg.clone()))),
                };
                if let Some(n) = node.node.as_ref() {
                    walk_sql_node_for_deps(n, from_qname, deps);
                }
            }
            if let Some(rarg) = &sel.rarg {
                let node = pg_query::protobuf::Node {
                    node: Some(N::SelectStmt(Box::new(*rarg.clone()))),
                };
                if let Some(n) = node.node.as_ref() {
                    walk_sql_node_for_deps(n, from_qname, deps);
                }
            }
            if let Some(with) = &sel.with_clause {
                for cte in &with.ctes {
                    if let Some(n) = cte.node.as_ref() {
                        walk_sql_node_for_deps(n, from_qname, deps);
                    }
                }
            }
            // Target list (for expressions that contain subqueries etc.).
            for t in &sel.target_list {
                if let Some(n) = t.node.as_ref() {
                    walk_sql_node_for_deps(n, from_qname, deps);
                }
            }
        }

        // INSERT: walk the target relation and SELECT source.
        N::InsertStmt(ins) => {
            if let Some(rel) = ins.relation.as_ref() {
                emit_range_var_dep(rel, from_qname, deps);
            }
            if let Some(sel) = ins.select_stmt.as_ref().and_then(|n| n.node.as_ref()) {
                walk_sql_node_for_deps(sel, from_qname, deps);
            }
        }

        // UPDATE: walk target relation, FROM clause, WHERE.
        N::UpdateStmt(upd) => {
            if let Some(rel) = upd.relation.as_ref() {
                emit_range_var_dep(rel, from_qname, deps);
            }
            for f in &upd.from_clause {
                if let Some(n) = f.node.as_ref() {
                    walk_sql_node_for_deps(n, from_qname, deps);
                }
            }
            if let Some(wc) = upd.where_clause.as_ref().and_then(|n| n.node.as_ref()) {
                walk_sql_node_for_deps(wc, from_qname, deps);
            }
        }

        // DELETE: walk target relation and WHERE.
        N::DeleteStmt(del) => {
            if let Some(rel) = del.relation.as_ref() {
                emit_range_var_dep(rel, from_qname, deps);
            }
            if let Some(wc) = del.where_clause.as_ref().and_then(|n| n.node.as_ref()) {
                walk_sql_node_for_deps(wc, from_qname, deps);
            }
        }

        // Relation reference in FROM clause.
        N::RangeVar(rv) => {
            emit_range_var_dep(rv, from_qname, deps);
        }

        // JOIN: walk both sides.
        N::JoinExpr(j) => {
            if let Some(l) = j.larg.as_ref().and_then(|n| n.node.as_ref()) {
                walk_sql_node_for_deps(l, from_qname, deps);
            }
            if let Some(r) = j.rarg.as_ref().and_then(|n| n.node.as_ref()) {
                walk_sql_node_for_deps(r, from_qname, deps);
            }
        }

        // Subquery in FROM.
        N::RangeSubselect(sub) => {
            if let Some(q) = sub.subquery.as_ref().and_then(|n| n.node.as_ref()) {
                walk_sql_node_for_deps(q, from_qname, deps);
            }
        }

        // CTE.
        N::CommonTableExpr(cte) => {
            if let Some(q) = cte.ctequery.as_ref().and_then(|n| n.node.as_ref()) {
                walk_sql_node_for_deps(q, from_qname, deps);
            }
        }

        // ResTarget (SELECT target list element).
        N::ResTarget(rt) => {
            if let Some(val) = rt.val.as_ref().and_then(|n| n.node.as_ref()) {
                walk_sql_node_for_deps(val, from_qname, deps);
            }
        }

        // Other node kinds don't contain relation references at this level.
        _ => {}
    }
}

/// Emit a `DepEdge` for a schema-qualified `RangeVar`, if the name is
/// schema-qualified. Unqualified names are skipped (search-path resolution
/// is out of scope for static analysis).
fn emit_range_var_dep(
    rv: &pg_query::protobuf::RangeVar,
    from_qname: &QualifiedName,
    deps: &mut Vec<DepEdge>,
) {
    if rv.schemaname.is_empty() || rv.relname.is_empty() {
        return;
    }
    let Ok(schema) = Identifier::from_unquoted(&rv.schemaname)
        .or_else(|_| Identifier::from_quoted(&rv.schemaname))
    else {
        return;
    };
    let Ok(name) =
        Identifier::from_unquoted(&rv.relname).or_else(|_| Identifier::from_quoted(&rv.relname))
    else {
        return;
    };
    let ref_qname = QualifiedName::new(schema, name);
    deps.push(DepEdge {
        from: NodeId::Table(from_qname.clone()),
        to: NodeId::Table(ref_qname),
        source: DepSource::AstExtracted,
    });
}

// ---------------------------------------------------------------------------
// Directive scanner
// ---------------------------------------------------------------------------

fn scan_dep_directives(
    body_text: &str,
    function_qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<Vec<DepEdge>, ParseError> {
    let mut out = Vec::new();
    for line in body_text.lines() {
        // Find `-- @pgevolve dep:` anywhere on the line, not just at the
        // start. The canonicalizer may join non-comment prefix text onto the
        // same line as the comment (e.g., `DECLARE -- @pgevolve dep: app.x`),
        // so a line-start-only check would miss valid directives.
        let Some(comment_pos) = line.find("-- @pgevolve dep:") else {
            continue;
        };
        let rest = &line[comment_pos + "-- @pgevolve dep:".len()..];
        let qname_text = rest.trim();
        let Some((schema, name)) = qname_text.split_once('.') else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "function {function_qname}: directive `-- @pgevolve dep:` must be \
                     schema-qualified (got {qname_text:?})"
                ),
            });
        };
        let schema_id =
            Identifier::from_unquoted(schema.trim()).map_err(|e| ParseError::Structural {
                location: location.clone(),
                message: format!("function {function_qname}: invalid schema in dep directive: {e}"),
            })?;
        let name_id =
            Identifier::from_unquoted(name.trim()).map_err(|e| ParseError::Structural {
                location: location.clone(),
                message: format!("function {function_qname}: invalid name in dep directive: {e}"),
            })?;
        let target_qname = QualifiedName::new(schema_id, name_id);

        // Directive target is ambiguous between table/view/MV/type/function/procedure.
        // We record NodeId::Table as a placeholder; the T6 AST resolution pass
        // probes all catalog collections for the qname and treats the directive
        // as satisfied if any matches.
        out.push(DepEdge {
            from: NodeId::Table(function_qname.clone()),
            to: NodeId::Table(target_qname),
            source: DepSource::AstDeclared,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Text canonicalization helpers
// ---------------------------------------------------------------------------

fn canonicalize_plpgsql_text(text: &str) -> String {
    // PL/pgSQL is line-sensitive only around `--` line comments: those extend
    // to end of line, so a line containing `--` MUST be terminated by a
    // newline (otherwise the comment would swallow the next statement).
    // All other whitespace (including newlines on non-comment lines) is
    // semantically irrelevant and gets collapsed to a single space.
    //
    // This produces the same canonical text whether the input was multiline
    // (source SQL file) or single-line (pg_get_functiondef output), at the
    // cost of accepting that comment-bearing lines keep their newline.
    let lines: Vec<String> = text
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect();

    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            // Use newline as separator only when the PREVIOUS line ends in a
            // way where `--` comment scope demands it. We detect this by
            // checking if the previous line contains a `--` outside of a
            // string literal (cheap heuristic: just look for `--`).
            let prev_has_comment = contains_line_comment(&lines[i - 1]);
            out.push(if prev_has_comment { '\n' } else { ' ' });
        }
        out.push_str(line);
    }
    out
}

/// True if `line` contains a `--` SQL line comment outside of a string literal.
///
/// Cheap two-state scanner: track whether we're inside a single-quoted string,
/// flip on each `'` (PG-style escape `''` is naturally handled since the two
/// flips cancel). If we hit `--` outside a string, return true.
fn contains_line_comment(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut in_str = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                in_str = !in_str;
                i += 1;
            }
            b'-' if !in_str && i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                return true;
            }
            _ => i += 1,
        }
    }
    false
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(
            Identifier::from_unquoted(schema).unwrap(),
            Identifier::from_unquoted(name).unwrap(),
        )
    }

    #[test]
    fn detects_commit_in_plpgsql_body() {
        let body = "BEGIN INSERT INTO app.log VALUES (1); COMMIT; END";
        let (_body, _deps, commits) = parse_routine_body(
            body,
            FunctionLanguage::PlPgSql,
            false,
            &qn("app", "p"),
            &loc(),
        )
        .unwrap();
        assert!(commits, "COMMIT must set commits_in_body");
    }

    #[test]
    fn no_commit_in_plain_plpgsql_body() {
        let body = "BEGIN INSERT INTO app.log VALUES (1); END";
        let (_body, _deps, commits) = parse_routine_body(
            body,
            FunctionLanguage::PlPgSql,
            false,
            &qn("app", "p"),
            &loc(),
        )
        .unwrap();
        assert!(
            !commits,
            "no COMMIT/ROLLBACK → commits_in_body must be false"
        );
    }

    #[test]
    fn detects_rollback_in_plpgsql_body() {
        let body = "BEGIN IF false THEN ROLLBACK; END IF; END";
        let (_body, _deps, commits) = parse_routine_body(
            body,
            FunctionLanguage::PlPgSql,
            false,
            &qn("app", "p"),
            &loc(),
        )
        .unwrap();
        assert!(commits, "ROLLBACK must also set commits_in_body");
    }

    #[test]
    fn sql_body_no_commit_flag() {
        let body = "SELECT 1";
        let (_body, _deps, commits) =
            parse_routine_body(body, FunctionLanguage::Sql, false, &qn("app", "f"), &loc())
                .unwrap();
        assert!(!commits, "SQL bodies cannot set commits_in_body");
    }

    #[test]
    fn plpgsql_body_extracts_relation_dep() {
        // The INSERT references app.log — should produce an AstExtracted edge.
        let body = "BEGIN INSERT INTO app.log(msg) VALUES ('x'); END";
        let (_body, deps, _commits) = parse_routine_body(
            body,
            FunctionLanguage::PlPgSql,
            false,
            &qn("app", "p"),
            &loc(),
        )
        .unwrap();
        let has_edge = deps.iter().any(|e| {
            e.to == NodeId::Table(qn("app", "log")) && e.source == DepSource::AstExtracted
        });
        assert!(
            has_edge,
            "expected AstExtracted edge to app.log; got {deps:?}"
        );
    }

    #[test]
    fn sql_body_extracts_relation_dep() {
        let body = "SELECT * FROM app.users WHERE id = $1";
        let (_body, deps, _commits) =
            parse_routine_body(body, FunctionLanguage::Sql, false, &qn("app", "f"), &loc())
                .unwrap();
        let has_edge = deps.iter().any(|e| {
            e.to == NodeId::Table(qn("app", "users")) && e.source == DepSource::AstExtracted
        });
        assert!(
            has_edge,
            "expected AstExtracted edge to app.users; got {deps:?}"
        );
    }

    #[test]
    fn directive_adds_declared_dep_edge() {
        let body = "-- @pgevolve dep: app.summary\n\
                    BEGIN EXECUTE 'REFRESH MATERIALIZED VIEW app.summary'; END";
        let (_body, deps, _commits) = parse_routine_body(
            body,
            FunctionLanguage::PlPgSql,
            false,
            &qn("app", "f"),
            &loc(),
        )
        .unwrap();
        let has_declared = deps.iter().any(|e| {
            e.to == NodeId::Table(qn("app", "summary")) && e.source == DepSource::AstDeclared
        });
        assert!(
            has_declared,
            "expected AstDeclared edge to app.summary; got {deps:?}"
        );
    }

    #[test]
    fn unqualified_directive_rejected() {
        let body = "-- @pgevolve dep: nonsense\nBEGIN NULL; END";
        let err = parse_routine_body(
            body,
            FunctionLanguage::PlPgSql,
            false,
            &qn("app", "f"),
            &loc(),
        )
        .unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("schema-qualified"), "{msg}");
    }

    #[test]
    fn canonical_text_collapses_whitespace() {
        let body = "BEGIN\n  NULL;\nEND";
        let (body_val, _deps, _commits) = parse_routine_body(
            body,
            FunctionLanguage::PlPgSql,
            false,
            &qn("app", "f"),
            &loc(),
        )
        .unwrap();
        assert_eq!(body_val.canonical_text(), "BEGIN NULL; END");
    }

    #[test]
    fn return_query_accepted_when_set_returning() {
        let body = "BEGIN RETURN QUERY SELECT 1; END";
        let result = parse_routine_body(
            body,
            FunctionLanguage::PlPgSql,
            true,
            &qn("app", "f"),
            &loc(),
        );
        assert!(
            result.is_ok(),
            "RETURN QUERY must be accepted in a set-returning function; got {result:?}"
        );
    }

    #[test]
    fn return_query_rejected_when_not_set_returning() {
        let body = "BEGIN RETURN QUERY SELECT 1; END";
        let result = parse_routine_body(
            body,
            FunctionLanguage::PlPgSql,
            false,
            &qn("app", "f"),
            &loc(),
        );
        assert!(
            result.is_err(),
            "RETURN QUERY must be rejected in a non-SETOF function (mirrors Postgres)"
        );
    }

    #[test]
    fn return_next_accepted_when_set_returning() {
        let body = "BEGIN RETURN NEXT 5; END";
        let result = parse_routine_body(
            body,
            FunctionLanguage::PlPgSql,
            true,
            &qn("app", "f"),
            &loc(),
        );
        assert!(
            result.is_ok(),
            "RETURN NEXT must be accepted in a set-returning function; got {result:?}"
        );
    }
}
