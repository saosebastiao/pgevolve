//! Parser for `CREATE TEXT SEARCH DICTIONARY`, `CREATE TEXT SEARCH CONFIGURATION`,
//! and their `ALTER` / `COMMENT ON` / `ALTER OWNER TO` counterparts.
//!
//! ## AST structure (`pg_query` 6.1.1)
//!
//! ### CREATE TEXT SEARCH DICTIONARY
//! `DefineStmt { kind: ObjectTsdictionary, defnames: [schema, name], definition: [DefElem…] }`
//! - `definition` contains:
//!   - `template = <TypeName>` — the template qname (e.g. `snowball`, `pg_catalog.snowball`).
//!   - Any other `DefElem` — arbitrary key/value option. Values arrive as `String`
//!     (bare identifier like `english`) or `TypeName` (when the parser sees a bare
//!     word that looks like a keyword/name).  Either is stringified as the option value.
//!
//! ### CREATE TEXT SEARCH CONFIGURATION
//! `DefineStmt { kind: ObjectTsconfiguration, defnames: …, definition: [DefElem…] }`
//! - `definition` has exactly one relevant key:
//!   - `parser = <TypeName>` — parser qname.
//!   - `copy = <TypeName>` — rejected (unsupported).
//!
//! ### ALTER TEXT SEARCH DICTIONARY
//! `AlterTsDictionaryStmt { dictname: [schema, name], options: [DefElem…] }`
//! Options are parsed identically to the CREATE options (excluding `template`).
//!
//! ### ALTER TEXT SEARCH CONFIGURATION
//! `AlterTsConfigurationStmt { kind: i32, cfgname, tokentype, dicts, … }`
//! - `kind` encodes the sub-command (add/alter/replace/drop).
//! - `tokentype` is a list of String nodes for the token aliases.
//! - `dicts` is a list of List nodes, each holding [schema, name] String nodes for one
//!   dictionary reference. (For AddMapping/AlterMappingForToken they are single-dict
//!   nodes; for ReplaceDict/ReplaceDictForToken `dicts[0]` is the old dict and `dicts[1]`
//!   the new dict.)

use std::collections::BTreeMap;

use pg_query::NodeEnum;
use pg_query::protobuf::{
    AlterOwnerStmt, AlterTsConfigType, AlterTsConfigurationStmt, AlterTsDictionaryStmt,
    CommentStmt, DefElem, DefineStmt,
};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::text_search::{TsConfiguration, TsDictionary, TsMapping};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

// ── CREATE TEXT SEARCH DICTIONARY ────────────────────────────────────────────

/// Build a [`TsDictionary`] from a `CREATE TEXT SEARCH DICTIONARY` `DefineStmt`
/// and append it to the accumulator.
///
/// Rejects missing `TEMPLATE`, unknown option values, and duplicate qnames.
pub(crate) fn parse_create_dictionary(
    stmt: &DefineStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    existing: &mut Vec<TsDictionary>,
) -> Result<(), ParseError> {
    let qname = shared::qname_from_string_list(&stmt.defnames, default_schema, location)?;

    let mut template: Option<QualifiedName> = None;
    let mut options: Vec<(String, String)> = Vec::new();

    for node in &stmt.definition {
        let Some(NodeEnum::DefElem(de)) = node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "CREATE TEXT SEARCH DICTIONARY {qname}: unexpected non-DefElem in option list"
                ),
            });
        };
        let key = de.defname.to_ascii_lowercase();
        if key == "template" {
            template = Some(qname_from_defelem_typename(
                de, "template", &qname, location,
            )?);
        } else {
            let value = string_value_from_defelem(de, &key, &qname, location)?;
            options.push((key, value));
        }
    }

    let template = template.ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: format!(
            "CREATE TEXT SEARCH DICTIONARY {qname}: missing required option `template`"
        ),
    })?;

    if existing.iter().any(|d| d.qname == qname) {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("duplicate text search dictionary {qname}"),
        });
    }

    existing.push(TsDictionary {
        qname,
        template,
        options,
        owner: None,
        comment: None,
    });
    Ok(())
}

