//! Enumerate every catalog object that depends on a user-defined type — i.e.
//! exactly what `DROP TYPE <t> CASCADE` would destroy. Used to make the type
//! replacement auditable (the destruction is inherent; this names it).

use std::fmt::Write as _;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::cast::CastMethod;
use crate::ir::catalog::Catalog;
use crate::ir::column_type::ColumnType;
use crate::ir::function::ReturnType;
use crate::ir::user_type::UserTypeKind;

/// One object that depends on a type being CASCADE-replaced.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum TypeDependent {
    /// `table.column` whose type is (or contains) the dropped type. DATA LOSS on drop.
    Column {
        /// The table holding the dependent column.
        table: QualifiedName,
        /// The dependent column.
        column: Identifier,
    },
    /// A view with a column of the dropped type.
    View(QualifiedName),
    /// A materialized view with a column of the dropped type.
    MaterializedView(QualifiedName),
    /// A function/procedure whose argument or return type is the dropped type.
    Routine(QualifiedName),
    /// Another user-defined type embedding the dropped type (composite attr,
    /// domain base, or range subtype).
    Type(QualifiedName),
    /// An aggregate whose argument or state (transition) type is the dropped type.
    Aggregate(QualifiedName),
    /// A cast whose source or target type is the dropped type.
    Cast {
        /// The cast's source type.
        source: QualifiedName,
        /// The cast's target type.
        target: QualifiedName,
    },
}

/// Every dependent of `ty` in `target`, deterministically ordered (sorted + deduped).
pub fn enumerate_type_dependents(ty: &QualifiedName, target: &Catalog) -> Vec<TypeDependent> {
    let mut out = Vec::new();

    // Tables: each column whose type is (or contains) the dropped type.
    for table in &target.tables {
        for col in &table.columns {
            if column_type_references(&col.ty, ty) {
                out.push(TypeDependent::Column {
                    table: table.qname.clone(),
                    column: col.name.clone(),
                });
            }
        }
    }

    // Views: each resolved column type (Option<ColumnType>).
    for view in &target.views {
        if view.columns.iter().any(|c| {
            c.column_type
                .as_ref()
                .is_some_and(|ct| column_type_references(ct, ty))
        }) {
            out.push(TypeDependent::View(view.qname.clone()));
        }
    }

    // Materialized views: same column shape as views.
    for mv in &target.materialized_views {
        if mv.columns.iter().any(|c| {
            c.column_type
                .as_ref()
                .is_some_and(|ct| column_type_references(ct, ty))
        }) {
            out.push(TypeDependent::MaterializedView(mv.qname.clone()));
        }
    }

    // Functions: argument types and the return type.
    for f in &target.functions {
        let arg_hit = f.args.iter().any(|a| column_type_references(&a.ty, ty));
        let ret_hit = return_type_references(&f.return_type, ty);
        if arg_hit || ret_hit {
            out.push(TypeDependent::Routine(f.qname.clone()));
        }
    }

    // Procedures: argument types only (procedures have no return type).
    for p in &target.procedures {
        if p.args.iter().any(|a| column_type_references(&a.ty, ty)) {
            out.push(TypeDependent::Routine(p.qname.clone()));
        }
    }

    // Types: composite attributes, domain bases, and range subtypes.
    for ut in &target.types {
        if &ut.qname == ty {
            continue; // the type itself is not its own dependent
        }
        let embeds = match &ut.kind {
            UserTypeKind::Composite { attributes } => attributes
                .iter()
                .any(|attr| column_type_references(&attr.ty, ty)),
            UserTypeKind::Domain { base, .. } => column_type_references(base, ty),
            UserTypeKind::Range { subtype, .. } => subtype == ty,
            UserTypeKind::Enum { .. } => false,
        };
        if embeds {
            out.push(TypeDependent::Type(ut.qname.clone()));
        }
    }

    // Aggregates: argument types and the state (transition) type.
    for agg in &target.aggregates {
        if column_type_references(&agg.state_type, ty)
            || agg.arg_types.iter().any(|t| column_type_references(t, ty))
        {
            out.push(TypeDependent::Aggregate(agg.qname.clone()));
        }
    }

    // Casts: source/target equality plus any conversion-function argument type.
    for cast in &target.casts {
        let func_arg_match = matches!(&cast.method, CastMethod::Function { arg_types, .. }
            if arg_types.iter().any(|t| column_type_references(t, ty)));
        if cast.source == *ty || cast.target == *ty || func_arg_match {
            out.push(TypeDependent::Cast {
                source: cast.source.clone(),
                target: cast.target.clone(),
            });
        }
    }

    out.sort();
    out.dedup();
    out
}

