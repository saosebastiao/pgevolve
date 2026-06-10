//! Diff functions and procedures.
//!
//! [`diff_functions`] compares two slices of [`Function`] — one from the live
//! catalog (`catalog`) and one from the declared source (`source`) — and
//! populates a [`ChangeSet`] with the minimal sequence of
//! [`FunctionChange`] variants required to
//! converge the catalog toward the source.
//!
//! [`diff_procedures`] does the same for [`Procedure`] /
//! [`ProcedureChange`].
//!
//! ## Identity keys
//!
//! Functions are keyed by `(qname, arg_types_normalized.canonical_hash)` to
//! correctly handle overloads. Procedures are keyed by `qname` alone (PG does
//! not overload procedures on argument types the same way functions are
//! overloaded for resolution purposes — but the catalog reader only reads one
//! procedure per qname in v0.2).

use std::collections::{BTreeMap, BTreeSet};

use crate::diff::change::{Change, FunctionChange, ProcedureChange};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::diff::owner_grants::{ColumnGrantMode, diff_owner_and_grants};
use crate::diff::owner_op::{CatalogObjectRef, RoutineSignature};
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::function::{ArgMode, Function, NormalizedArgTypes, ReturnType};
use crate::ir::procedure::Procedure;

/// Compute `Function`-level changes needed to converge `catalog` toward `source`.
///
/// Functions are paired by `(qname, arg_types_normalized.canonical_hash)` so
/// that overloads with different input-argument signatures are treated as
/// independent objects.
pub fn diff_functions(
    catalog: &[Function],
    source: &[Function],
    out: &mut ChangeSet,
    managed_roles: &BTreeSet<Identifier>,
) {
    // Key: (qname, arg-type identity hash).  Use a tuple so BTreeMap ordering
    // is deterministic across runs regardless of insertion order.
    type Key = (QualifiedName, [u8; 32]);

    let cat: BTreeMap<Key, &Function> = catalog
        .iter()
        .map(|f| ((f.qname.clone(), f.arg_types_normalized.canonical_hash), f))
        .collect();
    let src: BTreeMap<Key, &Function> = source
        .iter()
        .map(|f| ((f.qname.clone(), f.arg_types_normalized.canonical_hash), f))
        .collect();

    let all_keys: BTreeSet<Key> = cat.keys().chain(src.keys()).cloned().collect();

    for key in all_keys {
        match (cat.get(&key), src.get(&key)) {
            (None, Some(s)) => out.push(
                Change::Function(FunctionChange::Create((*s).clone())),
                Destructiveness::Safe,
            ),
            (Some(c), None) => out.push(
                Change::Function(FunctionChange::Drop {
                    qname: c.qname.clone(),
                    args: c.arg_types_normalized.clone(),
                }),
                Destructiveness::RequiresApprovalAndDataLossWarning {
                    reason: format!("drops function {}", c.qname),
                },
            ),
            (Some(c), Some(s)) => {
                diff_same_function(c, s, out);
                diff_function_owner_grants(c, s, out, managed_roles);
            }
            (None, None) => unreachable!(),
        }
    }
}

/// Diff owner and grants for a function pair that shares the same identity key.
fn diff_function_owner_grants(
    catalog: &Function,
    source: &Function,
    out: &mut ChangeSet,
    managed_roles: &BTreeSet<Identifier>,
) {
    // Build a human-readable label for the routine (for observations).
    let args_label = NormalizedArgTypes::from_args(&source.args)
        .types
        .iter()
        .map(crate::ir::column_type::ColumnType::render_sql)
        .collect::<Vec<_>>()
        .join(", ");
    let signature = format!("({args_label})");

    diff_owner_and_grants(
        &CatalogObjectRef::Function {
            name: source.qname.clone(),
            signature: RoutineSignature::new(signature),
        },
        catalog.owner.as_ref(),
        source.owner.as_ref(),
        &catalog.grants,
        &source.grants,
        managed_roles,
        ColumnGrantMode::ObjectOnly,
        out,
    );
}

