//! `ALTER TABLE` support for source DDL.
//!
//! Source SQL is declarative: tables, columns, constraints, etc. are stated as
//! the desired end-state via `CREATE`. Two classes of `ALTER TABLE` are accepted
//! because they cannot always be expressed inline:
//!
//! 1. **`ADD CONSTRAINT FOREIGN KEY`** — forward-referencing FKs: when two
//!    tables reference each other, neither can declare its FK inline because the
//!    other side does not exist yet at parse time.
//!
//! 2. **`ALTER COLUMN … SET STORAGE / SET COMPRESSION`** — per-column storage
//!    strategy and compression codec. These may appear after the `CREATE TABLE`
//!    (for example when the source is derived from `pg_dump`).
//!
//! Everything else raises [`ParseError::Structural`] pointing the user to the
//! declarative source-of-truth model.

use pg_query::NodeEnum;
use pg_query::protobuf::{
    AlterTableCmd, AlterTableStmt, AlterTableType, ConstrType, Constraint as PgConstraint,
    ObjectType, RoleSpecType,
};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::column::{Compression, StorageKind};
use crate::ir::constraint::Constraint;
use crate::ir::reloptions::TableStorageOptions;
use crate::parse::builder::create_stmt;
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// One forward-reference FK constraint to merge into a [`crate::ir::table::Table`]
/// after all tables have been built.
#[derive(Debug, Clone)]
pub struct PendingFk {
    /// Target table to attach the constraint to.
    pub target: QualifiedName,
    /// The constraint itself.
    pub constraint: Constraint,
}

/// A per-column attribute update (`SET STORAGE` / `SET COMPRESSION`) to apply
/// to an already-built [`crate::ir::column::Column`] once all tables exist.
#[derive(Debug, Clone)]
pub struct PendingColumnAttr {
    /// Table that owns the column.
    pub target: QualifiedName,
    /// Column to update.
    pub column: Identifier,
    /// The attribute change to apply.
    pub kind: PendingColumnAttrKind,
}

/// Which column attribute is being set.
#[derive(Debug, Clone)]
pub enum PendingColumnAttrKind {
    /// `ALTER COLUMN … SET STORAGE <strategy>`.
    Storage(StorageKind),
    /// `ALTER COLUMN … SET COMPRESSION <codec>` — `None` means `DEFAULT`
    /// (revert to cluster GUC).
    Compression(Option<Compression>),
}

/// An ownership assignment pending merge into the catalog.
#[derive(Debug, Clone)]
pub struct PendingOwner {
    /// Relation whose owner should be updated.
    pub target: QualifiedName,
    /// New owner role name.
    pub new_owner: Identifier,
}

/// An RLS mode toggle (`ENABLE / DISABLE / FORCE / NO FORCE ROW LEVEL SECURITY`)
/// pending application to its target table.
#[derive(Debug, Clone)]
pub struct PendingRlsToggle {
    /// Table to update.
    pub target: QualifiedName,
    /// The exact subcommand type — one of the four RLS `AlterTableType` variants.
    pub subtype: AlterTableType,
}

/// A `SET (...)` reloptions update from `ALTER TABLE / MATERIALIZED VIEW ... SET
/// (key = value, ...)`, pending merge into the target relation's `storage` field.
#[derive(Debug, Clone)]
pub struct PendingRelOptions {
    /// Relation whose storage options should be updated.
    pub target: QualifiedName,
    /// The decoded options to merge.
    pub options: TableStorageOptions,
}

/// A tablespace assignment from `ALTER TABLE … SET TABLESPACE <name>`, pending
/// merge into the target table's `tablespace` field.
#[derive(Debug, Clone)]
pub struct PendingTablespace {
    /// Table whose tablespace should be updated.
    pub target: QualifiedName,
    /// The new tablespace name.
    pub tablespace: crate::identifier::Identifier,
}