/// Render `deps` as a destructive-warning suffix naming everything a
/// `DROP TYPE <ty> CASCADE` will additionally destroy, e.g.
/// `"; DROP TYPE app.color CASCADE will also destroy: column app.t.c, view app.v,
/// function app.f, aggregate app.a, cast (app.color AS pg_catalog.text)"`.
///
/// `deps` is assumed already deterministically ordered (as produced by
/// [`enumerate_type_dependents`]). An empty slice yields an empty string — a
/// type with no dependents drops cleanly and needs no suffix.
pub fn render_cascade_destruction(ty: &QualifiedName, deps: &[TypeDependent]) -> String {
    if deps.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "; DROP TYPE {} CASCADE will also destroy: ",
        ty.render_sql()
    );
    for (i, dep) in deps.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        // Writing to a `String` is infallible; the result is discarded.
        let _ = match dep {
            TypeDependent::Column { table, column } => {
                write!(out, "column {}.{}", table.render_sql(), column.render_sql())
            }
            TypeDependent::View(q) => write!(out, "view {}", q.render_sql()),
            TypeDependent::MaterializedView(q) => {
                write!(out, "materialized view {}", q.render_sql())
            }
            TypeDependent::Routine(q) => write!(out, "routine {}", q.render_sql()),
            TypeDependent::Type(q) => write!(out, "type {}", q.render_sql()),
            TypeDependent::Aggregate(q) => write!(out, "aggregate {}", q.render_sql()),
            TypeDependent::Cast { source, target } => write!(
                out,
                "cast ({} AS {})",
                source.render_sql(),
                target.render_sql()
            ),
        };
    }
    out
}

/// `ColumnType` references `ty` directly or as an array element.
fn column_type_references(ct: &ColumnType, ty: &QualifiedName) -> bool {
    match ct {
        ColumnType::UserDefined(q) => q == ty,
        ColumnType::Array { element, .. } => column_type_references(element, ty),
        _ => false,
    }
}

