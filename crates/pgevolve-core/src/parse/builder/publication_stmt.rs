//! Parser for `CREATE PUBLICATION` and `ALTER PUBLICATION` statements.
//!
//! `pg_query` emits `CreatePublicationStmt` for CREATE and
//! `AlterPublicationStmt` for ALTER. Both are folded into one `Publication`
//! per name — the same pattern as v0.3.3 reloptions where `CREATE TABLE WITH
//! (...)` and `ALTER TABLE SET (...)` unified into one IR record.
//!
//! Spec: `docs/superpowers/specs/2026-05-26-publications-design.md`
//! Plan Stage 6: `docs/superpowers/plans/2026-05-26-publications.md`

use std::collections::{BTreeMap, BTreeSet};

use pg_query::NodeEnum;
use pg_query::protobuf::{
    AlterPublicationAction, AlterPublicationStmt, CreatePublicationStmt, PublicationObjSpec,
    PublicationObjSpecType,
};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::publication::{Publication, PublicationScope, PublishKinds, PublishedTable};
use crate::parse::error::{ParseError, SourceLocation};

/// Apply a `CREATE PUBLICATION` statement to the accumulator map.
///
/// Rejects duplicates. Parses scope (`FOR ALL TABLES` vs selective) and
/// `WITH (publish = '...', publish_via_partition_root = ...)` options.
pub(crate) fn parse_create_publication(
    stmt: &CreatePublicationStmt,
    source_loc: SourceLocation,
    existing: &mut BTreeMap<Identifier, Publication>,
) -> Result<(), ParseError> {
    let name = Identifier::from_unquoted(&stmt.pubname)
        .map_err(|e| ParseError::InvalidIdentifier(stmt.pubname.clone(), e.to_string()))?;

    if existing.contains_key(&name) {
        return Err(ParseError::DuplicatePublication(name, source_loc));
    }

    let scope = if stmt.for_all_tables {
        if !stmt.pubobjects.is_empty() {
            return Err(ParseError::PublicationAllTablesWithObjects(
                name, source_loc,
            ));
        }
        PublicationScope::AllTables
    } else {
        parse_selective_scope(&stmt.pubobjects, &name, &source_loc)?
    };

    let (publish, via_root) = parse_publication_options(&stmt.options, &name, source_loc)?;

    existing.insert(
        name.clone(),
        Publication {
            name,
            scope,
            publish: publish.unwrap_or_else(PublishKinds::pg_default),
            publish_via_partition_root: via_root.unwrap_or(false),
            owner: None,
            comment: None,
        },
    );
    Ok(())
}

/// Apply an `ALTER PUBLICATION` statement to the accumulator map.
///
/// Folds ADD / DROP / SET object changes and `WITH (...)` option updates
/// into the existing `Publication` record. Rejects ALTER-before-CREATE.
pub(crate) fn parse_alter_publication(
    stmt: &AlterPublicationStmt,
    source_loc: SourceLocation,
    existing: &mut BTreeMap<Identifier, Publication>,
) -> Result<(), ParseError> {
    let name = Identifier::from_unquoted(&stmt.pubname)
        .map_err(|e| ParseError::InvalidIdentifier(stmt.pubname.clone(), e.to_string()))?;

    let pub_ = existing.get_mut(&name).ok_or_else(|| {
        ParseError::AlterPublicationBeforeCreate(name.clone(), source_loc.clone())
    })?;

    if !stmt.pubobjects.is_empty() {
        apply_scope_change(
            stmt.action,
            &stmt.pubobjects,
            pub_,
            &name,
            source_loc.clone(),
        )?;
    } else if stmt.for_all_tables {
        // ALTER PUBLICATION p SET FOR ALL TABLES
        let action = AlterPublicationAction::try_from(stmt.action)
            .unwrap_or(AlterPublicationAction::Undefined);
        if matches!(action, AlterPublicationAction::ApSetObjects) {
            pub_.scope = PublicationScope::AllTables;
        }
    }

    if !stmt.options.is_empty() {
        let (publish, via_root) = parse_publication_options(&stmt.options, &name, source_loc)?;
        if let Some(k) = publish {
            pub_.publish = k;
        }
        if let Some(v) = via_root {
            pub_.publish_via_partition_root = v;
        }
    }

    Ok(())
}