/// Diff two function overloads that share the same identity key.
fn diff_same_function(catalog: &Function, source: &Function, out: &mut ChangeSet) {
    let body_changed = catalog.body.canonical_hash() != source.body.canonical_hash();
    let attrs_changed = catalog.return_type != source.return_type
        || catalog.language != source.language
        || catalog.volatility != source.volatility
        || catalog.strict != source.strict
        || catalog.security != source.security
        || catalog.parallel != source.parallel
        || catalog.leakproof != source.leakproof
        || catalog.cost.map(f32::to_bits) != source.cost.map(f32::to_bits)
        || catalog.rows.map(f32::to_bits) != source.rows.map(f32::to_bits)
        || catalog.args != source.args;

    if !body_changed && !attrs_changed {
        // Only emit a SetComment if the comment changed.
        if catalog.comment != source.comment {
            out.push(
                Change::Function(FunctionChange::SetComment {
                    qname: source.qname.clone(),
                    args: source.arg_types_normalized.clone(),
                    comment: source.comment.clone(),
                }),
                Destructiveness::Safe,
            );
        }
        return;
    }

    if function_can_or_replace(catalog, source) {
        let dest = if arg_default_removed(catalog, source) {
            Destructiveness::RequiresApproval {
                reason: format!(
                    "function {} removes an argument default (may break callers passing fewer args)",
                    source.qname
                ),
            }
        } else {
            Destructiveness::Safe
        };
        out.push(
            Change::Function(FunctionChange::CreateOrReplace(source.clone())),
            dest,
        );
    } else {
        out.push(
            Change::Function(FunctionChange::ReplaceViaDropCreate {
                catalog: catalog.clone(),
                source: source.clone(),
            }),
            Destructiveness::RequiresApprovalAndDataLossWarning {
                reason: format!(
                    "function {} return-type or language change requires DROP+CREATE CASCADE",
                    source.qname
                ),
            },
        );
    }
}

/// Return `true` when PG's `CREATE OR REPLACE FUNCTION` can be used to update
/// this function in-place.
///
/// PG rejects `CREATE OR REPLACE FUNCTION` when:
/// - The **language** changes (e.g., `sql` → `plpgsql`).
/// - The **return type kind or inner type** changes (e.g., scalar ↔ setof ↔
///   table ↔ trigger; or the underlying scalar type changes).
/// - The **OUT / INOUT parameter count or names** change (because they are
///   part of the effective return type).
pub(crate) fn function_can_or_replace(catalog: &Function, source: &Function) -> bool {
    if catalog.language != source.language {
        return false;
    }
    if !return_type_compatible(&catalog.return_type, &source.return_type) {
        return false;
    }
    // OUT / INOUT params form part of the implicit return type.
    let cat_outs: Vec<_> = catalog
        .args
        .iter()
        .filter(|a| matches!(a.mode, ArgMode::Out | ArgMode::InOut))
        .map(|a| (a.name.clone(), a.ty.clone()))
        .collect();
    let src_outs: Vec<_> = source
        .args
        .iter()
        .filter(|a| matches!(a.mode, ArgMode::Out | ArgMode::InOut))
        .map(|a| (a.name.clone(), a.ty.clone()))
        .collect();
    if cat_outs != src_outs {
        return false;
    }
    true
}

/// Two return types are compatible for `CREATE OR REPLACE FUNCTION` iff they
/// are exactly equal.
///
/// For v0.2 this is a strict equality check: any change to the return type
/// (kind or inner type) forces a `DROP + CREATE CASCADE` path. A future
/// version could allow appending columns to a `RETURNS TABLE` return type
/// (analogous to view-body compatibility) but that is out of scope here.
fn return_type_compatible(a: &ReturnType, b: &ReturnType) -> bool {
    a == b
}

/// Return `true` if any IN argument had a default in `catalog` that is absent
/// in `source`. Removing a default may break callers that relied on positional
/// omission and is therefore classified as `RequiresApproval`.
fn arg_default_removed(catalog: &Function, source: &Function) -> bool {
    catalog
        .args
        .iter()
        .zip(source.args.iter())
        .any(|(c, s)| c.default.is_some() && s.default.is_none())
}

