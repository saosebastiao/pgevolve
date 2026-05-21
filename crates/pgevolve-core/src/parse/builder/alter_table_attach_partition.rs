//! `ALTER TABLE parent ATTACH PARTITION child FOR VALUES ...` — back-fills
//! `partition_of` on an already-parsed child Table.

use pg_query::NodeEnum;
use pg_query::protobuf::{AlterTableStmt, AlterTableType};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::partition::PartitionOf;
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Parsed result of `ALTER TABLE parent ATTACH PARTITION child FOR VALUES ...`.
#[derive(Debug)]
pub struct AttachPartition {
    /// Schema-qualified name of the partitioned parent table.
    pub parent: QualifiedName,
    /// Schema-qualified name of the child partition table.
    pub child: QualifiedName,
    /// The `PartitionOf` record ready to be stored on the child `Table`.
    pub partition_of: PartitionOf,
}

/// Parse an `ALTER TABLE ... ATTACH PARTITION` statement.
///
/// Returns [`ParseError::Structural`] for any other `ALTER TABLE` subtype, for
/// `CONCURRENTLY` attach, or for missing structural components.
pub fn build_attach_partition(
    stmt: &AlterTableStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<AttachPartition, ParseError> {
    let parent_rangevar = stmt.relation.as_ref().ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: "ALTER TABLE ATTACH PARTITION missing relation".into(),
    })?;
    let parent = shared::resolve_qname(parent_rangevar, default_schema, location)?;

    if stmt.cmds.len() != 1 {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "ALTER TABLE with ATTACH PARTITION must be the only sub-command".into(),
        });
    }
    let Some(NodeEnum::AlterTableCmd(cmd)) = &stmt.cmds[0].node else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "expected AlterTableCmd".into(),
        });
    };

    let cmd_subtype = AlterTableType::try_from(cmd.subtype).unwrap_or(AlterTableType::Undefined);
    if !matches!(cmd_subtype, AlterTableType::AtAttachPartition) {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "only ATTACH PARTITION sub-command is supported on ALTER TABLE; got {cmd_subtype:?}",
            ),
        });
    }

    let part_cmd_node = cmd.def.as_ref().ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: "ATTACH PARTITION missing partition cmd".into(),
    })?;
    let Some(NodeEnum::PartitionCmd(part_cmd)) = &part_cmd_node.node else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "expected PartitionCmd on ATTACH PARTITION".into(),
        });
    };

    if part_cmd.concurrent {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "ATTACH PARTITION ... CONCURRENTLY is not supported".into(),
        });
    }

    let child_rangevar = part_cmd.name.as_ref().ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: "ATTACH PARTITION missing child name".into(),
    })?;
    let child = shared::resolve_qname(child_rangevar, default_schema, location)?;

    let bound_spec = part_cmd.bound.as_ref().ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: "ATTACH PARTITION missing FOR VALUES bounds".into(),
    })?;
    let bounds = crate::parse::builder::create_stmt::build_partition_bounds(bound_spec, location)?;

    Ok(AttachPartition {
        parent: parent.clone(),
        child,
        partition_of: PartitionOf { parent, bounds },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn parse_alter(sql: &str) -> AlterTableStmt {
        let parsed = pg_query::parse(sql).unwrap();
        match &parsed.protobuf.stmts[0].stmt.as_ref().unwrap().node {
            Some(pg_query::NodeEnum::AlterTableStmt(s)) => s.clone(),
            other => panic!("expected AlterTableStmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_attach_partition_range() {
        let stmt = parse_alter(
            "ALTER TABLE app.orders ATTACH PARTITION app.orders_2024 \
             FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');",
        );
        let r = build_attach_partition(&stmt, None, &loc()).unwrap();
        assert_eq!(r.parent.name.as_str(), "orders");
        assert_eq!(r.child.name.as_str(), "orders_2024");
        assert!(matches!(
            r.partition_of.bounds,
            crate::ir::partition::PartitionBounds::Range { .. }
        ));
    }

    #[test]
    fn parses_attach_partition_default() {
        let stmt = parse_alter(
            "ALTER TABLE app.orders ATTACH PARTITION app.orders_default DEFAULT;",
        );
        let r = build_attach_partition(&stmt, None, &loc()).unwrap();
        assert!(matches!(
            r.partition_of.bounds,
            crate::ir::partition::PartitionBounds::Default
        ));
    }

    #[test]
    fn rejects_concurrently() {
        let mut stmt = parse_alter(
            "ALTER TABLE app.orders ATTACH PARTITION app.orders_2024 \
             FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');",
        );
        // Flip concurrent to true to simulate that input.
        let cmd_node = stmt.cmds[0].node.as_mut().unwrap();
        match cmd_node {
            pg_query::NodeEnum::AlterTableCmd(c) => {
                let def_node = c.def.as_mut().unwrap().node.as_mut().unwrap();
                match def_node {
                    pg_query::NodeEnum::PartitionCmd(p) => p.concurrent = true,
                    _ => unreachable!(),
                }
            }
            _ => unreachable!(),
        }
        let err = build_attach_partition(&stmt, None, &loc()).unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }
}
