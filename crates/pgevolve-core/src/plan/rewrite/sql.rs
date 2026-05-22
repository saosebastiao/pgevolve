//! SQL rendering helpers for the rewrite pass.
//!
//! These functions produce canonical Postgres DDL strings from IR objects.
//! They are used both by the non-rewriting dispatcher in [`super`] and by the
//! online-rewrite submodules.
//!
//! Output is canonical (deterministic spacing, lowercase keywords, schema-qualified
//! names) so that two equal IR inputs produce byte-identical SQL — required by
//! the plan-id hash in spec §6.6.

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column::{
    Column, Compression, GeneratedKind, Identity, IdentityKind, SequenceOptions, StorageKind,
};
use crate::ir::constraint::{
    Constraint, ConstraintKind, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
};
use crate::ir::default_expr::{DefaultExpr, LiteralValue};
use crate::ir::index::{Index, IndexColumn, IndexColumnExpr, IndexMethod, NullsOrder, SortOrder};
use crate::ir::schema::Schema;
use crate::ir::sequence::{Sequence, SequenceOwner};
use crate::ir::table::Table;

// ---------------------------------------------------------------------------
// Top-level statements
// ---------------------------------------------------------------------------

/// `CREATE SCHEMA name;`
pub fn create_schema(s: &Schema) -> String {
    format!("CREATE SCHEMA {};", s.name.render_sql())
}

/// `DROP SCHEMA name;`
pub fn drop_schema(name: &Identifier) -> String {
    format!("DROP SCHEMA {};", name.render_sql())
}

/// `COMMENT ON SCHEMA name IS '...';` (or `IS NULL` to clear).
pub fn comment_on_schema(name: &Identifier, comment: Option<&str>) -> String {
    format!(
        "COMMENT ON SCHEMA {} IS {};",
        name.render_sql(),
        render_comment(comment),
    )
}

/// `CREATE TABLE schema.name ( ... );` with inline columns and constraints.
///
/// When `table.partition_of` is set the column list is omitted entirely
/// (partitions inherit their columns) and a `PARTITION OF parent FOR VALUES
/// …` clause is emitted instead.  When `table.partition_by` is set a
/// `PARTITION BY …` clause is appended before the trailing `;`.
pub fn create_table(t: &Table) -> String {
    let mut s = String::new();
    s.push_str("CREATE TABLE ");
    s.push_str(&t.qname.render_sql());

    if let Some(po) = &t.partition_of {
        // Child partition: no column list — columns are inherited from the
        // parent.  Emit the PARTITION OF clause directly.
        s.push(' ');
        s.push_str(&crate::plan::rewrite::partitions::render_partition_of(po));
    } else {
        // Normal table (possibly a partitioned parent): emit the column list.
        s.push_str(" (");
        let mut first = true;
        for col in &t.columns {
            if !first {
                s.push(',');
            }
            s.push('\n');
            s.push_str("    ");
            s.push_str(&column_def(col));
            first = false;
        }
        for c in &t.constraints {
            if !first {
                s.push(',');
            }
            s.push('\n');
            s.push_str("    ");
            s.push_str(&inline_constraint(c));
            first = false;
        }
        if !first {
            s.push('\n');
        }
        s.push(')');
    }

    if let Some(pb) = &t.partition_by {
        s.push(' ');
        s.push_str(&crate::plan::rewrite::partitions::render_partition_by(pb));
    }

    s.push(';');
    s
}

/// `DROP TABLE schema.name;`
pub fn drop_table(qname: &QualifiedName) -> String {
    format!("DROP TABLE {};", qname.render_sql())
}

/// `COMMENT ON TABLE qname IS '...';`
pub fn comment_on_table(qname: &QualifiedName, comment: Option<&str>) -> String {
    format!(
        "COMMENT ON TABLE {} IS {};",
        qname.render_sql(),
        render_comment(comment),
    )
}

