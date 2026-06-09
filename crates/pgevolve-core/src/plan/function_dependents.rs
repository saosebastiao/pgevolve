//! Enumerate every catalog object that depends on a function — i.e. exactly what
//! `DROP FUNCTION <f> CASCADE` would destroy. Used to make the function
//! replacement auditable (the destruction is inherent; this names it).
//!
//! Coverage mirrors `plan::edges::build_drop_graph`: every edge there whose
//! *target* is a [`NodeId::Function`](crate::plan::edges::NodeId::Function) is
//! reproduced here as a dependent category — triggers, event triggers,
//! aggregates (sfunc/finalfunc), casts (conversion function), and body-ref
//! objects (views / materialized views / functions whose body references the
//! function).
//!
//! ## Matching rule (audit safety)
//!
//! A function's identity is `(qname, arg_types)` (overloads share a name). Not
//! every reference site records the arg types, so the match precision varies:
//!
//! - **Triggers / event triggers** reference by **qname** only (their functions
//!   take no SQL arguments and don't meaningfully overload) → matched by qname.
//! - **Aggregates** record `sfunc`/`finalfunc` as bare **qnames** → matched by
//!   qname.
//! - **Casts** and **body-ref `DepEdge`s** record **qname + arg types** →
//!   matched by qname *and* arg types (precise; an overloaded sibling is not
//!   falsely reported).
//!
//! Where only the qname is available, an overloaded name may **over-report**
//! (naming dependents of a sibling overload). For an audit of what CASCADE
//! destroys, over-reporting is the safe direction — better to warn about
//! possibly-more than to hide a destruction. This enumerator therefore never
//! *under*-reports: when in doubt it matches by qname.

#![allow(dead_code)] // Consumer (the destructive DROP FUNCTION warning) lands in the next task; removed there.

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::cast::CastMethod;
use crate::ir::catalog::Catalog;
use crate::ir::function::NormalizedArgTypes;
use crate::plan::edges::NodeId;

/// One object that depends on a function being CASCADE-replaced.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum FunctionDependent {
    /// A trigger whose function is the dropped function.
    Trigger(QualifiedName),
    /// An event trigger whose function is the dropped function.
    EventTrigger(Identifier),
    /// An aggregate whose `sfunc` or `finalfunc` is the dropped function.
    Aggregate(QualifiedName),
    /// A cast whose conversion function is the dropped function.
    Cast {
        /// The cast's source type.
        source: QualifiedName,
        /// The cast's target type.
        target: QualifiedName,
    },
    /// A view / materialized view / function whose body references the dropped
    /// function.
    BodyRef(QualifiedName),
}

/// Every dependent of the function identified by (`qname`, `arg_types`) in
/// `target`, deterministically ordered (sorted + deduped).
///
/// See the module documentation for the per-category matching rule and the
/// over-reporting (never under-reporting) guarantee.
pub fn enumerate_function_dependents(
    qname: &QualifiedName,
    arg_types: &NormalizedArgTypes,
    target: &Catalog,
) -> Vec<FunctionDependent> {
    let mut out = Vec::new();

    // Triggers: reference the function by qname (trigger functions are no-arg).
    for trigger in &target.triggers {
        if &trigger.function_qname == qname {
            out.push(FunctionDependent::Trigger(trigger.qname.clone()));
        }
    }

    // Event triggers: reference the function by qname (no-arg, like triggers).
    for et in &target.event_triggers {
        if &et.function == qname {
            out.push(FunctionDependent::EventTrigger(et.name.clone()));
        }
    }

    // Aggregates: `sfunc` and `finalfunc` are recorded as bare qnames.
    for agg in &target.aggregates {
        let sfunc_hit = &agg.sfunc == qname;
        let finalfunc_hit = agg.finalfunc.as_ref().is_some_and(|f| f == qname);
        if sfunc_hit || finalfunc_hit {
            out.push(FunctionDependent::Aggregate(agg.qname.clone()));
        }
    }

    // Casts: the conversion function records qname + arg types — match precisely.
    for cast in &target.casts {
        if let CastMethod::Function {
            name,
            arg_types: recorded,
        } = &cast.method
            && name == qname
            && recorded == &arg_types.types
        {
            out.push(FunctionDependent::Cast {
                source: cast.source.clone(),
                target: cast.target.clone(),
            });
        }
    }

    // Body-ref objects: views / MVs / functions whose `body_dependencies` carry
    // an edge whose target is this exact function (qname + arg types). The
    // dependent object is `dep.from`'s qname.
    for view in &target.views {
        if body_deps_reference_function(&view.body_dependencies, qname, arg_types) {
            out.push(FunctionDependent::BodyRef(view.qname.clone()));
        }
    }
    for mv in &target.materialized_views {
        if body_deps_reference_function(&mv.body_dependencies, qname, arg_types) {
            out.push(FunctionDependent::BodyRef(mv.qname.clone()));
        }
    }
    for f in &target.functions {
        // A function is not its own dependent even if its body self-references.
        if &f.qname == qname && f.arg_types_normalized == *arg_types {
            continue;
        }
        if body_deps_reference_function(&f.body_dependencies, qname, arg_types) {
            out.push(FunctionDependent::BodyRef(f.qname.clone()));
        }
    }

    out.sort();
    out.dedup();
    out
}