/// Compute `Procedure`-level changes needed to converge `catalog` toward `source`.
///
/// Procedures are paired by `qname` only (v0.2 does not support procedure
/// overloads).
pub fn diff_procedures(
    catalog: &[Procedure],
    source: &[Procedure],
    out: &mut ChangeSet,
    managed_roles: &BTreeSet<Identifier>,
) {
    let cat: BTreeMap<QualifiedName, &Procedure> =
        catalog.iter().map(|p| (p.qname.clone(), p)).collect();
    let src: BTreeMap<QualifiedName, &Procedure> =
        source.iter().map(|p| (p.qname.clone(), p)).collect();
    let all: BTreeSet<QualifiedName> = cat.keys().chain(src.keys()).cloned().collect();

    for key in all {
        match (cat.get(&key), src.get(&key)) {
            (None, Some(s)) => out.push(
                Change::Procedure(ProcedureChange::Create((*s).clone())),
                Destructiveness::Safe,
            ),
            (Some(c), None) => out.push(
                Change::Procedure(ProcedureChange::Drop(c.qname.clone())),
                Destructiveness::RequiresApprovalAndDataLossWarning {
                    reason: format!("drops procedure {}", c.qname),
                },
            ),
            (Some(c), Some(s)) => {
                diff_same_procedure(c, s, out);
                diff_procedure_owner_grants(c, s, out, managed_roles);
            }
            (None, None) => unreachable!(),
        }
    }
}

/// Diff owner and grants for a procedure pair.
fn diff_procedure_owner_grants(
    catalog: &Procedure,
    source: &Procedure,
    out: &mut ChangeSet,
    managed_roles: &BTreeSet<Identifier>,
) {
    // Build the argument signature for SQL rendering (procedures use the same
    // IN/INOUT/VARIADIC identity subset as functions).
    let args_label = NormalizedArgTypes::from_args(&source.args)
        .types
        .iter()
        .map(crate::ir::column_type::ColumnType::render_sql)
        .collect::<Vec<_>>()
        .join(", ");
    let signature = format!("({args_label})");

    diff_owner_and_grants(
        &CatalogObjectRef::Procedure {
            name: source.qname.clone(),
            signature: RoutineSignature::new(signature),
        },
        catalog.owner.as_ref(),
        source.owner.as_ref(),
        &catalog.grants,
        &source.grants,
        managed_roles,
        ColumnGrantMode::ObjectOnly,
        out,
    );
}