// ── Scope parsing ────────────────────────────────────────────────────────────

fn parse_selective_scope(
    objs: &[pg_query::protobuf::Node],
    name: &Identifier,
    loc: &SourceLocation,
) -> Result<PublicationScope, ParseError> {
    let mut tables: Vec<PublishedTable> = Vec::new();
    let mut schemas = BTreeSet::new();

    for obj in objs {
        let Some(NodeEnum::PublicationObjSpec(spec)) = obj.node.as_ref() else {
            return Err(ParseError::PublicationObjectMalformed(
                name.clone(),
                loc.clone(),
            ));
        };
        let obj_type = PublicationObjSpecType::try_from(spec.pubobjtype)
            .unwrap_or(PublicationObjSpecType::Undefined);
        match obj_type {
            PublicationObjSpecType::PublicationobjTable => {
                let pt = extract_table_spec(spec, name, loc)?;
                tables.push(pt);
            }
            PublicationObjSpecType::PublicationobjTablesInSchema => {
                let sn = extract_schema_name(spec, name, loc)?;
                schemas.insert(sn);
            }
            PublicationObjSpecType::PublicationobjTablesInCurSchema => {
                return Err(ParseError::PublicationCurrentSchemaForm(
                    name.clone(),
                    loc.clone(),
                ));
            }
            PublicationObjSpecType::Undefined
            | PublicationObjSpecType::PublicationobjContinuation => {
                return Err(ParseError::UnknownPublicationObjectType(
                    spec.pubobjtype,
                    name.clone(),
                    loc.clone(),
                ));
            }
        }
    }

    if schemas.is_empty() && tables.is_empty() {
        return Err(ParseError::EmptyPublicationScope(name.clone(), loc.clone()));
    }

    Ok(PublicationScope::Selective { schemas, tables })
}

fn extract_table_spec(
    spec: &PublicationObjSpec,
    pub_name: &Identifier,
    loc: &SourceLocation,
) -> Result<PublishedTable, ParseError> {
    let pt = spec
        .pubtable
        .as_ref()
        .ok_or_else(|| ParseError::PublicationObjectMalformed(pub_name.clone(), loc.clone()))?;
    let relation = pt
        .relation
        .as_ref()
        .ok_or_else(|| ParseError::PublicationObjectMalformed(pub_name.clone(), loc.clone()))?;

    if relation.schemaname.is_empty() {
        return Err(ParseError::UnqualifiedPublicationTable(
            pub_name.clone(),
            loc.clone(),
        ));
    }

    let schema = Identifier::from_unquoted(&relation.schemaname)
        .map_err(|e| ParseError::InvalidIdentifier(relation.schemaname.clone(), e.to_string()))?;
    let table = Identifier::from_unquoted(&relation.relname)
        .map_err(|e| ParseError::InvalidIdentifier(relation.relname.clone(), e.to_string()))?;
    let qname = QualifiedName::new(schema, table);

    // Column list (PG 15+): each node is a String node.
    let columns = if pt.columns.is_empty() {
        None
    } else {
        let cols: Result<Vec<_>, _> = pt
            .columns
            .iter()
            .map(|c| {
                let s = extract_string_value(c).ok_or_else(|| {
                    ParseError::PublicationObjectMalformed(pub_name.clone(), loc.clone())
                })?;
                Identifier::from_unquoted(&s)
                    .map_err(|e| ParseError::InvalidIdentifier(s, e.to_string()))
            })
            .collect();
        Some(cols?)
    };

    // Row filter (PG 15+).
    let row_filter = if let Some(where_node) = &pt.where_clause {
        let inner = where_node.node.as_ref().ok_or_else(|| {
            ParseError::PublicationFilterParse(
                pub_name.clone(),
                qname.clone(),
                "empty where_clause node".to_string(),
                loc.clone(),
            )
        })?;
        Some(
            crate::parse::normalize_expr::from_pg_node(inner, None, loc).map_err(|e| {
                ParseError::PublicationFilterParse(
                    pub_name.clone(),
                    qname.clone(),
                    e.to_string(),
                    loc.clone(),
                )
            })?,
        )
    } else {
        None
    };

    Ok(PublishedTable {
        qname,
        row_filter,
        columns,
    })
}