/// The combined output of processing one `ALTER TABLE` statement.
#[derive(Debug, Default)]
pub struct AlterTableOutput {
    /// Forward-reference FK constraints to merge after all tables are parsed.
    pub pending_fks: Vec<PendingFk>,
    /// Per-column attribute updates to apply after all tables are parsed.
    pub pending_column_attrs: Vec<PendingColumnAttr>,
    /// Ownership assignments for relation-family objects.
    pub pending_owners: Vec<PendingOwner>,
    /// RLS mode toggles (ENABLE/DISABLE/FORCE/NO FORCE ROW LEVEL SECURITY).
    pub pending_rls_toggles: Vec<PendingRlsToggle>,
    /// Reloption SET (...) updates to apply after all tables/MVs are parsed.
    pub pending_rel_options: Vec<PendingRelOptions>,
    /// Tablespace assignments from `ALTER TABLE … SET TABLESPACE <name>`.
    pub pending_tablespaces: Vec<PendingTablespace>,
}

/// Process an `ALTER TABLE` statement.
///
/// Returns a combined [`AlterTableOutput`] covering the two supported
/// subcommand classes. Any other subcommand raises [`ParseError::Structural`]
/// pointing the user to the declarative source-of-truth model.
pub fn build_alter_table(
    stmt: &AlterTableStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<AlterTableOutput, ParseError> {
    let relation = stmt
        .relation
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "ALTER TABLE missing relation".into(),
        })?;
    let target = shared::resolve_qname(relation, default_schema, location)?;

    let mut out = AlterTableOutput::default();
    for cmd_node in &stmt.cmds {
        let Some(NodeEnum::AlterTableCmd(cmd)) = cmd_node.node.as_ref() else {
            return Err(unsupported_alter(location));
        };
        process_cmd(cmd, &target, default_schema, location, &mut out)?;
    }
    Ok(out)
}

fn process_cmd(
    cmd: &AlterTableCmd,
    target: &QualifiedName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    out: &mut AlterTableOutput,
) -> Result<(), ParseError> {
    let subtype = AlterTableType::try_from(cmd.subtype).unwrap_or(AlterTableType::Undefined);
    match subtype {
        AlterTableType::AtAddConstraint => {
            let pending = process_add_constraint_cmd(cmd, target, default_schema, location)?;
            out.pending_fks.push(pending);
        }
        AlterTableType::AtSetStorage => {
            let pending = process_set_storage_cmd(cmd, target, location)?;
            out.pending_column_attrs.push(pending);
        }
        AlterTableType::AtSetCompression => {
            let pending = process_set_compression_cmd(cmd, target, location)?;
            out.pending_column_attrs.push(pending);
        }
        AlterTableType::AtChangeOwner => {
            let pending = process_change_owner_cmd(cmd, target, location)?;
            out.pending_owners.push(pending);
        }
        AlterTableType::AtEnableRowSecurity
        | AlterTableType::AtDisableRowSecurity
        | AlterTableType::AtForceRowSecurity
        | AlterTableType::AtNoForceRowSecurity => {
            out.pending_rls_toggles.push(PendingRlsToggle {
                target: target.clone(),
                subtype,
            });
        }
        AlterTableType::AtSetRelOptions => {
            let pending = process_set_rel_options_cmd(cmd, target, location)?;
            out.pending_rel_options.push(pending);
        }
        AlterTableType::AtSetTableSpace => {
            let pending = process_set_tablespace_cmd(cmd, target, location)?;
            out.pending_tablespaces.push(pending);
        }
        AlterTableType::AtResetRelOptions | AlterTableType::AtReplaceRelOptions => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: "ALTER TABLE ... RESET (...) in source is not supported — \
                          clear options out-of-band, then remove from source"
                    .into(),
            });
        }
        _ => return Err(unsupported_alter(location)),
    }
    Ok(())
}