// ── ALTER TEXT SEARCH DICTIONARY ─────────────────────────────────────────────

/// Apply an `ALTER TEXT SEARCH DICTIONARY name (opts)` to the accumulator.
///
/// Replaces the entire `options` list (Postgres `ALTER TEXT SEARCH DICTIONARY`
/// sets the given options; we model source state declaratively so the last ALTER
/// wins — same semantics as the accumulator for collations/aggregates).
pub(crate) fn apply_alter_dictionary(
    stmt: &AlterTsDictionaryStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    existing: &mut [TsDictionary],
) -> Result<(), ParseError> {
    let qname = shared::qname_from_string_list(&stmt.dictname, default_schema, location)?;

    let mut options: Vec<(String, String)> = Vec::new();
    for node in &stmt.options {
        let Some(NodeEnum::DefElem(de)) = node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "ALTER TEXT SEARCH DICTIONARY {qname}: unexpected non-DefElem in option list"
                ),
            });
        };
        let key = de.defname.to_ascii_lowercase();
        let value = string_value_from_defelem(de, &key, &qname, location)?;
        options.push((key, value));
    }

    let dict = find_dict_mut(existing, &qname, location)?;
    dict.options = options;
    Ok(())
}

// ── CREATE TEXT SEARCH CONFIGURATION ─────────────────────────────────────────

/// Build a [`TsConfiguration`] from a `CREATE TEXT SEARCH CONFIGURATION`
/// `DefineStmt` and append it to the accumulator.
///
/// Rejects `COPY =` (unsupported), missing `PARSER`, and duplicate qnames.
pub(crate) fn parse_create_configuration(
    stmt: &DefineStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    existing: &mut Vec<TsConfiguration>,
) -> Result<(), ParseError> {
    let qname = shared::qname_from_string_list(&stmt.defnames, default_schema, location)?;

    let mut parser: Option<QualifiedName> = None;

    for node in &stmt.definition {
        let Some(NodeEnum::DefElem(de)) = node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "CREATE TEXT SEARCH CONFIGURATION {qname}: \
                     unexpected non-DefElem in option list"
                ),
            });
        };
        let key = de.defname.to_ascii_lowercase();
        if key == "copy" {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "CREATE TEXT SEARCH CONFIGURATION {qname} (COPY = …) is not supported — \
                     declare PARSER and explicit mappings instead"
                ),
            });
        }
        if key == "parser" {
            parser = Some(qname_from_defelem_typename(de, "parser", &qname, location)?);
        } else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "CREATE TEXT SEARCH CONFIGURATION {qname}: unknown option `{key}`"
                ),
            });
        }
    }

    let parser = parser.ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: format!(
            "CREATE TEXT SEARCH CONFIGURATION {qname}: missing required option `parser`"
        ),
    })?;

    if existing.iter().any(|c| c.qname == qname) {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("duplicate text search configuration {qname}"),
        });
    }

    existing.push(TsConfiguration {
        qname,
        parser,
        mappings: Vec::new(),
        owner: None,
        comment: None,
    });
    Ok(())
}

// ── ALTER TEXT SEARCH CONFIGURATION ──────────────────────────────────────────