fn extract_schema_name(
    spec: &PublicationObjSpec,
    pub_name: &Identifier,
    loc: &SourceLocation,
) -> Result<Identifier, ParseError> {
    if spec.name.is_empty() {
        return Err(ParseError::PublicationObjectMalformed(
            pub_name.clone(),
            loc.clone(),
        ));
    }
    Identifier::from_unquoted(&spec.name)
        .map_err(|e| ParseError::InvalidIdentifier(spec.name.clone(), e.to_string()))
}

// ── ALTER scope application ───────────────────────────────────────────────────

fn apply_scope_change(
    action_raw: i32,
    objs: &[pg_query::protobuf::Node],
    pub_: &mut Publication,
    name: &Identifier,
    loc: SourceLocation,
) -> Result<(), ParseError> {
    let action =
        AlterPublicationAction::try_from(action_raw).unwrap_or(AlterPublicationAction::Undefined);

    // Parse the incoming object specs.
    let mut new_tables: Vec<PublishedTable> = Vec::new();
    let mut new_schemas: BTreeSet<Identifier> = BTreeSet::new();

    for obj in objs {
        let Some(NodeEnum::PublicationObjSpec(spec)) = obj.node.as_ref() else {
            return Err(ParseError::PublicationObjectMalformed(name.clone(), loc));
        };
        let obj_type = PublicationObjSpecType::try_from(spec.pubobjtype)
            .unwrap_or(PublicationObjSpecType::Undefined);
        match obj_type {
            PublicationObjSpecType::PublicationobjTable => {
                let pt = extract_table_spec(spec, name, &loc)?;
                new_tables.push(pt);
            }
            PublicationObjSpecType::PublicationobjTablesInSchema => {
                let sn = extract_schema_name(spec, name, &loc)?;
                new_schemas.insert(sn);
            }
            PublicationObjSpecType::PublicationobjTablesInCurSchema => {
                return Err(ParseError::PublicationCurrentSchemaForm(name.clone(), loc));
            }
            PublicationObjSpecType::Undefined
            | PublicationObjSpecType::PublicationobjContinuation => {
                return Err(ParseError::UnknownPublicationObjectType(
                    spec.pubobjtype,
                    name.clone(),
                    loc,
                ));
            }
        }
    }

    match action {
        AlterPublicationAction::ApAddObjects => {
            // Add to Selective. If currently AllTables, wrap it first.
            ensure_selective(pub_);
            let PublicationScope::Selective { schemas, tables } = &mut pub_.scope else {
                unreachable!("ensure_selective guarantees Selective");
            };
            tables.extend(new_tables);
            schemas.extend(new_schemas);
        }
        AlterPublicationAction::ApDropObjects => {
            // Remove from Selective.
            if let PublicationScope::Selective { schemas, tables } = &mut pub_.scope {
                let drop_qnames: Vec<_> = new_tables.iter().map(|t| &t.qname).collect();
                tables.retain(|t| !drop_qnames.contains(&&t.qname));
                for s in &new_schemas {
                    schemas.remove(s);
                }
            }
        }
        AlterPublicationAction::ApSetObjects => {
            // Replace the entire scope with the new spec.
            if new_tables.is_empty() && new_schemas.is_empty() {
                // SET with an empty list — keep current, no-op for scope.
            } else {
                pub_.scope = PublicationScope::Selective {
                    schemas: new_schemas,
                    tables: new_tables,
                };
            }
        }
        AlterPublicationAction::Undefined => {
            // Undefined action with non-empty pubobjects — treat as SET.
            if !new_tables.is_empty() || !new_schemas.is_empty() {
                pub_.scope = PublicationScope::Selective {
                    schemas: new_schemas,
                    tables: new_tables,
                };
            }
        }
    }

    Ok(())
}

/// If `pub_` is currently `AllTables`, convert it to an empty `Selective`
/// so that ADD operations can target the selective lists.
fn ensure_selective(pub_: &mut Publication) {
    if matches!(pub_.scope, PublicationScope::AllTables) {
        pub_.scope = PublicationScope::Selective {
            schemas: BTreeSet::new(),
            tables: Vec::new(),
        };
    }
}

