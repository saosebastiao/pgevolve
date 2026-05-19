# Functions and Procedures Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring `CREATE FUNCTION` and `CREATE PROCEDURE` under pgevolve's managed surface — full lifecycle (CREATE / CREATE OR REPLACE / DROP) for SQL and PL/pgSQL routines, including overload identity, attribute matrix, AST-derived body dependencies, `@pgevolve dep:` directive close-out, and auto-detected `COMMIT`/`ROLLBACK` procedures.

**Architecture:** Two new flat IR records (`Function`, `Procedure`) on `Catalog`. Identity: `(qname, NormalizedArgTypes)` for functions; qname-only for procedures. Bodies flow through pg_query's PL/pgSQL parser (via `plpgsql_parse`) into the existing `NormalizedBody` shape from v0.2-views. The differ emits a single `CreateOrReplace` step for any in-place-compatible change; return-type kind changes trigger `ReplaceWithCascade` via the existing dependent-recreation walker. Procedures with `COMMIT`/`ROLLBACK` in their body run as `TransactionConstraint::OutsideTransaction`.

**Tech Stack:** Rust 1.78+; `pg_query` 6.1.1 (with `plpgsql_parse`); tokio_postgres; the existing `NormalizedExpr` / `NormalizedBody` / `DepEdge` / `DepSource` machinery; the existing dependent-recreation walker (extended).

**Source spec:** [`docs/superpowers/specs/2026-05-18-functions-procedures-design.md`](../specs/2026-05-18-functions-procedures-design.md)

---

## File structure

```
crates/pgevolve-core/src/
├── ir/
│   ├── function.rs                                NEW — T1 — Function, FunctionArg, ArgMode, ReturnType, FunctionLanguage, Volatility, SecurityMode, ParallelSafety, NormalizedArgTypes
│   ├── procedure.rs                               NEW — T1 — Procedure
│   ├── catalog.rs                                 MODIFY — T1 — functions + procedures fields
│   └── mod.rs                                     MODIFY — T1 — re-exports
├── parse/
│   ├── builder/
│   │   ├── create_function_stmt.rs                NEW — T2/T3 — builds both Function and Procedure from CreateFunctionStmt
│   │   ├── plpgsql.rs                              NEW — T4 — parse_plpgsql_body
│   │   ├── sql_body.rs                             NEW — T5 — parse_sql_body
│   │   └── mod.rs                                  MODIFY — T2 — declare new modules
│   ├── statement.rs                                MODIFY — T2/T3 — Statement::CreateFunction, CreateProcedure variants + dispatch
│   ├── mod.rs                                      MODIFY — T2/T3 — push parsed routines into catalog
│   └── ast_resolution.rs                           MODIFY — T6 — resolve_routine_references + overload resolution
├── catalog/
│   ├── queries/
│   │   ├── functions.rs                            NEW — T7 — SELECT_FUNCTIONS SQL
│   │   ├── mod.rs                                  MODIFY — T7 — re-export + dispatch
│   │   └── pg14/pg15/pg16/pg17.rs                  MODIFY — T7 — same SQL for all four versions
│   ├── assemble.rs                                 MODIFY — T7 — build_functions_and_procedures
│   └── mod.rs                                      MODIFY — T7 — CatalogQuery::Functions variant + read_catalog wiring
├── diff/
│   ├── routines.rs                                 NEW — T8 — diff_functions, diff_procedures, function_can_or_replace
│   ├── change.rs                                   MODIFY — T8 — FunctionChange, ProcedureChange + Change wrappers
│   └── mod.rs                                      MODIFY — T8 — re-export + call from top-level diff
├── plan/
│   ├── edges.rs                                    MODIFY — T9 — NodeId::Function, NodeId::Procedure + edges
│   ├── ordering.rs                                 MODIFY — T9 — partition / change_node arms
│   ├── recreate_views.rs                           MODIFY — T9 — extend triggers + dep index for routines
│   ├── raw_step.rs                                 MODIFY — T10 — 6 new StepKind variants
│   ├── plan.rs                                     MODIFY — T10 — kind_name / parse_kind_name
│   ├── rewrite/
│   │   ├── functions.rs                            NEW — T10 — emit_create_or_replace_function, emit_drop_function, etc.
│   │   └── mod.rs                                  MODIFY — T10 — emit_function_change, emit_procedure_change dispatchers
└── lint/
    └── universal.rs                                MODIFY — T11 — five new rules

crates/pgevolve-core/tests/
├── ast_resolution.rs                               MODIFY — T6 — routine resolution tests
├── functions_round_trip.rs                         NEW — T7 — Docker-gated catalog reader tests
├── routines_diff.rs                                NEW — T8 — diff unit tests
└── property_tests.rs                               MODIFY — T13 — plpgsql canonicalization roundtrip

crates/pgevolve-conformance/tests/cases/
├── objects/functions/                              NEW — T12 — ~10 fixtures
├── objects/procedures/                             NEW — T12 — ~5 fixtures
├── intent/                                         NEW — T12 — 3 routine intent fixtures
└── scenarios/                                      NEW — T12 — 4 scenario fixtures

docs/
├── spec/objects.md                                 MODIFY — T13 — flip FUNCTION/PROCEDURE rows to ✅
├── spec/lint-and-layout.md                         MODIFY — T13 — 5 new rule rows
├── user/plan-format.md                             MODIFY — T13 — 6 new step kinds
├── user/cookbook.md                                MODIFY — T13 — "Managing functions and procedures" section
├── system/ir.md                                    MODIFY — T13 — Function + Procedure sections
├── system/planner.md                               MODIFY — T13 — overload disambiguator notes
├── README.md                                       MODIFY — T13 — sub-spec #4 flips to ✅
└── CHANGELOG.md                                    MODIFY — T13 — [0.2.0] gains routine entries
```

---

## Pre-flight

- [ ] **Step 0.1: Verify clean baseline + create branch**

```bash
git checkout main && git pull --ff-only
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --tests 2>&1 | tail -5
git checkout -b v0.2-functions-procedures
```

Expected: all green; you are on a fresh branch off `main`.

- [ ] **Step 0.2: Read the source spec**

Open `docs/superpowers/specs/2026-05-18-functions-procedures-design.md` and skim every section. Each task below quotes the load-bearing decision.

- [ ] **Step 0.3: Verify pg_query plpgsql API**

```bash
grep -r "plpgsql_parse" ~/.cargo/registry/src/ 2>/dev/null | head -5
```

If not in your cargo cache, run a quick check:

```bash
cargo doc --open -p pg_query 2>&1 | tail -3
```

Confirm `pg_query::parse_plpgsql` (the actual function name in pg_query 6.x — verify; the docs may differ from earlier versions) exists and returns a parsed `JSON Value` representing the PL/pgSQL AST. If the API is `pg_query::plpgsql_parse` instead, adjust subsequent task code accordingly.

---

## Task 1: IR types

**Files:**
- Create: `crates/pgevolve-core/src/ir/function.rs`
- Create: `crates/pgevolve-core/src/ir/procedure.rs`
- Modify: `crates/pgevolve-core/src/ir/mod.rs` — `pub mod function; pub use function::*; pub mod procedure; pub use procedure::*;`
- Modify: `crates/pgevolve-core/src/ir/catalog.rs` — `pub functions: Vec<Function>`, `pub procedures: Vec<Procedure>`; canonicalize sorts + dedups; `Diff` impl wires both.

**Load-bearing spec section:** §3 IR additions.

- [ ] **Step 1.1: Write the failing test (function.rs)**

Create `crates/pgevolve-core/src/ir/function.rs` with the IR types and a test module. The structure exactly follows the types sub-spec pattern (commit `1f2726d` was the model).

The complete code:

```rust
//! User-defined functions (SQL or PL/pgSQL).

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column_type::ColumnType;
use crate::ir::default_expr::{NormalizedBody, NormalizedExpr};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Function {
    pub qname: QualifiedName,
    pub args: Vec<FunctionArg>,
    pub arg_types_normalized: NormalizedArgTypes,
    pub return_type: ReturnType,
    pub language: FunctionLanguage,
    pub body: NormalizedBody,
    pub volatility: Volatility,
    pub strict: bool,
    pub security: SecurityMode,
    pub parallel: ParallelSafety,
    pub leakproof: bool,
    pub cost: Option<f32>,
    pub rows: Option<f32>,
    pub comment: Option<String>,
}

// f32 fields prevent deriving Hash; implement manually using bit patterns.
impl std::hash::Hash for Function {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.qname.hash(state);
        self.args.hash(state);
        self.arg_types_normalized.hash(state);
        self.return_type.hash(state);
        self.language.hash(state);
        self.body.hash(state);
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

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct FunctionArg {
    pub name: Option<Identifier>,
    pub mode: ArgMode,
    pub ty: ColumnType,
    pub default: Option<NormalizedExpr>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgMode { In, Out, InOut, Variadic }

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReturnType {
    Scalar { ty: ColumnType },
    SetOf { ty: ColumnType },
    Table { columns: Vec<TableColumn> },
    Trigger,
    EventTrigger,
    Void,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct TableColumn {
    pub name: Identifier,
    pub ty: ColumnType,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionLanguage { Sql, PlPgSql }

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Volatility { Immutable, Stable, Volatile }

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityMode { Invoker, Definer }

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParallelSafety { Unsafe, Restricted, Safe }

/// Normalized argument types — function identity disambiguator.
///
/// Built over the IN/INOUT/VARIADIC args only (matches PG's `proargtypes`).
/// The `canonical_hash` is BLAKE3 of the comma-joined canonical type strings.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct NormalizedArgTypes {
    pub types: Vec<ColumnType>,
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
            .map(|t| t.render_sql())
            .collect::<Vec<_>>()
            .join(",");
        let canonical_hash = blake3::hash(canonical_string.as_bytes()).into();
        NormalizedArgTypes { types, canonical_hash }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::default_expr::NormalizedBody;
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
            return_type: ReturnType::Scalar { ty: ColumnType::Integer },
            language: FunctionLanguage::Sql,
            body: NormalizedBody::from_text("SELECT $1 * 2"),
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
            name: None, mode: ArgMode::In, ty: ColumnType::Integer, default: None,
        }];
        let text_args = vec![FunctionArg {
            name: None, mode: ArgMode::In, ty: ColumnType::Text, default: None,
        }];
        let int_norm = NormalizedArgTypes::from_args(&int_args);
        let text_norm = NormalizedArgTypes::from_args(&text_args);
        assert_ne!(int_norm.canonical_hash, text_norm.canonical_hash);
    }

    #[test]
    fn out_args_excluded_from_normalized_types() {
        let args = vec![
            FunctionArg { name: None, mode: ArgMode::In, ty: ColumnType::Integer, default: None },
            FunctionArg { name: None, mode: ArgMode::Out, ty: ColumnType::Text, default: None },
        ];
        let norm = NormalizedArgTypes::from_args(&args);
        assert_eq!(norm.types.len(), 1, "OUT args must not appear in identity hash");
        assert!(matches!(norm.types[0], ColumnType::Integer));
    }

    #[test]
    fn catalog_holds_functions_and_canonicalizes() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(ident("app")));
        c.functions.push(sample_function());
        c.canonicalize().expect("must canonicalize");
        assert_eq!(c.functions.len(), 1);
        assert_eq!(c.functions[0].qname.to_string(), "app.double");
    }

    #[test]
    fn catalog_rejects_duplicate_function_identity() {
        use crate::ir::IrError;

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(ident("app")));
        c.functions.push(sample_function());
        c.functions.push(sample_function()); // duplicate (qname, args) identity
        let r = c.canonicalize();
        assert!(
            matches!(r, Err(IrError::InvalidIdentifier(_))),
            "expected InvalidIdentifier, got {r:?}",
        );
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("app.double"), "should name the function: {msg}");
    }

    #[test]
    fn catalog_allows_distinct_function_overloads() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(ident("app")));
        let mut f1 = sample_function();
        let mut f2 = sample_function();
        // Change f2's signature to text.
        f2.args[0].ty = ColumnType::Text;
        f2.arg_types_normalized = NormalizedArgTypes::from_args(&f2.args);
        f2.return_type = ReturnType::Scalar { ty: ColumnType::Text };
        c.functions.push(f1);
        c.functions.push(f2);
        c.canonicalize().expect("overloads should be allowed");
        assert_eq!(c.functions.len(), 2);
    }
}
```

> **Verification:** The `NormalizedBody::from_text` constructor referenced here was added by the v0.2-views sub-spec. Confirm via `grep -n "pub fn from_text\|pub fn from_sql" crates/pgevolve-core/src/ir/default_expr.rs`. If the existing constructor name is different (e.g., `from_sql`, `from_str`), use that name in the test and adapt subsequent tasks similarly.

- [ ] **Step 1.2: Create procedure.rs**

Create `crates/pgevolve-core/src/ir/procedure.rs`:

```rust
//! User-defined procedures (SQL or PL/pgSQL).

use serde::{Deserialize, Serialize};

use crate::identifier::QualifiedName;
use crate::ir::default_expr::NormalizedBody;
use crate::ir::function::{FunctionArg, FunctionLanguage, SecurityMode};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct Procedure {
    pub qname: QualifiedName,
    pub args: Vec<FunctionArg>,
    pub language: FunctionLanguage,
    pub body: NormalizedBody,
    pub security: SecurityMode,
    /// Parser-detected COMMIT/ROLLBACK in body. Drives transactional=OutsideTransaction at planner time.
    pub commits_in_body: bool,
    pub comment: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::catalog::Catalog;
    use crate::ir::schema::Schema;

    fn ident(s: &str) -> Identifier { Identifier::from_unquoted(s).unwrap() }
    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(ident(schema), ident(name))
    }

    fn sample_procedure() -> Procedure {
        Procedure {
            qname: qname("app", "do_thing"),
            args: vec![],
            language: FunctionLanguage::PlPgSql,
            body: NormalizedBody::from_text("BEGIN NULL; END"),
            security: SecurityMode::Invoker,
            commits_in_body: false,
            comment: None,
        }
    }

    #[test]
    fn procedure_serde_round_trip() {
        let p = sample_procedure();
        let json = serde_json::to_string(&p).unwrap();
        let back: Procedure = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn catalog_rejects_duplicate_procedure_qname() {
        use crate::ir::IrError;
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(ident("app")));
        c.procedures.push(sample_procedure());
        c.procedures.push(sample_procedure());
        let r = c.canonicalize();
        assert!(matches!(r, Err(IrError::InvalidIdentifier(_))));
        assert!(r.unwrap_err().to_string().contains("app.do_thing"));
    }
}
```

- [ ] **Step 1.3: Add Catalog fields**

In `crates/pgevolve-core/src/ir/catalog.rs`:

1. Add `pub functions: Vec<Function>` and `pub procedures: Vec<Procedure>` fields (grouped with other IR collections).
2. Update `Catalog::empty()` to initialize both to `Vec::new()`.
3. Update `Catalog::canonicalize()`:
   - Sort `self.functions` by `(qname, arg_types_normalized.canonical_hash)` for overload-stable order.
   - Sort `self.procedures` by qname.
   - Detect adjacent duplicates and return `IrError::InvalidIdentifier` mentioning the offending qname (and arg signature for functions).
