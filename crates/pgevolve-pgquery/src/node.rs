//! Deparsing a single AST node.

use crate::error::Result;
use crate::protobuf;

impl crate::NodeEnum {
    /// Render this single node back into SQL.
    ///
    /// Wraps the node in a one-statement `ParseResult` and hands it to the
    /// deparser, which is the only way in: the C entry point takes a whole parse
    /// result, not a node.
    ///
    /// Upstream reached this through `NodeEnum::to_ref()` → `NodeRef::deparse()`
    /// → `NodeRef::to_enum()`, which cloned the node into a borrowed view and
    /// then cloned it back, to arrive at exactly this call. That round trip cost
    /// ~4,100 lines of generated conversion tables for two call sites in
    /// pgevolve, so it is gone and this is called directly.
    ///
    /// The `version` field matters: the deparser reads it, and a mismatch against
    /// the linked library changes its output. It comes from the C library rather
    /// than being declared here.
    ///
    /// # Example
    ///
    /// ```
    /// use pgevolve_pgquery::NodeEnum;
    ///
    /// let parsed = pgevolve_pgquery::parse("CREATE VIEW v AS SELECT 1").expect("parses");
    /// let stmt = parsed.protobuf.stmts[0].stmt.as_ref().expect("has a statement");
    /// let node = stmt.node.as_ref().expect("has a node");
    /// assert!(matches!(node, NodeEnum::ViewStmt(_)));
    /// assert_eq!(node.deparse().expect("deparses"), "CREATE VIEW v AS SELECT 1");
    /// ```
    pub fn deparse(&self) -> Result<String> {
        crate::query::deparse(&protobuf::ParseResult {
            version: crate::vendored_pg_version_num(),
            stmts: vec![protobuf::RawStmt {
                stmt: Some(Box::new(protobuf::Node {
                    node: Some(self.clone()),
                })),
                stmt_location: 0,
                stmt_len: 0,
            }],
        })
    }
}