// ── WITH (…) option parsing ───────────────────────────────────────────────────

/// Parse a publication `WITH (...)` options list.
///
/// Returns `(publish, publish_via_partition_root)`. Both are `None` when the
/// relevant key is absent from the list.
fn parse_publication_options(
    options: &[pg_query::protobuf::Node],
    name: &Identifier,
    loc: SourceLocation,
) -> Result<(Option<PublishKinds>, Option<bool>), ParseError> {
    let mut publish: Option<PublishKinds> = None;
    let mut via_root: Option<bool> = None;

    for opt in options {
        let Some(NodeEnum::DefElem(def)) = opt.node.as_ref() else {
            return Err(ParseError::PublicationOptionMalformed(name.clone(), loc));
        };
        match def.defname.as_str() {
            "publish" => {
                let s = extract_def_elem_text(def, name, &loc)?;
                publish = Some(parse_publish_string(&s, name, &loc)?);
            }
            "publish_via_partition_root" => {
                via_root = Some(extract_def_elem_bool(def, name, &loc)?);
            }
            other => {
                return Err(ParseError::UnknownPublicationOption(
                    other.to_string(),
                    name.clone(),
                    loc,
                ));
            }
        }
    }

    Ok((publish, via_root))
}

fn parse_publish_string(
    s: &str,
    name: &Identifier,
    loc: &SourceLocation,
) -> Result<PublishKinds, ParseError> {
    let mut k = PublishKinds {
        insert: false,
        update: false,
        delete: false,
        truncate: false,
    };
    for part in s.split(',') {
        match part.trim().to_ascii_lowercase().as_str() {
            "insert" => k.insert = true,
            "update" => k.update = true,
            "delete" => k.delete = true,
            "truncate" => k.truncate = true,
            other => {
                return Err(ParseError::UnknownPublishKind(
                    other.to_string(),
                    name.clone(),
                    loc.clone(),
                ));
            }
        }
    }
    if k.is_empty() {
        return Err(ParseError::EmptyPublishBitset(name.clone(), loc.clone()));
    }
    Ok(k)
}

// ── Node extraction helpers ───────────────────────────────────────────────────

fn extract_string_value(node: &pg_query::protobuf::Node) -> Option<String> {
    match node.node.as_ref()? {
        NodeEnum::String(s) => Some(s.sval.clone()),
        _ => None,
    }
}

/// Extract the string value from a `DefElem.arg`, supporting all the encoding
/// forms `pg_query` may use for `publish = '...'` and similar string options.
fn extract_def_elem_text(
    def: &pg_query::protobuf::DefElem,
    name: &Identifier,
    loc: &SourceLocation,
) -> Result<String, ParseError> {
    let arg = def
        .arg
        .as_ref()
        .and_then(|n| n.node.as_ref())
        .ok_or_else(|| ParseError::PublicationOptionMalformed(name.clone(), loc.clone()))?;

    match arg {
        NodeEnum::String(s) => Ok(s.sval.clone()),
        NodeEnum::AConst(ac) => {
            use pg_query::protobuf::a_const::Val;
            match ac.val.as_ref() {
                Some(Val::Sval(s)) => Ok(s.sval.clone()),
                _ => Err(ParseError::PublicationOptionMalformed(
                    name.clone(),
                    loc.clone(),
                )),
            }
        }
        _ => Err(ParseError::PublicationOptionMalformed(
            name.clone(),
            loc.clone(),
        )),
    }
}