/// A function's return type references `ty` directly, as a `SETOF` element, or
/// in any `RETURNS TABLE` column.
fn return_type_references(rt: &ReturnType, ty: &QualifiedName) -> bool {
    match rt {
        ReturnType::Scalar { ty: t } | ReturnType::SetOf { ty: t } => column_type_references(t, ty),
        ReturnType::Table { columns } => columns.iter().any(|c| column_type_references(&c.ty, ty)),
        ReturnType::Trigger | ReturnType::EventTrigger | ReturnType::Void => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::aggregate::Aggregate;
    use crate::ir::cast::{Cast, CastContext};
    use crate::ir::column::Column;
    use crate::ir::function::{
        ArgMode, Function, FunctionArg, FunctionLanguage, NormalizedArgTypes, ParallelSafety,
        SecurityMode, Volatility,
    };
    use crate::ir::procedure::Procedure;
    use crate::ir::table::Table;
    use crate::ir::user_type::{CompositeAttribute, UserType};
    use crate::ir::view::{MaterializedView, View, ViewColumn};
    use crate::parse::normalize_body::NormalizedBody;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(name: &str) -> QualifiedName {
        QualifiedName::new(id("app"), id(name))
    }

    fn color_qname() -> QualifiedName {
        qn("color")
    }

    fn color_type() -> ColumnType {
        ColumnType::UserDefined(color_qname())
    }

    fn color_array() -> ColumnType {
        ColumnType::Array {
            element: Box::new(color_type()),
            dims: 1,
        }
    }

    fn col(name: &str, ty: ColumnType) -> Column {
        Column {
            name: id(name),
            ty,
            nullable: false,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        }
    }

    fn table(name: &str, columns: Vec<Column>) -> Table {
        Table {
            qname: qn(name),
            columns,
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
            tablespace: None,
        }
    }

    fn view_with_color(name: &str) -> View {
        View {
            qname: qn(name),
            columns: vec![ViewColumn {
                name: id("c"),
                column_type: Some(color_type()),
                comment: None,
            }],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        }
    }

    fn mv_with_color(name: &str) -> MaterializedView {
        MaterializedView {
            qname: qn(name),
            columns: vec![ViewColumn {
                name: id("c"),
                column_type: Some(color_type()),
                comment: None,
            }],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        }
    }

    fn function_arg_color(name: &str) -> Function {
        let args = vec![FunctionArg {
            name: Some(id("c")),
            mode: ArgMode::In,
            ty: color_type(),
            default: None,
        }];
        let arg_types_normalized = NormalizedArgTypes::from_args(&args);
        Function {
            qname: qn(name),
            args,
            arg_types_normalized,
            return_type: ReturnType::Void,
            language: FunctionLanguage::Sql,
            body: NormalizedBody::from_sql("SELECT 1").unwrap(),
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

    fn composite_embedding_color(name: &str) -> UserType {
        UserType {
            qname: qn(name),
            kind: UserTypeKind::Composite {
                attributes: vec![CompositeAttribute {
                    name: id("shade"),
                    ty: color_type(),
                    collation: None,
                }],
            },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn color_type_def() -> UserType {
        UserType {
            qname: color_qname(),
            kind: UserTypeKind::Composite {
                attributes: vec![CompositeAttribute {
                    name: id("r"),
                    ty: ColumnType::Integer,
                    collation: None,
                }],
            },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    #[test]
    fn enumerates_all_dependent_categories() {
        let mut cat = Catalog::empty();
        cat.types.push(color_type_def());
        cat.tables
            .push(table("scalar_tbl", vec![col("c", color_type())]));
        cat.tables
            .push(table("array_tbl", vec![col("cs", color_array())]));
        cat.views.push(view_with_color("v"));
        cat.functions.push(function_arg_color("paint"));
        cat.types.push(composite_embedding_color("palette"));

        let deps = enumerate_type_dependents(&color_qname(), &cat);

        assert!(deps.contains(&TypeDependent::Column {
            table: qn("scalar_tbl"),
            column: id("c"),
        }));
        assert!(deps.contains(&TypeDependent::Column {
            table: qn("array_tbl"),
            column: id("cs"),
        }));
        assert!(deps.contains(&TypeDependent::View(qn("v"))));
        assert!(deps.contains(&TypeDependent::Routine(qn("paint"))));
        assert!(deps.contains(&TypeDependent::Type(qn("palette"))));
        assert_eq!(deps.len(), 5);

        // Deterministic ordering: result is sorted and deduped.
        let mut sorted = deps.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(deps, sorted);
    }

    #[test]
    fn materialized_view_and_return_type_dependents() {
        let mut cat = Catalog::empty();
        cat.types.push(color_type_def());
        cat.materialized_views.push(mv_with_color("mv"));

        // Function returning the type (SETOF) — return-type detection.
        let mut returns_color = function_arg_color("make");
        returns_color.args.clear();
        returns_color.arg_types_normalized = NormalizedArgTypes::from_args(&returns_color.args);
        returns_color.return_type = ReturnType::SetOf { ty: color_type() };
        cat.functions.push(returns_color);

        // Procedure with an arg of the type.
        cat.procedures.push(Procedure {
            qname: qn("apply"),
            args: vec![FunctionArg {
                name: Some(id("c")),
                mode: ArgMode::In,
                ty: color_type(),
                default: None,
            }],
            language: FunctionLanguage::PlPgSql,
            body: NormalizedBody::empty(),
            body_dependencies: vec![],
            security: SecurityMode::Invoker,
            commits_in_body: false,
            comment: None,
            owner: None,
            grants: vec![],
        });

        let deps = enumerate_type_dependents(&color_qname(), &cat);
        assert!(deps.contains(&TypeDependent::MaterializedView(qn("mv"))));
        assert!(deps.contains(&TypeDependent::Routine(qn("make"))));
        assert!(deps.contains(&TypeDependent::Routine(qn("apply"))));
        assert_eq!(deps.len(), 3);
    }

    #[test]
    fn aggregate_with_state_type_is_reported() {
        let mut cat = Catalog::empty();
        cat.types.push(color_type_def());
        // Aggregate whose STATE (transition) type is the dropped type; args are built-in.
        cat.aggregates.push(Aggregate {
            qname: qn("blend"),
            arg_types: vec![ColumnType::Integer],
            state_type: color_type(),
            sfunc: qn("blend_sfunc"),
            finalfunc: None,
            initcond: None,
            owner: None,
            comment: None,
        });

        let deps = enumerate_type_dependents(&color_qname(), &cat);
        assert!(deps.contains(&TypeDependent::Aggregate(qn("blend"))));
        assert_eq!(deps.len(), 1);
    }

    #[test]
    fn cast_with_source_type_is_reported() {
        let mut cat = Catalog::empty();
        cat.types.push(color_type_def());
        // Cast whose SOURCE is the dropped type; target + method args are built-in.
        cat.casts.push(Cast {
            source: color_qname(),
            target: QualifiedName::new(id("pg_catalog"), id("text")),
            method: CastMethod::Inout,
            context: CastContext::Explicit,
            comment: None,
        });

        let deps = enumerate_type_dependents(&color_qname(), &cat);
        assert!(deps.contains(&TypeDependent::Cast {
            source: color_qname(),
            target: QualifiedName::new(id("pg_catalog"), id("text")),
        }));
        assert_eq!(deps.len(), 1);
    }

    #[test]
    fn no_dependents_yields_empty() {
        let mut cat = Catalog::empty();
        cat.types.push(color_type_def());
        // A table with only built-in types.
        cat.tables
            .push(table("plain", vec![col("id", ColumnType::BigInt)]));
        assert!(enumerate_type_dependents(&color_qname(), &cat).is_empty());
    }

    #[test]
    fn unrelated_type_column_not_reported() {
        let mut cat = Catalog::empty();
        cat.types.push(color_type_def());
        // Column of a DIFFERENT user-defined type.
        let other = ColumnType::UserDefined(qn("shape"));
        cat.tables.push(table("things", vec![col("s", other)]));

        assert!(enumerate_type_dependents(&color_qname(), &cat).is_empty());
    }

    #[test]
    fn type_is_not_its_own_dependent() {
        let mut cat = Catalog::empty();
        // color is a composite whose attrs are all built-in; it must not list
        // itself even though it appears in catalog.types.
        cat.types.push(color_type_def());
        assert!(enumerate_type_dependents(&color_qname(), &cat).is_empty());
    }
}