/// `CREATE [UNIQUE] INDEX [CONCURRENTLY] name ON table USING method (...) [INCLUDE (...)] [WHERE ...];`
pub fn create_index(idx: &Index, concurrently: bool) -> String {
    let mut s = String::from("CREATE ");
    if idx.unique {
        s.push_str("UNIQUE ");
    }
    s.push_str("INDEX ");
    if concurrently {
        s.push_str("CONCURRENTLY ");
    }
    s.push_str(&idx.qname.name.render_sql());
    s.push_str(" ON ");
    s.push_str(&idx.on.qname().render_sql());
    s.push_str(" USING ");
    s.push_str(index_method(idx.method));
    s.push_str(" (");
    s.push_str(&render_index_columns(&idx.columns));
    s.push(')');
    if !idx.include.is_empty() {
        s.push_str(" INCLUDE (");
        s.push_str(&render_idents(&idx.include));
        s.push(')');
    }
    if idx.unique && idx.nulls_not_distinct {
        s.push_str(" NULLS NOT DISTINCT");
    }
    if let Some(ts) = &idx.tablespace {
        s.push_str(" TABLESPACE ");
        s.push_str(&ts.render_sql());
    }
    if let Some(pred) = &idx.predicate {
        s.push_str(" WHERE ");
        s.push_str(&pred.canonical_text);
    }
    s.push(';');
    s
}

/// `DROP INDEX [CONCURRENTLY] name;`
pub fn drop_index(qname: &QualifiedName, concurrently: bool) -> String {
    if concurrently {
        format!("DROP INDEX CONCURRENTLY {};", qname.render_sql())
    } else {
        format!("DROP INDEX {};", qname.render_sql())
    }
}

/// `CREATE SEQUENCE schema.name AS T [INCREMENT BY n] ...`.
pub fn create_sequence(s: &Sequence) -> String {
    let mut out = String::from("CREATE SEQUENCE ");
    out.push_str(&s.qname.render_sql());
    out.push_str(" AS ");
    out.push_str(&s.data_type.render_sql());
    out.push_str(&format!(" INCREMENT BY {}", s.increment));
    if let Some(min) = s.min_value {
        out.push_str(&format!(" MINVALUE {min}"));
    } else {
        out.push_str(" NO MINVALUE");
    }
    if let Some(max) = s.max_value {
        out.push_str(&format!(" MAXVALUE {max}"));
    } else {
        out.push_str(" NO MAXVALUE");
    }
    out.push_str(&format!(" START WITH {}", s.start));
    out.push_str(&format!(" CACHE {}", s.cache));
    if s.cycle {
        out.push_str(" CYCLE");
    } else {
        out.push_str(" NO CYCLE");
    }
    if let Some(owner) = &s.owned_by {
        out.push_str(" OWNED BY ");
        out.push_str(&render_owner(owner));
    }
    out.push(';');
    out
}

/// `DROP SEQUENCE schema.name;`
pub fn drop_sequence(qname: &QualifiedName) -> String {
    format!("DROP SEQUENCE {};", qname.render_sql())
}

// ---------------------------------------------------------------------------
// ALTER TABLE column / constraint operations
// ---------------------------------------------------------------------------

/// `ALTER TABLE qname ADD COLUMN ...;`
pub fn alter_table_add_column(qname: &QualifiedName, c: &Column) -> String {
    format!(
        "ALTER TABLE {} ADD COLUMN {};",
        qname.render_sql(),
        column_def(c),
    )
}

/// `ALTER TABLE qname DROP COLUMN name;`
pub fn alter_table_drop_column(qname: &QualifiedName, name: &Identifier) -> String {
    format!(
        "ALTER TABLE {} DROP COLUMN {};",
        qname.render_sql(),
        name.render_sql(),
    )
}

/// `ALTER TABLE qname ALTER COLUMN name TYPE T [USING expr];`
pub fn alter_column_type(
    qname: &QualifiedName,
    name: &Identifier,
    to: &crate::ir::column_type::ColumnType,
    using: Option<&crate::ir::default_expr::NormalizedExpr>,
) -> String {
    let mut s = format!(
        "ALTER TABLE {} ALTER COLUMN {} TYPE {}",
        qname.render_sql(),
        name.render_sql(),
        to.render_sql(),
    );
    if let Some(u) = using {
        s.push_str(" USING ");
        s.push_str(&u.canonical_text);
    }
    s.push(';');
    s
}