fn process_add_constraint_cmd(
    cmd: &AlterTableCmd,
    target: &QualifiedName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<PendingFk, ParseError> {
    let con = cmd
        .def
        .as_ref()
        .and_then(|d| d.node.as_ref())
        .and_then(|n| match n {
            NodeEnum::Constraint(c) => Some(c.as_ref()),
            _ => None,
        })
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "ALTER TABLE ADD CONSTRAINT missing constraint definition".into(),
        })?;

    let kind = ConstrType::try_from(con.contype).unwrap_or(ConstrType::Undefined);
    if !matches!(kind, ConstrType::ConstrForeign) {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "ALTER TABLE may only ADD CONSTRAINT FOREIGN KEY in source DDL — \
                     other constraint kinds belong inline in the CREATE TABLE that \
                     declares them"
                .into(),
        });
    }

    let constraint = build_fk_constraint(con, target, default_schema, location)?;
    Ok(PendingFk {
        target: target.clone(),
        constraint,
    })
}

fn process_set_storage_cmd(
    cmd: &AlterTableCmd,
    target: &QualifiedName,
    location: &SourceLocation,
) -> Result<PendingColumnAttr, ParseError> {
    // cmd.name = column name; cmd.def = String node with lowercase strategy keyword.
    let column = shared::ident(&cmd.name, location)?;
    let keyword = def_as_string(cmd, location)?;
    let storage = match keyword.to_ascii_lowercase().as_str() {
        "plain" => StorageKind::Plain,
        "external" => StorageKind::External,
        "extended" => StorageKind::Extended,
        "main" => StorageKind::Main,
        other => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("unknown STORAGE attribute '{other}'"),
            });
        }
    };
    Ok(PendingColumnAttr {
        target: target.clone(),
        column,
        kind: PendingColumnAttrKind::Storage(storage),
    })
}

fn process_set_compression_cmd(
    cmd: &AlterTableCmd,
    target: &QualifiedName,
    location: &SourceLocation,
) -> Result<PendingColumnAttr, ParseError> {
    // cmd.name = column name; cmd.def = String node with lowercase codec name.
    let column = shared::ident(&cmd.name, location)?;
    let keyword = def_as_string(cmd, location)?;
    let compression = match keyword.to_ascii_lowercase().as_str() {
        "default" => None,
        "pglz" => Some(Compression::Pglz),
        "lz4" => Some(Compression::Lz4),
        other => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("unknown COMPRESSION codec '{other}'"),
            });
        }
    };
    Ok(PendingColumnAttr {
        target: target.clone(),
        column,
        kind: PendingColumnAttrKind::Compression(compression),
    })
}

/// Decode an `AT_ChangeOwner` sub-command into a [`PendingOwner`].
///
/// `pg_query` encodes the new owner as a `RoleSpec` in `cmd.newowner` (a
/// dedicated field on [`AlterTableCmd`], not in the generic `cmd.def`).
fn process_change_owner_cmd(
    cmd: &AlterTableCmd,
    target: &QualifiedName,
    location: &SourceLocation,
) -> Result<PendingOwner, ParseError> {
    let rs = cmd
        .newowner
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "ALTER TABLE OWNER TO missing role specification".into(),
        })?;
    let roletype = RoleSpecType::try_from(rs.roletype).unwrap_or(RoleSpecType::Undefined);
    if roletype == RoleSpecType::RolespecPublic {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "ALTER TABLE OWNER TO PUBLIC is not valid — PUBLIC is not a role name".into(),
        });
    }
    let new_owner = shared::ident(&rs.rolename, location)?;
    Ok(PendingOwner {
        target: target.clone(),
        new_owner,
    })
}

fn process_set_rel_options_cmd(
    cmd: &AlterTableCmd,
    target: &QualifiedName,
    location: &SourceLocation,
) -> Result<PendingRelOptions, ParseError> {
    let items = crate::parse::builder::reloptions::extract_def_list(cmd.def.as_deref(), location)?;
    let options = crate::parse::builder::reloptions::decode_table_options(&items, location)?;
    Ok(PendingRelOptions {
        target: target.clone(),
        options,
    })
}

/// Decode an `AT_SetTableSpace` sub-command.
///
/// `pg_query` encodes the tablespace name in `cmd.name` for this subtype.
fn process_set_tablespace_cmd(
    cmd: &AlterTableCmd,
    target: &QualifiedName,
    location: &SourceLocation,
) -> Result<PendingTablespace, ParseError> {
    if cmd.name.is_empty() {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "ALTER TABLE SET TABLESPACE missing tablespace name".into(),
        });
    }
    let tablespace = shared::ident(&cmd.name, location)?;
    Ok(PendingTablespace {
        target: target.clone(),
        tablespace,
    })
}

