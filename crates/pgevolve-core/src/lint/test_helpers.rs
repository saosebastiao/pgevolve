//! Shared `#[cfg(test)]` helpers used by per-rule lint tests.

#![cfg(test)]
#![allow(dead_code)]

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::lint::source_tree::SourceTree;

pub fn id(s: &str) -> Identifier {
    Identifier::from_unquoted(s).unwrap()
}

pub fn qn(s: &str, n: &str) -> QualifiedName {
    QualifiedName::new(id(s), id(n))
}

pub fn empty_tree(catalog: Catalog) -> SourceTree {
    SourceTree::new(catalog, std::collections::HashMap::new())
}

pub fn make_plpgsql_function(
    schema: &str,
    name: &str,
    body_text: &str,
    deps: Vec<crate::plan::edges::DepEdge>,
) -> crate::ir::function::Function {
    use crate::ir::function::{
        FunctionLanguage, NormalizedArgTypes, ParallelSafety, ReturnType, SecurityMode, Volatility,
    };
    use crate::parse::normalize_body::NormalizedBody;
    let args = vec![];
    let arg_types_normalized = NormalizedArgTypes::from_args(&args);
    crate::ir::function::Function {
        qname: qn(schema, name),
        args,
        arg_types_normalized,
        return_type: ReturnType::Void,
        language: FunctionLanguage::PlPgSql,
        // PL/pgSQL bodies can't be parsed by pg_query — use from_raw_canonical.
        body: NormalizedBody::from_raw_canonical(body_text.to_string()),
        body_dependencies: deps,
        volatility: Volatility::Volatile,
        strict: false,
        security: SecurityMode::Invoker,
        parallel: ParallelSafety::Unsafe,
        leakproof: false,
        cost: None,
        rows: None,
        comment: None,
        owner: None,
        grants: vec![],
    }
}

/// Build a zero-arg `NormalizedArgTypes` for use in test `NodeId::Function` variants.
pub fn empty_arg_types() -> crate::ir::function::NormalizedArgTypes {
    crate::ir::function::NormalizedArgTypes::from_args(&[])
}

pub fn make_procedure(
    schema: &str,
    name: &str,
    body_text: &str,
    commits_in_body: bool,
    deps: Vec<crate::plan::edges::DepEdge>,
) -> crate::ir::procedure::Procedure {
    use crate::ir::function::{FunctionLanguage, SecurityMode};
    use crate::parse::normalize_body::NormalizedBody;
    crate::ir::procedure::Procedure {
        qname: qn(schema, name),
        args: vec![],
        language: FunctionLanguage::PlPgSql,
        // PL/pgSQL bodies can't be parsed by pg_query — use from_raw_canonical.
        body: NormalizedBody::from_raw_canonical(body_text.to_string()),
        body_dependencies: deps,
        security: SecurityMode::Invoker,
        commits_in_body,
        comment: None,
        owner: None,
        grants: vec![],
    }
}

pub fn make_trigger(
    schema: &str,
    name: &str,
    table_schema: &str,
    table_name: &str,
    fn_schema: &str,
    fn_name: &str,
) -> crate::ir::trigger::Trigger {
    use crate::ir::constraint::Deferrable;
    use crate::ir::trigger::{TriggerEvent, TriggerLevel, TriggerTiming};
    crate::ir::trigger::Trigger {
        qname: qn(schema, name),
        table: qn(table_schema, table_name),
        timing: TriggerTiming::Before,
        events: vec![TriggerEvent::Insert],
        level: TriggerLevel::Row,
        when_clause: None,
        transition_tables: vec![],
        function_qname: qn(fn_schema, fn_name),
        function_args: vec![],
        is_constraint: false,
        deferrable: Deferrable::NotDeferrable,
        comment: None,
    }
}

pub fn make_function_bare(schema: &str, name: &str) -> crate::ir::function::Function {
    make_plpgsql_function(schema, name, "BEGIN NULL; END", vec![])
}