/// `ALTER TABLE qname ALTER COLUMN name {SET|DROP} NOT NULL;`
pub fn alter_column_set_nullable(
    qname: &QualifiedName,
    name: &Identifier,
    nullable: bool,
) -> String {
    let action = if nullable {
        "DROP NOT NULL"
    } else {
        "SET NOT NULL"
    };
    format!(
        "ALTER TABLE {} ALTER COLUMN {} {};",
        qname.render_sql(),
        name.render_sql(),
        action,
    )
}

/// `ALTER TABLE qname ALTER COLUMN name {SET DEFAULT expr|DROP DEFAULT};`
pub fn alter_column_set_default(
    qname: &QualifiedName,
    name: &Identifier,
    default: Option<&DefaultExpr>,
) -> String {
    match default {
        Some(d) => format!(
            "ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {};",
            qname.render_sql(),
            name.render_sql(),
            render_default_expr(d),
        ),
        None => format!(
            "ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT;",
            qname.render_sql(),
            name.render_sql(),
        ),
    }
}

/// `ALTER TABLE qname ALTER COLUMN name { ADD GENERATED ... AS IDENTITY | DROP IDENTITY };`
pub fn alter_column_set_identity(
    qname: &QualifiedName,
    name: &Identifier,
    identity: Option<&Identity>,
) -> String {
    match identity {
        Some(id) => format!(
            "ALTER TABLE {} ALTER COLUMN {} ADD GENERATED {} AS IDENTITY{};",
            qname.render_sql(),
            name.render_sql(),
            identity_kind(id.kind),
            render_sequence_options(&id.sequence),
        ),
        None => format!(
            "ALTER TABLE {} ALTER COLUMN {} DROP IDENTITY;",
            qname.render_sql(),
            name.render_sql(),
        ),
    }
}

/// `ALTER TABLE qname ALTER COLUMN name DROP EXPRESSION;`
///
/// Note: Postgres has no direct `ALTER COLUMN ... ADD GENERATED ... STORED`
/// for non-identity stored expressions. Setting a generated expression on
/// an existing column requires drop + readd of the column. v0.1 emits
/// `DROP EXPRESSION` for `None`; for `Some`, emits a marker statement that
/// makes the unsupported case visible as a plan error rather than silent.
pub fn alter_column_set_generated(
    qname: &QualifiedName,
    name: &Identifier,
    generated: Option<&crate::ir::column::Generated>,
) -> String {
    match generated {
        None => format!(
            "ALTER TABLE {} ALTER COLUMN {} DROP EXPRESSION;",
            qname.render_sql(),
            name.render_sql(),
        ),
        Some(g) => {
            // Best-effort: emit Postgres' currently-unsupported syntax so the
            // executor surfaces a clear error rather than silently no-op'ing.
            // The expected resolution is for the differ to produce a column
            // recreate in this case; until then this string serves as an
            // explicit, debuggable marker.
            format!(
                "ALTER TABLE {} ALTER COLUMN {} SET EXPRESSION AS ({}) {};",
                qname.render_sql(),
                name.render_sql(),
                g.expression.canonical_text,
                generated_kind(g.kind),
            )
        }
    }
}

/// `ALTER TABLE qname ALTER COLUMN name SET STORAGE {PLAIN|EXTERNAL|EXTENDED|MAIN};`
pub fn alter_column_set_storage(
    qname: &QualifiedName,
    name: &Identifier,
    storage: StorageKind,
) -> String {
    let kw = match storage {
        StorageKind::Plain => "PLAIN",
        StorageKind::External => "EXTERNAL",
        StorageKind::Extended => "EXTENDED",
        StorageKind::Main => "MAIN",
    };
    format!(
        "ALTER TABLE {} ALTER COLUMN {} SET STORAGE {};",
        qname.render_sql(),
        name.render_sql(),
        kw,
    )
}

/// `ALTER TABLE qname ALTER COLUMN name SET COMPRESSION {pglz|lz4|DEFAULT};`
pub fn alter_column_set_compression(
    qname: &QualifiedName,
    name: &Identifier,
    compression: Option<Compression>,
) -> String {
    let kw = match compression {
        Some(Compression::Pglz) => "pglz",
        Some(Compression::Lz4) => "lz4",
        None => "DEFAULT",
    };
    format!(
        "ALTER TABLE {} ALTER COLUMN {} SET COMPRESSION {};",
        qname.render_sql(),
        name.render_sql(),
        kw,
    )
}