/// Apply an `ALTER TEXT SEARCH CONFIGURATION` statement to the accumulator.
///
/// Handles the five sub-command kinds defined by [`AlterTsConfigType`]:
/// - `AddMapping` / `AlterMappingForToken` — set token→dict chain mappings.
/// - `DropMapping` — remove token-type entries.
/// - `ReplaceDict` — replace one dict across all mappings.
/// - `ReplaceDictForToken` — replace one dict within specific token mappings.
pub(crate) fn apply_alter_configuration(
    stmt: &AlterTsConfigurationStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    existing: &mut [TsConfiguration],
) -> Result<(), ParseError> {
    let qname = shared::qname_from_string_list(&stmt.cfgname, default_schema, location)?;

    let kind = AlterTsConfigType::try_from(stmt.kind)
        .unwrap_or(AlterTsConfigType::AlterTsconfigTypeUndefined);

    let cfg = find_cfg_mut(existing, &qname, location)?;

    match kind {
        AlterTsConfigType::AlterTsconfigAddMapping
        | AlterTsConfigType::AlterTsconfigAlterMappingForToken => {
            // Build the dict chain from `dicts` — each is a List of String nodes.
            let dict_chain = dict_chain_from_nodes(&stmt.dicts, default_schema, location)?;
            // Apply to each token alias in `tokentype`.
            let tokens = token_list(&stmt.tokentype, location)?;
            let mut mapping_map: BTreeMap<String, Vec<QualifiedName>> = cfg
                .mappings
                .drain(..)
                .map(|m| (m.token_type, m.dictionaries))
                .collect();
            for token in tokens {
                mapping_map.insert(token, dict_chain.clone());
            }
            cfg.mappings = mapping_map
                .into_iter()
                .map(|(token_type, dictionaries)| TsMapping {
                    token_type,
                    dictionaries,
                })
                .collect();
        }
        AlterTsConfigType::AlterTsconfigDropMapping => {
            let tokens = token_list(&stmt.tokentype, location)?;
            cfg.mappings.retain(|m| !tokens.contains(&m.token_type));
        }
        AlterTsConfigType::AlterTsconfigReplaceDict => {
            // `dicts` holds [old_dict_node, new_dict_node].
            let (old_dict, new_dict) = two_dict_pair(&stmt.dicts, default_schema, location)?;
            for mapping in &mut cfg.mappings {
                for d in &mut mapping.dictionaries {
                    if *d == old_dict {
                        *d = new_dict.clone();
                    }
                }
            }
        }
        AlterTsConfigType::AlterTsconfigReplaceDictForToken => {
            let (old_dict, new_dict) = two_dict_pair(&stmt.dicts, default_schema, location)?;
            let tokens = token_list(&stmt.tokentype, location)?;
            for mapping in &mut cfg.mappings {
                if tokens.contains(&mapping.token_type) {
                    for d in &mut mapping.dictionaries {
                        if *d == old_dict {
                            *d = new_dict.clone();
                        }
                    }
                }
            }
        }
        AlterTsConfigType::AlterTsconfigTypeUndefined => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "ALTER TEXT SEARCH CONFIGURATION {qname}: unknown sub-command kind"
                ),
            });
        }
    }
    Ok(())
}

// ── OWNER TO ─────────────────────────────────────────────────────────────────

/// Apply `ALTER TEXT SEARCH DICTIONARY name OWNER TO role` to the accumulator.
pub(crate) fn apply_dictionary_owner(
    stmt: &AlterOwnerStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    existing: &mut [TsDictionary],
) -> Result<(), ParseError> {
    let qname = ts_qname_from_alter_owner(stmt, default_schema, location)?;
    let new_owner = crate::parse::builder::owner_stmt::extract_new_owner(stmt, location)?;
    let dict = find_dict_mut(existing, &qname, location)?;
    dict.owner = Some(new_owner);
    Ok(())
}

/// Apply `ALTER TEXT SEARCH CONFIGURATION name OWNER TO role` to the accumulator.
pub(crate) fn apply_configuration_owner(
    stmt: &AlterOwnerStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    existing: &mut [TsConfiguration],
) -> Result<(), ParseError> {
    let qname = ts_qname_from_alter_owner(stmt, default_schema, location)?;
    let new_owner = crate::parse::builder::owner_stmt::extract_new_owner(stmt, location)?;
    let cfg = find_cfg_mut(existing, &qname, location)?;
    cfg.owner = Some(new_owner);
    Ok(())
}

// ── COMMENT ON ───────────────────────────────────────────────────────────────

