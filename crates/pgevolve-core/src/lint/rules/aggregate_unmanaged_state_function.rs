//! `aggregate-references-unmanaged-function` lint rule.

use crate::identifier::QualifiedName;
use crate::ir::aggregate::Aggregate;
use crate::ir::catalog::Catalog;
use crate::ir::column_type::ColumnType;
use crate::ir::function::{ArgMode, Function};
use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `aggregate-references-unmanaged-function` — fires (Error) when an
/// aggregate's `SFUNC` (or `FINALFUNC`) does not resolve to a managed
/// (SQL/plpgsql) function in the source catalog.
///
/// pgevolve can only manage the aggregate → function dependency when the
/// referenced function is itself a managed source object. A reference to a
/// built-in / C / internal function (which the parser never populates into
/// `catalog.functions`) leaves the dependency unmanageable, so the source
/// aggregate is rejected.
///
/// The implied signature of `SFUNC` is `(state_type, arg_types…)`; the implied
/// signature of `FINALFUNC` is `(state_type)`. We resolve the overload with the
/// same matching logic as the dependency-graph's `find_sfunc` (qname + implied
/// arity/types) so the lint and the edge agree on which function is referenced.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for agg in &tree.catalog.aggregates {
        // sfunc implied signature: (state_type, arg_types…)
        let mut sfunc_sig = Vec::with_capacity(1 + agg.arg_types.len());
        sfunc_sig.push(agg.state_type.clone());
        sfunc_sig.extend(agg.arg_types.iter().cloned());
        if find_managed_function(&tree.catalog, &agg.sfunc, &sfunc_sig).is_none() {
            out.push(unmanaged_finding(agg, &agg.sfunc));
        }

        // finalfunc implied signature: (state_type)
        if let Some(finalfunc) = &agg.finalfunc
            && find_managed_function(
                &tree.catalog,
                finalfunc,
                std::slice::from_ref(&agg.state_type),
            )
            .is_none()
        {
            out.push(unmanaged_finding(agg, finalfunc));
        }
    }

    out
}

/// Build the Error finding for an aggregate referencing an unmanaged function.
fn unmanaged_finding(agg: &Aggregate, func: &QualifiedName) -> Finding {
    let argtypes = agg
        .arg_types
        .iter()
        .map(ColumnType::render_sql)
        .collect::<Vec<_>>()
        .join(", ");
    Finding::error(
        "aggregate-references-unmanaged-function",
        format!(
            "aggregate `{qname}({argtypes})` references function `{func}` which is not a managed \
             (SQL/plpgsql) function — v0.4.1 requires managed state/final functions",
            qname = agg.qname,
        ),
    )
}

/// Resolve a managed function overload by qname + implied signature, mirroring
/// the dependency-graph's `find_sfunc` logic so the check and the edge agree.
fn find_managed_function<'a>(
    catalog: &'a Catalog,
    fn_qname: &QualifiedName,
    implied_arg_types: &[ColumnType],
) -> Option<&'a Function> {
    let candidates: Vec<&Function> = catalog
        .functions
        .iter()
        .filter(|f| &f.qname == fn_qname)
        .collect();
    match candidates.as_slice() {
        [] => None,
        [only] => Some(only),
        many => {
            let by_arity: Vec<&Function> = many
                .iter()
                .copied()
                .filter(|f| positional_arg_types(f).len() == implied_arg_types.len())
                .collect();
            match by_arity.as_slice() {
                [] => None,
                [only] => Some(only),
                still_many => still_many.iter().copied().find(|f| {
                    positional_arg_types(f)
                        .into_iter()
                        .eq(implied_arg_types.iter())
                }),
            }
        }
    }
}