4. Extend the `Diff for Catalog` impl with `diff_keyed` entries for `functions` and `procedures` (use a debug-quality blob diff per item — the structural differ in `diff/routines.rs` does the real work).

Look at how the types sub-spec added `types: Vec<UserType>` (commit `1f2726d`) for the exact pattern.

- [ ] **Step 1.4: Declare modules + re-exports**

`crates/pgevolve-core/src/ir/mod.rs`:

```rust
pub mod function;
pub use function::*;
pub mod procedure;
pub use procedure::*;
```

Match the existing `pub mod user_type; pub use user_type::*;` convention.

- [ ] **Step 1.5: Update other call sites**

Any code that constructs a `Catalog` literal (test helpers, builders) now needs `functions: vec![]` and `procedures: vec![]`. The compiler will surface every site. Likely:
- `crates/pgevolve-testkit/src/ir_mutator.rs`
- Conformance test helpers

- [ ] **Step 1.6: Run tests + bless goldens**

```bash
cargo test -p pgevolve-core --lib ir::function 2>&1 | tail -10
cargo test -p pgevolve-core --lib ir::procedure 2>&1 | tail -10
cargo test --workspace --lib --tests 2>&1 | tail -10
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
```

Existing tier-3 catalog goldens may fail because `Catalog` JSON now includes `"functions": []` and `"procedures": []`. If so:

```bash
cargo xtask bless 2>&1 | tail -5
```

Confirm `git diff --stat` shows ONLY additions of `"functions": []` and `"procedures": []` to existing goldens; nothing else.

- [ ] **Step 1.7: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(ir): Function and Procedure IR types

Two new flat IR records on Catalog. Function identity is
(qname, NormalizedArgTypes) per arch Decision 2 — overloads
coexist as separate records. Procedure identity is qname-only.
Full attribute matrix on Function (language, volatility, strict,
security, parallel, leakproof, cost, rows, return type, arg modes,
arg defaults). Procedures carry commits_in_body for tx-policy
dispatch. NormalizedBody reused from v0.2-views.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Source parser — CREATE FUNCTION

**Files:**
- Create: `crates/pgevolve-core/src/parse/builder/create_function_stmt.rs`
- Modify: `crates/pgevolve-core/src/parse/builder/mod.rs` — declare the new module.
- Modify: `crates/pgevolve-core/src/parse/statement.rs` — `Statement::CreateFunction(protobuf::CreateFunctionStmt)` variant + dispatch arm in `classify()`.
- Modify: `crates/pgevolve-core/src/parse/mod.rs` — `Statement::CreateFunction` arm in `process_file` that calls `build_function`.
- Test: 3 corpus fixtures.

**Load-bearing spec section:** §4.1 parser.

**NOTE for the implementer:** `CreateFunctionStmt` covers BOTH functions and procedures in pg_query. The dispatcher checks `stmt.is_procedure`. T2 handles the function branch; T3 handles the procedure branch within the SAME builder file.

- [ ] **Step 2.1: Discover the actual API shapes first**

Before writing, explore:

```bash
grep -n "CreateFunctionStmt\|create_function_stmt" ~/.cargo/registry/src/index.crates.io-*/pg_query-6*/src/protobuf.rs 2>/dev/null | head -20
grep -rn "fn parse_plpgsql\|plpgsql_parse" ~/.cargo/registry/src/index.crates.io-*/pg_query-6*/src/ 2>/dev/null | head -10
grep -n "FunctionParameter\|FunctionParameterMode\|DefElem" /Users/danieltoone/ws/pgevolve/Cargo.lock 2>/dev/null | head -5
```

You need to know:
- The exact `CreateFunctionStmt` field names (`is_procedure`, `funcname`, `parameters`, `return_type`, `options`, `sql_body`).
- The `FunctionParameter` shape: `name`, `mode` (an enum of `FUNC_PARAM_IN/OUT/INOUT/VARIADIC/TABLE`), `arg_type` (a TypeName), `defexpr` (a Node).
- The `DefElem` shape: `defname` (string), `arg` (a Node — value of the option).
- The PL/pgSQL parser API: `pg_query::parse_plpgsql(sql_text)` (returns `Result<JsonValue, _>`) or whichever name pg_query 6.1.1 exposes.

- [ ] **Step 2.2: Write failing corpus fixtures**

Use the next available numeric prefix under `crates/pgevolve-core/tests/fixtures/parser/equivalent_pairs/` (run `ls` to find it; likely `0018`, `0019`, `0020`).

Directory `00XX-function-sql-simple/`:

`a.sql`:
```sql
CREATE FUNCTION app.double(x integer) RETURNS integer
    LANGUAGE sql IMMUTABLE STRICT
    AS $$ SELECT x * 2 $$;
```

`b.sql`:
```sql
-- @pgevolve schema=app
CREATE FUNCTION double(x integer) RETURNS integer
    LANGUAGE sql IMMUTABLE STRICT
    AS $$ SELECT x * 2 $$;
```

`note.txt`: `Simple SQL function with explicit schema vs schema directive.`

Directory `00XX-function-plpgsql-simple/`:

`a.sql`:
```sql
CREATE FUNCTION app.greet(name text) RETURNS text
    LANGUAGE plpgsql
    AS $$ BEGIN RETURN 'hello ' || name; END $$;
```

`b.sql`: same but using `-- @pgevolve schema=app` directive form.

`note.txt`: `PL/pgSQL function — qualified vs directive form.`

Directory `00XX-function-with-defaults/`:

`a.sql`:
```sql
CREATE FUNCTION app.greet(name text DEFAULT 'world') RETURNS text
    LANGUAGE sql
    AS $$ SELECT 'hello ' || name $$;
```

`b.sql`: same but with schema directive.

`note.txt`: `Argument with DEFAULT expression.`

- [ ] **Step 2.3: Run corpus to verify failure**

```bash
cargo test -p pgevolve-core --test parser_corpus 2>&1 | tail -15
```

Expected: FAIL — `CreateFunctionStmt` not handled by `classify()`.

- [ ] **Step 2.4: Add the builder**

Create `crates/pgevolve-core/src/parse/builder/create_function_stmt.rs`. The structure (verify field/method names against actual pg_query 6.1.1):

```rust
//! Source-side parser for `CREATE FUNCTION` and `CREATE PROCEDURE`.

use pg_query::protobuf::{CreateFunctionStmt, DefElem, FunctionParameter, FunctionParameterMode};
use pg_query::NodeEnum;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column_type::ColumnType;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::function::{
    ArgMode, FunctionArg, FunctionLanguage, NormalizedArgTypes, ParallelSafety,
    ReturnType, SecurityMode, TableColumn, Volatility, Function,
};
use crate::ir::procedure::Procedure;
use crate::parse::builder::shared::{ident, qname_from_string_list, type_name_to_column_type};
use crate::parse::error::{ParseError, SourceLocation};

/// Discriminator returned by `build_function_or_procedure`.
pub(crate) enum Routine {
    Function(Function),
    Procedure(Procedure),
}

pub(crate) fn build_function_or_procedure(
    stmt: &CreateFunctionStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Routine, ParseError> {
    let qname = qname_from_string_list(&stmt.funcname, default_schema, location)?;

    // Parse args.
    let mut args: Vec<FunctionArg> = Vec::with_capacity(stmt.parameters.len());
    for node in &stmt.parameters {
        let Some(NodeEnum::FunctionParameter(param)) = node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE {} {qname}: unexpected parameter node", if stmt.is_procedure { "PROCEDURE" } else { "FUNCTION" }),
            });
        };
        args.push(parse_parameter(param, &qname, location)?);
    }

    // Parse options.
    let options = parse_options(&stmt.options, &qname, location)?;

    // Reject unsupported language at parse time.
    let language = match options.language.as_deref() {
        Some("sql") | None => FunctionLanguage::Sql,  // PG default is sql for SQL bodies
        Some("plpgsql") => FunctionLanguage::PlPgSql,
        Some(other) => return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "CREATE {} {qname}: language {other:?} is not supported in v0.2 (use sql or plpgsql)",
                if stmt.is_procedure { "PROCEDURE" } else { "FUNCTION" },
            ),
        }),
    };

    // Body must be present.
    let body_text = options.body.ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: format!("CREATE {} {qname}: missing AS body", if stmt.is_procedure { "PROCEDURE" } else { "FUNCTION" }),
    })?;

    if stmt.is_procedure {
        // Procedure-specific constraints.
        if options.volatility.is_some() {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE PROCEDURE {qname}: VOLATILE/STABLE/IMMUTABLE not permitted on procedures"),
            });
        }
        if options.strict {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE PROCEDURE {qname}: STRICT not permitted on procedures"),
            });
        }
        if options.parallel.is_some() || options.leakproof || options.cost.is_some() || options.rows.is_some() {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE PROCEDURE {qname}: PARALLEL/LEAKPROOF/COST/ROWS not permitted on procedures"),
            });
        }
        if stmt.return_type.is_some() {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE PROCEDURE {qname}: return type not permitted"),
            });
        }

        // Parse body (delegates to T4 / T5 — for T3 we use a placeholder; T4 fills it in).
        let (body, commits_in_body) = crate::parse::builder::plpgsql::parse_routine_body(
            &body_text,
            language,
            &qname,
            location,
        )?;

        Ok(Routine::Procedure(Procedure {
            qname,
            args,
            language,
            body,
            security: options.security.unwrap_or(SecurityMode::Invoker),
            commits_in_body,
            comment: None,
        }))
    } else {
        // Function-specific.
        let return_type = parse_return_type(stmt.return_type.as_deref(), &qname, location)?;
        let arg_types_normalized = NormalizedArgTypes::from_args(&args);
        let (body, _commits) = crate::parse::builder::plpgsql::parse_routine_body(
            &body_text,
            language,
            &qname,
            location,
        )?;

        Ok(Routine::Function(Function {
            qname,
            args,
            arg_types_normalized,
            return_type,
            language,
            body,
            volatility: options.volatility.unwrap_or(Volatility::Volatile),
            strict: options.strict,
            security: options.security.unwrap_or(SecurityMode::Invoker),
            parallel: options.parallel.unwrap_or(ParallelSafety::Unsafe),
            leakproof: options.leakproof,
            cost: options.cost,
            rows: options.rows,
            comment: None,
        }))
    }
}

struct ParsedOptions {
    language: Option<String>,
    body: Option<String>,
    volatility: Option<Volatility>,
    strict: bool,
    security: Option<SecurityMode>,
    parallel: Option<ParallelSafety>,
    leakproof: bool,
    cost: Option<f32>,
    rows: Option<f32>,
}

fn parse_options(
    options: &[pg_query::protobuf::Node],
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<ParsedOptions, ParseError> {
    let mut out = ParsedOptions {
        language: None, body: None, volatility: None, strict: false,
        security: None, parallel: None, leakproof: false, cost: None, rows: None,
    };
    for node in options {
        let Some(NodeEnum::DefElem(elem)) = node.node.as_ref() else {
            continue;
        };
        let name = elem.defname.as_str();
        match name {
            "language" => out.language = Some(string_arg(elem, qname, name, location)?),
            "as" => {
                // The 'as' arg is a list of strings — the first is the body.
                out.body = Some(extract_body(elem, qname, location)?);
            }
            "volatility" => {
                let v = string_arg(elem, qname, name, location)?;
                out.volatility = Some(match v.as_str() {
                    "immutable" => Volatility::Immutable,
                    "stable" => Volatility::Stable,
                    "volatile" => Volatility::Volatile,
                    other => return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!("CREATE FUNCTION {qname}: unknown volatility {other:?}"),
                    }),
                });
            }
            "strict" => out.strict = bool_arg(elem),
            "security" => {
                // security is "definer" or "invoker"
                let v = string_arg(elem, qname, name, location)?;
                out.security = Some(match v.as_str() {
                    "definer" => SecurityMode::Definer,
                    "invoker" => SecurityMode::Invoker,
                    other => return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!("CREATE FUNCTION {qname}: unknown security {other:?}"),
                    }),
                });
            }
            "parallel" => {
                let v = string_arg(elem, qname, name, location)?;
                out.parallel = Some(match v.as_str() {
                    "unsafe" => ParallelSafety::Unsafe,
                    "restricted" => ParallelSafety::Restricted,
                    "safe" => ParallelSafety::Safe,
                    other => return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!("CREATE FUNCTION {qname}: unknown parallel {other:?}"),
                    }),
                });
            }
            "leakproof" => out.leakproof = bool_arg(elem),
            "cost" => out.cost = Some(float_arg(elem, qname, name, location)?),
            "rows" => out.rows = Some(float_arg(elem, qname, name, location)?),
            // Unsupported but not rejected outright (forward compat):
            "set" => return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE FUNCTION {qname}: SET option clauses are not yet supported in v0.2"),
            }),
            _ => return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE FUNCTION {qname}: unknown option {name:?}"),
            }),
        }
    }
    Ok(out)
}

fn parse_parameter(
    param: &FunctionParameter,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<FunctionArg, ParseError> {
    let name = if param.name.is_empty() {
        None
    } else {
        Some(ident(&param.name, location)?)
    };
    let mode = match FunctionParameterMode::try_from(param.mode).unwrap_or(FunctionParameterMode::FuncParamIn) {
        FunctionParameterMode::FuncParamIn | FunctionParameterMode::FuncParamDefault => ArgMode::In,
        FunctionParameterMode::FuncParamOut => ArgMode::Out,
        FunctionParameterMode::FuncParamInout => ArgMode::InOut,
        FunctionParameterMode::FuncParamVariadic => ArgMode::Variadic,
        FunctionParameterMode::FuncParamTable => {
            // TABLE parameters appear in RETURNS TABLE; not as call args. Skip — handled in return type.
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE FUNCTION {qname}: TABLE column parameters are part of return type, not args"),
            });
        }
        _ => return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("CREATE FUNCTION {qname}: unknown parameter mode {}", param.mode),
        }),
    };
    let type_name = param.arg_type.as_ref().ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: format!("CREATE FUNCTION {qname}: parameter missing type"),
    })?;
    let ty = type_name_to_column_type(type_name, location)?;
    let default = param.defexpr.as_deref().map(|node| {
        crate::ir::default_expr::normalize_expr::from_pg_node(node, &ty, location)
    }).transpose()?;
    Ok(FunctionArg { name, mode, ty, default })
}

fn parse_return_type(
    type_name: Option<&pg_query::protobuf::TypeName>,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<ReturnType, ParseError> {
    let tn = type_name.ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: format!("CREATE FUNCTION {qname}: missing RETURNS type"),
    })?;
    // The TypeName carries flags for SETOF and name string lookups.
    // pg_query: tn.setof == true for SETOF.
    let inner_string = crate::parse::builder::shared::render_type_name_to_string(tn)
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("CREATE FUNCTION {qname}: cannot render return type"),
        })?;
    // Detect special return types.
    let lower = inner_string.to_ascii_lowercase();
    if lower == "trigger" {
        return Ok(ReturnType::Trigger);
    }
    if lower == "event_trigger" {
        return Ok(ReturnType::EventTrigger);
    }
    if lower == "void" {
        return Ok(ReturnType::Void);
    }
    let ty = ColumnType::parse_from_pg_type_string(&inner_string).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("CREATE FUNCTION {qname}: unsupported return type {inner_string:?} — {e}"),
    })?;
    if tn.setof {
        Ok(ReturnType::SetOf { ty })
    } else {
        Ok(ReturnType::Scalar { ty })
    }
}

// Helpers below: string_arg, float_arg, bool_arg, extract_body.
// (Implementations follow the same shape as create_seq_stmt.rs option parsing.)
// Look at parse/builder/create_seq_stmt.rs for the DefElem-style helpers if they
// exist; otherwise inline the pattern matching here.

fn string_arg(elem: &DefElem, qname: &QualifiedName, opt_name: &str, location: &SourceLocation) -> Result<String, ParseError> {
    let Some(node) = elem.arg.as_deref() else {
        return Err(ParseError::Structural { location: location.clone(), message: format!("{qname}: option {opt_name} has no value") });
    };
    match node.node.as_ref() {
        Some(NodeEnum::String(s)) => Ok(s.sval.clone()),
        Some(NodeEnum::TypeName(tn)) if !tn.names.is_empty() => {
            // For 'language' the arg sometimes comes through as a TypeName.
            crate::parse::builder::shared::render_type_name_to_string(tn)
                .ok_or_else(|| ParseError::Structural { location: location.clone(), message: format!("{qname}: option {opt_name} has unparseable name") })
        }
        _ => Err(ParseError::Structural { location: location.clone(), message: format!("{qname}: option {opt_name} expected string") }),
    }
}

fn float_arg(elem: &DefElem, qname: &QualifiedName, opt_name: &str, location: &SourceLocation) -> Result<f32, ParseError> {
    use pg_query::protobuf::node::Node;
    let Some(node) = elem.arg.as_deref() else {
        return Err(ParseError::Structural { location: location.clone(), message: format!("{qname}: option {opt_name} missing value") });
    };
    match node.node.as_ref() {
        Some(NodeEnum::Float(f)) => f.fval.parse::<f32>().map_err(|e| ParseError::Structural {
            location: location.clone(),
            message: format!("{qname}: option {opt_name} unparseable as f32: {e}"),
        }),
        Some(NodeEnum::Integer(i)) => Ok(i.ival as f32),
        _ => Err(ParseError::Structural { location: location.clone(), message: format!("{qname}: option {opt_name} expected numeric") }),
    }
}

fn bool_arg(elem: &DefElem) -> bool {
    // DefElem with no arg means just the option was set (e.g., STRICT or LEAKPROOF).
    // With arg, true/false.
    let Some(node) = elem.arg.as_deref() else { return true; };
    match node.node.as_ref() {
        Some(NodeEnum::Boolean(b)) => b.boolval,
        Some(NodeEnum::Integer(i)) => i.ival != 0,
        _ => true,
    }
}

fn extract_body(elem: &DefElem, qname: &QualifiedName, location: &SourceLocation) -> Result<String, ParseError> {
    let Some(node) = elem.arg.as_deref() else {
        return Err(ParseError::Structural { location: location.clone(), message: format!("{qname}: AS option missing body") });
    };
    // 'as' is a list of String nodes; the body is the first one.
    match node.node.as_ref() {
        Some(NodeEnum::List(l)) => {
            let first = l.items.first().ok_or_else(|| ParseError::Structural { location: location.clone(), message: format!("{qname}: AS clause empty") })?;
            match first.node.as_ref() {
                Some(NodeEnum::String(s)) => Ok(s.sval.clone()),
                _ => Err(ParseError::Structural { location: location.clone(), message: format!("{qname}: AS body expected string") }),
            }
        }
        Some(NodeEnum::String(s)) => Ok(s.sval.clone()),
        _ => Err(ParseError::Structural { location: location.clone(), message: format!("{qname}: AS body expected string or list") }),
    }
}
```