/// Diff two procedures that share the same qualified name.
fn diff_same_procedure(catalog: &Procedure, source: &Procedure, out: &mut ChangeSet) {
    let body_changed = catalog.body.canonical_hash() != source.body.canonical_hash();
    let attrs_changed = catalog.language != source.language
        || catalog.security != source.security
        || catalog.args != source.args
        || catalog.commits_in_body != source.commits_in_body;

    if !body_changed && !attrs_changed {
        if catalog.comment != source.comment {
            out.push(
                Change::Procedure(ProcedureChange::SetComment {
                    qname: source.qname.clone(),
                    comment: source.comment.clone(),
                }),
                Destructiveness::Safe,
            );
        }
        return;
    }

    // Procedures always support CREATE OR REPLACE PROCEDURE in PG 11+.
    out.push(
        Change::Procedure(ProcedureChange::CreateOrReplace(source.clone())),
        Destructiveness::Safe,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use crate::diff::change::{Change, FunctionChange, ProcedureChange};
    use crate::diff::changeset::ChangeSet;
    use crate::diff::destructiveness::Destructiveness;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::column_type::ColumnType;
    use crate::ir::function::{
        ArgMode, FunctionArg, FunctionLanguage, NormalizedArgTypes, ParallelSafety, ReturnType,
        SecurityMode, Volatility,
    };
    use crate::ir::procedure::Procedure;
    use crate::parse::normalize_body::NormalizedBody;

    fn ident(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(ident(schema), ident(name))
    }

    fn make_func_no_args(qname: QualifiedName, body: &str) -> Function {
        let args: Vec<FunctionArg> = vec![];
        let arg_types_normalized = NormalizedArgTypes::from_args(&args);
        Function {
            qname,
            args,
            arg_types_normalized,
            return_type: ReturnType::Scalar {
                ty: ColumnType::Integer,
            },
            language: FunctionLanguage::Sql,
            body: NormalizedBody::from_sql(body).unwrap(),
            body_dependencies: vec![],
            volatility: Volatility::Immutable,
            strict: false,
            security: SecurityMode::Invoker,
            parallel: ParallelSafety::Safe,
            leakproof: false,
            cost: None,
            rows: None,
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn make_func_with_arg(
        qname: QualifiedName,
        arg_ty: ColumnType,
        ret_ty: ColumnType,
    ) -> Function {
        let args = vec![FunctionArg {
            name: Some(ident("x")),
            mode: ArgMode::In,
            ty: arg_ty,
            default: None,
        }];
        let arg_types_normalized = NormalizedArgTypes::from_args(&args);
        Function {
            qname,
            args,
            arg_types_normalized,
            return_type: ReturnType::Scalar { ty: ret_ty },
            language: FunctionLanguage::Sql,
            body: NormalizedBody::from_sql("SELECT $1").unwrap(),
            body_dependencies: vec![],
            volatility: Volatility::Immutable,
            strict: false,
            security: SecurityMode::Invoker,
            parallel: ParallelSafety::Safe,
            leakproof: false,
            cost: None,
            rows: None,
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn make_procedure(qname: QualifiedName, body_text: &str) -> Procedure {
        // Use from_raw_canonical so tests can provide distinguishable bodies
        // without depending on a PL/pgSQL parser.
        let body = NormalizedBody::from_raw_canonical(body_text.to_string());
        Procedure {
            qname,
            args: vec![],
            language: FunctionLanguage::PlPgSql,
            body,
            body_dependencies: vec![],
            security: SecurityMode::Invoker,
            commits_in_body: false,
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    // ── Function tests ────────────────────────────────────────────────────────

    #[test]
    fn function_create_emits_create() {
        let f = make_func_no_args(qn("app", "add_one"), "SELECT 1");
        let mut cs = ChangeSet::new();
        diff_functions(&[], std::slice::from_ref(&f), &mut cs, &BTreeSet::new());
        assert_eq!(cs.len(), 1);
        assert!(matches!(
            &cs.entries[0].change,
            Change::Function(FunctionChange::Create(ff)) if ff.qname == f.qname
        ));
        assert!(matches!(
            cs.entries[0].destructiveness,
            Destructiveness::Safe
        ));
    }

    #[test]
    fn function_drop_is_data_loss() {
        let f = make_func_no_args(qn("app", "old_fn"), "SELECT 99");
        let mut cs = ChangeSet::new();
        diff_functions(std::slice::from_ref(&f), &[], &mut cs, &BTreeSet::new());
        assert_eq!(cs.len(), 1);
        assert!(matches!(
            &cs.entries[0].change,
            Change::Function(FunctionChange::Drop { qname, .. }) if *qname == f.qname
        ));
        assert!(matches!(
            cs.entries[0].destructiveness,
            Destructiveness::RequiresApprovalAndDataLossWarning { .. }
        ));
    }

    #[test]
    fn function_body_change_emits_create_or_replace() {
        let f_cat = make_func_no_args(qn("app", "fn1"), "SELECT 1");
        let f_src = make_func_no_args(qn("app", "fn1"), "SELECT 2");
        let mut cs = ChangeSet::new();
        diff_functions(
            std::slice::from_ref(&f_cat),
            std::slice::from_ref(&f_src),
            &mut cs,
            &BTreeSet::new(),
        );
        assert_eq!(cs.len(), 1);
        assert!(matches!(
            &cs.entries[0].change,
            Change::Function(FunctionChange::CreateOrReplace(ff)) if ff.qname == f_src.qname
        ));
        assert!(matches!(
            cs.entries[0].destructiveness,
            Destructiveness::Safe
        ));
    }

    #[test]
    fn function_return_type_kind_change_emits_cascade() {
        let mut f_cat = make_func_no_args(qn("app", "fn2"), "SELECT 1");
        f_cat.return_type = ReturnType::Scalar {
            ty: ColumnType::Integer,
        };
        let mut f_src = make_func_no_args(qn("app", "fn2"), "SELECT 1");
        f_src.return_type = ReturnType::SetOf {
            ty: ColumnType::Integer,
        };
        let mut cs = ChangeSet::new();
        diff_functions(
            std::slice::from_ref(&f_cat),
            std::slice::from_ref(&f_src),
            &mut cs,
            &BTreeSet::new(),
        );
        assert_eq!(cs.len(), 1);
        assert!(matches!(
            &cs.entries[0].change,
            Change::Function(FunctionChange::ReplaceViaDropCreate { source, .. }) if source.qname == f_src.qname
        ));
        assert!(matches!(
            cs.entries[0].destructiveness,
            Destructiveness::RequiresApprovalAndDataLossWarning { .. }
        ));
    }

    #[test]
    fn function_overloads_treated_independently() {
        // Two overloads of the same name but different arg types.
        let f_int = make_func_with_arg(
            qn("app", "process"),
            ColumnType::Integer,
            ColumnType::Integer,
        );
        let f_txt = make_func_with_arg(qn("app", "process"), ColumnType::Text, ColumnType::Text);

        // catalog has only the int overload; source has both.
        let src = [f_int.clone(), f_txt];
        let mut cs = ChangeSet::new();
        diff_functions(
            std::slice::from_ref(&f_int),
            &src,
            &mut cs,
            &BTreeSet::new(),
        );

        // Only the text overload should be emitted as a Create.
        assert_eq!(cs.len(), 1);
        assert!(matches!(
            &cs.entries[0].change,
            Change::Function(FunctionChange::Create(ff)) if ff.arg_types_normalized.types == [ColumnType::Text]
        ));
    }

    #[test]
    fn function_comment_only_change_is_safe() {
        let f_cat = make_func_no_args(qn("app", "fn3"), "SELECT 1");
        let mut f_src = make_func_no_args(qn("app", "fn3"), "SELECT 1");
        f_src.comment = Some("new comment".to_string());
        let mut cs = ChangeSet::new();
        diff_functions(
            std::slice::from_ref(&f_cat),
            std::slice::from_ref(&f_src),
            &mut cs,
            &BTreeSet::new(),
        );
        assert_eq!(cs.len(), 1);
        assert!(matches!(
            &cs.entries[0].change,
            Change::Function(FunctionChange::SetComment { .. })
        ));
        assert!(matches!(
            cs.entries[0].destructiveness,
            Destructiveness::Safe
        ));
    }

    #[test]
    fn function_arg_default_removed_requires_approval() {
        use crate::ir::default_expr::NormalizedExpr;

        let make_with_default = |has_default: bool| {
            let default = if has_default {
                Some(NormalizedExpr::from_text("42"))
            } else {
                None
            };
            let args = vec![FunctionArg {
                name: Some(ident("x")),
                mode: ArgMode::In,
                ty: ColumnType::Integer,
                default,
            }];
            let norm = NormalizedArgTypes::from_args(&args);
            Function {
                qname: qn("app", "fn_default"),
                args,
                arg_types_normalized: norm,
                return_type: ReturnType::Scalar {
                    ty: ColumnType::Integer,
                },
                language: FunctionLanguage::Sql,
                body: NormalizedBody::from_sql("SELECT $1").unwrap(),
                body_dependencies: vec![],
                volatility: Volatility::Immutable,
                strict: false,
                security: SecurityMode::Invoker,
                parallel: ParallelSafety::Safe,
                leakproof: false,
                cost: None,
                rows: None,
                comment: None,
                owner: None,
                grants: vec![],
            }
        };

        let f_cat = make_with_default(true);
        let f_src = make_with_default(false);
        let mut cs = ChangeSet::new();
        // Body is the same, but the default was removed — attrs_changed=true because args differ.
        diff_functions(
            std::slice::from_ref(&f_cat),
            std::slice::from_ref(&f_src),
            &mut cs,
            &BTreeSet::new(),
        );
        assert_eq!(cs.len(), 1);
        assert!(matches!(
            &cs.entries[0].change,
            Change::Function(FunctionChange::CreateOrReplace(_))
        ));
        assert!(matches!(
            cs.entries[0].destructiveness,
            Destructiveness::RequiresApproval { .. }
        ));
    }

    #[test]
    fn function_identical_emits_nothing() {
        let f = make_func_no_args(qn("app", "unchanged"), "SELECT 42");
        let mut cs = ChangeSet::new();
        diff_functions(
            std::slice::from_ref(&f),
            std::slice::from_ref(&f),
            &mut cs,
            &BTreeSet::new(),
        );
        assert!(cs.is_empty());
    }

    // ── Procedure tests ───────────────────────────────────────────────────────

    #[test]
    fn procedure_create_emits_create() {
        let p = make_procedure(qn("app", "do_work"), "BEGIN NULL; END");
        let mut cs = ChangeSet::new();
        diff_procedures(&[], std::slice::from_ref(&p), &mut cs, &BTreeSet::new());
        assert_eq!(cs.len(), 1);
        assert!(matches!(
            &cs.entries[0].change,
            Change::Procedure(ProcedureChange::Create(pp)) if pp.qname == p.qname
        ));
        assert!(matches!(
            cs.entries[0].destructiveness,
            Destructiveness::Safe
        ));
    }

    #[test]
    fn procedure_drop_is_data_loss() {
        let p = make_procedure(qn("app", "old_proc"), "BEGIN NULL; END");
        let mut cs = ChangeSet::new();
        diff_procedures(std::slice::from_ref(&p), &[], &mut cs, &BTreeSet::new());
        assert_eq!(cs.len(), 1);
        assert!(matches!(
            &cs.entries[0].change,
            Change::Procedure(ProcedureChange::Drop(q)) if *q == p.qname
        ));
        assert!(matches!(
            cs.entries[0].destructiveness,
            Destructiveness::RequiresApprovalAndDataLossWarning { .. }
        ));
    }

    #[test]
    fn procedure_body_change_emits_create_or_replace() {
        let p_cat = make_procedure(qn("app", "proc1"), "BEGIN NULL; END");
        let p_src = make_procedure(qn("app", "proc1"), "BEGIN RAISE NOTICE 'hi'; END");
        let mut cs = ChangeSet::new();
        diff_procedures(
            std::slice::from_ref(&p_cat),
            std::slice::from_ref(&p_src),
            &mut cs,
            &BTreeSet::new(),
        );
        assert_eq!(cs.len(), 1);
        assert!(matches!(
            &cs.entries[0].change,
            Change::Procedure(ProcedureChange::CreateOrReplace(pp)) if pp.qname == p_src.qname
        ));
        assert!(matches!(
            cs.entries[0].destructiveness,
            Destructiveness::Safe
        ));
    }

    #[test]
    fn procedure_identical_emits_nothing() {
        let p = make_procedure(qn("app", "stable_proc"), "BEGIN NULL; END");
        let mut cs = ChangeSet::new();
        diff_procedures(
            std::slice::from_ref(&p),
            std::slice::from_ref(&p),
            &mut cs,
            &BTreeSet::new(),
        );
        assert!(cs.is_empty());
    }

    // ── Grant signature tests ─────────────────────────────────────────────────

    #[test]
    fn function_grant_change_carries_signature() {
        use crate::ir::grant::{Grant, GrantTarget, Privilege};

        // Function with one IN arg: signature should be "(integer)".
        let f_cat = make_func_with_arg(qn("app", "foo"), ColumnType::Integer, ColumnType::Integer);
        let mut f_src = f_cat.clone();

        // Add a grant only in source → differ must emit GrantObjectPrivilege.
        let managed_role = ident("app_user");
        f_src.grants = vec![Grant {
            grantee: GrantTarget::Role(managed_role.clone()),
            privilege: Privilege::Execute,
            with_grant_option: false,
            columns: None,
        }];

        let mut managed = BTreeSet::new();
        managed.insert(managed_role);

        let mut cs = ChangeSet::new();
        diff_functions(
            std::slice::from_ref(&f_cat),
            std::slice::from_ref(&f_src),
            &mut cs,
            &managed,
        );

        // Expect exactly one GrantObjectPrivilege change.
        let grant_entry = cs
            .entries
            .iter()
            .find(|e| matches!(&e.change, Change::GrantObjectPrivilege { .. }));
        assert!(
            grant_entry.is_some(),
            "expected GrantObjectPrivilege change"
        );

        if let Change::GrantObjectPrivilege { object, .. } = &grant_entry.unwrap().change {
            assert!(
                matches!(
                    object,
                    CatalogObjectRef::Function { signature, .. }
                        if signature == &RoutineSignature::new("(integer)".to_string())
                ),
                "signature must carry the IN arg type on a Function variant"
            );
        }
    }

    #[test]
    fn function_grant_change_no_args_has_empty_parens_signature() {
        use crate::ir::grant::{Grant, GrantTarget, Privilege};

        let f_cat = make_func_no_args(qn("app", "bar"), "SELECT 1");
        let mut f_src = f_cat.clone();

        let managed_role = ident("app_user");
        f_src.grants = vec![Grant {
            grantee: GrantTarget::Role(managed_role.clone()),
            privilege: Privilege::Execute,
            with_grant_option: false,
            columns: None,
        }];

        let mut managed = BTreeSet::new();
        managed.insert(managed_role);

        let mut cs = ChangeSet::new();
        diff_functions(
            std::slice::from_ref(&f_cat),
            std::slice::from_ref(&f_src),
            &mut cs,
            &managed,
        );

        let grant_entry = cs
            .entries
            .iter()
            .find(|e| matches!(&e.change, Change::GrantObjectPrivilege { .. }));
        assert!(
            grant_entry.is_some(),
            "expected GrantObjectPrivilege change"
        );

        if let Change::GrantObjectPrivilege { object, .. } = &grant_entry.unwrap().change {
            assert!(
                matches!(
                    object,
                    CatalogObjectRef::Function { signature, .. }
                        if signature == &RoutineSignature::new("()".to_string())
                ),
                "no-arg function signature must be ()"
            );
        }
    }

    #[test]
    fn procedure_grant_change_carries_signature() {
        use crate::ir::function::{ArgMode, FunctionArg};
        use crate::ir::grant::{Grant, GrantTarget, Privilege};

        // Procedure with one IN arg of type integer.
        let mut p_cat = make_procedure(qn("app", "do_work"), "BEGIN NULL; END");
        p_cat.args = vec![FunctionArg {
            name: Some(ident("n")),
            mode: ArgMode::In,
            ty: ColumnType::Integer,
            default: None,
        }];
        let mut p_src = p_cat.clone();

        let managed_role = ident("app_user");
        p_src.grants = vec![Grant {
            grantee: GrantTarget::Role(managed_role.clone()),
            privilege: Privilege::Execute,
            with_grant_option: false,
            columns: None,
        }];

        let mut managed = BTreeSet::new();
        managed.insert(managed_role);

        let mut cs = ChangeSet::new();
        diff_procedures(
            std::slice::from_ref(&p_cat),
            std::slice::from_ref(&p_src),
            &mut cs,
            &managed,
        );

        let grant_entry = cs
            .entries
            .iter()
            .find(|e| matches!(&e.change, Change::GrantObjectPrivilege { .. }));
        assert!(
            grant_entry.is_some(),
            "expected GrantObjectPrivilege change"
        );

        if let Change::GrantObjectPrivilege { object, .. } = &grant_entry.unwrap().change {
            assert!(
                matches!(
                    object,
                    CatalogObjectRef::Procedure { signature, .. }
                        if signature == &RoutineSignature::new("(integer)".to_string())
                ),
                "procedure signature must carry the IN arg type on a Procedure variant"
            );
        }
    }
}
