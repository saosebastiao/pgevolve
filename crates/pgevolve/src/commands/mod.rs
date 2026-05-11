//! Command implementations.
//!
//! Each module is one subcommand from [`crate::cli::Command`]. Most
//! commands return an exit code (0 / 1 / 2 / 3 / 4 per spec §13) or an
//! error which the dispatcher prints and maps to exit code 1.

pub mod apply;
pub mod bootstrap;
pub mod diff;
pub mod init;
pub mod lint;
pub mod plan;
pub mod status;
pub mod validate;