/// `COMMENT ON COLUMN qname.col IS '...';`
pub fn comment_on_column(qname: &QualifiedName, col: &Identifier, comment: Option<&str>) -> String {
    format!(
        "COMMENT ON COLUMN {}.{} IS {};",
        qname.render_sql(),
        col.render_sql(),
        render_comment(comment),
    )
}

/// `ALTER TABLE qname ADD CONSTRAINT ...;` (validated form).
pub fn alter_table_add_constraint(qname: &QualifiedName, c: &Constraint) -> String {
    format!(
        "ALTER TABLE {} ADD {};",
        qname.render_sql(),
        constraint_def_with_name(c),
    )
}

/// `ALTER TABLE qname ADD CONSTRAINT ... NOT VALID;`
pub fn alter_table_add_constraint_not_valid(qname: &QualifiedName, c: &Constraint) -> String {
    format!(
        "ALTER TABLE {} ADD {} NOT VALID;",
        qname.render_sql(),
        constraint_def_with_name(c),
    )
}

/// `ALTER TABLE qname VALIDATE CONSTRAINT name;`
pub fn alter_table_validate_constraint(qname: &QualifiedName, cname: &Identifier) -> String {
    format!(
        "ALTER TABLE {} VALIDATE CONSTRAINT {};",
        qname.render_sql(),
        cname.render_sql(),
    )
}

/// `ALTER TABLE qname DROP CONSTRAINT name;`
pub fn alter_table_drop_constraint(qname: &QualifiedName, cname: &Identifier) -> String {
    format!(
        "ALTER TABLE {} DROP CONSTRAINT {};",
        qname.render_sql(),
        cname.render_sql(),
    )
}

/// `COMMENT ON CONSTRAINT name ON qname IS '...';`
pub fn comment_on_constraint(
    qname: &QualifiedName,
    cname: &Identifier,
    comment: Option<&str>,
) -> String {
    format!(
        "COMMENT ON CONSTRAINT {} ON {} IS {};",
        cname.render_sql(),
        qname.render_sql(),
        render_comment(comment),
    )
}

/// `COMMENT ON INDEX qname IS '...';`
pub fn comment_on_index(qname: &QualifiedName, comment: Option<&str>) -> String {
    format!(
        "COMMENT ON INDEX {} IS {};",
        qname.render_sql(),
        render_comment(comment),
    )
}

/// `COMMENT ON SEQUENCE qname IS '...';`
pub fn comment_on_sequence(qname: &QualifiedName, comment: Option<&str>) -> String {
    format!(
        "COMMENT ON SEQUENCE {} IS {};",
        qname.render_sql(),
        render_comment(comment),
    )
}

// ---------------------------------------------------------------------------
// ALTER SEQUENCE field-level ops
// ---------------------------------------------------------------------------

/// `ALTER SEQUENCE qname INCREMENT BY n;`
pub fn alter_sequence_increment(qname: &QualifiedName, n: i64) -> String {
    format!("ALTER SEQUENCE {} INCREMENT BY {n};", qname.render_sql())
}

/// `ALTER SEQUENCE qname { MINVALUE n | NO MINVALUE };`
pub fn alter_sequence_min_value(qname: &QualifiedName, v: Option<i64>) -> String {
    match v {
        Some(n) => format!("ALTER SEQUENCE {} MINVALUE {n};", qname.render_sql()),
        None => format!("ALTER SEQUENCE {} NO MINVALUE;", qname.render_sql()),
    }
}

/// `ALTER SEQUENCE qname { MAXVALUE n | NO MAXVALUE };`
pub fn alter_sequence_max_value(qname: &QualifiedName, v: Option<i64>) -> String {
    match v {
        Some(n) => format!("ALTER SEQUENCE {} MAXVALUE {n};", qname.render_sql()),
        None => format!("ALTER SEQUENCE {} NO MAXVALUE;", qname.render_sql()),
    }
}