/// Apply `COMMENT ON TEXT SEARCH DICTIONARY name IS '…'` to the accumulator.
pub(crate) fn apply_dictionary_comment(
    stmt: &CommentStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    existing: &mut [TsDictionary],
) -> Result<(), ParseError> {
    let qname = ts_qname_from_comment(stmt, default_schema, location)?;
    let comment = if stmt.comment.is_empty() {
        None
    } else {
        Some(stmt.comment.clone())
    };
    let dict = find_dict_mut(existing, &qname, location)?;
    dict.comment = comment;
    Ok(())
}

/// Apply `COMMENT ON TEXT SEARCH CONFIGURATION name IS '…'` to the accumulator.
pub(crate) fn apply_configuration_comment(
    stmt: &CommentStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    existing: &mut [TsConfiguration],
) -> Result<(), ParseError> {
    let qname = ts_qname_from_comment(stmt, default_schema, location)?;
    let comment = if stmt.comment.is_empty() {
        None
    } else {
        Some(stmt.comment.clone())
    };
    let cfg = find_cfg_mut(existing, &qname, location)?;
    cfg.comment = comment;
    Ok(())
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Extract a `QualifiedName` from a `DefElem.arg` that holds a `TypeName` node.
///
/// Used for `template = snowball`, `parser = pg_catalog.default`, etc.
/// The `TypeName.names` field is a list of String nodes (1–2 parts).
fn qname_from_defelem_typename(
    de: &DefElem,
    option: &str,
    context_qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let arg = de
        .arg
        .as_ref()
        .and_then(|n| n.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("TEXT SEARCH object {context_qname}: option `{option}` has no value"),
        })?;
    // An unqualified template/parser name resolves to `pg_catalog` — that is
    // the implicit home of every built-in template (`snowball`, `simple`, …)
    // and parser (`default`), and is exactly what the reader produces from
    // `pg_ts_template`/`pg_ts_parser`. Defaulting to the object's own schema
    // instead would make `TEMPLATE = snowball` round-trip as `<schema>.snowball`
    // and diverge from the reader's `pg_catalog.snowball` on every run. Mirrors
    // the unqualified-type default in `cast_stmt`.
    let pg_catalog = Identifier::from_unquoted("pg_catalog").expect("static identifier");
    match arg {
        NodeEnum::TypeName(tn) => {
            shared::qname_from_string_list(&tn.names, Some(&pg_catalog), location)
        }
        NodeEnum::List(list) => {
            // Some pg_query versions encode the name as a raw List of Strings.
            shared::qname_from_string_list(&list.items, Some(&pg_catalog), location)
        }
        NodeEnum::String(s) => {
            // Bare unqualified keyword — built-in template/parser in pg_catalog.
            let name = shared::ident(&s.sval, location)?;
            Ok(QualifiedName::new(pg_catalog, name))
        }
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "TEXT SEARCH object {context_qname}: option `{option}` \
                 has unexpected value kind {:?}",
                std::mem::discriminant(other)
            ),
        }),
    }
}

/// Extract the string value of a `DefElem` for an arbitrary text-search option.
///
/// Accepts `String` (quoted literal), `TypeName` (bare keyword/name), and
/// `AConst { Sval }` variants.
fn string_value_from_defelem(
    de: &DefElem,
    key: &str,
    context_qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<String, ParseError> {
    let arg = de
        .arg
        .as_ref()
        .and_then(|n| n.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("TEXT SEARCH object {context_qname}: option `{key}` has no value"),
        })?;
    match arg {
        NodeEnum::String(s) => Ok(s.sval.clone()),
        NodeEnum::TypeName(tn) => {
            // Bare keyword: take the last String segment of the names list.
            tn.names
                .iter()
                .rev()
                .find_map(|n| match n.node.as_ref() {
                    Some(NodeEnum::String(s)) if !s.sval.is_empty() => Some(s.sval.clone()),
                    _ => None,
                })
                .ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: format!(
                        "TEXT SEARCH object {context_qname}: option `{key}` value is empty"
                    ),
                })
        }
        NodeEnum::AConst(ac) => {
            use pg_query::protobuf::a_const::Val;
            match ac.val.as_ref() {
                Some(Val::Sval(s)) => Ok(s.sval.clone()),
                Some(Val::Ival(i)) => Ok(i.ival.to_string()),
                _ => Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!(
                        "TEXT SEARCH object {context_qname}: option `{key}` has unrecognized value"
                    ),
                }),
            }
        }
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "TEXT SEARCH object {context_qname}: option `{key}` \
                 has unexpected value kind {:?}",
                std::mem::discriminant(other)
            ),
        }),
    }
}