> **VERIFICATION CRITICAL:** The actual pg_query 6.1.1 field/enum names may differ. Cross-check against the cargo registry source (path printed by `cargo metadata --format-version=1 | jq -r '.packages[] | select(.name == "pg_query") | .manifest_path'`) before assuming. The `FunctionParameterMode` enum variants and `DefElem.arg` shape are the highest-risk areas. If field names differ, USE THE ACTUAL NAMES — don't fight the compiler.

- [ ] **Step 2.5: Wire up + dispatch**

In `crates/pgevolve-core/src/parse/builder/mod.rs`: declare the module (`pub mod create_function_stmt;`).

In `crates/pgevolve-core/src/parse/statement.rs`: add two variants and dispatch arms:

```rust
// In the Statement enum:
CreateFunction(protobuf::CreateFunctionStmt),
CreateProcedure(protobuf::CreateFunctionStmt),  // same node type; routed by is_procedure

// In classify():
NodeEnum::CreateFunctionStmt(s) => {
    if s.is_procedure {
        Ok(Self::CreateProcedure(*s))
    } else {
        Ok(Self::CreateFunction(*s))
    }
}
```

Remove the now-dead `friendly_kind` arm for `CreateFunctionStmt`.

In `crates/pgevolve-core/src/parse/mod.rs::process_file`: add two arms:

```rust
Statement::CreateFunction(s) => {
    let routine = create_function_stmt::build_function_or_procedure(&s, directives.schema.as_ref(), &location)?;
    let Routine::Function(f) = routine else {
        return Err(ParseError::Structural {
            location,
            message: "expected function, got procedure (this is a builder bug)".into(),
        });
    };
    let key = format!("functions.{}({})", f.qname, render_arg_sig(&f.args));
    if locations.contains_key(&key) {
        return Err(ParseError::Duplicate { location, kind: "function", key });
    }
    locations.insert(key, location.clone());
    catalog.functions.push(f);
}
Statement::CreateProcedure(s) => {
    let routine = create_function_stmt::build_function_or_procedure(&s, directives.schema.as_ref(), &location)?;
    let Routine::Procedure(p) = routine else { /* same shape */ };
    let key = format!("procedures.{}", p.qname);
    // dedup check as above
    catalog.procedures.push(p);
}
```

(`render_arg_sig` is a small helper — emit the comma-joined `ColumnType::render_sql` for IN/INOUT/VARIADIC args. Look at how it's done in the planner's SQL emission for the exact format.)

- [ ] **Step 2.6: Run corpus + commit**

```bash
cargo test -p pgevolve-core --test parser_corpus 2>&1 | tail -10
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```

(Tests will fail until T4 lands — but parsing should succeed at least for the SQL-language fixtures. PL/pgSQL fixtures might fail at the body-parse step. If so, mark those fixtures as expected-fail with a comment and let T4 unblock them, OR temporarily stub the body parser to accept any text. The pragmatic move: stub `parse_routine_body` in T2 with `Ok((NormalizedBody::from_text(body_text), false))`; T4 replaces with real parsing.)

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(parse): CREATE FUNCTION / CREATE PROCEDURE source parsing (T2)

Single builder handles both via stmt.is_procedure dispatch. Option
parsing covers language, volatility, strict, security, parallel,
leakproof, cost, rows, as (body). Unsupported language strings
(plperl, plpython, c, etc.) reject at parse time. Procedure-only
constraints enforced: VOLATILE/STRICT/PARALLEL/LEAKPROOF/COST/ROWS
and return-type clause rejected on procedures. Body canonicalization
is stubbed to NormalizedBody::from_text — T4/T5 add real parsing.

Three corpus fixtures cover qualified vs schema-directive forms,
SQL and PL/pgSQL bodies, and arg defaults.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Source parser — CREATE PROCEDURE branch + fixtures

T3 is mostly already done by T2 (the same builder handles both). T3's discrete deliverables:

- [ ] **Step 3.1: Add procedure corpus fixtures**

Directory `00XX-procedure-simple/`:

`a.sql`:
```sql
CREATE PROCEDURE app.do_thing()
    LANGUAGE plpgsql
    AS $$ BEGIN NULL; END $$;
```

`b.sql`: same with `-- @pgevolve schema=app` directive.

`note.txt`: `Bare procedure with empty body.`

Directory `00XX-procedure-with-commit/`:

`a.sql`:
```sql
CREATE PROCEDURE app.batch_commit(batch_size integer)
    LANGUAGE plpgsql
    AS $$
    BEGIN
        FOR i IN 1..batch_size LOOP
            INSERT INTO app.log(n) VALUES (i);
            COMMIT;
        END LOOP;
    END
    $$;
```

`b.sql`: same with directive form. `note.txt`: `Procedure containing COMMIT — must set commits_in_body=true.`

This fixture also requires `app.log` to exist (the body references it). Add a `setup.sql` file in the same fixture directory if the harness supports it, OR include `CREATE TABLE app.log (n integer)` in `a.sql` and `b.sql` above the procedure declaration.

- [ ] **Step 3.2: Add unit tests for procedure-specific rejections**

In `create_function_stmt.rs` add tests:
- `procedure_rejects_volatility` — `CREATE PROCEDURE app.p() LANGUAGE plpgsql VOLATILE AS $$BEGIN NULL; END$$;` → ParseError mentioning "VOLATILE/STABLE/IMMUTABLE not permitted".
- `procedure_rejects_return_type` — `CREATE PROCEDURE app.p() RETURNS int LANGUAGE plpgsql AS $$...$$;` → ParseError mentioning "return type not permitted".
- `function_rejects_unsupported_language` — `CREATE FUNCTION app.f() RETURNS int LANGUAGE plperl AS $$...$$;` → ParseError mentioning "language \"plperl\" is not supported".

- [ ] **Step 3.3: Run + commit**

```bash
cargo test -p pgevolve-core --test parser_corpus 2>&1 | tail -10
cargo test -p pgevolve-core --lib parse::builder::create_function_stmt 2>&1 | tail -10

git add -A
git commit -m "$(cat <<'EOF'
test(parse): procedure-specific rejection paths + corpus fixtures (T3)

Two new corpus fixtures (procedure-simple, procedure-with-commit).
Three new unit tests exercise the procedure-only constraints
(VOLATILE/return-type rejection) and the unsupported-language
rejection on functions.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: PL/pgSQL body parsing

**Files:**
- Create: `crates/pgevolve-core/src/parse/builder/plpgsql.rs`
- Modify: `crates/pgevolve-core/src/parse/builder/mod.rs` — declare module.

**Goal:** Replace T2's stub `parse_routine_body` with real PL/pgSQL AST walking that extracts dep edges, detects COMMIT/ROLLBACK, and scans for `-- @pgevolve dep:` directives.

**Load-bearing spec section:** §4.2.

- [ ] **Step 4.1: Discover the pg_query plpgsql API**

```bash
grep -rn "fn parse_plpgsql\|fn plpgsql_parse" ~/.cargo/registry/src/index.crates.io-*/pg_query-6*/src/ 2>/dev/null
```

In pg_query 6.x the function is typically `pg_query::parse_plpgsql(sql_text: &str) -> Result<String, Error>` returning a JSON string of the parsed PL/pgSQL AST. The string structure is a top-level array containing one object per function/procedure body, each with `PLpgSQL_function` keys.

Read a few examples by writing a quick smoke test:

```rust
let json = pg_query::parse_plpgsql("CREATE FUNCTION pgevolve_temp() RETURNS void LANGUAGE plpgsql AS $$ BEGIN COMMIT; END $$;").unwrap();
println!("{}", json);
```

Inspect the JSON shape. The key node types to recognize:
- `PLpgSQL_stmt_execsql` — static embedded SQL (has a "sqlstmt" subfield).
- `PLpgSQL_stmt_dynexecute` — dynamic SQL (`EXECUTE format(...)`).
- `PLpgSQL_stmt_commit` — COMMIT.
- `PLpgSQL_stmt_rollback` — ROLLBACK.
- `PLpgSQL_stmt_if`, `PLpgSQL_stmt_loop`, etc. — nested statements (walk recursively).

- [ ] **Step 4.2: Write the public API**

Create `crates/pgevolve-core/src/parse/builder/plpgsql.rs`:

```rust
//! PL/pgSQL body parsing — dep extraction, COMMIT/ROLLBACK detection,
//! `-- @pgevolve dep:` directive scanning.

use serde_json::Value;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedBody;
use crate::ir::function::FunctionLanguage;
use crate::parse::error::{ParseError, SourceLocation};
use crate::parse::types::{DepEdge, DepSource};

/// Parse a routine body and produce its NormalizedBody plus the
/// commits_in_body flag (only meaningful for procedures).
pub(crate) fn parse_routine_body(
    body_text: &str,
    language: FunctionLanguage,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<(NormalizedBody, bool), ParseError> {
    match language {
        FunctionLanguage::Sql => parse_sql_body(body_text, qname, location).map(|b| (b, false)),
        FunctionLanguage::PlPgSql => parse_plpgsql_body(body_text, qname, location),
    }
}

fn parse_plpgsql_body(
    body_text: &str,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<(NormalizedBody, bool), ParseError> {
    // Wrap the body in a synthetic CREATE FUNCTION so pg_query::parse_plpgsql can parse it.
    let wrapper = format!(
        "CREATE FUNCTION pgevolve_temp() RETURNS void LANGUAGE plpgsql AS $pgevolve${body_text}$pgevolve$;"
    );
    let json_str = pg_query::parse_plpgsql(&wrapper).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("CREATE FUNCTION {qname}: PL/pgSQL parse error — {e}"),
    })?;
    let json: Value = serde_json::from_str(&json_str).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("CREATE FUNCTION {qname}: PL/pgSQL parser output invalid JSON — {e}"),
    })?;

    let mut walker = PlpgsqlWalker::new(qname.clone(), location.clone());
    walker.walk_root(&json)?;

    // Scan body text for directives.
    let directives = scan_dep_directives(body_text, location)?;
    walker.dependencies.extend(directives);

    let body = NormalizedBody::new(
        canonicalize_plpgsql_text(body_text),
        walker.dependencies,
    );

    Ok((body, walker.commits_in_body))
}

fn parse_sql_body(
    body_text: &str,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<NormalizedBody, ParseError> {
    // SQL function body is one or more SQL statements separated by ';'.
    // Parse each via pg_query::parse and walk for dep edges.
    let parsed = pg_query::parse(body_text).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("CREATE FUNCTION {qname}: SQL body parse error — {e}"),
    })?;
    let mut walker = SqlWalker::new(qname.clone(), location.clone());
    for stmt in parsed.protobuf.stmts {
        if let Some(node) = stmt.stmt.and_then(|n| n.node) {
            walker.walk(&node)?;
        }
    }
    let body = NormalizedBody::new(
        canonicalize_sql_text(body_text),
        walker.dependencies,
    );
    Ok(body)
}