/// Extract a boolean value from a `DefElem.arg`, handling the multiple forms
/// `pg_query` may use (`Boolean`, `AConst`, `TypeName` as bare keyword).
fn extract_def_elem_bool(
    def: &pg_query::protobuf::DefElem,
    name: &Identifier,
    loc: &SourceLocation,
) -> Result<bool, ParseError> {
    let Some(arg_node) = def.arg.as_ref().and_then(|n| n.node.as_ref()) else {
        // Bare boolean (`WITH (publish_via_partition_root)`) means true.
        return Ok(true);
    };

    match arg_node {
        NodeEnum::Boolean(b) => Ok(b.boolval),
        NodeEnum::AConst(ac) => {
            use pg_query::protobuf::a_const::Val;
            match ac.val.as_ref() {
                Some(Val::Boolval(b)) => Ok(b.boolval),
                Some(Val::Sval(s)) => parse_bool_str(&s.sval, &def.defname, name, loc),
                _ => Err(ParseError::PublicationOptionMalformed(
                    name.clone(),
                    loc.clone(),
                )),
            }
        }
        NodeEnum::TypeName(tn) => {
            // pg_query encodes bare-keyword booleans (true/false/on/off) as TypeName.
            let raw = tn
                .names
                .iter()
                .rev()
                .find_map(|n| match n.node.as_ref() {
                    Some(NodeEnum::String(s)) if !s.sval.is_empty() => Some(s.sval.clone()),
                    _ => None,
                })
                .ok_or_else(|| ParseError::PublicationOptionMalformed(name.clone(), loc.clone()))?;
            parse_bool_str(&raw, &def.defname, name, loc)
        }
        NodeEnum::String(s) => parse_bool_str(&s.sval, &def.defname, name, loc),
        _ => Err(ParseError::PublicationOptionMalformed(
            name.clone(),
            loc.clone(),
        )),
    }
}