/// `ALTER SEQUENCE qname CACHE n;`
pub fn alter_sequence_cache(qname: &QualifiedName, n: i64) -> String {
    format!("ALTER SEQUENCE {} CACHE {n};", qname.render_sql())
}

/// `ALTER SEQUENCE qname { CYCLE | NO CYCLE };`
pub fn alter_sequence_cycle(qname: &QualifiedName, cycle: bool) -> String {
    let kw = if cycle { "CYCLE" } else { "NO CYCLE" };
    format!("ALTER SEQUENCE {} {kw};", qname.render_sql())
}

/// `ALTER SEQUENCE qname AS T;`
pub fn alter_sequence_data_type(
    qname: &QualifiedName,
    ty: &crate::ir::column_type::ColumnType,
) -> String {
    format!(
        "ALTER SEQUENCE {} AS {};",
        qname.render_sql(),
        ty.render_sql(),
    )
}

/// `ALTER SEQUENCE qname OWNED BY { table.col | NONE };`
pub fn alter_sequence_owned_by(qname: &QualifiedName, owner: Option<&SequenceOwner>) -> String {
    match owner {
        Some(o) => format!(
            "ALTER SEQUENCE {} OWNED BY {};",
            qname.render_sql(),
            render_owner(o),
        ),
        None => format!("ALTER SEQUENCE {} OWNED BY NONE;", qname.render_sql()),
    }
}

// ---------------------------------------------------------------------------
// Helpers — column / constraint / index / sequence sub-pieces
// ---------------------------------------------------------------------------

/// One column in `CREATE TABLE` or `ALTER TABLE ADD COLUMN`.
pub fn column_def(c: &Column) -> String {
    let mut s = String::new();
    s.push_str(&c.name.render_sql());
    s.push(' ');
    s.push_str(&c.ty.render_sql());
    if let Some(coll) = &c.collation {
        s.push_str(" COLLATE ");
        s.push_str(&coll.render_sql());
    }
    if !c.nullable {
        s.push_str(" NOT NULL");
    }
    if let Some(d) = &c.default {
        s.push_str(" DEFAULT ");
        s.push_str(&render_default_expr(d));
    }
    if let Some(id) = &c.identity {
        s.push_str(" GENERATED ");
        s.push_str(identity_kind(id.kind));
        s.push_str(" AS IDENTITY");
        s.push_str(&render_sequence_options(&id.sequence));
    }
    if let Some(g) = &c.generated {
        s.push_str(" GENERATED ALWAYS AS (");
        s.push_str(&g.expression.canonical_text);
        s.push_str(") ");
        s.push_str(generated_kind(g.kind));
    }
    s
}

/// A constraint clause as it appears inline in `CREATE TABLE`.
fn inline_constraint(c: &Constraint) -> String {
    constraint_def_with_name(c)
}

/// `CONSTRAINT name <body>` — used for both inline and `ADD CONSTRAINT` forms.
pub fn constraint_def_with_name(c: &Constraint) -> String {
    let mut s = format!(
        "CONSTRAINT {} {}",
        c.qname.name.render_sql(),
        constraint_body(&c.kind)
    );
    match c.deferrable {
        Deferrable::NotDeferrable => {}
        Deferrable::Deferrable {
            initially_deferred: true,
        } => s.push_str(" DEFERRABLE INITIALLY DEFERRED"),
        Deferrable::Deferrable {
            initially_deferred: false,
        } => s.push_str(" DEFERRABLE INITIALLY IMMEDIATE"),
    }
    s
}

fn constraint_body(k: &ConstraintKind) -> String {
    match k {
        ConstraintKind::PrimaryKey { columns, include } => {
            let mut s = format!("PRIMARY KEY ({})", render_idents(columns));
            if !include.is_empty() {
                s.push_str(&format!(" INCLUDE ({})", render_idents(include)));
            }
            s
        }
        ConstraintKind::Unique {
            columns,
            include,
            nulls_distinct,
        } => {
            let mut s = String::from("UNIQUE");
            if !nulls_distinct {
                s.push_str(" NULLS NOT DISTINCT");
            }
            s.push_str(&format!(" ({})", render_idents(columns)));
            if !include.is_empty() {
                s.push_str(&format!(" INCLUDE ({})", render_idents(include)));
            }
            s
        }
        ConstraintKind::ForeignKey(fk) => render_fk(fk),
        ConstraintKind::Check {
            expression,
            no_inherit,
        } => {
            let mut s = format!("CHECK ({})", expression.canonical_text);
            if *no_inherit {
                s.push_str(" NO INHERIT");
            }
            s
        }
    }
}

