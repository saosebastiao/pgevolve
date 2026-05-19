//! User-defined functions (SQL or PL/pgSQL).

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column_type::ColumnType;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::difference::Difference;
use crate::ir::eq::Diff;
use crate::parse::normalize_body::NormalizedBody;
use crate::plan::edges::DepEdge;

/// A user-defined function (SQL or PL/pgSQL).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Function {
    /// Schema-qualified function name.
    pub qname: QualifiedName,
    /// Declared argument list (all modes).
    pub args: Vec<FunctionArg>,
    /// Normalized identity hash over IN/INOUT/VARIADIC args only.
    pub arg_types_normalized: NormalizedArgTypes,
    /// Return type.
    pub return_type: ReturnType,
    /// Implementation language.
    pub language: FunctionLanguage,
    /// Canonicalized function body.
    pub body: NormalizedBody,
    /// Dependency edges extracted from the function body AST.
    ///
    /// Filled by the T4 PL/pgSQL body parser and the T6 AST resolution pass.
    /// Empty until those passes run.
    #[serde(default)]
    pub body_dependencies: Vec<DepEdge>,
    /// Volatility category.
    pub volatility: Volatility,
    /// Whether the function returns NULL immediately for any NULL input.
    pub strict: bool,
    /// Security context (INVOKER or DEFINER).
    pub security: SecurityMode,
    /// Parallel safety classification.
    pub parallel: ParallelSafety,
    /// Whether the function is marked LEAKPROOF.
    pub leakproof: bool,
    /// Estimated per-call cost (units: sequential page fetches).
    pub cost: Option<f32>,
    /// Estimated rows returned per call (for set-returning functions).
    pub rows: Option<f32>,
    /// Optional `COMMENT ON FUNCTION` text.
    pub comment: Option<String>,
}

// f32 fields prevent deriving Eq, PartialEq, and Hash;
// implement manually using bit patterns so that equality and hashing are
// consistent (same bit pattern ↔ equal ↔ same hash).
impl PartialEq for Function {
    fn eq(&self, other: &Self) -> bool {
        self.qname == other.qname
            && self.args == other.args
            && self.arg_types_normalized == other.arg_types_normalized
            && self.return_type == other.return_type
            && self.language == other.language
            && self.body == other.body
            && self.body_dependencies == other.body_dependencies
            && self.volatility == other.volatility
            && self.strict == other.strict
            && self.security == other.security
            && self.parallel == other.parallel
            && self.leakproof == other.leakproof
            && self.cost.map(f32::to_bits) == other.cost.map(f32::to_bits)
            && self.rows.map(f32::to_bits) == other.rows.map(f32::to_bits)
            && self.comment == other.comment
    }
}

impl Eq for Function {}

impl std::hash::Hash for Function {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.qname.hash(state);
        self.args.hash(state);
        self.arg_types_normalized.hash(state);
        self.return_type.hash(state);
        self.language.hash(state);
        self.body.hash(state);
        self.body_dependencies.hash(state);
        self.volatility.hash(state);
        self.strict.hash(state);
        self.security.hash(state);
        self.parallel.hash(state);
        self.leakproof.hash(state);
        self.cost.map(f32::to_bits).hash(state);
        self.rows.map(f32::to_bits).hash(state);
        self.comment.hash(state);
    }
}

impl Diff for Function {
    // The structural differ at the change level lives in `crate::diff::routines`
    // (T8) and produces granular FunctionChange variants. This `Diff` impl is
    // the debug/equivalence-rule hook used by `Catalog::diff` for reporting
    // only; a single top-level entry per changed function — keyed by qname
    // and arg signature — is intentional here. Format the body hash hex
    // rather than the whole struct to keep output legible.
    fn diff(&self, other: &Self) -> Vec<Difference> {
        if self == other {
            Vec::new()
        } else {
            let key = format!(
                "{}({})",
                self.qname,
                self.args
                    .iter()
                    .map(|a| a.ty.render_sql())
                    .collect::<Vec<_>>()
                    .join(","),
            );
            vec![Difference::new(
                key,
                format!("body_hash={}", hex::encode(self.body.canonical_hash())),
                format!("body_hash={}", hex::encode(other.body.canonical_hash())),
            )]
        }
    }
}

/// A single argument declaration in a function or procedure.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct FunctionArg {
    /// Optional argument name.
    pub name: Option<Identifier>,
    /// Argument mode (IN, OUT, INOUT, VARIADIC).
    pub mode: ArgMode,
    /// Data type of the argument.
    pub ty: ColumnType,
    /// Optional default expression for this argument.
    pub default: Option<NormalizedExpr>,
}

/// Argument passing mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgMode {
    /// Standard input argument.
    In,
    /// Output argument.
    Out,
    /// Input/output argument.
    InOut,
    /// Variadic input argument (must be last).
    Variadic,
}

/// Return type of a function.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReturnType {
    /// Returns a single scalar value.
    Scalar {
        /// The scalar type.
        ty: ColumnType,
    },
    /// Returns a set of scalar values (SETOF).
    SetOf {
        /// The element type.
        ty: ColumnType,
    },
    /// Returns a virtual table (RETURNS TABLE).
    Table {
        /// Column definitions.
        columns: Vec<TableColumn>,
    },
    /// Trigger function (returns trigger).
    Trigger,
    /// Event trigger function.
    EventTrigger,
    /// Returns nothing (void).
    Void,
}

/// A column definition in a RETURNS TABLE clause.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct TableColumn {
    /// Column name.
    pub name: Identifier,
    /// Column data type.
    pub ty: ColumnType,
}