/// Collect the token-type strings from an `AlterTsConfigurationStmt.tokentype` list.
///
/// Each item in the list is a String node holding the alias name.
fn token_list(
    nodes: &[pg_query::protobuf::Node],
    location: &SourceLocation,
) -> Result<Vec<String>, ParseError> {
    nodes
        .iter()
        .map(|n| match n.node.as_ref() {
            Some(NodeEnum::String(s)) => Ok(s.sval.clone()),
            other => Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "ALTER TEXT SEARCH CONFIGURATION: expected String in tokentype list, \
                     got {:?}",
                    other.map(std::mem::discriminant)
                ),
            }),
        })
        .collect()
}

/// Build the dictionary `QualifiedName` chain from an `AlterTsConfigurationStmt.dicts` list.
///
/// For `ADD MAPPING … WITH d1, d2` the dicts list holds items where each is either:
/// - A `List` of String nodes `[schema, name]` — a schema-qualified dict reference.
/// - A `TypeName` node — a dict reference expressed as a type-name.
fn dict_chain_from_nodes(
    nodes: &[pg_query::protobuf::Node],
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Vec<QualifiedName>, ParseError> {
    nodes
        .iter()
        .map(|n| dict_qname_from_node(n, default_schema, location))
        .collect()
}

/// Resolve a single dictionary-reference node to a `QualifiedName`.
fn dict_qname_from_node(
    node: &pg_query::protobuf::Node,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    // An unqualified mapping dictionary defaults to `pg_catalog` (the home of
    // the built-in dictionaries — `simple`, `english_stem`, …, the common
    // unqualified case — and what the reader produces for them). A managed
    // dictionary in a user schema must be written qualified (`WITH app.en`),
    // matching the reader's qualified output. A qualified `[schema, name]` list
    // ignores the default. Consistent with the template/parser default above.
    let pg_catalog = Identifier::from_unquoted("pg_catalog").expect("static identifier");
    let default = default_schema.unwrap_or(&pg_catalog);
    match node.node.as_ref() {
        Some(NodeEnum::List(list)) => {
            shared::qname_from_string_list(&list.items, Some(default), location)
        }
        Some(NodeEnum::TypeName(tn)) => {
            shared::qname_from_string_list(&tn.names, Some(default), location)
        }
        Some(NodeEnum::String(s)) => {
            // Single unqualified name — built-in dictionary in pg_catalog.
            let name = shared::ident(&s.sval, location)?;
            Ok(QualifiedName::new(default.clone(), name))
        }
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "ALTER TEXT SEARCH CONFIGURATION: unexpected dict reference node kind {:?}",
                other.map(std::mem::discriminant)
            ),
        }),
    }
}

/// For `REPLACE DICT old WITH new` commands: extract the (old, new) pair from `dicts`.
fn two_dict_pair(
    nodes: &[pg_query::protobuf::Node],
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<(QualifiedName, QualifiedName), ParseError> {
    if nodes.len() != 2 {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "ALTER TEXT SEARCH CONFIGURATION REPLACE DICT: expected 2 dict references, \
                 got {}",
                nodes.len()
            ),
        });
    }
    let old = dict_qname_from_node(&nodes[0], default_schema, location)?;
    let new = dict_qname_from_node(&nodes[1], default_schema, location)?;
    Ok((old, new))
}