fn parse_bool_str(
    v: &str,
    key: &str,
    name: &Identifier,
    loc: &SourceLocation,
) -> Result<bool, ParseError> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "on" | "1" | "yes" => Ok(true),
        "false" | "off" | "0" | "no" => Ok(false),
        _ => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("publication {name:?}: option {key:?} = {v:?} is not a valid boolean"),
        }),
    }
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::parse::parse_directory;

    fn write(dir: &std::path::Path, rel: &str, contents: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    fn parse_source(sql: &str) -> Result<Catalog, ParseError> {
        let tmp = tempdir().expect("tempdir");
        write(tmp.path(), "schema.sql", sql);
        parse_directory(tmp.path(), &[])
    }

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    // ── helpers to exercise parse_create_publication / parse_alter_publication
    // directly (with a local accumulator) ─────────────────────────────────────

    fn parse_one_create_stmt(sql: &str) -> pg_query::protobuf::CreatePublicationStmt {
        let parsed = pg_query::parse(sql).expect("pg_query parse");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreatePublicationStmt(s) = node else {
            panic!("expected CreatePublicationStmt");
        };
        s
    }

    fn parse_one_alter_stmt(sql: &str) -> pg_query::protobuf::AlterPublicationStmt {
        let parsed = pg_query::parse(sql).expect("pg_query parse");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::AlterPublicationStmt(s) = node else {
            panic!("expected AlterPublicationStmt");
        };
        s
    }

    // ── CREATE tests ──────────────────────────────────────────────────────────

    #[test]
    fn create_for_all_tables() {
        let sql = "CREATE PUBLICATION p FOR ALL TABLES;";
        let stmt = parse_one_create_stmt(sql);
        let mut acc: BTreeMap<Identifier, Publication> = BTreeMap::new();
        parse_create_publication(&stmt, loc(), &mut acc).expect("ok");
        let p = acc.values().next().unwrap();
        assert_eq!(p.name.as_str(), "p");
        assert!(matches!(p.scope, PublicationScope::AllTables));
        assert_eq!(p.publish, PublishKinds::pg_default());
        assert!(!p.publish_via_partition_root);
    }

    #[test]
    fn create_for_table() {
        let sql = "CREATE PUBLICATION p FOR TABLE app.t;";
        let stmt = parse_one_create_stmt(sql);
        let mut acc: BTreeMap<Identifier, Publication> = BTreeMap::new();
        parse_create_publication(&stmt, loc(), &mut acc).expect("ok");
        let p = acc.values().next().unwrap();
        let PublicationScope::Selective { tables, schemas } = &p.scope else {
            panic!("expected Selective");
        };
        assert!(schemas.is_empty());
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].qname.schema.as_str(), "app");
        assert_eq!(tables[0].qname.name.as_str(), "t");
        assert!(tables[0].row_filter.is_none());
        assert!(tables[0].columns.is_none());
    }

    #[test]
    fn create_for_table_with_columns_and_filter() {
        let sql = "CREATE PUBLICATION p FOR TABLE app.t (col1, col2) WHERE (status = 'active');";
        let stmt = parse_one_create_stmt(sql);
        let mut acc: BTreeMap<Identifier, Publication> = BTreeMap::new();
        parse_create_publication(&stmt, loc(), &mut acc).expect("ok");
        let p = acc.values().next().unwrap();
        let PublicationScope::Selective { tables, .. } = &p.scope else {
            panic!("expected Selective");
        };
        let t = &tables[0];
        let cols = t.columns.as_ref().expect("columns");
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].as_str(), "col1");
        assert_eq!(cols[1].as_str(), "col2");
        assert!(t.row_filter.is_some());
    }

    #[test]
    fn create_for_tables_in_schema() {
        let sql = "CREATE PUBLICATION p FOR TABLES IN SCHEMA app;";
        let stmt = parse_one_create_stmt(sql);
        let mut acc: BTreeMap<Identifier, Publication> = BTreeMap::new();
        parse_create_publication(&stmt, loc(), &mut acc).expect("ok");
        let p = acc.values().next().unwrap();
        let PublicationScope::Selective { schemas, tables } = &p.scope else {
            panic!("expected Selective");
        };
        assert_eq!(schemas.len(), 1);
        assert!(schemas.contains(&Identifier::from_unquoted("app").unwrap()));
        assert!(tables.is_empty());
    }

    #[test]
    fn create_with_publish_options() {
        let sql = "CREATE PUBLICATION p FOR ALL TABLES WITH (publish = 'insert, update', publish_via_partition_root = true);";
        let stmt = parse_one_create_stmt(sql);
        let mut acc: BTreeMap<Identifier, Publication> = BTreeMap::new();
        parse_create_publication(&stmt, loc(), &mut acc).expect("ok");
        let p = acc.values().next().unwrap();
        assert!(p.publish.insert);
        assert!(p.publish.update);
        assert!(!p.publish.delete);
        assert!(!p.publish.truncate);
        assert!(p.publish_via_partition_root);
    }

    // ── ALTER folding tests ───────────────────────────────────────────────────

    #[test]
    fn alter_add_table_folds_with_create() {
        let create_sql = "CREATE PUBLICATION p FOR TABLE app.t1;";
        let alter_sql = "ALTER PUBLICATION p ADD TABLE app.t2;";
        let create = parse_one_create_stmt(create_sql);
        let alter = parse_one_alter_stmt(alter_sql);
        let mut acc: BTreeMap<Identifier, Publication> = BTreeMap::new();
        parse_create_publication(&create, loc(), &mut acc).expect("create ok");
        parse_alter_publication(&alter, loc(), &mut acc).expect("alter ok");
        let p = acc.values().next().unwrap();
        let PublicationScope::Selective { tables, .. } = &p.scope else {
            panic!("expected Selective");
        };
        assert_eq!(tables.len(), 2);
        let names: Vec<_> = tables.iter().map(|t| t.qname.name.as_str()).collect();
        assert!(names.contains(&"t1"));
        assert!(names.contains(&"t2"));
    }

    #[test]
    fn alter_drop_table() {
        let create_sql = "CREATE PUBLICATION p FOR TABLE app.t1, app.t2;";
        let alter_sql = "ALTER PUBLICATION p DROP TABLE app.t1;";
        let create = parse_one_create_stmt(create_sql);
        let alter = parse_one_alter_stmt(alter_sql);
        let mut acc: BTreeMap<Identifier, Publication> = BTreeMap::new();
        parse_create_publication(&create, loc(), &mut acc).expect("create ok");
        parse_alter_publication(&alter, loc(), &mut acc).expect("alter ok");
        let p = acc.values().next().unwrap();
        let PublicationScope::Selective { tables, .. } = &p.scope else {
            panic!("expected Selective");
        };
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].qname.name.as_str(), "t2");
    }

    #[test]
    fn alter_set_publish() {
        let create_sql = "CREATE PUBLICATION p FOR ALL TABLES;";
        let alter_sql = "ALTER PUBLICATION p SET (publish = 'insert');";
        let create = parse_one_create_stmt(create_sql);
        let alter = parse_one_alter_stmt(alter_sql);
        let mut acc: BTreeMap<Identifier, Publication> = BTreeMap::new();
        parse_create_publication(&create, loc(), &mut acc).expect("create ok");
        parse_alter_publication(&alter, loc(), &mut acc).expect("alter ok");
        let p = acc.values().next().unwrap();
        assert!(p.publish.insert);
        assert!(!p.publish.update);
        assert!(!p.publish.delete);
        assert!(!p.publish.truncate);
    }

    // ── Error cases ───────────────────────────────────────────────────────────

    #[test]
    fn alter_before_create_errors() {
        let alter_sql = "ALTER PUBLICATION p ADD TABLE app.t;";
        let alter = parse_one_alter_stmt(alter_sql);
        let mut acc: BTreeMap<Identifier, Publication> = BTreeMap::new();
        let err = parse_alter_publication(&alter, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::AlterPublicationBeforeCreate(_, _)),
            "got: {err:?}"
        );
    }

    #[test]
    fn for_all_tables_with_objects_errors() {
        // This SQL is rejected by pg_query at parse time (PG doesn't allow
        // FOR ALL TABLES combined with FOR TABLE). Test the guard anyway via
        // direct function call with a crafted stmt.
        let mut stmt = parse_one_create_stmt("CREATE PUBLICATION p FOR ALL TABLES;");
        // Inject a fake pubobjects to simulate the guard.
        let dummy_node = pg_query::protobuf::Node { node: None };
        stmt.pubobjects.push(dummy_node);
        stmt.for_all_tables = true;
        let mut acc: BTreeMap<Identifier, Publication> = BTreeMap::new();
        let err = parse_create_publication(&stmt, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::PublicationAllTablesWithObjects(_, _)),
            "got: {err:?}"
        );
    }

    #[test]
    fn empty_publication_scope_errors() {
        // pg_query rejects `CREATE PUBLICATION p;` (no FOR clause) as a syntax
        // error, but we can reach the guard via parse_selective_scope directly.
        let err = parse_selective_scope(&[], &Identifier::from_unquoted("p").unwrap(), &loc())
            .unwrap_err();
        assert!(
            matches!(err, ParseError::EmptyPublicationScope(_, _)),
            "got: {err:?}"
        );
    }

    #[test]
    fn unknown_publish_kind_errors() {
        let sql = "CREATE PUBLICATION p FOR ALL TABLES WITH (publish = 'insert, bogus');";
        let stmt = parse_one_create_stmt(sql);
        let mut acc: BTreeMap<Identifier, Publication> = BTreeMap::new();
        let err = parse_create_publication(&stmt, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::UnknownPublishKind(ref s, _, _) if s == "bogus"),
            "got: {err:?}"
        );
    }

    // ── Integration tests via parse_directory ──────────────────────────────────

    #[test]
    fn parse_directory_create_for_all_tables() {
        let sql = "CREATE PUBLICATION audit_all FOR ALL TABLES;";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.publications.len(), 1);
        assert_eq!(cat.publications[0].name.as_str(), "audit_all");
        assert!(matches!(
            cat.publications[0].scope,
            PublicationScope::AllTables
        ));
    }

    #[test]
    fn parse_directory_folded_create_and_alter() {
        let sql = "
            CREATE PUBLICATION pub1 FOR TABLE app.t1;
            ALTER PUBLICATION pub1 ADD TABLE app.t2;
        ";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.publications.len(), 1);
        let PublicationScope::Selective { tables, .. } = &cat.publications[0].scope else {
            panic!("expected Selective");
        };
        assert_eq!(tables.len(), 2);
    }

    #[test]
    fn rename_publication_in_source_errors() {
        // RENAME TO is encoded as RenameStmt, which isn't in the whitelist yet.
        // Verify we get UnsupportedObjectKind (the stmt.classify path).
        let sql = "ALTER PUBLICATION p RENAME TO q;";
        let err = parse_source(sql).expect_err("should fail");
        // RenameStmt falls through to the unsupported arm in statement.rs.
        assert!(
            matches!(
                err,
                ParseError::UnsupportedObjectKind { .. } | ParseError::Structural { .. }
            ),
            "got: {err:?}"
        );
    }
}