/// Implementation language of a function or procedure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionLanguage {
    /// Plain SQL body.
    Sql,
    /// PL/pgSQL procedural body.
    PlPgSql,
}

/// Volatility category of a function.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Volatility {
    /// Result never changes for the same input (allows aggressive caching).
    Immutable,
    /// Result is stable within a single transaction.
    Stable,
    /// Result may change at any time (default).
    Volatile,
}

/// Security execution context.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityMode {
    /// Executes with the privileges of the calling role (default).
    Invoker,
    /// Executes with the privileges of the defining role.
    Definer,
}

/// Parallel safety classification for a function.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParallelSafety {
    /// Cannot be executed in parallel mode.
    Unsafe,
    /// Can be executed in parallel but must run in the parallel leader.
    Restricted,
    /// Can be executed safely in parallel workers.
    Safe,
}

/// Normalized argument types — function identity disambiguator.
///
/// Built over the IN/INOUT/VARIADIC args only (matches PG's `proargtypes`).
/// The `canonical_hash` is BLAKE3 of the comma-joined canonical type strings.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct NormalizedArgTypes {
    /// IN/INOUT/VARIADIC types in declaration order.
    pub types: Vec<ColumnType>,
    /// BLAKE3 hash of the comma-joined canonical type strings.
    pub canonical_hash: [u8; 32],
}

impl NormalizedArgTypes {
    /// Construct from a list of args, filtering to IN/INOUT/VARIADIC and
    /// computing the BLAKE3 hash of the canonical type-string list.
    pub fn from_args(args: &[FunctionArg]) -> Self {
        let types: Vec<ColumnType> = args
            .iter()
            .filter(|a| matches!(a.mode, ArgMode::In | ArgMode::InOut | ArgMode::Variadic))
            .map(|a| a.ty.clone())
            .collect();
        let canonical_string = types
            .iter()
            .map(ColumnType::render_sql)
            .collect::<Vec<_>>()
            .join(",");
        let canonical_hash = blake3::hash(canonical_string.as_bytes()).into();
        Self {
            types,
            canonical_hash,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::schema::Schema;

    fn ident(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(ident(schema), ident(name))
    }

    fn sample_function() -> Function {
        let args = vec![FunctionArg {
            name: Some(ident("x")),
            mode: ArgMode::In,
            ty: ColumnType::Integer,
            default: None,
        }];
        let arg_types_normalized = NormalizedArgTypes::from_args(&args);
        Function {
            qname: qname("app", "double"),
            args,
            arg_types_normalized,
            return_type: ReturnType::Scalar {
                ty: ColumnType::Integer,
            },
            language: FunctionLanguage::Sql,
            body: NormalizedBody::from_sql("SELECT $1 * 2").unwrap(),
            body_dependencies: vec![],
            volatility: Volatility::Immutable,
            strict: true,
            security: SecurityMode::Invoker,
            parallel: ParallelSafety::Safe,
            leakproof: false,
            cost: Some(1.0),
            rows: None,
            comment: None,
        }
    }

    #[test]
    fn function_serde_round_trip() {
        let f = sample_function();
        let json = serde_json::to_string(&f).unwrap();
        let back: Function = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn function_overloads_have_distinct_arg_hashes() {
        let int_args = vec![FunctionArg {
            name: None,
            mode: ArgMode::In,
            ty: ColumnType::Integer,
            default: None,
        }];
        let text_args = vec![FunctionArg {
            name: None,
            mode: ArgMode::In,
            ty: ColumnType::Text,
            default: None,
        }];
        let int_norm = NormalizedArgTypes::from_args(&int_args);
        let text_norm = NormalizedArgTypes::from_args(&text_args);
        assert_ne!(int_norm.canonical_hash, text_norm.canonical_hash);
    }

    #[test]
    fn out_args_excluded_from_normalized_types() {
        let args = vec![
            FunctionArg {
                name: None,
                mode: ArgMode::In,
                ty: ColumnType::Integer,
                default: None,
            },
            FunctionArg {
                name: None,
                mode: ArgMode::Out,
                ty: ColumnType::Text,
                default: None,
            },
        ];
        let norm = NormalizedArgTypes::from_args(&args);
        assert_eq!(
            norm.types.len(),
            1,
            "OUT args must not appear in identity hash"
        );
        assert!(matches!(norm.types[0], ColumnType::Integer));
    }

    #[test]
    fn catalog_holds_functions_and_canonicalizes() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(ident("app")));
        c.functions.push(sample_function());
        c = c.canonicalize().expect("must canonicalize");
        assert_eq!(c.functions.len(), 1);
        assert_eq!(c.functions[0].qname.to_string(), "app.double");
    }

    #[test]
    fn catalog_rejects_duplicate_function_identity() {
        use crate::ir::IrError;

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(ident("app")));
        c.functions.push(sample_function());
        c.functions.push(sample_function());
        let r = c.canonicalize();
        assert!(
            matches!(r, Err(IrError::InvalidIdentifier(_))),
            "expected InvalidIdentifier, got {r:?}",
        );
        let msg = r.unwrap_err().to_string();
        assert!(
            msg.contains("app.double"),
            "should name the function: {msg}"
        );
    }

    #[test]
    fn catalog_allows_distinct_function_overloads() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(ident("app")));
        let f1 = sample_function();
        let mut f2 = sample_function();
        f2.args[0].ty = ColumnType::Text;
        f2.arg_types_normalized = NormalizedArgTypes::from_args(&f2.args);
        f2.return_type = ReturnType::Scalar {
            ty: ColumnType::Text,
        };
        c.functions.push(f1);
        c.functions.push(f2);
        let c = c.canonicalize().expect("overloads should be allowed");
        assert_eq!(c.functions.len(), 2);
    }
}