fn render_fk(fk: &ForeignKey) -> String {
    let mut s = format!(
        "FOREIGN KEY ({}) REFERENCES {} ({})",
        render_idents(&fk.columns),
        fk.referenced_table.render_sql(),
        render_idents(&fk.referenced_columns),
    );
    if !matches!(fk.match_type, FkMatchType::Simple) {
        s.push_str(" MATCH ");
        s.push_str(match fk.match_type {
            FkMatchType::Simple => "SIMPLE",
            FkMatchType::Full => "FULL",
        });
    }
    if !matches!(fk.on_update, ReferentialAction::NoAction) {
        s.push_str(" ON UPDATE ");
        s.push_str(&referential_action(&fk.on_update));
    }
    if !matches!(fk.on_delete, ReferentialAction::NoAction) {
        s.push_str(" ON DELETE ");
        s.push_str(&referential_action(&fk.on_delete));
    }
    s
}

fn referential_action(a: &ReferentialAction) -> String {
    match a {
        ReferentialAction::NoAction => "NO ACTION".into(),
        ReferentialAction::Restrict => "RESTRICT".into(),
        ReferentialAction::Cascade => "CASCADE".into(),
        ReferentialAction::SetNull(cols) => {
            if cols.is_empty() {
                "SET NULL".into()
            } else {
                format!("SET NULL ({})", render_idents(cols))
            }
        }
        ReferentialAction::SetDefault(cols) => {
            if cols.is_empty() {
                "SET DEFAULT".into()
            } else {
                format!("SET DEFAULT ({})", render_idents(cols))
            }
        }
    }
}

fn render_index_columns(cols: &[IndexColumn]) -> String {
    let mut parts = Vec::with_capacity(cols.len());
    for c in cols {
        let mut s = match &c.expr {
            IndexColumnExpr::Column(id) => id.render_sql(),
            IndexColumnExpr::Expression(e) => format!("({})", e.canonical_text),
        };
        if let Some(coll) = &c.collation {
            s.push_str(" COLLATE ");
            s.push_str(&coll.render_sql());
        }
        if let Some(opc) = &c.opclass {
            s.push(' ');
            s.push_str(&opc.render_sql());
        }
        match c.sort_order {
            SortOrder::Asc => {} // ASC is the default; emit only DESC.
            SortOrder::Desc => s.push_str(" DESC"),
        }
        match c.nulls_order {
            NullsOrder::NullsFirst => s.push_str(" NULLS FIRST"),
            NullsOrder::NullsLast => {} // NULLS LAST is btree default for ASC.
        }
        parts.push(s);
    }
    parts.join(", ")
}

const fn index_method(m: IndexMethod) -> &'static str {
    match m {
        IndexMethod::BTree => "btree",
        IndexMethod::Hash => "hash",
        IndexMethod::Gin => "gin",
        IndexMethod::Gist => "gist",
        IndexMethod::Brin => "brin",
        IndexMethod::Spgist => "spgist",
    }
}

const fn identity_kind(k: IdentityKind) -> &'static str {
    match k {
        IdentityKind::Always => "ALWAYS",
        IdentityKind::ByDefault => "BY DEFAULT",
    }
}

const fn generated_kind(k: GeneratedKind) -> &'static str {
    match k {
        GeneratedKind::Stored => "STORED",
    }
}