/// Extract a `QualifiedName` from an `AlterOwnerStmt.object` for text-search objects.
///
/// `pg_query` encodes these as a `List` of String nodes.
fn ts_qname_from_alter_owner(
    stmt: &AlterOwnerStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let node = stmt
        .object
        .as_ref()
        .and_then(|o| o.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "ALTER TEXT SEARCH … OWNER: missing object reference".into(),
        })?;
    match node {
        NodeEnum::List(list) => {
            shared::qname_from_string_list(&list.items, default_schema, location)
        }
        NodeEnum::TypeName(tn) => {
            shared::qname_from_string_list(&tn.names, default_schema, location)
        }
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "ALTER TEXT SEARCH … OWNER: unexpected object node kind {:?}",
                std::mem::discriminant(other)
            ),
        }),
    }
}

/// Extract a `QualifiedName` from a `CommentStmt.object` for text-search objects.
///
/// `pg_query` encodes these as a `List` of String nodes.
fn ts_qname_from_comment(
    stmt: &CommentStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let node = stmt
        .object
        .as_ref()
        .and_then(|o| o.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "COMMENT ON TEXT SEARCH …: missing object reference".into(),
        })?;
    match node {
        NodeEnum::List(list) => {
            shared::qname_from_string_list(&list.items, default_schema, location)
        }
        NodeEnum::TypeName(tn) => {
            shared::qname_from_string_list(&tn.names, default_schema, location)
        }
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "COMMENT ON TEXT SEARCH …: unexpected object node kind {:?}",
                std::mem::discriminant(other)
            ),
        }),
    }
}

/// Find a [`TsDictionary`] by qname for mutation, or return an error.
fn find_dict_mut<'a>(
    existing: &'a mut [TsDictionary],
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<&'a mut TsDictionary, ParseError> {
    existing
        .iter_mut()
        .find(|d| d.qname == *qname)
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!(
                "text search dictionary {qname} referenced before it is created in source"
            ),
        })
}