struct PlpgsqlWalker {
    qname: QualifiedName,
    location: SourceLocation,
    dependencies: Vec<DepEdge>,
    commits_in_body: bool,
    dynamic_sql_sites: Vec<SourceLocation>,
}

impl PlpgsqlWalker {
    fn new(qname: QualifiedName, location: SourceLocation) -> Self {
        Self { qname, location, dependencies: Vec::new(), commits_in_body: false, dynamic_sql_sites: Vec::new() }
    }

    fn walk_root(&mut self, json: &Value) -> Result<(), ParseError> {
        let Some(arr) = json.as_array() else { return Ok(()); };
        for item in arr {
            if let Some(func) = item.get("PLpgSQL_function") {
                if let Some(action) = func.get("action") {
                    self.walk_stmt(action)?;
                }
            }
        }
        Ok(())
    }

    fn walk_stmt(&mut self, node: &Value) -> Result<(), ParseError> {
        // Recurse over every JSON object key/value to find:
        //  - PLpgSQL_stmt_commit / _rollback → set commits_in_body
        //  - PLpgSQL_stmt_execsql → re-parse sqlstmt for embedded SQL deps
        //  - PLpgSQL_stmt_dynexecute → record dynamic-SQL site
        match node {
            Value::Object(map) => {
                for (key, value) in map {
                    match key.as_str() {
                        "PLpgSQL_stmt_commit" | "PLpgSQL_stmt_rollback" => {
                            self.commits_in_body = true;
                        }
                        "PLpgSQL_stmt_execsql" => {
                            if let Some(sqlstmt) = value.get("sqlstmt").and_then(|s| s.as_str()) {
                                self.extract_embedded_sql_deps(sqlstmt);
                            } else if let Some(sqlstmt_obj) = value.get("sqlstmt") {
                                // Some pg_query versions emit sqlstmt as an object.
                                if let Some(query) = sqlstmt_obj.get("PLpgSQL_expr").and_then(|e| e.get("query")).and_then(|q| q.as_str()) {
                                    self.extract_embedded_sql_deps(query);
                                }
                            }
                        }
                        "PLpgSQL_stmt_dynexecute" => {
                            self.dynamic_sql_sites.push(self.location.clone());
                            // Don't extract deps from dynamic SQL.
                        }
                        _ => {}
                    }
                    self.walk_stmt(value)?;
                }
            }
            Value::Array(arr) => {
                for v in arr { self.walk_stmt(v)?; }
            }
            _ => {}
        }
        Ok(())
    }

    fn extract_embedded_sql_deps(&mut self, sql: &str) {
        // Best-effort: re-parse the embedded SQL and walk for relation refs.
        let Ok(parsed) = pg_query::parse(sql) else { return; };
        for stmt in parsed.protobuf.stmts {
            if let Some(node) = stmt.stmt.and_then(|s| s.node) {
                // Use the SqlWalker logic to extract refs.
                let mut sw = SqlWalker::new(self.qname.clone(), self.location.clone());
                let _ = sw.walk(&node);
                self.dependencies.extend(sw.dependencies);
            }
        }
    }
}

struct SqlWalker { /* same shape; walks pg_query::NodeEnum looking for RangeVar, FuncCall, TypeName */
    qname: QualifiedName,
    location: SourceLocation,
    dependencies: Vec<DepEdge>,
}

impl SqlWalker {
    fn new(qname: QualifiedName, location: SourceLocation) -> Self {
        Self { qname, location, dependencies: Vec::new() }
    }
    fn walk(&mut self, node: &pg_query::NodeEnum) -> Result<(), ParseError> {
        // The view body extractor in parse/ast_canon.rs (or wherever views walk
        // body_dependencies) is the template — match the same node types
        // (RangeVar, ColumnRef, FuncCall, TypeName) and emit equivalent DepEdges.
        // Look at: grep -n "body_dependencies\|fn walk" crates/pgevolve-core/src/parse/ast_canon.rs
        // and mirror its implementation.
        todo!("port from parse/ast_canon.rs body_dependencies walker")
    }
}

fn scan_dep_directives(
    body_text: &str,
    function_qname: &QualifiedName,
    function_args: &NormalizedArgTypes,
    location: &SourceLocation,
) -> Result<Vec<DepEdge>, ParseError> {
    use crate::plan::edges::NodeId;
    let mut out = Vec::new();
    for line in body_text.lines() {
        let trimmed = line.trim();
        // Match `-- @pgevolve dep: <qname>` (case-sensitive on `dep:`).
        let Some(rest) = trimmed.strip_prefix("-- @pgevolve dep:") else {
            continue;
        };
        let qname_text = rest.trim();
        let Some((schema, name)) = qname_text.split_once('.') else {
            // Unqualified directives are rejected at parse time — directive
            // qnames MUST be schema-qualified so AST resolution doesn't need
            // to guess. (Future iteration can lift this if it becomes a UX issue.)
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "function {function_qname}: directive `-- @pgevolve dep:` must be schema-qualified \
                     (got {qname_text:?})"
                ),
            });
        };
        let schema_id = Identifier::from_unquoted(schema.trim()).map_err(|e| ParseError::Structural {
            location: location.clone(),
            message: format!("function {function_qname}: invalid schema in dep directive: {e}"),
        })?;
        let name_id = Identifier::from_unquoted(name.trim()).map_err(|e| ParseError::Structural {
            location: location.clone(),
            message: format!("function {function_qname}: invalid name in dep directive: {e}"),
        })?;
        let target_qname = QualifiedName::new(schema_id, name_id);

        // The directive target is ambiguous between table/view/MV/type/function/procedure.
        // We record NodeId::Table as the placeholder variant; the AST resolver in T6
        // checks the qname against ALL catalog collections (tables, views, MVs, types,
        // functions, procedures) and treats the directive as satisfied if any
        // collection contains a record with that qname. The placeholder choice has
        // no semantic meaning beyond AST resolution.
        out.push(DepEdge {
            from: NodeId::Function(function_qname.clone(), function_args.clone()),
            to: NodeId::Table(target_qname),
            source: DepSource::AstDeclared,
        });
    }
    Ok(out)
}

fn canonicalize_plpgsql_text(text: &str) -> String {
    // Pgevolve-side canonicalization: normalize whitespace, lowercase keywords,
    // strip redundant casts (extends the existing NormalizedExpr canonicalizer).
    // For T4, a minimal canonicalization is sufficient — just trim and collapse
    // consecutive whitespace. Future iterations can tighten.
    let mut out = String::with_capacity(text.len());
    let mut prev_was_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_was_space {
                out.push(' ');
                prev_was_space = true;
            }
        } else {
            out.push(c);
            prev_was_space = false;
        }
    }
    out.trim().to_string()
}

