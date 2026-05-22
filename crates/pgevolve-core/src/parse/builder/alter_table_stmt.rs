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

/// The combined output of processing one `ALTER TABLE` statement.
#[derive(Debug, Default)]
pub struct AlterTableOutput {
    /// Forward-reference FK constraints to merge after all tables are parsed.
    pub pending_fks: Vec<PendingFk>,
    /// Per-column attribute updates to apply after all tables are parsed.
    pub pending_column_attrs: Vec<PendingColumnAttr>,
    /// Ownership assignments for relation-family objects.
    pub pending_owners: Vec<PendingOwner>,
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
/// `pg_query` encodes the new owner as a `RoleSpec` in `cmd.def`.
fn process_change_owner_cmd(
    cmd: &AlterTableCmd,
    target: &QualifiedName,
    location: &SourceLocation,
) -> Result<PendingOwner, ParseError> {
    let node = cmd
        .def
        .as_ref()
        .and_then(|d| d.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "ALTER TABLE OWNER TO missing role specification".into(),
        })?;
    let NodeEnum::RoleSpec(rs) = node else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "expected RoleSpec in ALTER TABLE OWNER TO, got {:?}",
                std::mem::discriminant(node)
            ),
        });
    };
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
        message: "ALTER TABLE in source DDL is restricted to ADD CONSTRAINT FOREIGN KEY \
                 and ALTER COLUMN SET STORAGE/COMPRESSION; pgevolve treats source SQL as \
                 declarative — express the desired schema state via CREATE statements"
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
}