/// Find a [`TsConfiguration`] by qname for mutation, or return an error.
fn find_cfg_mut<'a>(
    existing: &'a mut [TsConfiguration],
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<&'a mut TsConfiguration, ParseError> {
    existing
        .iter_mut()
        .find(|c| c.qname == *qname)
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!(
                "text search configuration {qname} referenced before it is created in source"
            ),
        })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;

    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::parse::{ParseError, parse_directory};

    fn write(dir: &Path, rel: &str, contents: &str) {
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

    fn loc() -> crate::parse::SourceLocation {
        crate::parse::SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    // ── Dictionary tests ──────────────────────────────────────────────────────

    #[test]
    fn create_dictionary_template_and_option() {
        let sql = "CREATE SCHEMA app;\n\
            CREATE TEXT SEARCH DICTIONARY app.d \
            (TEMPLATE = snowball, language = 'english');";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.ts_dictionaries.len(), 1);
        let d = &cat.ts_dictionaries[0];
        assert_eq!(d.qname.to_string(), "app.d");
        // Unqualified `TEMPLATE = snowball` resolves to the built-in's home
        // (`pg_catalog`), matching what the reader produces — not the dict's
        // own schema (which would diff-loop forever).
        assert_eq!(d.template.to_string(), "pg_catalog.snowball");
        assert_eq!(d.options.len(), 1);
        assert_eq!(
            d.options[0],
            ("language".to_string(), "english".to_string())
        );
        assert!(d.owner.is_none());
        assert!(d.comment.is_none());
        // Print debug representation for caller verification.
        println!("CREATE DICTIONARY debug: {d:#?}");
    }

    #[test]
    fn create_dictionary_with_pg_catalog_template() {
        let sql = "CREATE SCHEMA app;\n\
            CREATE TEXT SEARCH DICTIONARY app.d \
            (TEMPLATE = pg_catalog.snowball, language = 'english');";
        let cat = parse_source(sql).expect("parses");
        let d = &cat.ts_dictionaries[0];
        assert_eq!(d.template.to_string(), "pg_catalog.snowball");
    }

    #[test]
    fn alter_dictionary_updates_options() {
        let sql = "CREATE SCHEMA app;\n\
            CREATE TEXT SEARCH DICTIONARY app.d \
            (TEMPLATE = snowball, language = 'english');\n\
            ALTER TEXT SEARCH DICTIONARY app.d (language = 'french');";
        let cat = parse_source(sql).expect("parses");
        let d = &cat.ts_dictionaries[0];
        assert_eq!(d.options.len(), 1);
        assert_eq!(d.options[0], ("language".to_string(), "french".to_string()));
    }

    #[test]
    fn create_dictionary_rejects_missing_template() {
        let sql = "CREATE SCHEMA app;\n\
            CREATE TEXT SEARCH DICTIONARY app.d (language = 'english');";
        let err = parse_source(sql).expect_err("should reject");
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("template"), "msg: {msg}");
    }

    #[test]
    fn create_dictionary_rejects_duplicate() {
        let sql = "CREATE SCHEMA app;\n\
            CREATE TEXT SEARCH DICTIONARY app.d (TEMPLATE = snowball);\n\
            CREATE TEXT SEARCH DICTIONARY app.d (TEMPLATE = simple);";
        let err = parse_source(sql).expect_err("should reject");
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("duplicate"), "msg: {msg}");
    }

    #[test]
    fn dictionary_comment_applies() {
        let sql = "CREATE SCHEMA app;\n\
            CREATE TEXT SEARCH DICTIONARY app.d (TEMPLATE = snowball);\n\
            COMMENT ON TEXT SEARCH DICTIONARY app.d IS 'x';";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.ts_dictionaries[0].comment.as_deref(), Some("x"));
    }

    #[test]
    fn rejects_drop_dictionary_in_source() {
        let sql = "CREATE SCHEMA app;\n\
            CREATE TEXT SEARCH DICTIONARY app.d (TEMPLATE = snowball);\n\
            DROP TEXT SEARCH DICTIONARY app.d;";
        let err = parse_source(sql).expect_err("should reject");
        assert!(matches!(err, ParseError::Structural { .. }), "got: {err:?}");
    }

    // ── Configuration tests ───────────────────────────────────────────────────

    #[test]
    fn create_configuration_with_parser() {
        let sql = r#"CREATE SCHEMA app;
            CREATE TEXT SEARCH CONFIGURATION app.c (PARSER = pg_catalog."default");"#;
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.ts_configurations.len(), 1);
        let c = &cat.ts_configurations[0];
        assert_eq!(c.qname.to_string(), "app.c");
        assert_eq!(c.parser.to_string(), "pg_catalog.default");
        assert!(c.mappings.is_empty());
        println!("CREATE CONFIGURATION debug: {c:#?}");
    }

    #[test]
    fn create_configuration_rejects_copy() {
        let sql = "CREATE SCHEMA app;\n\
            CREATE TEXT SEARCH CONFIGURATION app.c (COPY = app.other);";
        let err = parse_source(sql).expect_err("should reject COPY");
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("COPY") || msg.contains("copy"), "msg: {msg}");
    }

    #[test]
    fn add_mapping_for_two_tokens() {
        let sql = "CREATE SCHEMA app;\n\
            CREATE TEXT SEARCH DICTIONARY app.d (TEMPLATE = snowball);\n\
            CREATE TEXT SEARCH CONFIGURATION app.c (PARSER = pg_catalog.default);\n\
            ALTER TEXT SEARCH CONFIGURATION app.c \
            ADD MAPPING FOR word, asciiword WITH app.d;";
        let cat = parse_source(sql).expect("parses");
        let c = &cat.ts_configurations[0];
        assert_eq!(c.mappings.len(), 2, "expected 2 mappings");
        // Mappings are sorted by token_type in canon.
        let word = c
            .mappings
            .iter()
            .find(|m| m.token_type == "word")
            .expect("word mapping");
        let ascii = c
            .mappings
            .iter()
            .find(|m| m.token_type == "asciiword")
            .expect("asciiword mapping");
        assert_eq!(word.dictionaries.len(), 1);
        assert_eq!(word.dictionaries[0].to_string(), "app.d");
        assert_eq!(ascii.dictionaries.len(), 1);
        println!("ADD MAPPING debug: {c:#?}");
    }

    #[test]
    fn alter_mapping_for_token_chain() {
        // `pg_catalog.simple` is the built-in simple dictionary; must be qualified
        // because there is no default schema directive in this test.
        let sql = "CREATE SCHEMA app;\n\
            CREATE TEXT SEARCH DICTIONARY app.d (TEMPLATE = snowball);\n\
            CREATE TEXT SEARCH DICTIONARY app.simple (TEMPLATE = simple);\n\
            CREATE TEXT SEARCH CONFIGURATION app.c (PARSER = pg_catalog.default);\n\
            ALTER TEXT SEARCH CONFIGURATION app.c \
            ADD MAPPING FOR word WITH app.d;\n\
            ALTER TEXT SEARCH CONFIGURATION app.c \
            ALTER MAPPING FOR word WITH app.d, app.simple;";
        let cat = parse_source(sql).expect("parses");
        let c = &cat.ts_configurations[0];
        let word = c
            .mappings
            .iter()
            .find(|m| m.token_type == "word")
            .expect("word mapping");
        assert_eq!(word.dictionaries.len(), 2, "expected chain of 2");
        assert_eq!(word.dictionaries[0].to_string(), "app.d");
        assert_eq!(word.dictionaries[1].to_string(), "app.simple");
    }

    #[test]
    fn drop_mapping_removes_token() {
        let sql = "CREATE SCHEMA app;\n\
            CREATE TEXT SEARCH DICTIONARY app.d (TEMPLATE = snowball);\n\
            CREATE TEXT SEARCH CONFIGURATION app.c (PARSER = pg_catalog.default);\n\
            ALTER TEXT SEARCH CONFIGURATION app.c \
            ADD MAPPING FOR word, asciiword WITH app.d;\n\
            ALTER TEXT SEARCH CONFIGURATION app.c DROP MAPPING FOR word;";
        let cat = parse_source(sql).expect("parses");
        let c = &cat.ts_configurations[0];
        assert_eq!(c.mappings.len(), 1, "word should be removed");
        assert_eq!(c.mappings[0].token_type, "asciiword");
    }

    #[test]
    fn configuration_comment_applies() {
        let sql = "CREATE SCHEMA app;\n\
            CREATE TEXT SEARCH CONFIGURATION app.c (PARSER = pg_catalog.default);\n\
            COMMENT ON TEXT SEARCH CONFIGURATION app.c IS 'my config';";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(
            cat.ts_configurations[0].comment.as_deref(),
            Some("my config")
        );
    }

    #[test]
    fn rejects_drop_configuration_in_source() {
        let sql = "CREATE SCHEMA app;\n\
            CREATE TEXT SEARCH CONFIGURATION app.c (PARSER = pg_catalog.default);\n\
            DROP TEXT SEARCH CONFIGURATION app.c;";
        let err = parse_source(sql).expect_err("should reject");
        assert!(matches!(err, ParseError::Structural { .. }), "got: {err:?}");
    }

    // ── Unit-level parse_create_dictionary test (mirrors aggregate_stmt unit test) ──

    #[test]
    fn parse_create_dictionary_unit() {
        let parsed = pg_query::parse(
            "CREATE TEXT SEARCH DICTIONARY app.d (TEMPLATE = snowball, language = 'english');",
        )
        .unwrap();
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .unwrap();
        let NodeEnum::DefineStmt(stmt) = node else {
            panic!("expected DefineStmt");
        };
        let mut acc: Vec<TsDictionary> = Vec::new();
        parse_create_dictionary(&stmt, None, &loc(), &mut acc).expect("ok");
        assert_eq!(acc.len(), 1);
        assert_eq!(acc[0].template.to_string(), "pg_catalog.snowball");
        assert_eq!(acc[0].options[0].0, "language");
        assert_eq!(acc[0].options[0].1, "english");
    }
}