/// Apply accumulated `ALTER TABLE ... SET (...)` reloption updates to the
/// catalog. Tables and materialized views are both searched.
///
/// Called from `parse/mod.rs` after all relations are built.
pub fn apply_pending_rel_options(
    catalog: &mut Catalog,
    pending: Vec<PendingRelOptions>,
    location: &SourceLocation,
) -> Result<(), ParseError> {
    for p in pending {
        // Search tables first.
        if let Some(table) = catalog.tables.iter_mut().find(|t| t.qname == p.target) {
            merge_table_options(&mut table.storage, p.options);
            continue;
        }
        // Then materialized views.
        if let Some(mv) = catalog
            .materialized_views
            .iter_mut()
            .find(|m| m.qname == p.target)
        {
            merge_table_options(&mut mv.storage, p.options);
            continue;
        }
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "ALTER ... SET (...) referenced unknown relation {}",
                p.target
            ),
        });
    }
    Ok(())
}

/// Merge `src` options into `dst`, overwriting only the fields that are `Some`
/// in `src`. Fields that are `None` in `src` are left unchanged in `dst`.
fn merge_table_options(
    dst: &mut crate::ir::reloptions::TableStorageOptions,
    src: crate::ir::reloptions::TableStorageOptions,
) {
    if src.fillfactor.is_some() {
        dst.fillfactor = src.fillfactor;
    }
    if src.parallel_workers.is_some() {
        dst.parallel_workers = src.parallel_workers;
    }
    if src.toast_tuple_target.is_some() {
        dst.toast_tuple_target = src.toast_tuple_target;
    }
    if src.user_catalog_table.is_some() {
        dst.user_catalog_table = src.user_catalog_table;
    }
    if src.vacuum_truncate.is_some() {
        dst.vacuum_truncate = src.vacuum_truncate;
    }
    // `extra` carries unknown keys plus the autovacuum_* family.
    for (k, v) in src.extra {
        dst.extra.insert(k, v);
    }
}

/// Apply a list of ownership assignments to the catalog.
///
/// Called from `parse/mod.rs` after all relation-family objects are built.
pub fn apply_pending_owners(
    catalog: &mut Catalog,
    pending: Vec<PendingOwner>,
    location: &SourceLocation,
) -> Result<(), ParseError> {
    for po in pending {
        super::owner_stmt::set_owner_for_relation(
            catalog,
            &po.target,
            ObjectType::ObjectTable, // hint: try all relation types
            po.new_owner,
            location,
        )?;
    }
    Ok(())
}

/// Apply accumulated RLS mode toggles to the catalog.
///
/// Called from `parse/mod.rs` after all tables are built.
pub fn apply_pending_rls_toggles(
    catalog: &mut Catalog,
    pending: Vec<PendingRlsToggle>,
    location: &SourceLocation,
) -> Result<(), ParseError> {
    for toggle in pending {
        let table = catalog
            .tables
            .iter_mut()
            .find(|t| t.qname == toggle.target)
            .ok_or_else(|| ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "ALTER TABLE … ROW LEVEL SECURITY referenced unknown table {}",
                    toggle.target
                ),
            })?;
        match toggle.subtype {
            AlterTableType::AtEnableRowSecurity => {
                table.rls_enabled = true;
            }
            AlterTableType::AtDisableRowSecurity => {
                table.rls_enabled = false;
            }
            AlterTableType::AtForceRowSecurity => {
                table.rls_forced = true;
            }
            AlterTableType::AtNoForceRowSecurity => {
                table.rls_forced = false;
            }
            _ => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: "unexpected subtype in PendingRlsToggle".into(),
                });
            }
        }
    }
    Ok(())
}