/// Positional (IN/INOUT/VARIADIC) argument types of `f`, in declaration order —
/// the call signature, matching how `find_sfunc` resolves overloads.
fn positional_arg_types(f: &Function) -> Vec<&ColumnType> {
    f.args
        .iter()
        .filter(|a| matches!(a.mode, ArgMode::In | ArgMode::InOut | ArgMode::Variadic))
        .map(|a| &a.ty)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::aggregate::Aggregate;
    use crate::ir::function::{ArgMode, FunctionArg, NormalizedArgTypes};
    use crate::ir::schema::Schema;
    use crate::lint::test_helpers::{empty_tree, id, make_plpgsql_function, qn};

    /// Build a managed plpgsql function with the given positional arg types.
    fn managed_fn(schema: &str, name: &str, arg_tys: &[ColumnType]) -> Function {
        let mut f = make_plpgsql_function(schema, name, "BEGIN NULL; END", vec![]);
        let args: Vec<FunctionArg> = arg_tys
            .iter()
            .map(|ty| FunctionArg {
                name: None,
                mode: ArgMode::In,
                ty: ty.clone(),
                default: None,
            })
            .collect();
        f.arg_types_normalized = NormalizedArgTypes::from_args(&args);
        f.args = args;
        f
    }

    fn sum_aggregate(sfunc: QualifiedName, finalfunc: Option<QualifiedName>) -> Aggregate {
        Aggregate {
            qname: qn("app", "my_sum"),
            arg_types: vec![ColumnType::Integer],
            state_type: ColumnType::BigInt,
            sfunc,
            finalfunc,
            initcond: None,
            owner: None,
            comment: None,
        }
    }

    fn count_findings(findings: &[Finding]) -> usize {
        findings
            .iter()
            .filter(|f| f.rule == "aggregate-references-unmanaged-function")
            .count()
    }

    #[test]
    fn managed_sfunc_matching_signature_ok() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // sfunc implied signature: (state_type=bigint, arg_types=integer)
        c.functions.push(managed_fn(
            "app",
            "my_sum_sfunc",
            &[ColumnType::BigInt, ColumnType::Integer],
        ));
        c.aggregates
            .push(sum_aggregate(qn("app", "my_sum_sfunc"), None));
        let tree = empty_tree(c);
        assert_eq!(count_findings(&check(&tree)), 0);
    }

    #[test]
    fn missing_sfunc_qname_fires() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // No function declared at all.
        c.aggregates
            .push(sum_aggregate(qn("pg_catalog", "int8_avg_accum"), None));
        let tree = empty_tree(c);
        let findings = check(&tree);
        assert_eq!(count_findings(&findings), 1);
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "aggregate-references-unmanaged-function")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn sfunc_qname_present_wrong_arity_fires() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // Function exists by qname but with the WRONG signature: a single
        // overload that takes no args — implied sig is (bigint, integer).
        c.functions.push(managed_fn("app", "my_sum_sfunc", &[]));
        // Add a second overload with the same qname so the single-candidate
        // fast path doesn't accept it; both have the wrong arity.
        c.functions
            .push(managed_fn("app", "my_sum_sfunc", &[ColumnType::Text]));
        c.aggregates
            .push(sum_aggregate(qn("app", "my_sum_sfunc"), None));
        let tree = empty_tree(c);
        assert_eq!(count_findings(&check(&tree)), 1);
    }

    #[test]
    fn unmanaged_finalfunc_fires() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // sfunc is managed and matches; finalfunc is not declared.
        c.functions.push(managed_fn(
            "app",
            "my_sum_sfunc",
            &[ColumnType::BigInt, ColumnType::Integer],
        ));
        c.aggregates.push(sum_aggregate(
            qn("app", "my_sum_sfunc"),
            Some(qn("pg_catalog", "int8_avg_final")),
        ));
        let tree = empty_tree(c);
        let findings = check(&tree);
        assert_eq!(count_findings(&findings), 1);
        // The finding names the finalfunc, not the sfunc.
        assert!(
            findings
                .iter()
                .any(|f| f.message.contains("int8_avg_final")),
        );
    }
}