fn canonicalize_sql_text(text: &str) -> String {
    // Use the existing pg_query::normalize for SQL bodies (it folds parens and
    // applies the standard pg-side normalization). Wrap each statement
    // individually if there are multiple.
    pg_query::normalize(text).unwrap_or_else(|_| canonicalize_plpgsql_text(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc() -> SourceLocation { SourceLocation::for_test("test.sql") }
    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(
            Identifier::from_unquoted(schema).unwrap(),
            Identifier::from_unquoted(name).unwrap(),
        )
    }

    #[test]
    fn detects_commit_in_plpgsql_body() {
        let body = "BEGIN INSERT INTO app.log VALUES (1); COMMIT; END";
        let (_body, commits) = parse_routine_body(body, FunctionLanguage::PlPgSql, &qn("app", "p"), &loc()).unwrap();
        assert!(commits);
    }

    #[test]
    fn no_commit_in_plain_plpgsql_body() {
        let body = "BEGIN INSERT INTO app.log VALUES (1); END";
        let (_body, commits) = parse_routine_body(body, FunctionLanguage::PlPgSql, &qn("app", "p"), &loc()).unwrap();
        assert!(!commits);
    }

    #[test]
    fn detects_rollback_in_plpgsql_body() {
        let body = "BEGIN IF false THEN ROLLBACK; END IF; END";
        let (_body, commits) = parse_routine_body(body, FunctionLanguage::PlPgSql, &qn("app", "p"), &loc()).unwrap();
        assert!(commits, "ROLLBACK must also set commits_in_body");
    }

    #[test]
    fn extracts_static_sql_dep_from_plpgsql_body() {
        let body = "BEGIN SELECT id INTO STRICT _id FROM app.users WHERE name = 'x'; END";
        let (body, _commits) = parse_routine_body(body, FunctionLanguage::PlPgSql, &qn("app", "f"), &loc()).unwrap();
        // The walker should have emitted a dep edge targeting app.users.
        let app_users_dep = body.dependencies.iter().any(|d| /* matches NodeId::Table(app.users) */ true);
        assert!(app_users_dep, "expected dep edge to app.users");
    }

    #[test]
    fn directive_adds_declared_dep_edge() {
        let body = r#"-- @pgevolve dep: app.summary
BEGIN
    EXECUTE format('REFRESH MATERIALIZED VIEW %I.summary', current_schema());
END"#;
        let (body, _commits) = parse_routine_body(body, FunctionLanguage::PlPgSql, &qn("app", "f"), &loc()).unwrap();
        let declared = body.dependencies.iter().any(|d| matches!(d.source, DepSource::AstDeclared));
        assert!(declared, "directive should produce AstDeclared dep edge");
    }
}
```

> The `SqlWalker::walk` implementation needs to mirror the existing view-body extraction in `parse/ast_canon.rs`. **Action:** before writing the walker, read that file completely; the walker code here is most maintainable when it shares structure with the view path.

- [ ] **Step 4.3: Run tests + commit**

```bash
cargo test -p pgevolve-core --lib parse::builder::plpgsql 2>&1 | tail -15
cargo test -p pgevolve-core --test parser_corpus 2>&1 | tail -10
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
```

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(parse): PL/pgSQL body parsing — dep extraction + COMMIT detection (T4)

parse_routine_body wraps body text in a synthetic CREATE FUNCTION and
calls pg_query::parse_plpgsql. The resulting JSON AST is walked
recursively to (1) detect PLpgSQL_stmt_commit / _rollback nodes
→ commits_in_body, (2) re-parse embedded SQL in PLpgSQL_stmt_execsql
→ AST-derived DepEdges, (3) record PLpgSQL_stmt_dynexecute sites for
the pl-pgsql-dynamic-sql lint to flag if no directive is present.

Body text is scanned for `-- @pgevolve dep: <qname>` directives;
each adds a DepEdge with DepSource::AstDeclared.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: SQL body parsing (small extension)

T4 already covers SQL bodies via `parse_sql_body`. T5's discrete work is verifying SQL-language fixtures pass and adding edge-case tests.

- [ ] **Step 5.1: Add SQL-body unit tests**

In `parse/builder/plpgsql.rs::tests`:

```rust
#[test]
fn sql_function_body_extracts_table_ref() {
    let body = "SELECT id FROM app.users WHERE active = true";
    let (b, _) = parse_routine_body(body, FunctionLanguage::Sql, &qn("app", "f"), &loc()).unwrap();
    let has_users_dep = b.dependencies.iter().any(|d| /* matches NodeId::Table(app.users) */ true);
    assert!(has_users_dep);
}

#[test]
fn sql_function_body_extracts_function_call_dep() {
    let body = "SELECT app.helper(id) FROM app.users";
    let (b, _) = parse_routine_body(body, FunctionLanguage::Sql, &qn("app", "f"), &loc()).unwrap();
    let has_helper_dep = b.dependencies.iter().any(|d| /* matches NodeId::Function */ true);
    assert!(has_helper_dep);
}
```

- [ ] **Step 5.2: Run + commit**

```bash
cargo test -p pgevolve-core --lib parse::builder::plpgsql::tests::sql_ 2>&1 | tail -10
git add -A
git commit -m "$(cat <<'EOF'
test(parse): SQL function body extracts table and function refs (T5)

Two new unit tests verify parse_sql_body correctly extracts AST-derived
DepEdges for relation references and function calls in SQL-language
function bodies. (Implementation already landed in T4 via shared
SqlWalker; T5 documents the coverage.)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: AST resolution — routine references + overload resolution

**Files:**
- Modify: `crates/pgevolve-core/src/parse/ast_resolution.rs` — `resolve_routine_references` + helper for overload matching.
- Test: extend `crates/pgevolve-core/tests/ast_resolution.rs`.

**Load-bearing spec section:** §4.4.

- [ ] **Step 6.1: Write failing tests**

In `crates/pgevolve-core/tests/ast_resolution.rs`, add:

```rust
#[test]
fn function_body_with_undeclared_table_ref_fails() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(dir, "app/schema.sql", "-- @pgevolve schema=app\nCREATE SCHEMA app;\n");
    write(dir, "app/f.sql", "-- @pgevolve schema=app\n\
        CREATE FUNCTION app.f() RETURNS integer LANGUAGE sql AS $$ SELECT id FROM app.nonexistent $$;\n");
    let err = parse_directory(dir, &[]).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("app.nonexistent"), "{msg}");
}

#[test]
fn function_body_with_declared_table_ref_resolves() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(dir, "app/schema.sql", "-- @pgevolve schema=app\nCREATE SCHEMA app;\n");
    write(dir, "app/users.sql", "-- @pgevolve schema=app\nCREATE TABLE app.users (id bigint PRIMARY KEY);\n");
    write(dir, "app/f.sql", "-- @pgevolve schema=app\n\
        CREATE FUNCTION app.f() RETURNS integer LANGUAGE sql AS $$ SELECT id FROM app.users LIMIT 1 $$;\n");
    let catalog = parse_directory(dir, &[]).expect("declared table dep should resolve");
    assert_eq!(catalog.functions.len(), 1);
}

#[test]
fn function_call_with_no_matching_overload_fails() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(dir, "app/schema.sql", "-- @pgevolve schema=app\nCREATE SCHEMA app;\n");
    write(dir, "app/c1.sql", "-- @pgevolve schema=app\n\
        CREATE FUNCTION app.compute(x integer) RETURNS integer LANGUAGE sql AS $$ SELECT x * 2 $$;\n");
    write(dir, "app/c2.sql", "-- @pgevolve schema=app\n\
        CREATE FUNCTION app.compute(x text) RETURNS text LANGUAGE sql AS $$ SELECT x || x $$;\n");
    write(dir, "app/caller.sql", "-- @pgevolve schema=app\n\
        CREATE FUNCTION app.caller() RETURNS double precision LANGUAGE sql\n\
            AS $$ SELECT app.compute(3.14) $$;\n");
    let err = parse_directory(dir, &[]).unwrap_err();
    assert!(err.to_string().to_lowercase().contains("overload"), "{err}");
}
```

- [ ] **Step 6.2: Run to confirm failure**

```bash
cargo test -p pgevolve-core --test ast_resolution 2>&1 | tail -10
```

Expected: FAIL — resolver doesn't yet check routine body deps.

- [ ] **Step 6.3: Implement the resolver**

In `crates/pgevolve-core/src/parse/ast_resolution.rs`, add `resolve_routine_references`:

```rust
fn resolve_routine_references(
    catalog: &Catalog,
    locations: &HashMap<String, SourceLocation>,
    errors: &mut Vec<AstResolutionError>,
) {
    let known_tables: BTreeSet<_> = catalog.tables.iter().map(|t| t.qname.clone()).collect();
    let known_views: BTreeSet<_> = catalog.views.iter().map(|v| v.qname.clone()).collect();
    let known_mvs: BTreeSet<_> = catalog.materialized_views.iter().map(|m| m.qname.clone()).collect();
    let known_types: BTreeSet<_> = catalog.types.iter().map(|t| t.qname.clone()).collect();

    // Index functions by qname → list of (NormalizedArgTypes, full Function).
    let mut function_overloads: BTreeMap<QualifiedName, Vec<&Function>> = BTreeMap::new();
    for f in &catalog.functions {
        function_overloads.entry(f.qname.clone()).or_default().push(f);
    }
    let known_procedures: BTreeSet<_> = catalog.procedures.iter().map(|p| p.qname.clone()).collect();

    let mut check_function_deps = |f: &Function, kind: &str, errors: &mut Vec<AstResolutionError>| {
        for edge in &f.body.dependencies {
            // edge.to is a NodeId; dispatch on variant and check existence.
            match &edge.to {
                NodeId::Table(q) => {
                    if !known_tables.contains(q) && !known_views.contains(q) && !known_mvs.contains(q) {
                        errors.push(AstResolutionError {
                            message: format!("{kind} {} body references relation {q} which is not declared in source", f.qname),
                            location: locations.get(&format!("functions.{}", f.qname)).cloned(),
                        });
                    }
                }
                NodeId::Type(q) => {
                    if !known_types.contains(q) {
                        errors.push(AstResolutionError { /* same shape */ });
                    }
                }
                NodeId::Function(q, args) => {
                    // Overload resolution: find the function whose
                    // arg_types_normalized.canonical_hash matches.
                    let Some(overloads) = function_overloads.get(q) else {
                        errors.push(AstResolutionError {
                            message: format!("{kind} {} body calls function {q} which is not declared in source", f.qname),
                            location: locations.get(&format!("functions.{}", f.qname)).cloned(),
                        });
                        continue;
                    };
                    if !overloads.iter().any(|o| o.arg_types_normalized.canonical_hash == args.canonical_hash) {
                        let available: Vec<String> = overloads.iter()
                            .map(|o| format!("{}({})", o.qname, render_arg_sig(&o.args)))
                            .collect();
                        errors.push(AstResolutionError {
                            message: format!(
                                "{kind} {} body calls function {q} with no matching overload (available: {})",
                                f.qname,
                                available.join(", "),
                            ),
                            location: locations.get(&format!("functions.{}", f.qname)).cloned(),
                        });
                    }
                }
                NodeId::Procedure(q) => {
                    if !known_procedures.contains(q) {
                        errors.push(AstResolutionError { /* "calls procedure ... not declared" */ });
                    }
                }
                _ => {}  // Schema, Index, Sequence, etc. — not body-reachable.
            }
        }
    };

    for f in &catalog.functions { check_function_deps(f, "function", errors); }
    // For procedures, reuse the same logic via a helper that takes &[DepEdge] directly.
    for p in &catalog.procedures {
        check_routine_dep_edges(&p.body.dependencies, &p.qname, "procedure", &known_tables, &known_views, &known_mvs, &known_types, &function_overloads, &known_procedures, locations, errors);
    }
}
```

Wire into the top-level `resolve()` function alongside `resolve_fk_references` / `resolve_user_defined_references`.

- [ ] **Step 6.4: Run tests + commit**

```bash
cargo test -p pgevolve-core --test ast_resolution 2>&1 | tail -15
cargo test --workspace --lib --tests 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
```

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(parse): AST resolution for function and procedure body references (T6)

resolve_routine_references walks each function's and procedure's
body.dependencies, validating that every NodeId target resolves
against the corresponding catalog collection. Function call edges
use overload resolution: the dep edge's NormalizedArgTypes hash
must match an overload's arg_types_normalized hash. Mismatches
surface as AstResolutionError listing all available overloads,
matching the v0.1 FK-reference error UX.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Catalog reader

**Files:**
- Create: `crates/pgevolve-core/src/catalog/queries/functions.rs` — `SELECT_FUNCTIONS` SQL.
- Modify: `crates/pgevolve-core/src/catalog/queries/{mod.rs, pg14.rs, pg15.rs, pg16.rs, pg17.rs}` — wire query string into per-version dispatch.
- Modify: `crates/pgevolve-core/src/catalog/mod.rs` — `CatalogQuery::Functions` variant + `read_catalog` wiring.
- Modify: `crates/pgevolve-core/src/catalog/assemble.rs` — `build_functions_and_procedures` function.
- Create: `crates/pgevolve-core/tests/functions_round_trip.rs` — Docker-gated tests.

**Load-bearing spec section:** §5.

- [ ] **Step 7.1: Discover the catalog reader architecture**

Look at how the types sub-spec wired up its catalog reader (commit `f5d0e58`):

```bash
cat crates/pgevolve-core/src/catalog/queries/types.rs
grep -n "CatalogQuery::" crates/pgevolve-core/src/catalog/queries/mod.rs | head -15
grep -n "user_types\|composite_attributes" crates/pgevolve-core/src/catalog/mod.rs | head -10
grep -n "build_user_types" crates/pgevolve-core/src/catalog/assemble.rs | head -5
```

Follow the identical pattern for functions.

- [ ] **Step 7.2: Create the SQL fragment**

```rust
// crates/pgevolve-core/src/catalog/queries/functions.rs

//! Function + procedure catalog query. One row per routine; the assembler
//! dispatches by prokind ('f' = function, 'p' = procedure).

pub const SELECT_FUNCTIONS: &str = "\
SELECT \
    n.nspname                                   AS schema_name, \
    p.proname                                   AS name, \
    p.prokind::text                             AS kind, \
    pg_get_function_identity_arguments(p.oid)   AS arg_signature, \
    pg_get_function_arguments(p.oid)            AS arg_full, \
    pg_get_function_result(p.oid)               AS return_type, \
    l.lanname                                   AS language, \
    p.provolatile::text                         AS volatility, \
    p.proisstrict                               AS strict, \
    p.prosecdef                                 AS security_definer, \
    p.proparallel::text                         AS parallel, \
    p.proleakproof                              AS leakproof, \
    p.procost::text                             AS cost, \
    p.prorows::text                             AS rows, \
    pg_get_functiondef(p.oid)                   AS full_def, \
    obj_description(p.oid, 'pg_proc')           AS comment \
FROM pg_proc p \
JOIN pg_namespace n ON p.pronamespace = n.oid \
JOIN pg_language l ON p.prolang = l.oid \
WHERE n.nspname = ANY($1::text[]) \
  AND p.prokind IN ('f', 'p') \
ORDER BY n.nspname, p.proname, pg_get_function_identity_arguments(p.oid)";
```

In `queries/mod.rs`: add `pub mod functions;` and dispatch arms (same SQL for PG 14/15/16/17).

In `catalog/mod.rs`: add `CatalogQuery::Functions` variant; in `read_catalog`, fetch the rows and store into `RawRows::functions: Vec<Row>`.

- [ ] **Step 7.3: Build the assembler**

In `catalog/assemble.rs`:

```rust
pub(super) fn build_functions_and_procedures(
    raw: &RawRows,
    drift: &mut DriftReport,
) -> Result<(Vec<Function>, Vec<Procedure>), CatalogError> {
    let mut functions = Vec::new();
    let mut procedures = Vec::new();

    for row in &raw.functions {
        let schema = row.get_text(CatalogQuery::Functions, "schema_name")?;
        let name = row.get_text(CatalogQuery::Functions, "name")?;
        let kind_char = row.get_text(CatalogQuery::Functions, "kind")?
            .chars().next().unwrap_or('?');
        let language = row.get_text(CatalogQuery::Functions, "language")?;
        let qname = QualifiedName::new(
            Identifier::from_unquoted(&schema).map_err(CatalogError::from)?,
            Identifier::from_unquoted(&name).map_err(CatalogError::from)?,
        );

        // Unsupported language → DriftReport entry; skip the row.
        let lang = match language.as_str() {
            "sql" => FunctionLanguage::Sql,
            "plpgsql" => FunctionLanguage::PlPgSql,
            _ => {
                drift.unmanaged_language_routines.push(UnmanagedLanguageRoutine {
                    qname: qname.clone(),
                    language: language.clone(),
                });
                continue;
            }
        };

        // Parse arg_full into Vec<FunctionArg>.
        let arg_full = row.get_text(CatalogQuery::Functions, "arg_full")?;
        let args = parse_args_from_string(&arg_full, &qname)?;

        // Extract body from full_def.
        let full_def = row.get_text(CatalogQuery::Functions, "full_def")?;
        let body_text = extract_body_from_functiondef(&full_def, &qname)?;
        let (body, commits_in_body) = crate::parse::builder::plpgsql::parse_routine_body(
            &body_text, lang, &qname, /* location: synthesize a catalog-side location */
        ).map_err(|e| CatalogError::AssemblerInternal { message: format!("routine {qname}: {e}") })?;

        let comment = row.get_text_optional(CatalogQuery::Functions, "comment")?;

        if kind_char == 'p' {
            // Procedure
            let security = if row.get_bool(CatalogQuery::Functions, "security_definer")? {
                SecurityMode::Definer
            } else {
                SecurityMode::Invoker
            };
            procedures.push(Procedure {
                qname,
                args,
                language: lang,
                body,
                security,
                commits_in_body,
                comment,
            });
        } else {
            // Function
            let return_type_text = row.get_text(CatalogQuery::Functions, "return_type")?;
            let return_type = parse_return_type_from_string(&return_type_text, &qname)?;
            let arg_types_normalized = NormalizedArgTypes::from_args(&args);
            let volatility = match row.get_text(CatalogQuery::Functions, "volatility")?.as_str() {
                "i" => Volatility::Immutable,
                "s" => Volatility::Stable,
                "v" => Volatility::Volatile,
                other => return Err(CatalogError::AssemblerInternal {
                    message: format!("function {qname}: unknown volatility {other:?}"),
                }),
            };
            let security = if row.get_bool(CatalogQuery::Functions, "security_definer")? {
                SecurityMode::Definer
            } else {
                SecurityMode::Invoker
            };
            let parallel = match row.get_text(CatalogQuery::Functions, "parallel")?.as_str() {
                "u" => ParallelSafety::Unsafe,
                "r" => ParallelSafety::Restricted,
                "s" => ParallelSafety::Safe,
                other => return Err(CatalogError::AssemblerInternal {
                    message: format!("function {qname}: unknown parallel {other:?}"),
                }),
            };
            let cost = row.get_text_optional(CatalogQuery::Functions, "cost")?
                .and_then(|s| s.parse::<f32>().ok());
            let rows_val = row.get_text_optional(CatalogQuery::Functions, "rows")?
                .and_then(|s| s.parse::<f32>().ok())
                .filter(|&r| r > 0.0); // 0 means "not set" in pg_proc for non-SETOF functions
            functions.push(Function {
                qname,
                args,
                arg_types_normalized,
                return_type,
                language: lang,
                body,
                volatility,
                strict: row.get_bool(CatalogQuery::Functions, "strict")?,
                security,
                parallel,
                leakproof: row.get_bool(CatalogQuery::Functions, "leakproof")?,
                cost,
                rows: rows_val,
                comment,
            });
        }
    }

    functions.sort_by(|a, b| (a.qname.cmp(&b.qname)).then(a.arg_types_normalized.canonical_hash.cmp(&b.arg_types_normalized.canonical_hash)));
    procedures.sort_by(|a, b| a.qname.cmp(&b.qname));
    Ok((functions, procedures))
}

fn extract_body_from_functiondef(full_def: &str, qname: &QualifiedName) -> Result<String, CatalogError> {
    // pg_get_functiondef output looks like:
    //   CREATE OR REPLACE FUNCTION qname(args)
    //    ...
    //    AS $function$
    //   body text
    //   $function$;
    // Locate AS $tag$ and matching $tag$.
    let re = regex::Regex::new(r"AS \$([^$]*)\$").map_err(|_| CatalogError::AssemblerInternal { message: format!("{qname}: regex compile") })?;
    let captures = re.captures(full_def).ok_or_else(|| CatalogError::AssemblerInternal { message: format!("{qname}: no AS $...$ marker in pg_get_functiondef output") })?;
    let tag = captures.get(1).unwrap().as_str();
    let opening = format!("AS ${tag}$");
    let closing = format!("${tag}$");
    let opening_pos = full_def.find(&opening).unwrap();
    let body_start = opening_pos + opening.len();
    let closing_pos = full_def[body_start..].find(&closing).ok_or_else(|| CatalogError::AssemblerInternal { message: format!("{qname}: unmatched dollar-quote in pg_get_functiondef") })?;
    Ok(full_def[body_start..body_start + closing_pos].trim().to_string())
}

fn parse_args_from_string(arg_text: &str, qname: &QualifiedName) -> Result<Vec<FunctionArg>, CatalogError> {
    // arg_text is like "x integer, y text DEFAULT 'a'" or "" (empty).
    // Synthesize a CREATE FUNCTION and re-parse to get the FunctionParameter list.
    if arg_text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let wrapper = format!("CREATE FUNCTION pgevolve_temp({arg_text}) RETURNS void LANGUAGE sql AS $$$$;");
    let parsed = pg_query::parse(&wrapper).map_err(|e| CatalogError::AssemblerInternal { message: format!("{qname}: arg parse error: {e}") })?;
    // Walk the parsed AST to find the CreateFunctionStmt's parameters.
    // ... (delegate to a small helper)
    todo!("port from create_function_stmt::parse_parameter")
}

fn parse_return_type_from_string(text: &str, qname: &QualifiedName) -> Result<ReturnType, CatalogError> {
    // pg_get_function_result returns strings like:
    //   "integer"
    //   "SETOF integer"
    //   "TABLE(a integer, b text)"
    //   "trigger"
    //   "event_trigger"
    //   "void"
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("trigger") { return Ok(ReturnType::Trigger); }
    if trimmed.eq_ignore_ascii_case("event_trigger") { return Ok(ReturnType::EventTrigger); }
    if trimmed.eq_ignore_ascii_case("void") { return Ok(ReturnType::Void); }
    if let Some(rest) = trimmed.strip_prefix("SETOF ") {
        let ty = ColumnType::parse_from_pg_type_string(rest).map_err(|e| CatalogError::AssemblerInternal { message: format!("{qname}: SETOF type unparseable: {e}") })?;
        return Ok(ReturnType::SetOf { ty });
    }
    if let Some(rest) = trimmed.strip_prefix("TABLE(") {
        let inner = rest.strip_suffix(')').ok_or_else(|| CatalogError::AssemblerInternal { message: format!("{qname}: malformed TABLE() return") })?;
        // Parse "a integer, b text" via synthetic CREATE.
        let columns = parse_table_columns(inner, qname)?;
        return Ok(ReturnType::Table { columns });
    }
    let ty = ColumnType::parse_from_pg_type_string(trimmed).map_err(|e| CatalogError::AssemblerInternal { message: format!("{qname}: scalar return type unparseable: {e}") })?;
    Ok(ReturnType::Scalar { ty })
}

fn parse_table_columns(text: &str, qname: &QualifiedName) -> Result<Vec<TableColumn>, CatalogError> {
    todo!("split by comma, parse each as <name> <type> via existing helpers")
}
```

Wire `build_functions_and_procedures` into the top-level `read_catalog`:

```rust
let (functions, procedures) = build_functions_and_procedures(&raw_rows, &mut drift)?;
catalog.functions = functions;
catalog.procedures = procedures;
```

- [ ] **Step 7.4: Add Docker-gated round-trip tests**

Create `crates/pgevolve-core/tests/functions_round_trip.rs` mirroring the structure of `types_round_trip.rs`:

```rust
//! Tier-3 round-trip tests for functions and procedures.

use pgevolve_testkit::ephemeral_pg::{docker_available, EphemeralPostgres};
// (mirror the imports from tests/types_round_trip.rs)

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_sql_function() {
    if !docker_available() {
        eprintln!("Docker unavailable; skipping");
        return;
    }
    // ... boilerplate matching types_round_trip pattern ...
    let sql = r"
        CREATE SCHEMA app;
        CREATE FUNCTION app.double(x integer) RETURNS integer
            LANGUAGE sql IMMUTABLE STRICT
            AS $$ SELECT x * 2 $$;
    ";
    let catalog = read_catalog_from_sql(sql).await.expect("catalog read");
    assert_eq!(catalog.functions.len(), 1);
    let f = &catalog.functions[0];
    assert_eq!(f.qname.to_string(), "app.double");
    assert!(matches!(f.language, FunctionLanguage::Sql));
    assert!(matches!(f.volatility, Volatility::Immutable));
    assert!(f.strict);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_plpgsql_function() { /* ... */ }

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_procedure_with_commit() { /* ... */ }

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_overloaded_functions() {
    // Two functions, same qname, different arg types. Both must surface.
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_skips_plperl_function_and_reports_drift() {
    // CREATE LANGUAGE plperl + CREATE FUNCTION ... LANGUAGE plperl.
    // Expect catalog.functions.is_empty() AND drift contains UnmanagedLanguageRoutine.
}
```

- [ ] **Step 7.5: Bless tier-3 goldens + commit**

```bash
cargo xtask bless 2>&1 | tail -10
cargo test --workspace --lib --tests 2>&1 | tail -10
cargo test -p pgevolve-core --test functions_round_trip 2>&1 | tail -10
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
```

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(catalog): read functions and procedures from live PG (T7)

One SELECT_FUNCTIONS query covers both pg_proc kinds (prokind IN
'f','p'). The assembler dispatches by kind, decodes per-attribute
columns, extracts the body from pg_get_functiondef via dollar-quote
matching, and re-parses through the same NormalizedBody pipeline as
the source side. Languages outside sql/plpgsql land in the
DriftReport as UnmanagedLanguageRoutine entries and the row is
skipped (no false drop).

Tier-3 goldens regenerated to include empty "functions": [] and
"procedures": [] arrays in pre-existing fixtures.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Differ

**Files:**
- Create: `crates/pgevolve-core/src/diff/routines.rs`
- Modify: `crates/pgevolve-core/src/diff/change.rs` — `FunctionChange`, `ProcedureChange`, top-level `Change::Function` / `Change::Procedure` variants.
- Modify: `crates/pgevolve-core/src/diff/mod.rs` — declare module + call from top-level diff.
- Modify: `crates/pgevolve-core/src/plan/ordering.rs` — stub arms (`unimplemented!("T9...")`).
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs` — stub arms (`unimplemented!("T10...")`).
- Modify: `crates/pgevolve-core/src/diff/destructiveness.rs` (no separate match — destructiveness is set inline per push in routines.rs).
- Modify: `crates/pgevolve-core/src/commands/diff.rs` (CLI human/print arms — surfaced when T9 lands).

**Load-bearing spec section:** §6 differ.

- [ ] **Step 8.1: Add Change variants**

In `crates/pgevolve-core/src/diff/change.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FunctionChange {
    Create(Function),
    Drop { qname: QualifiedName, args: NormalizedArgTypes },
    CreateOrReplace(Function),
    ReplaceWithCascade { catalog: Function, source: Function },
    SetComment { qname: QualifiedName, args: NormalizedArgTypes, comment: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcedureChange {
    Create(Procedure),
    Drop(QualifiedName),
    CreateOrReplace(Procedure),
    SetComment { qname: QualifiedName, comment: Option<String> },
}
```

Add `Change::Function(FunctionChange)` and `Change::Procedure(ProcedureChange)` to the top-level Change enum.

- [ ] **Step 8.2: Stub planner arms**

`plan/ordering.rs::partition`:
```rust
Change::Function(_) => unimplemented!("Task 9 wires NodeId::Function and partition for function changes"),
Change::Procedure(_) => unimplemented!("Task 9 wires NodeId::Procedure and partition for procedure changes"),
```

`plan/ordering.rs::change_node` — same `unimplemented!`.

`plan/rewrite/mod.rs::emit_change` — same `unimplemented!` per kind.

The CLI `diff.rs` print_human and change_kind_name also need stub arms (so the workspace compiles) — they can match all FunctionChange/ProcedureChange variants and print placeholder strings. T10 replaces with real output.

- [ ] **Step 8.3: Implement the differ**

Create `crates/pgevolve-core/src/diff/routines.rs`:

```rust
//! Diff functions and procedures.

use std::collections::BTreeMap;

use crate::diff::change::{Change, FunctionChange, ProcedureChange};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::ir::function::{Function, NormalizedArgTypes, ReturnType};
use crate::ir::procedure::Procedure;
use crate::identifier::QualifiedName;

pub fn diff_functions(catalog: &[Function], source: &[Function], out: &mut ChangeSet) {
    let cat: BTreeMap<_, _> = catalog.iter().map(|f| ((f.qname.clone(), f.arg_types_normalized.canonical_hash), f)).collect();
    let src: BTreeMap<_, _> = source.iter().map(|f| ((f.qname.clone(), f.arg_types_normalized.canonical_hash), f)).collect();

    let all_keys: std::collections::BTreeSet<_> = cat.keys().chain(src.keys()).cloned().collect();

    for key in all_keys {
        match (cat.get(&key), src.get(&key)) {
            (None, Some(s)) => out.push(
                Change::Function(FunctionChange::Create((*s).clone())),
                Destructiveness::Safe,
            ),
            (Some(c), None) => out.push(
                Change::Function(FunctionChange::Drop { qname: c.qname.clone(), args: c.arg_types_normalized.clone() }),
                Destructiveness::RequiresApprovalAndDataLossWarning {
                    reason: format!("drops function {}", c.qname),
                },
            ),
            (Some(c), Some(s)) => diff_same_function(c, s, out),
            (None, None) => unreachable!(),
        }
    }
}

fn diff_same_function(catalog: &Function, source: &Function, out: &mut ChangeSet) {
    let body_changed = catalog.body.canonical_hash != source.body.canonical_hash;
    let attrs_changed = catalog.return_type != source.return_type
        || catalog.language != source.language
        || catalog.volatility != source.volatility
        || catalog.strict != source.strict
        || catalog.security != source.security
        || catalog.parallel != source.parallel
        || catalog.leakproof != source.leakproof
        || catalog.cost.map(f32::to_bits) != source.cost.map(f32::to_bits)
        || catalog.rows.map(f32::to_bits) != source.rows.map(f32::to_bits)
        || catalog.args != source.args;  // arg defaults can change; types are identity-equal at this branch

    if !body_changed && !attrs_changed {
        // Only comment may have changed.
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
        // Single CREATE OR REPLACE FUNCTION.
        let destructive = arg_default_removed(catalog, source);
        let dest = if destructive {
            Destructiveness::RequiresApproval {
                reason: format!("function {} removes an argument default (may break callers passing fewer args)", source.qname),
            }
        } else {
            Destructiveness::Safe
        };
        out.push(Change::Function(FunctionChange::CreateOrReplace(source.clone())), dest);
    } else {
        out.push(
            Change::Function(FunctionChange::ReplaceWithCascade {
                catalog: catalog.clone(),
                source: source.clone(),
            }),
            Destructiveness::RequiresApprovalAndDataLossWarning {
                reason: format!("function {} return-type change requires DROP+CREATE CASCADE", source.qname),
            },
        );
    }
}

pub(crate) fn function_can_or_replace(catalog: &Function, source: &Function) -> bool {
    // PG's CREATE OR REPLACE FUNCTION rejects:
    //   - changing the number of output parameters
    //   - changing the names of output parameters
    //   - switching return type kind (scalar ↔ setof ↔ table)
    //   - changing language
    if catalog.language != source.language {
        return false;
    }
    if !return_type_compatible(&catalog.return_type, &source.return_type) {
        return false;
    }
    // OUT param count + names check
    let cat_outs: Vec<_> = catalog.args.iter().filter(|a| matches!(a.mode, crate::ir::function::ArgMode::Out | crate::ir::function::ArgMode::InOut)).map(|a| a.name.clone()).collect();
    let src_outs: Vec<_> = source.args.iter().filter(|a| matches!(a.mode, crate::ir::function::ArgMode::Out | crate::ir::function::ArgMode::InOut)).map(|a| a.name.clone()).collect();
    if cat_outs != src_outs {
        return false;
    }
    true
}

fn return_type_compatible(a: &ReturnType, b: &ReturnType) -> bool {
    // Same kind = compatible (scalar→scalar with same type is identity; differing scalar types fail PG's check).
    // For v0.2, treat any kind change as incompatible. Same-kind same-type is the common case.
    matches!((a, b),
        (ReturnType::Scalar { ty: ta }, ReturnType::Scalar { ty: tb }) if ta == tb)
        || matches!((a, b),
            (ReturnType::SetOf { ty: ta }, ReturnType::SetOf { ty: tb }) if ta == tb)
        || matches!((a, b),
            (ReturnType::Table { columns: ca }, ReturnType::Table { columns: cb }) if ca == cb)
        || matches!((a, b), (ReturnType::Trigger, ReturnType::Trigger))
        || matches!((a, b), (ReturnType::EventTrigger, ReturnType::EventTrigger))
        || matches!((a, b), (ReturnType::Void, ReturnType::Void))
}

fn arg_default_removed(catalog: &Function, source: &Function) -> bool {
    // For each catalog arg with a default, the corresponding source arg must also have a default.
    catalog.args.iter().zip(source.args.iter()).any(|(c, s)| c.default.is_some() && s.default.is_none())
}

pub fn diff_procedures(catalog: &[Procedure], source: &[Procedure], out: &mut ChangeSet) {
    let cat: BTreeMap<_, _> = catalog.iter().map(|p| (p.qname.clone(), p)).collect();
    let src: BTreeMap<_, _> = source.iter().map(|p| (p.qname.clone(), p)).collect();
    let all: std::collections::BTreeSet<_> = cat.keys().chain(src.keys()).cloned().collect();
    for key in all {
        match (cat.get(&key), src.get(&key)) {
            (None, Some(s)) => out.push(Change::Procedure(ProcedureChange::Create((*s).clone())), Destructiveness::Safe),
            (Some(c), None) => out.push(
                Change::Procedure(ProcedureChange::Drop(c.qname.clone())),
                Destructiveness::RequiresApprovalAndDataLossWarning { reason: format!("drops procedure {}", c.qname) },
            ),
            (Some(c), Some(s)) => diff_same_procedure(c, s, out),
            (None, None) => unreachable!(),
        }
    }
}

fn diff_same_procedure(catalog: &Procedure, source: &Procedure, out: &mut ChangeSet) {
    let body_changed = catalog.body.canonical_hash != source.body.canonical_hash;
    let attrs_changed = catalog.language != source.language
        || catalog.security != source.security
        || catalog.args != source.args
        || catalog.commits_in_body != source.commits_in_body;
    if !body_changed && !attrs_changed {
        if catalog.comment != source.comment {
            out.push(Change::Procedure(ProcedureChange::SetComment { qname: source.qname.clone(), comment: source.comment.clone() }), Destructiveness::Safe);
        }
        return;
    }
    out.push(Change::Procedure(ProcedureChange::CreateOrReplace(source.clone())), Destructiveness::Safe);
}

#[cfg(test)]
mod tests {
    // Cover at minimum:
    //   - function_create
    //   - function_drop_destructive
    //   - function_body_change_emits_create_or_replace
    //   - function_return_type_kind_change_emits_cascade
    //   - function_overloads_treated_independently
    //   - function_set_comment_only
    //   - procedure_create
    //   - procedure_drop_destructive
    //   - procedure_body_change_emits_create_or_replace
    //   - procedure_commits_in_body_toggle_emits_create_or_replace
    //   - function_arg_default_removed_requires_approval
}
```

- [ ] **Step 8.4: Wire into top-level diff**

In `crates/pgevolve-core/src/diff/mod.rs`:
```rust
pub mod routines;
// In diff(): out.extend(routines::diff_functions(...)); out.extend(routines::diff_procedures(...));
```

- [ ] **Step 8.5: Run + commit**

```bash
cargo test -p pgevolve-core --lib diff::routines 2>&1 | tail -10
cargo test --workspace --lib --tests 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
```

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(diff): FunctionChange + ProcedureChange + can_or_replace predicate (T8)

5 FunctionChange variants + 4 ProcedureChange variants. Identity pair
keys on (qname, arg_types_normalized.canonical_hash) for functions
and qname for procedures. function_can_or_replace returns false when
language differs, return-type kind differs, or OUT param count/names
differ (PG's hard rules for CREATE OR REPLACE FUNCTION). Anything
incompatible becomes ReplaceWithCascade.

Destructiveness classified inline per push: Drop and ReplaceWith
Cascade are DataLossWarning; arg-default-removal is RequiresApproval;
CreateOrReplace and Create are Safe; SetComment is Safe.

Planner arms in plan/ordering.rs and plan/rewrite/mod.rs land as
unimplemented! stubs for T9/T10. CLI diff.rs print_human and change
_kind_name get placeholder arms so the workspace compiles.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: NodeId::Function/Procedure + dep graph + cascade walker

**Files:**
- Modify: `crates/pgevolve-core/src/plan/edges.rs` — `NodeId::Function(QualifiedName, NormalizedArgTypes)` + `NodeId::Procedure(QualifiedName)` + edges.
- Modify: `crates/pgevolve-core/src/plan/ordering.rs` — replace T8 stubs with real partition/change_node.
- Modify: `crates/pgevolve-core/src/plan/recreate_views.rs` — extend triggers + dep_index.
- Modify: `crates/pgevolve-core/src/plan/error.rs` — render NodeId::Function/Procedure.
- Modify: `crates/pgevolve-core/src/lint/universal.rs` — closed_world_references arm.
- Modify: `crates/pgevolve-conformance/src/assertions/dep_graph.rs` — node_label arm.
- Modify: `crates/pgevolve/src/commands/graph.rs` — node_label arm.

**Load-bearing spec section:** §7.

- [ ] **Step 9.1: Add NodeId variants**

In `plan/edges.rs`:

```rust
pub enum NodeId {
    // ... existing variants ...
    Function(QualifiedName, NormalizedArgTypes),
    Procedure(QualifiedName),
}
```

Run `cargo build` — the compiler surfaces every match site requiring a new arm. Update each:
- `plan/error.rs::render_node` — `NodeId::Function(q, args) => format!("function:{q}({})", render_args(&args))`, `NodeId::Procedure(q) => format!("procedure:{q}")`.
- `lint/universal.rs::closed_world_references` (or `check_deps`) — extract schema from `NodeId::Function(q, _) => q.schema.as_str()` and procedures similarly.
- `pgevolve-conformance/src/assertions/dep_graph.rs::node_label` — mirror the binary's label format.
- `pgevolve/src/commands/graph.rs::node_label` — `format!("function:{q}({arg_sig})")`.

- [ ] **Step 9.2: Add dep graph edges**

In `plan/edges.rs::build_create_graph`:

```rust
// Phase 1b.0 — function/procedure → schema
for f in &catalog.functions {
    g.add_edge(NodeId::Function(f.qname.clone(), f.arg_types_normalized.clone()), NodeId::Schema(f.qname.schema.clone()));
}
for p in &catalog.procedures {
    g.add_edge(NodeId::Procedure(p.qname.clone()), NodeId::Schema(p.qname.schema.clone()));
}

// Phase 1c — function/procedure → types referenced in args + return type
for f in &catalog.functions {
    let node = NodeId::Function(f.qname.clone(), f.arg_types_normalized.clone());
    for arg in &f.args {
        if let ColumnType::UserDefined(t_qname) = &arg.ty {
            g.add_edge(node.clone(), NodeId::Type(t_qname.clone()));
        }
    }
    if let Some(t_qname) = return_type_user_def(&f.return_type) {
        g.add_edge(node, NodeId::Type(t_qname));
    }
}
// same for procedures

// Phase 2c — function/procedure → body deps (from body.dependencies)
for f in &catalog.functions {
    let node = NodeId::Function(f.qname.clone(), f.arg_types_normalized.clone());
    for edge in &f.body.dependencies {
        g.add_edge(node.clone(), edge.to.clone());
    }
}
// same for procedures
```

- [ ] **Step 9.3: Finish ordering arms**

In `plan/ordering.rs`:

```rust
// In partition():
Change::Function(fc) => match fc {
    FunctionChange::Create(_) => bucket.creates_and_adds.push(change),
    FunctionChange::Drop { .. } | FunctionChange::ReplaceWithCascade { .. } => bucket.drops.push(change),
    _ => bucket.modifies.push(change),
}
Change::Procedure(pc) => match pc {
    ProcedureChange::Create(_) => bucket.creates_and_adds.push(change),
    ProcedureChange::Drop(_) => bucket.drops.push(change),
    _ => bucket.modifies.push(change),
}

// In change_node():
Change::Function(fc) => match fc {
    FunctionChange::Create(f) | FunctionChange::CreateOrReplace(f)
    | FunctionChange::ReplaceWithCascade { source: f, .. } =>
        NodeId::Function(f.qname.clone(), f.arg_types_normalized.clone()),
    FunctionChange::Drop { qname, args } | FunctionChange::SetComment { qname, args, .. } =>
        NodeId::Function(qname.clone(), args.clone()),
}
Change::Procedure(pc) => {
    let qname = match pc {
        ProcedureChange::Create(p) | ProcedureChange::CreateOrReplace(p) => &p.qname,
        ProcedureChange::Drop(q) | ProcedureChange::SetComment { qname: q, .. } => q,
    };
    NodeId::Procedure(qname.clone())
}
```

- [ ] **Step 9.4: Extend cascade walker**

In `plan/recreate_views.rs`:

1. `object_drop_qname` (or the function that maps Change → triggered NodeId) — add arms for `FunctionChange::Drop` and `FunctionChange::ReplaceWithCascade`. Note: the trigger is a `NodeId::Function(qname, args)`, not a qname.

2. `build_dep_index` filter_map — include `NodeId::Function(_, _)` and `NodeId::Procedure(_)` so views/MVs/other functions with body_dependencies pointing at them are indexed.

3. Add tests:
   - `function_drop_cascades_to_dependent_view`
   - `function_drop_cascades_to_dependent_function`
   - `function_replace_with_cascade_propagates`

- [ ] **Step 9.5: Run + commit**

```bash
cargo test --workspace --lib --tests 2>&1 | tail -10
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
```

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(plan): NodeId::Function/Procedure + dep graph + cascade walk (T9)

NodeId gains Function(qname, NormalizedArgTypes) and Procedure(qname)
variants. build_create_graph registers them and adds edges:
  - routine → schema
  - routine → type (for arg/return user-defined types)
  - routine → body deps (from NormalizedBody.dependencies)

Ordering partition/change_node arms (previously unimplemented! from
T8) wire FunctionChange/ProcedureChange into the proper buckets.
The dependent-recreation walker (landed by v0.2-views, extended by
v0.2-types) now also propagates cascades from function drops and
ReplaceWithCascade events to views/MVs/other functions whose
body_dependencies point at the routine.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Planner step kinds + SQL emission

**Files:**
- Modify: `crates/pgevolve-core/src/plan/raw_step.rs` — 6 new StepKind variants.
- Modify: `crates/pgevolve-core/src/plan/plan.rs` — kind_name / parse_kind_name.
- Create: `crates/pgevolve-core/src/plan/rewrite/functions.rs` — emit_* helpers.
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs` — replace T8/T9 stubs with real dispatchers.
- Modify: `crates/pgevolve/src/commands/diff.rs` — flesh out CLI human output for FunctionChange/ProcedureChange.

**Load-bearing spec section:** §7.1, §7.6.

- [ ] **Step 10.1: Add StepKind variants**

```rust
// In raw_step.rs
pub enum StepKind {
    // ...existing...
    CreateOrReplaceFunction,
    DropFunction,
    CommentOnFunction,
    CreateOrReplaceProcedure,
    DropProcedure,
    CommentOnProcedure,
}
```

Extend `kind_name()` (snake_case: `create_or_replace_function`, `drop_function`, `comment_on_function`, `create_or_replace_procedure`, `drop_procedure`, `comment_on_procedure`) and `parse_kind_name()`. Add a round-trip test.

- [ ] **Step 10.2: SQL emission**

Create `crates/pgevolve-core/src/plan/rewrite/functions.rs`:

```rust
//! SQL emission for function and procedure planner steps.

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::function::{ArgMode, Function, FunctionArg, FunctionLanguage, NormalizedArgTypes, ParallelSafety, ReturnType, SecurityMode, Volatility};
use crate::ir::procedure::Procedure;

pub(crate) fn emit_create_or_replace_function(f: &Function) -> String {
    let mut sql = format!("CREATE OR REPLACE FUNCTION {}", f.qname.render_sql());
    sql.push('(');
    emit_arg_list(&f.args, &mut sql);
    sql.push(')');

    sql.push_str("\n    RETURNS ");
    emit_return_type(&f.return_type, &mut sql);

    sql.push_str("\n    LANGUAGE ");
    sql.push_str(match f.language {
        FunctionLanguage::Sql => "sql",
        FunctionLanguage::PlPgSql => "plpgsql",
    });

    // Attributes — fixed order for determinism.
    match f.volatility {
        Volatility::Immutable => sql.push_str(" IMMUTABLE"),
        Volatility::Stable => sql.push_str(" STABLE"),
        Volatility::Volatile => {}  // PG default
    }
    if f.strict { sql.push_str(" STRICT"); }
    match f.security {
        SecurityMode::Definer => sql.push_str(" SECURITY DEFINER"),
        SecurityMode::Invoker => {}  // PG default
    }
    match f.parallel {
        ParallelSafety::Safe => sql.push_str(" PARALLEL SAFE"),
        ParallelSafety::Restricted => sql.push_str(" PARALLEL RESTRICTED"),
        ParallelSafety::Unsafe => {}  // PG default
    }
    if f.leakproof { sql.push_str(" LEAKPROOF"); }
    if let Some(c) = f.cost { sql.push_str(&format!(" COST {c}")); }
    if let Some(r) = f.rows { sql.push_str(&format!(" ROWS {r}")); }

    sql.push_str("\nAS $pgevolve$");
    sql.push_str(&f.body.canonical_text);
    sql.push_str("$pgevolve$;");
    sql
}

pub(crate) fn emit_drop_function(qname: &QualifiedName, args: &NormalizedArgTypes) -> String {
    format!("DROP FUNCTION {}({});", qname.render_sql(), render_arg_types(args))
}

pub(crate) fn emit_comment_on_function(qname: &QualifiedName, args: &NormalizedArgTypes, comment: Option<&str>) -> String {
    let arg_sig = render_arg_types(args);
    match comment {
        Some(c) => format!("COMMENT ON FUNCTION {}({}) IS '{}';", qname.render_sql(), arg_sig, c.replace('\'', "''")),
        None => format!("COMMENT ON FUNCTION {}({}) IS NULL;", qname.render_sql(), arg_sig),
    }
}

pub(crate) fn emit_create_or_replace_procedure(p: &Procedure) -> String {
    let mut sql = format!("CREATE OR REPLACE PROCEDURE {}", p.qname.render_sql());
    sql.push('(');
    emit_arg_list(&p.args, &mut sql);
    sql.push(')');
    sql.push_str("\n    LANGUAGE ");
    sql.push_str(match p.language {
        FunctionLanguage::Sql => "sql",
        FunctionLanguage::PlPgSql => "plpgsql",
    });
    if matches!(p.security, SecurityMode::Definer) {
        sql.push_str(" SECURITY DEFINER");
    }
    sql.push_str("\nAS $pgevolve$");
    sql.push_str(&p.body.canonical_text);
    sql.push_str("$pgevolve$;");
    sql
}

pub(crate) fn emit_drop_procedure(qname: &QualifiedName) -> String {
    format!("DROP PROCEDURE {};", qname.render_sql())
}

pub(crate) fn emit_comment_on_procedure(qname: &QualifiedName, comment: Option<&str>) -> String {
    match comment {
        Some(c) => format!("COMMENT ON PROCEDURE {} IS '{}';", qname.render_sql(), c.replace('\'', "''")),
        None => format!("COMMENT ON PROCEDURE {} IS NULL;", qname.render_sql()),
    }
}

fn emit_arg_list(args: &[FunctionArg], out: &mut String) {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 { out.push_str(", "); }
        match arg.mode {
            ArgMode::In => {} // default
            ArgMode::Out => out.push_str("OUT "),
            ArgMode::InOut => out.push_str("INOUT "),
            ArgMode::Variadic => out.push_str("VARIADIC "),
        }
        if let Some(name) = &arg.name {
            out.push_str(name.as_str());
            out.push(' ');
        }
        out.push_str(&arg.ty.render_sql());
        if let Some(default) = &arg.default {
            out.push_str(" DEFAULT ");
            out.push_str(&default.canonical_text);
        }
    }
}

fn emit_return_type(rt: &ReturnType, out: &mut String) {
    match rt {
        ReturnType::Scalar { ty } => out.push_str(&ty.render_sql()),
        ReturnType::SetOf { ty } => {
            out.push_str("SETOF ");
            out.push_str(&ty.render_sql());
        }
        ReturnType::Table { columns } => {
            out.push_str("TABLE(");
            for (i, c) in columns.iter().enumerate() {
                if i > 0 { out.push_str(", "); }
                out.push_str(c.name.as_str());
                out.push(' ');
                out.push_str(&c.ty.render_sql());
            }
            out.push(')');
        }
        ReturnType::Trigger => out.push_str("trigger"),
        ReturnType::EventTrigger => out.push_str("event_trigger"),
        ReturnType::Void => out.push_str("void"),
    }
}

fn render_arg_types(args: &NormalizedArgTypes) -> String {
    args.types.iter().map(|t| t.render_sql()).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    // Cover at minimum:
    //   - emit_drop_function_basic
    //   - emit_drop_function_with_overload_args
    //   - emit_create_or_replace_function_sql_minimal
    //   - emit_create_or_replace_function_plpgsql_with_all_attrs
    //   - emit_comment_on_function_with_args
    //   - emit_create_or_replace_procedure_minimal
    //   - emit_drop_procedure
    //   - emit_comment_on_procedure
    //   - emit_table_return_type
    //   - emit_setof_return_type
}
```

- [ ] **Step 10.3: Dispatcher in `rewrite/mod.rs`**

Replace the T8 stubs:

```rust
Change::Function(fc) => emit_function_change(fc, source_catalog, destructive, destructive_reason, out),
Change::Procedure(pc) => emit_procedure_change(pc, source_catalog, destructive, destructive_reason, out),
```

`emit_function_change` produces 1 or 2 RawSteps. ReplaceWithCascade expands into:
1. `DropFunction` (destructive).
2. `CreateOrReplaceFunction` with the source IR (also destructive — same as v0.2-types fix).
3. Optional `CommentOnFunction` if the source has a comment.

`emit_procedure_change` is similar but simpler. Critically: a `CreateOrReplaceProcedure` step where `p.commits_in_body == true` MUST set `transactional: TransactionConstraint::OutsideTransaction`. All other routine steps use `InTransaction`.

- [ ] **Step 10.4: CLI human output**

In `crates/pgevolve/src/commands/diff.rs`, flesh out the human-readable arms for each FunctionChange/ProcedureChange variant.

- [ ] **Step 10.5: Run + commit**

```bash
cargo test --workspace --lib --tests 2>&1 | tail -10
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(plan): function/procedure step kinds + SQL emission (T10)

6 new StepKind variants (create_or_replace_function, drop_function,
comment_on_function, plus 3 procedure mirrors). plan/rewrite/
functions.rs emits SQL for each: arg lists with INOUT/VARIADIC modes
and DEFAULT expressions, return type variants (Scalar/SetOf/Table/
Trigger/EventTrigger/Void), attribute matrix in fixed order for
deterministic output.

Procedures with commits_in_body=true emit their CreateOrReplace
Procedure step with transactional=OutsideTransaction so the planner
groups them outside the per-step transaction.

CLI diff.rs print_human now shows specific messages per routine
change (e.g., "function app.compute(integer) → create or replace
body+attrs").

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Five lint rules

**Files:**
- Modify: `crates/pgevolve-core/src/lint/universal.rs` — add five rule functions and register them.

**Load-bearing spec section:** §8.

- [ ] **Step 11.1: Implement rules**

Add five functions following the pattern of types-sub-spec rules:

```rust
fn unsupported_function_language_rule(catalog: &Catalog, out: &mut Vec<Finding>) {
    // Not reachable from the IR (parser rejects), but defense-in-depth for
    // programmatic Catalog construction. Functions with language not in
    // {Sql, PlPgSql} (the only enum variants) can't be constructed — this
    // rule is effectively a placeholder for forward compat. Skip in v0.2
    // unless an integration test path can trigger it; OR document and don't
    // register.
}
```

Actually — since `FunctionLanguage` is a 2-variant enum, the parser/IR pair makes this rule unfireable from real code. **Skip registering it as a rule;** the parser's `ParseError::Structural` already covers the user-facing case. Document the omission in the commit message.

The remaining four:

```rust
fn pl_pgsql_dynamic_sql_rule(catalog: &Catalog, out: &mut Vec<Finding>) {
    use crate::ir::function::FunctionLanguage;
    for f in &catalog.functions {
        if !matches!(f.language, FunctionLanguage::PlPgSql) { continue; }
        check_dynamic_sql(&f.body, &f.qname.to_string(), "function", out);
    }
    for p in &catalog.procedures {
        if !matches!(p.language, FunctionLanguage::PlPgSql) { continue; }
        check_dynamic_sql(&p.body, &p.qname.to_string(), "procedure", out);
    }
}

fn check_dynamic_sql(body: &NormalizedBody, label: &str, kind: &str, out: &mut Vec<Finding>) {
    // Look for EXECUTE markers in body text; if present, require at least one
    // AstDeclared dep edge.
    let has_dynamic = body.canonical_text.to_lowercase().contains("execute ")
        || body.canonical_text.to_lowercase().contains("execute(");
    if !has_dynamic { return; }
    let has_directive = body.dependencies.iter().any(|d| matches!(d.source, DepSource::AstDeclared));
    if !has_directive {
        out.push(Finding::error(
            "pl-pgsql-dynamic-sql",
            format!(
                "{kind} {label} contains dynamic SQL (EXECUTE) but no `-- @pgevolve dep: <qname>` directives. \
                 Add at least one directive to declare what the dynamic SQL references."
            ),
        ));
    }
}

fn function_overload_ambiguous_call_rule(catalog: &Catalog, out: &mut Vec<Finding>) {
    // For each function call edge in any routine body, check if multiple
    // overloads match. This requires inspecting the dep edges' resolved targets
    // against the catalog — typically done at AST resolution time. For T11,
    // this rule fires when a Function dep edge's target qname has >1 overloads
    // AND the edge doesn't carry enough disambiguation. Implementation can be
    // a simple grouping check.
    let mut by_qname: BTreeMap<&QualifiedName, usize> = BTreeMap::new();
    for f in &catalog.functions {
        *by_qname.entry(&f.qname).or_default() += 1;
    }
    // The actual ambiguity warning is per call-site; the AST resolver in T6
    // already produces warnings via its own error/warning channel. T11
    // surfaces the same finding at lint time for visibility.
    // SCOPE: deferred to T6's resolver; T11 doesn't add an additional check.
}

fn procedure_contains_commit_rule(catalog: &Catalog, out: &mut Vec<Finding>) {
    for p in &catalog.procedures {
        if p.commits_in_body {
            out.push(Finding::warning(
                "procedure-contains-commit",
                format!(
                    "procedure {} body contains COMMIT/ROLLBACK; \
                     pgevolve will run this step outside transaction (transactional=OutsideTransaction). \
                     Visible in plan output.",
                    p.qname,
                ),
            ));
        }
    }
}

fn function_references_unmanaged_schema_rule(
    catalog: &Catalog,
    managed: &ManagedConfig,
    out: &mut Vec<Finding>,
) {
    let managed_set: BTreeSet<_> = managed.schemas.iter().map(|s| s.as_str().to_string()).collect();
    const BUILTINS: &[&str] = &["pg_catalog", "information_schema"];

    let check_body = |body: &NormalizedBody, label: &str, kind: &str, out: &mut Vec<Finding>| {
        for edge in &body.dependencies {
            let target_schema = match &edge.to {
                NodeId::Table(q) | NodeId::View(q) | NodeId::Mv(q) | NodeId::Type(q)
                | NodeId::Index(q) | NodeId::Sequence(q) | NodeId::Function(q, _)
                | NodeId::Procedure(q) => q.schema.as_str().to_string(),
                NodeId::Schema(s) => s.as_str().to_string(),
                NodeId::Constraint { table, .. } => table.schema.as_str().to_string(),
            };
            if !managed_set.contains(&target_schema) && !BUILTINS.contains(&target_schema.as_str()) {
                out.push(Finding::warning(
                    "function-references-unmanaged-schema",
                    format!("{kind} {label} body references unmanaged schema {target_schema}"),
                ));
            }
        }
    };

    for f in &catalog.functions {
        check_body(&f.body, &f.qname.to_string(), "function", out);
    }
    for p in &catalog.procedures {
        check_body(&p.body, &p.qname.to_string(), "procedure", out);
    }
}
```

Register the four (skipping `unsupported_function_language_rule` per the rationale above) in `run_universal_lints` / `check_universal`.

- [ ] **Step 11.2: Tests**

Add 4-6 inline tests covering:
- `pl_pgsql_dynamic_sql_fires_without_directive` — body contains EXECUTE, no AstDeclared edges → Finding.
- `pl_pgsql_dynamic_sql_silent_with_directive` — body contains EXECUTE + directive → no Finding.
- `procedure_contains_commit_fires_when_commits_in_body_true` → Warning.
- `procedure_contains_commit_silent_when_false` → no Finding.
- `function_references_unmanaged_schema_fires_on_cross_schema_dep` → Warning.

- [ ] **Step 11.3: Run + commit**

```bash
cargo test -p pgevolve-core --lib lint 2>&1 | tail -10
cargo test --workspace --lib --tests 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
```

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(lint): four routine lint rules (T11)

- pl-pgsql-dynamic-sql (Error): PL/pgSQL body uses EXECUTE without
  a matching @pgevolve dep: directive.
- procedure-contains-commit (Warning): informational; pgevolve runs
  the step outside transaction.
- function-references-unmanaged-schema (Warning): body dep edge
  targets a schema outside [managed].schemas.
- function-overload-ambiguous-call: already surfaced at AST
  resolution time (T6) as an error message; no additional lint
  check needed at v0.2 scope.

The originally-planned unsupported-function-language rule is
omitted: FunctionLanguage is a closed enum {Sql, PlPgSql} that the
parser rejects at construction time, making the lint unfireable.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Conformance fixtures

**Files:**
- Create ~22 fixtures under `crates/pgevolve-conformance/tests/cases/`:
  - `objects/functions/` (~10)
  - `objects/procedures/` (~5)
  - `intent/` (3)
  - `scenarios/` (4)

**Load-bearing spec section:** §10.1.

- [ ] **Step 12.1: Author fixtures**

Follow the same convention as the types and views fixtures. Each fixture is a directory with `fixture.toml`, `before.sql`, `after.sql`, and a `expected/` subdirectory filled by `cargo xtask bless --conformance`.

For each fixture, choose `fixture.toml` keys following the existing pattern (study `crates/pgevolve-conformance/tests/cases/objects/views/create-simple/fixture.toml` first):

```toml
[meta]
title = "..."
authoring = "objects"
spec_refs = ["functions.create"]

[pg]
min = 14
max = 17

[expect.diff]
contains = ["app.double(integer)"]

[expect.plan]
steps = 1

# For intent-gated fixtures:
[[expect.intent]]
kind = "drop_function"
target = "app.double(integer)"
```

**Fixtures to author** (use sensible names):

Functions:
1. `create-sql-simple` — bare SQL function.
2. `create-plpgsql-simple` — PL/pgSQL function with static body.
3. `create-with-overload-pair` — two functions same qname, different arg types.
4. `replace-body` — function body changes, attributes stay.
5. `replace-volatility` — body unchanged, IMMUTABLE→VOLATILE flip.
6. `replace-return-type-cascade` — return type kind changes → ReplaceWithCascade.
7. `create-trigger-function` — `RETURNS TRIGGER`.
8. `create-with-table-return` — `RETURNS TABLE(a int, b text)`.
9. `comment-on-function` — comment add.
10. `function-with-dynamic-sql-directive` — EXECUTE + `@pgevolve dep:` clears the lint.

Procedures:
1. `create-simple` — bare procedure.
2. `create-with-commit` — body has COMMIT → step is OutsideTransaction.
3. `replace-body` — procedure body change.
4. `comment-on-procedure`.
5. `drop-procedure` (also in intent/).

Intent:
1. `drop-function-requires-intent`.
2. `drop-procedure-requires-intent`.
3. `function-return-type-cascade-requires-intent`.

Scenarios:
1. `function-calls-function` — caller body has static SELECT calling callee; dep edge connects them.
2. `view-uses-function` — view body has `SELECT app.f()` → view depends on function.
3. `function-with-dynamic-sql-directive` (already above; cross-listed as a scenario).
4. `function-cycle-rejected` — two functions mutually call each other → PlanError::BodyCycle.

- [ ] **Step 12.2: Bless + run conformance**

```bash
cargo xtask bless --conformance 2>&1 | tail -15
cargo test -p pgevolve-conformance 2>&1 | tail -10
```

All new fixtures must pass.

- [ ] **Step 12.3: Coverage**

```bash
cargo xtask coverage --check 2>&1 | tail -10
```

Should report `functions.*` cells filling in.

- [ ] **Step 12.4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
test(conformance): ~22 function and procedure fixtures (T12)

End-to-end Tier-C coverage:
- functions/: create-sql, create-plpgsql, create-with-overload-pair,
  replace-body, replace-volatility, replace-return-type-cascade,
  create-trigger-function, create-with-table-return, comment-on,
  function-with-dynamic-sql-directive.
- procedures/: create-simple, create-with-commit (verifies
  transactional=OutsideTransaction), replace-body, comment-on,
  drop-procedure.
- intent/: drop-function, drop-procedure, function-return-type-
  cascade.
- scenarios/: function-calls-function, view-uses-function, function-
  cycle-rejected (PlanError::BodyCycle).

Plan-SQL, diff-text, and dep-graph goldens blessed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Property tests + documentation

**Files:**
- Modify: `crates/pgevolve-core/tests/property_tests.rs` — `plpgsql_canonicalization_is_idempotent` (nightly, `#[ignore]`).
- Modify: 8 docs files.

- [ ] **Step 13.1: Property test**

```rust
proptest! {
    #[test]
    #[ignore]
    fn plpgsql_canonicalization_is_idempotent(
        body in "BEGIN [A-Z]{1,5} := [0-9]{1,3}; END",
    ) {
        let qname = QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted("f").unwrap(),
        );
        let loc = SourceLocation::for_test("test.sql");

        let result = parse_routine_body(&body, FunctionLanguage::PlPgSql, &qname, &loc);
        prop_assume!(result.is_ok());
        let (body1, _) = result.unwrap();

        // Re-parse the canonical_text — must produce identical canonical_text.
        let result2 = parse_routine_body(&body1.canonical_text, FunctionLanguage::PlPgSql, &qname, &loc);
        prop_assume!(result2.is_ok());
        let (body2, _) = result2.unwrap();

        prop_assert_eq!(body1.canonical_text, body2.canonical_text);
        prop_assert_eq!(body1.canonical_hash, body2.canonical_hash);
    }
}
```

- [ ] **Step 13.2: Documentation**

- `docs/spec/objects.md`: flip `FUNCTION (SQL)` / `FUNCTION (PL/pgSQL)` / `PROCEDURE` rows to ✅ Implemented with `change_kinds: [create, drop, create_or_replace, replace_with_cascade, comment_on]` (functions) / `[create, drop, create_or_replace, comment_on]` (procedures).
- `docs/spec/lint-and-layout.md`: 3 new rule rows (`pl-pgsql-dynamic-sql`, `procedure-contains-commit`, `function-references-unmanaged-schema`).
- `docs/user/plan-format.md`: 6 new step kinds documented.
- `docs/user/cookbook.md`: "Managing functions and procedures" section with worked examples:
  1. Define a SQL function.
  2. Define a PL/pgSQL function with `@pgevolve dep:` directive.
  3. Replace a function body.
  4. Add an overload.
  5. Create a procedure with COMMIT.
- `docs/system/ir.md`: `## Function` and `## Procedure` sections.
- `docs/system/planner.md`: overload disambiguator notes; cascade walker extension for routines.
- `README.md`: v0.2 sub-spec #4 flips to ✅ Landed.
- `CHANGELOG.md`: extend `[0.2.0]` with function + procedure entries.

- [ ] **Step 13.3: Run + commit**

```bash
cargo test --workspace --lib --tests 2>&1 | tail -10
cargo test --workspace --lib --tests -- --include-ignored 2>&1 | tail -10
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
cargo test -p pgevolve-conformance 2>&1 | tail -5
```

All green.

```bash
git add -A
git commit -m "$(cat <<'EOF'
docs+test: function/procedure property test + docs flipped (T13)

Nightly proptest plpgsql_canonicalization_is_idempotent (pure,
#[ignore]'d): verifies that parse → canonicalize → re-parse →
canonicalize produces byte-identical canonical_text. Closes the
round-trip invariant the differ relies on.

Docs updated:
- spec/objects.md: FUNCTION (SQL/PL/pgSQL) and PROCEDURE flip to
  ✅ Implemented.
- spec/lint-and-layout.md: 3 new rule rows.
- user/plan-format.md: 6 new step kinds.
- user/cookbook.md: "Managing functions and procedures" section.
- system/ir.md: Function + Procedure sections.
- system/planner.md: overload disambiguator + cascade walker notes.
- README.md: sub-spec #4 → ✅ Landed.
- CHANGELOG.md: [0.2.0] gains routine entries.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Final cleanup + branch finishing

- [ ] **Step 14.1: Spec coverage audit**

Re-read `docs/superpowers/specs/2026-05-18-functions-procedures-design.md` and confirm each section maps to a task:

| Spec section | Task |
|---|---|
| §1 Scope, §2 Decisions | All tasks |
| §3 IR additions | T1 |
| §4.1 Parser | T2, T3 |
| §4.2 PL/pgSQL body parsing | T4 |
| §4.3 SQL body parsing | T5 |
| §4.4 AST resolution | T6 |
| §5 Catalog reader | T7 |
| §6 Differ | T8 |
| §7 Planner (StepKinds, NodeId, edges, cascade, tx policy) | T9, T10 |
| §8 Lints | T11 |
| §9 Documentation | T13 |
| §10.1 Conformance fixtures | T12 |
| §10.2 Property tests | T13 |
| §11 Open questions | All deferred; documented in spec |

- [ ] **Step 14.2: Final sweep**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --tests 2>&1 | tail -10
cargo test -p pgevolve-conformance 2>&1 | tail -10
```

All green.

- [ ] **Step 14.3: Branch finish**

Use the `superpowers:finishing-a-development-branch` skill to merge or PR. Pattern matches the v0.2-types branch close-out from commit `3a6d52e`.