/// Apply accumulated `ALTER TABLE … SET TABLESPACE` updates to the catalog.
///
/// Called from `parse/mod.rs` after all tables are built.
pub fn apply_pending_tablespaces(
    catalog: &mut crate::ir::catalog::Catalog,
    pending: Vec<PendingTablespace>,
    location: &SourceLocation,
) -> Result<(), ParseError> {
    for p in pending {
        let table = catalog
            .tables
            .iter_mut()
            .find(|t| t.qname == p.target)
            .ok_or_else(|| ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "ALTER TABLE … SET TABLESPACE referenced unknown table {}",
                    p.target
                ),
            })?;
        table.tablespace = Some(p.tablespace);
    }
    Ok(())
}

/// Extract the String node from `cmd.def` and return its `sval`.
fn def_as_string(cmd: &AlterTableCmd, location: &SourceLocation) -> Result<String, ParseError> {
    cmd.def
        .as_ref()
        .and_then(|d| d.node.as_ref())
        .and_then(|n| match n {
            NodeEnum::String(s) => Some(s.sval.clone()),
            _ => None,
        })
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "ALTER COLUMN SET STORAGE/COMPRESSION missing keyword node".into(),
        })
}

/// Reuse the FK builder from `create_stmt` so source ALTER and inline
/// `REFERENCES` produce identical IR.
fn build_fk_constraint(
    con: &PgConstraint,
    target_table: &QualifiedName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Constraint, ParseError> {
    // Delegate to a public helper exposed by create_stmt to avoid duplicating
    // FK extraction logic.
    create_stmt::build_fk_for_alter(con, target_table, default_schema, location)
}

fn unsupported_alter(location: &SourceLocation) -> ParseError {
    ParseError::Structural {
        location: location.clone(),
        message: "ALTER TABLE in source DDL is restricted to ADD CONSTRAINT FOREIGN KEY, \
                 ALTER COLUMN SET STORAGE/COMPRESSION, SET (reloptions), and SET TABLESPACE; \
                 pgevolve treats source SQL as declarative — express the desired schema \
                 state via CREATE statements"
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::constraint::ConstraintKind;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn build(sql: &str) -> Result<AlterTableOutput, ParseError> {
        let parsed = pg_query::parse(sql).expect("parses");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::AlterTableStmt(s) = stmt else {
            panic!("not AlterTableStmt")
        };
        build_alter_table(&s, None, &loc())
    }

    #[test]
    fn allowed_add_fk() {
        let out = build(
            "ALTER TABLE app.invoices ADD CONSTRAINT invoices_customer_fk \
             FOREIGN KEY (customer_id) REFERENCES app.customers (id);",
        )
        .expect("builds");
        assert_eq!(out.pending_fks.len(), 1);
        let p = &out.pending_fks[0];
        assert_eq!(p.target.to_string(), "app.invoices");
        assert!(matches!(p.constraint.kind, ConstraintKind::ForeignKey(_)));
    }

    #[test]
    fn alter_column_set_storage_external() {
        let out =
            build("ALTER TABLE app.t ALTER COLUMN doc SET STORAGE EXTERNAL;").expect("builds");
        assert_eq!(out.pending_column_attrs.len(), 1);
        let attr = &out.pending_column_attrs[0];
        assert_eq!(attr.target.to_string(), "app.t");
        assert_eq!(attr.column.as_str(), "doc");
        assert!(matches!(
            attr.kind,
            PendingColumnAttrKind::Storage(StorageKind::External)
        ));
    }

    #[test]
    fn alter_column_set_storage_plain() {
        let out = build("ALTER TABLE app.t ALTER COLUMN n SET STORAGE PLAIN;").expect("builds");
        let attr = &out.pending_column_attrs[0];
        assert!(matches!(
            attr.kind,
            PendingColumnAttrKind::Storage(StorageKind::Plain)
        ));
    }

    #[test]
    fn alter_column_set_storage_main() {
        let out = build("ALTER TABLE app.t ALTER COLUMN n SET STORAGE MAIN;").expect("builds");
        let attr = &out.pending_column_attrs[0];
        assert!(matches!(
            attr.kind,
            PendingColumnAttrKind::Storage(StorageKind::Main)
        ));
    }

    #[test]
    fn alter_column_set_compression_lz4() {
        let out = build("ALTER TABLE app.t ALTER COLUMN doc SET COMPRESSION lz4;").expect("builds");
        assert_eq!(out.pending_column_attrs.len(), 1);
        let attr = &out.pending_column_attrs[0];
        assert!(matches!(
            attr.kind,
            PendingColumnAttrKind::Compression(Some(Compression::Lz4))
        ));
    }

    #[test]
    fn alter_column_set_compression_pglz() {
        let out =
            build("ALTER TABLE app.t ALTER COLUMN doc SET COMPRESSION pglz;").expect("builds");
        let attr = &out.pending_column_attrs[0];
        assert!(matches!(
            attr.kind,
            PendingColumnAttrKind::Compression(Some(Compression::Pglz))
        ));
    }

    #[test]
    fn alter_column_set_compression_default() {
        let out =
            build("ALTER TABLE app.t ALTER COLUMN doc SET COMPRESSION DEFAULT;").expect("builds");
        let attr = &out.pending_column_attrs[0];
        assert!(matches!(
            attr.kind,
            PendingColumnAttrKind::Compression(None)
        ));
    }

    #[test]
    fn rejects_drop_column() {
        let err = build("ALTER TABLE app.users DROP COLUMN email;").unwrap_err();
        match err {
            ParseError::Structural { message, .. } => {
                assert!(
                    message.contains("declarative"),
                    "expected declarative message, got: {message}"
                );
            }
            other => panic!("expected Structural, got {other:?}"),
        }
    }

    #[test]
    fn rejects_add_column() {
        let err = build("ALTER TABLE app.users ADD COLUMN email text;").unwrap_err();
        match err {
            ParseError::Structural { message, .. } => {
                assert!(
                    message.contains("declarative") || message.contains("FOREIGN KEY"),
                    "got: {message}"
                );
            }
            other => panic!("expected Structural, got {other:?}"),
        }
    }

    /// Verify that `SET STORAGE BOGUS` always surfaces as an error, regardless
    /// of whether `pg_query` catches it at parse time or our decoder catches it.
    ///
    /// The error path under test is `process_set_storage_cmd` line 176-181
    /// (`unknown STORAGE attribute '…'`). If `pg_query` happens to accept the
    /// keyword and pass it down, that arm is exercised. If `pg_query` rejects it
    /// first, we confirm via a parse-level error — either way the contract holds.
    #[test]
    fn alter_column_set_storage_unknown_errors() {
        let sql = "ALTER TABLE app.t ALTER COLUMN doc SET STORAGE BOGUS;";
        // pg_query may reject this SQL outright (returning Err), or it may
        // accept it and pass the unknown keyword to our decoder.
        match pg_query::parse(sql) {
            Err(_pg_err) => {
                // pg_query rejected BOGUS before our decoder was reached.
                // The contract is satisfied: malformed SQL fails at parse time.
            }
            Ok(parsed) => {
                // pg_query accepted the keyword — our decoder must reject it.
                let stmt = parsed
                    .protobuf
                    .stmts
                    .into_iter()
                    .next()
                    .and_then(|raw| raw.stmt)
                    .and_then(|n| n.node)
                    .expect("stmt");
                let NodeEnum::AlterTableStmt(s) = stmt else {
                    panic!("expected AlterTableStmt");
                };
                let err = build_alter_table(&s, None, &loc())
                    .expect_err("BOGUS storage keyword must be rejected by our decoder");
                match err {
                    ParseError::Structural { ref message, .. } => {
                        assert!(
                            message.contains("STORAGE"),
                            "expected error to mention STORAGE, got: {message}"
                        );
                    }
                    other => panic!("expected Structural error, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn rejects_add_check_via_alter() {
        let err = build("ALTER TABLE app.t ADD CONSTRAINT c1 CHECK (n > 0);").unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }

    /// `pg_query` encodes the new owner in `cmd.newowner` (a dedicated
    /// [`pg_query::protobuf::RoleSpec`] field), not in `cmd.def`.  This test
    /// guards that `process_change_owner_cmd` reads from the right field.
    #[test]
    fn alter_table_owner_to_role_name() {
        let out = build("ALTER TABLE app.t OWNER TO app_owner;").expect("builds");
        assert_eq!(out.pending_owners.len(), 1);
        let po = &out.pending_owners[0];
        assert_eq!(po.target.to_string(), "app.t");
        assert_eq!(po.new_owner.as_str(), "app_owner");
    }

    // ── RLS toggle tests ──────────────────────────────────────────────────────

    #[test]
    fn enable_row_security_produces_toggle() {
        let out = build("ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;").expect("builds");
        assert_eq!(out.pending_rls_toggles.len(), 1);
        let t = &out.pending_rls_toggles[0];
        assert_eq!(t.target.to_string(), "app.docs");
        assert!(matches!(t.subtype, AlterTableType::AtEnableRowSecurity));
    }

    #[test]
    fn disable_row_security_produces_toggle() {
        let out = build("ALTER TABLE app.docs DISABLE ROW LEVEL SECURITY;").expect("builds");
        assert_eq!(out.pending_rls_toggles.len(), 1);
        let t = &out.pending_rls_toggles[0];
        assert!(matches!(t.subtype, AlterTableType::AtDisableRowSecurity));
    }

    #[test]
    fn force_row_security_produces_toggle() {
        let out = build("ALTER TABLE app.docs FORCE ROW LEVEL SECURITY;").expect("builds");
        assert_eq!(out.pending_rls_toggles.len(), 1);
        let t = &out.pending_rls_toggles[0];
        assert!(matches!(t.subtype, AlterTableType::AtForceRowSecurity));
    }

    #[test]
    fn no_force_row_security_produces_toggle() {
        let out = build("ALTER TABLE app.docs NO FORCE ROW LEVEL SECURITY;").expect("builds");
        assert_eq!(out.pending_rls_toggles.len(), 1);
        let t = &out.pending_rls_toggles[0];
        assert!(matches!(t.subtype, AlterTableType::AtNoForceRowSecurity));
    }

    // ── SET / RESET reloption tests ───────────────────────────────────────────

    #[test]
    fn alter_table_set_reloption_fillfactor() {
        let out = build("ALTER TABLE app.t SET (fillfactor = 80);").expect("builds");
        assert_eq!(out.pending_rel_options.len(), 1);
        let p = &out.pending_rel_options[0];
        assert_eq!(p.target.to_string(), "app.t");
        assert_eq!(p.options.fillfactor, Some(80));
    }

    #[test]
    fn alter_table_set_reloption_autovacuum_enabled() {
        let out = build("ALTER TABLE app.t SET (autovacuum_enabled = false);").expect("builds");
        assert_eq!(out.pending_rel_options.len(), 1);
        let p = &out.pending_rel_options[0];
        assert_eq!(
            p.options
                .extra
                .get("autovacuum_enabled")
                .map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn alter_table_set_reloption_multiple_options() {
        let out = build("ALTER TABLE app.t SET (fillfactor = 70, parallel_workers = 2);")
            .expect("builds");
        let p = &out.pending_rel_options[0];
        assert_eq!(p.options.fillfactor, Some(70));
        assert_eq!(p.options.parallel_workers, Some(2));
    }

    #[test]
    fn alter_table_reset_reloption_errors() {
        let err = build("ALTER TABLE app.t RESET (fillfactor);").unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { ref message, .. }
                if message.contains("RESET") || message.contains("not supported")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn alter_table_set_fillfactor_out_of_range_errors() {
        let err = build("ALTER TABLE app.t SET (fillfactor = 5);").unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { ref message, .. } if message.contains("out of range")),
            "unexpected error: {err:?}"
        );
    }

    // ── SET TABLESPACE tests ──────────────────────────────────────────────────

    #[test]
    fn alter_table_set_tablespace_produces_pending() {
        let out = build("ALTER TABLE app.t SET TABLESPACE ts;").expect("builds");
        assert_eq!(out.pending_tablespaces.len(), 1);
        let p = &out.pending_tablespaces[0];
        assert_eq!(p.target.to_string(), "app.t");
        assert_eq!(p.tablespace.as_str(), "ts");
    }
}
