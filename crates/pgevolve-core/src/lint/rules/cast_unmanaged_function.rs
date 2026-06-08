//! `cast-references-unmanaged-function` lint rule.

use crate::identifier::QualifiedName;
use crate::ir::cast::{Cast, CastMethod};
use crate::ir::catalog::Catalog;
use crate::ir::column_type::ColumnType;
use crate::ir::function::{ArgMode, Function};
use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `cast-references-unmanaged-function` — fires (Error) when a
/// `CREATE CAST ... WITH FUNCTION` references a function that is not a
/// managed (SQL/plpgsql) function in the source catalog.
///
/// pgevolve can only manage the cast → function dependency when the
/// referenced function is itself a managed source object. A reference to a
/// built-in / C / internal function (which the parser never populates into
/// `catalog.functions`) leaves the dependency unmanageable, so the source
/// cast is rejected.
///
/// The conversion-function signature is matched by qname + arg types
/// (the explicit arg types stored on the `CastMethod::Function` variant).
/// `CastMethod::Inout` and `CastMethod::Binary` casts require no function
/// and are always accepted.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for cast in &tree.catalog.casts {
        if let CastMethod::Function { name, arg_types } = &cast.method
            && find_managed_function(&tree.catalog, name, arg_types).is_none()
        {
            out.push(unmanaged_finding(cast, name));
        }
    }

    out
}

/// Build the Error finding for a cast referencing an unmanaged function.
fn unmanaged_finding(cast: &Cast, func: &QualifiedName) -> Finding {
    Finding::error(
        "cast-references-unmanaged-function",
        format!(
            "cast `({source} AS {target})` references function `{func}` which is not a managed \
             (SQL/plpgsql) function — pgevolve requires managed cast functions",
            source = cast.source,
            target = cast.target,
        ),
    )
}

/// Resolve a managed function overload by qname + arg types, mirroring the
/// aggregate rule's `find_managed_function` matcher (consistent with the
/// existing aggregate/edges duplication pattern).
fn find_managed_function<'a>(
    catalog: &'a Catalog,
    fn_qname: &QualifiedName,
    arg_types: &[ColumnType],
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
                .filter(|f| positional_arg_types(f).len() == arg_types.len())
                .collect();
            match by_arity.as_slice() {
                [] => None,
                [only] => Some(only),
                still_many => still_many
                    .iter()
                    .copied()
                    .find(|f| positional_arg_types(f).into_iter().eq(arg_types.iter())),
            }
        }
    }
}

/// Positional (IN/INOUT/VARIADIC) argument types of `f`, in declaration order —
/// the call signature, mirroring the aggregate rule's `positional_arg_types`.
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
    use crate::ir::cast::{CastContext, CastMethod};
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

    fn cast_with_function(fn_schema: &str, fn_name: &str, arg_tys: Vec<ColumnType>) -> Cast {
        Cast {
            source: qn("app", "my_type"),
            target: qn("pg_catalog", "text"),
            method: CastMethod::Function {
                name: qn(fn_schema, fn_name),
                arg_types: arg_tys,
            },
            context: CastContext::Explicit,
            comment: None,
        }
    }

    fn count_findings(findings: &[Finding]) -> usize {
        findings
            .iter()
            .filter(|f| f.rule == "cast-references-unmanaged-function")
            .count()
    }

    #[test]
    fn managed_function_matching_signature_ok() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // Cast WITH FUNCTION referencing a managed function with the same arg types.
        c.functions
            .push(managed_fn("app", "my_type_to_text", &[ColumnType::Integer]));
        c.casts.push(cast_with_function(
            "app",
            "my_type_to_text",
            vec![ColumnType::Integer],
        ));
        let tree = empty_tree(c);
        assert_eq!(count_findings(&check(&tree)), 0);
    }

    #[test]
    fn missing_function_fires() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // No function declared at all — reference to an unmanaged function.
        c.casts.push(cast_with_function(
            "pg_catalog",
            "some_builtin_cast_fn",
            vec![ColumnType::Integer],
        ));
        let tree = empty_tree(c);
        let findings = check(&tree);
        assert_eq!(count_findings(&findings), 1);
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "cast-references-unmanaged-function")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn function_qname_present_wrong_arity_fires() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // Function exists by qname but with the WRONG arity — two overloads to
        // bypass the single-candidate fast path, both with the wrong arg count.
        // The cast expects arity 2 (Integer, Integer); neither overload matches.
        c.functions.push(managed_fn("app", "my_type_to_text", &[]));
        c.functions
            .push(managed_fn("app", "my_type_to_text", &[ColumnType::Text]));
        // Cast expects arg_types = [Integer, Integer] (arity 2).
        c.casts.push(cast_with_function(
            "app",
            "my_type_to_text",
            vec![ColumnType::Integer, ColumnType::Integer],
        ));
        let tree = empty_tree(c);
        assert_eq!(count_findings(&check(&tree)), 1);
    }

    #[test]
    fn without_function_cast_no_finding() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // WITHOUT FUNCTION (Binary) — no function reference; must not fire.
        c.casts.push(Cast {
            source: qn("app", "type_x"),
            target: qn("app", "type_y"),
            method: CastMethod::Binary,
            context: CastContext::Implicit,
            comment: None,
        });
        let tree = empty_tree(c);
        assert_eq!(count_findings(&check(&tree)), 0);
    }

    #[test]
    fn with_inout_cast_no_finding() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // WITH INOUT — uses type I/O functions; no managed function required.
        c.casts.push(Cast {
            source: qn("app", "domain_a"),
            target: qn("app", "domain_b"),
            method: CastMethod::Inout,
            context: CastContext::Assignment,
            comment: None,
        });
        let tree = empty_tree(c);
        assert_eq!(count_findings(&check(&tree)), 0);
    }
}
