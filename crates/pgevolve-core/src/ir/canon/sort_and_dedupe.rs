//! Sort each `Catalog` collection by its canonical key and reject
//! duplicate keys.
//!
//! Runs last in the pipeline so that any rule that may rewrite IR
//! values (e.g., `filter_pg_defaults`) has already completed — duplicate
//! detection sees the post-normalization state.

use crate::ir::IrError;
use crate::ir::catalog::Catalog;
use crate::ir::column_type::ColumnType;

/// Sort + dedupe every `Catalog` collection. Fallible: returns
/// [`IrError::InvalidIdentifier`] on the first duplicate key.
pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    cat.schemas.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(dupe) = first_duplicate(cat.schemas.iter().map(|s| s.name.as_str())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate schema: {dupe}"
        )));
    }

    cat.extensions.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(dupe) = first_duplicate(cat.extensions.iter().map(|e| e.name.as_str())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate extension: {dupe}"
        )));
    }

    cat.tables.sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) = first_duplicate(cat.tables.iter().map(|t| t.qname.to_string())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate table: {dupe}"
        )));
    }

    cat.indexes.sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) = first_duplicate(cat.indexes.iter().map(|i| i.qname.to_string())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate index: {dupe}"
        )));
    }

    cat.sequences.sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) = first_duplicate(cat.sequences.iter().map(|s| s.qname.to_string())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate sequence: {dupe}"
        )));
    }

    cat.views.sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) = first_duplicate(cat.views.iter().map(|v| v.qname.to_string())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate view: {dupe}"
        )));
    }

    cat.materialized_views.sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) = first_duplicate(cat.materialized_views.iter().map(|m| m.qname.to_string()))
    {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate materialized view: {dupe}"
        )));
    }

    cat.types.sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) = first_duplicate(cat.types.iter().map(|t| t.qname.to_string())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate type: {dupe}"
        )));
    }

    // Functions: identity is (qname, arg_types_normalized.canonical_hash).
    // Overloads with the same qname but different arg types are permitted.
    cat.functions.sort_by(|a, b| {
        a.qname.cmp(&b.qname).then_with(|| {
            a.arg_types_normalized
                .canonical_hash
                .cmp(&b.arg_types_normalized.canonical_hash)
        })
    });
    if let Some(dupe) = first_duplicate(cat.functions.iter().map(|f| {
        format!(
            "{}({})",
            f.qname,
            f.arg_types_normalized
                .types
                .iter()
                .map(ColumnType::render_sql)
                .collect::<Vec<_>>()
                .join(",")
        )
    })) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate function: {dupe}"
        )));
    }

    cat.procedures.sort_by(|a, b| a.qname.cmp(&b.qname));
    if let Some(dupe) = first_duplicate(cat.procedures.iter().map(|p| p.qname.to_string())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate procedure: {dupe}"
        )));
    }

    Ok(())
}

fn first_duplicate<T: Ord, I: IntoIterator<Item = T>>(items: I) -> Option<T> {
    let mut seen: Vec<T> = items.into_iter().collect();
    seen.sort();
    let mut iter = seen.into_iter();
    let mut prev = iter.next()?;
    for cur in iter {
        if cur == prev {
            return Some(cur);
        }
        prev = cur;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::schema::Schema;
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    #[test]
    fn sorts_schemas_by_name() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("billing")));
        cat.schemas.push(Schema::new(id("app")));
        run(&mut cat).expect("must canonicalize");
        let names: Vec<_> = cat.schemas.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["app", "billing"]);
    }

    #[test]
    fn rejects_duplicate_table() {
        let mut cat = Catalog::empty();
        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![],
            constraints: vec![],
            comment: None,
        });
        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![],
            constraints: vec![],
            comment: None,
        });
        let err = run(&mut cat).expect_err("duplicate must error");
        assert!(err.to_string().contains("duplicate table"));
    }
}