fn render_sequence_options(o: &SequenceOptions) -> String {
    // Only emit the parenthesized clause if any value differs from PG defaults.
    let defaults = SequenceOptions {
        start: 1,
        increment: 1,
        min_value: None,
        max_value: None,
        cache: 1,
        cycle: false,
    };
    if o == &defaults {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    if o.start != defaults.start {
        parts.push(format!("START WITH {}", o.start));
    }
    if o.increment != defaults.increment {
        parts.push(format!("INCREMENT BY {}", o.increment));
    }
    if let Some(min) = o.min_value {
        parts.push(format!("MINVALUE {min}"));
    }
    if let Some(max) = o.max_value {
        parts.push(format!("MAXVALUE {max}"));
    }
    if o.cache != defaults.cache {
        parts.push(format!("CACHE {}", o.cache));
    }
    if o.cycle {
        parts.push("CYCLE".into());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(" "))
    }
}

fn render_default_expr(d: &DefaultExpr) -> String {
    match d {
        DefaultExpr::Literal(LiteralValue::Bool(b)) => {
            if *b {
                "true".into()
            } else {
                "false".into()
            }
        }
        DefaultExpr::Literal(LiteralValue::Integer(i)) => i.to_string(),
        DefaultExpr::Literal(LiteralValue::Float(f)) => f.to_string(),
        DefaultExpr::Literal(LiteralValue::Text(t)) => format!("'{}'", t.replace('\'', "''")),
        DefaultExpr::Literal(LiteralValue::Bytea(b)) => format!("'\\x{}'", hex(b)),
        DefaultExpr::Literal(LiteralValue::Null) => "NULL".into(),
        DefaultExpr::Sequence(q) => format!("nextval('{}')", q.render_sql()),
        DefaultExpr::Expr(e) => e.canonical_text.clone(),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn render_idents(v: &[Identifier]) -> String {
    let mut s = String::new();
    for (i, id) in v.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&id.render_sql());
    }
    s
}

fn render_owner(o: &SequenceOwner) -> String {
    format!("{}.{}", o.table.render_sql(), o.column.render_sql())
}

fn render_comment(comment: Option<&str>) -> String {
    match comment {
        Some(t) => format!("'{}'", t.replace('\'', "''")),
        None => "NULL".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::column_type::ColumnType;
    use crate::ir::partition::{
        BoundDatum, PartitionBounds, PartitionBy, PartitionColumn, PartitionColumnKind,
        PartitionOf, PartitionStrategy,
    };

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn simple_col(name: &str) -> Column {
        Column {
            name: id(name),
            ty: ColumnType::Text,
            nullable: true,
            collation: None,
            default: None,
            identity: None,
            generated: None,
            storage: None,
            compression: None,
            comment: None,
        }
    }

    fn lit(s: &str) -> crate::ir::default_expr::NormalizedExpr {
        crate::ir::default_expr::NormalizedExpr::from_text(s)
    }

    fn empty_table(qname: QualifiedName) -> Table {
        Table {
            qname,
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
        }
    }

    #[test]
    fn partitioned_parent_includes_partition_by() {
        let mut t = empty_table(qn("app", "orders"));
        t.columns = vec![simple_col("region")];
        t.partition_by = Some(PartitionBy {
            strategy: PartitionStrategy::List,
            columns: vec![PartitionColumn {
                kind: PartitionColumnKind::Column(id("region")),
                collation: None,
                opclass: None,
            }],
        });
        let sql = create_table(&t);
        assert!(
            sql.ends_with("PARTITION BY LIST (region);"),
            "expected PARTITION BY LIST (region); at end, got: {sql}"
        );
        assert!(
            sql.contains("region text"),
            "expected column def, got: {sql}"
        );
    }

    #[test]
    fn child_partition_emits_partition_of_no_column_list() {
        let mut t = empty_table(qn("app", "orders_2024"));
        t.partition_of = Some(PartitionOf {
            parent: qn("app", "orders"),
            bounds: PartitionBounds::Range {
                from: vec![BoundDatum::Literal(lit("'2024-01-01'"))],
                to: vec![BoundDatum::Literal(lit("'2025-01-01'"))],
            },
        });
        let sql = create_table(&t);
        assert_eq!(
            sql,
            "CREATE TABLE app.orders_2024 PARTITION OF app.orders FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');"
        );
        // Must not contain a column list parenthesis block
        assert!(
            !sql.contains('(') || sql.contains("FOR VALUES FROM ("),
            "should not contain a column list opening paren, got: {sql}"
        );
    }
}