/// `true` iff any `DepEdge` in `deps` targets the function `(qname, arg_types)`.
///
/// Body-dependency edges record the referenced function's full identity in
/// `dep.to` as [`NodeId::Function(qname, arg_types)`](NodeId::Function)
/// (see `plan::edges` body-dependency wiring), so the match is precise.
fn body_deps_reference_function(
    deps: &[crate::plan::edges::DepEdge],
    qname: &QualifiedName,
    arg_types: &NormalizedArgTypes,
) -> bool {
    deps.iter()
        .any(|edge| matches!(&edge.to, NodeId::Function(q, a) if q == qname && a == arg_types))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::aggregate::Aggregate;
    use crate::ir::cast::{Cast, CastContext};
    use crate::ir::column_type::ColumnType;
    use crate::ir::event_trigger::{EventTrigger, EventTriggerEnabled, EventTriggerEvent};
    use crate::ir::function::{
        ArgMode, Function, FunctionArg, FunctionLanguage, ParallelSafety, ReturnType, SecurityMode,
        Volatility,
    };
    use crate::ir::trigger::{Trigger, TriggerEvent, TriggerLevel, TriggerTiming};
    use crate::ir::view::{MaterializedView, View, ViewColumn};
    use crate::parse::normalize_body::NormalizedBody;
    use crate::plan::edges::{DepEdge, DepSource};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(name: &str) -> QualifiedName {
        QualifiedName::new(id("app"), id(name))
    }

    /// `app.f(integer)` — the function whose dependents we enumerate.
    fn f_qname() -> QualifiedName {
        qn("f")
    }

    fn int_args() -> Vec<FunctionArg> {
        vec![FunctionArg {
            name: Some(id("x")),
            mode: ArgMode::In,
            ty: ColumnType::Integer,
            default: None,
        }]
    }

    fn f_arg_types() -> NormalizedArgTypes {
        NormalizedArgTypes::from_args(&int_args())
    }

    fn function(name: &str, args: Vec<FunctionArg>, body_deps: Vec<DepEdge>) -> Function {
        let arg_types_normalized = NormalizedArgTypes::from_args(&args);
        Function {
            qname: qn(name),
            args,
            arg_types_normalized,
            return_type: ReturnType::Void,
            language: FunctionLanguage::Sql,
            body: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: body_deps,
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

    /// A body-dependency edge from `dependent` (a function) to `app.f(integer)`.
    fn dep_on_f(dependent: &QualifiedName, dependent_args: &NormalizedArgTypes) -> DepEdge {
        DepEdge {
            from: NodeId::Function(dependent.clone(), dependent_args.clone()),
            to: NodeId::Function(f_qname(), f_arg_types()),
            source: DepSource::AstExtracted,
        }
    }

    fn trigger_calling_f(name: &str) -> Trigger {
        Trigger {
            qname: qn(name),
            table: qn("users"),
            timing: TriggerTiming::Before,
            events: vec![TriggerEvent::Insert],
            level: TriggerLevel::Row,
            when_clause: None,
            transition_tables: vec![],
            function_qname: f_qname(),
            function_args: vec![],
            is_constraint: false,
            deferrable: crate::ir::constraint::Deferrable::NotDeferrable,
            comment: None,
        }
    }

    fn event_trigger_calling_f(name: &str) -> EventTrigger {
        EventTrigger {
            name: id(name),
            event: EventTriggerEvent::DdlCommandStart,
            tag_filter: vec![],
            function: f_qname(),
            enabled: EventTriggerEnabled::Enabled,
            owner: None,
            comment: None,
        }
    }

    fn aggregate_with_sfunc_f(name: &str) -> Aggregate {
        Aggregate {
            qname: qn(name),
            arg_types: vec![ColumnType::Integer],
            state_type: ColumnType::Integer,
            sfunc: f_qname(),
            finalfunc: None,
            initcond: None,
            owner: None,
            comment: None,
        }
    }

    fn cast_with_conversion_f(source: &str, target: &str) -> Cast {
        Cast {
            source: qn(source),
            target: qn(target),
            method: CastMethod::Function {
                name: f_qname(),
                arg_types: vec![ColumnType::Integer],
            },
            context: CastContext::Explicit,
            comment: None,
        }
    }

    fn view_calling_f(name: &str) -> View {
        View {
            qname: qn(name),
            columns: vec![ViewColumn {
                name: id("c"),
                column_type: Some(ColumnType::Integer),
                comment: None,
            }],
            body_canonical: NormalizedBody::from_sql("SELECT app.f(1)").unwrap(),
            body_dependencies: vec![DepEdge {
                from: NodeId::View(qn(name)),
                to: NodeId::Function(f_qname(), f_arg_types()),
                source: DepSource::AstExtracted,
            }],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        }
    }

    fn mv_calling_f(name: &str) -> MaterializedView {
        MaterializedView {
            qname: qn(name),
            columns: vec![ViewColumn {
                name: id("c"),
                column_type: Some(ColumnType::Integer),
                comment: None,
            }],
            body_canonical: NormalizedBody::from_sql("SELECT app.f(1)").unwrap(),
            body_dependencies: vec![DepEdge {
                from: NodeId::Mv(qn(name)),
                to: NodeId::Function(f_qname(), f_arg_types()),
                source: DepSource::AstExtracted,
            }],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        }
    }

    #[test]
    fn enumerates_all_dependent_categories() {
        let mut cat = Catalog::empty();
        // The function itself.
        cat.functions.push(function("f", int_args(), vec![]));
        // One of each dependent category.
        cat.triggers.push(trigger_calling_f("trg"));
        cat.event_triggers.push(event_trigger_calling_f("evt"));
        cat.aggregates.push(aggregate_with_sfunc_f("agg"));
        cat.casts.push(cast_with_conversion_f("src_t", "dst_t"));
        cat.views.push(view_calling_f("v"));
        // A function whose body references f.
        let caller_args = NormalizedArgTypes::from_args(&[]);
        cat.functions.push(function(
            "caller",
            vec![],
            vec![dep_on_f(&qn("caller"), &caller_args)],
        ));

        let deps = enumerate_function_dependents(&f_qname(), &f_arg_types(), &cat);

        assert!(deps.contains(&FunctionDependent::Trigger(qn("trg"))));
        assert!(deps.contains(&FunctionDependent::EventTrigger(id("evt"))));
        assert!(deps.contains(&FunctionDependent::Aggregate(qn("agg"))));
        assert!(deps.contains(&FunctionDependent::Cast {
            source: qn("src_t"),
            target: qn("dst_t"),
        }));
        assert!(deps.contains(&FunctionDependent::BodyRef(qn("v"))));
        assert!(deps.contains(&FunctionDependent::BodyRef(qn("caller"))));
        assert_eq!(deps.len(), 6);

        // Deterministic ordering: sorted + deduped.
        let mut sorted = deps.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(deps, sorted);
    }

    #[test]
    fn aggregate_finalfunc_is_reported() {
        let mut cat = Catalog::empty();
        cat.functions.push(function("f", int_args(), vec![]));
        // Aggregate whose FINALFUNC (not sfunc) is f.
        cat.aggregates.push(Aggregate {
            qname: qn("agg"),
            arg_types: vec![ColumnType::Integer],
            state_type: ColumnType::Integer,
            sfunc: qn("other_sfunc"),
            finalfunc: Some(f_qname()),
            initcond: None,
            owner: None,
            comment: None,
        });

        let deps = enumerate_function_dependents(&f_qname(), &f_arg_types(), &cat);
        assert_eq!(deps, vec![FunctionDependent::Aggregate(qn("agg"))]);
    }

    #[test]
    fn materialized_view_body_ref_is_reported() {
        let mut cat = Catalog::empty();
        cat.functions.push(function("f", int_args(), vec![]));
        cat.materialized_views.push(mv_calling_f("mv"));

        let deps = enumerate_function_dependents(&f_qname(), &f_arg_types(), &cat);
        assert_eq!(deps, vec![FunctionDependent::BodyRef(qn("mv"))]);
    }

    #[test]
    fn no_dependents_yields_empty() {
        let mut cat = Catalog::empty();
        cat.functions.push(function("f", int_args(), vec![]));
        // A trigger/aggregate that reference a DIFFERENT function.
        let mut trg = trigger_calling_f("trg");
        trg.function_qname = qn("g");
        cat.triggers.push(trg);
        cat.aggregates.push(Aggregate {
            qname: qn("agg"),
            arg_types: vec![ColumnType::Integer],
            state_type: ColumnType::Integer,
            sfunc: qn("g"),
            finalfunc: None,
            initcond: None,
            owner: None,
            comment: None,
        });

        assert!(enumerate_function_dependents(&f_qname(), &f_arg_types(), &cat).is_empty());
    }

    #[test]
    fn unrelated_function_dependents_not_reported() {
        // Enumerate dependents of `app.g(integer)`, but the catalog's trigger,
        // aggregate, cast, and view all depend on `app.f(integer)`.
        let mut cat = Catalog::empty();
        cat.functions.push(function("f", int_args(), vec![]));
        cat.functions.push(function("g", int_args(), vec![]));
        cat.triggers.push(trigger_calling_f("trg"));
        cat.aggregates.push(aggregate_with_sfunc_f("agg"));
        cat.casts.push(cast_with_conversion_f("src_t", "dst_t"));
        cat.views.push(view_calling_f("v"));

        let g_args = NormalizedArgTypes::from_args(&int_args());
        let deps = enumerate_function_dependents(&qn("g"), &g_args, &cat);
        assert!(
            deps.is_empty(),
            "g's dependents must not include f's dependents, got {deps:?}"
        );
    }

    #[test]
    fn function_is_not_its_own_body_ref_dependent() {
        // A recursive function whose body self-references must not list itself.
        let self_args = f_arg_types();
        let recursive = function("f", int_args(), vec![dep_on_f(&f_qname(), &self_args)]);
        let mut cat = Catalog::empty();
        cat.functions.push(recursive);

        assert!(enumerate_function_dependents(&f_qname(), &f_arg_types(), &cat).is_empty());
    }

    #[test]
    fn cast_matches_arg_types_precisely_no_overload_over_report() {
        // Two same-named overloads: app.f(integer) and app.f(text). A cast uses
        // the (text) overload as its conversion function. Enumerating dependents
        // of the (integer) overload must NOT report that cast — casts record arg
        // types, so the match is precise.
        let mut cat = Catalog::empty();
        cat.functions.push(function("f", int_args(), vec![]));
        let text_args = vec![FunctionArg {
            name: Some(id("x")),
            mode: ArgMode::In,
            ty: ColumnType::Text,
            default: None,
        }];
        cat.functions.push(function("f", text_args, vec![]));
        // Cast whose conversion function is f(text).
        cat.casts.push(Cast {
            source: qn("src_t"),
            target: qn("dst_t"),
            method: CastMethod::Function {
                name: f_qname(),
                arg_types: vec![ColumnType::Text],
            },
            context: CastContext::Explicit,
            comment: None,
        });

        // Dependents of f(integer): the f(text) cast is NOT reported.
        let deps = enumerate_function_dependents(&f_qname(), &f_arg_types(), &cat);
        assert!(
            deps.is_empty(),
            "cast on f(text) must not be reported as a dependent of f(integer), got {deps:?}"
        );
    }

    #[test]
    fn trigger_over_reports_on_overloaded_name_safely() {
        // Triggers reference functions by qname only. If a name is overloaded,
        // a trigger on `app.f` is reported for ANY f overload being dropped —
        // over-reporting is the safe (audit) direction. This test documents and
        // asserts that accepted imprecision.
        let mut cat = Catalog::empty();
        cat.functions.push(function("f", int_args(), vec![]));
        let text_args = vec![FunctionArg {
            name: Some(id("x")),
            mode: ArgMode::In,
            ty: ColumnType::Text,
            default: None,
        }];
        cat.functions.push(function("f", text_args.clone(), vec![]));
        cat.triggers.push(trigger_calling_f("trg")); // references app.f by qname

        // Dropping the f(text) overload still reports the trigger (qname match):
        // over-reporting, not under-reporting.
        let text_norm = NormalizedArgTypes::from_args(&text_args);
        let deps = enumerate_function_dependents(&f_qname(), &text_norm, &cat);
        assert_eq!(deps, vec![FunctionDependent::Trigger(qn("trg"))]);
    }
}
