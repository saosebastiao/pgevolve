//! AST → IR builders.
//!
//! Each submodule consumes one classified [`crate::parse::Statement`] variant
//! and produces zero-or-more IR objects, optionally appended to a partial
//! [`crate::ir::catalog::Catalog`] via [`Builder`].

pub mod aggregate_stmt;
pub mod alter_table_attach_partition;
pub mod alter_table_stmt;
pub mod cast_stmt;
pub mod choose_name;
pub mod comment_stmt;
pub mod create_collation_stmt;
pub mod create_composite_type_stmt;
pub mod create_domain_stmt;
pub mod create_enum_stmt;
pub mod create_extension_stmt;
pub mod create_function_stmt;
pub mod create_materialized_view_stmt;
pub mod create_range_stmt;
pub mod create_schema_stmt;
pub mod create_seq_stmt;
pub mod create_stmt;
pub mod create_trigger_stmt;
pub mod create_view_stmt;
pub mod default_privileges;
pub mod desugar_serial;
pub mod event_trigger_stmt;
pub mod grants;
pub mod index_stmt;
pub mod owner_stmt;
pub mod plpgsql;
pub mod policy_stmt;
pub mod publication_stmt;
pub mod reloptions;
pub mod shared;
pub mod statistic_stmt;
pub mod subscription_stmt;
pub mod table_like;
pub mod text_search_stmt;
