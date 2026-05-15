//! `clap`-derived CLI surface. Spec §10.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

/// `pgevolve` — Postgres declarative schema management.
#[derive(Parser, Debug)]
#[command(name = "pgevolve", version, about, long_about = None)]
pub struct Cli {
    /// Path to `pgevolve.toml`. Defaults to `./pgevolve.toml`.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Output format. `sql` is only meaningful for `diff`.
    #[arg(long, value_enum, global = true, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,

    /// Increase verbosity (`-v`, `-vv`).
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub verbose: u8,

    /// Quiet mode: errors only.
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Subcommand to invoke.
    #[command(subcommand)]
    pub cmd: Command,
}

/// The full set of pgevolve commands (v0.1 surface + v0.2 readiness additions).
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Scaffold a new pgevolve project.
    Init(InitArgs),
    /// Lint the source tree against the configured layout profile.
    Lint(LintArgs),
    /// Validate the source tree; with `--shadow`, round-trip through ephemeral PG.
    Validate(ValidateArgs),
    /// Show the diff between the source IR and a live database.
    Diff(DiffArgs),
    /// Produce a plan directory for a live database.
    Plan(PlanArgs),
    /// Apply a plan directory to a live database.
    Apply(ApplyArgs),
    /// Show recent applies and per-step state.
    Status(StatusArgs),
    /// Introspect a live database and write source SQL files. (Deferred to v0.1.1.)
    Dump(DumpArgs),
    /// Install or upgrade the `pgevolve` metadata schema.
    Bootstrap(BootstrapArgs),
    /// Report project health: bootstrap status, drift, recent failures.
    Doctor {
        /// Environment name (looked up in `[environments.<name>]`).
        #[arg(long)]
        db: String,
        /// Override the resolved DSN.
        #[arg(long)]
        url: Option<String>,
    },
    /// Destructive table rewrite (v0.2 skeleton; implementation lands later).
    RewriteTable {
        /// Qualified table name (e.g., `app.users`).
        qname: String,
        /// Environment name.
        #[arg(long)]
        db: String,
        /// Override the resolved DSN.
        #[arg(long)]
        url: Option<String>,
        /// Required confirmation — without it the command refuses to run.
        #[arg(long)]
        confirm_rewrite: bool,
    },
    /// Render the dep graph (name-derived + AST-derived edges).
    Graph {
        /// Graph output format (dot or mermaid). Note: the global `--format`
        /// flag is for human/json/sql output; this flag controls the graph
        /// renderer.
        #[arg(long = "graph-format", value_enum, default_value_t = GraphFormat::Dot)]
        graph_format: GraphFormat,
        /// Write to file instead of stdout.
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
        /// Plan directory to render the post-plan dep graph. When absent,
        /// renders the current source graph.
        #[arg(long)]
        plan: Option<PathBuf>,
    },
}

/// Output format for `pgevolve graph`.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum GraphFormat {
    /// DOT format (for use with graphviz).
    Dot,
    /// Mermaid graph format.
    Mermaid,
}

/// Top-level output format.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable hierarchical output. Default.
    Human,
    /// Stable JSON for automation.
    Json,
    /// Naive ALTER SQL — only valid for `diff`.
    Sql,
}

/// `init` arguments.
#[derive(Args, Debug)]
pub struct InitArgs {
    /// Directory to initialize. Defaults to the current directory.
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// Overwrite an existing `pgevolve.toml`.
    #[arg(long)]
    pub force: bool,
}

/// `lint` arguments.
#[derive(Args, Debug)]
pub struct LintArgs {}

/// `validate` arguments.
#[derive(Args, Debug)]
pub struct ValidateArgs {
    /// Round-trip the source through an ephemeral PG. Phase 12 wires the logic.
    #[arg(long)]
    pub shadow: bool,
}

/// `diff` arguments.
#[derive(Args, Debug)]
pub struct DiffArgs {
    /// Environment name (looked up in `[environments.<name>]`).
    #[arg(long)]
    pub db: String,
    /// Override the resolved DSN.
    #[arg(long)]
    pub url: Option<String>,
}

/// `plan` arguments.
#[derive(Args, Debug)]
pub struct PlanArgs {
    /// Environment name.
    #[arg(long)]
    pub db: String,
    /// Override the resolved DSN.
    #[arg(long)]
    pub url: Option<String>,
    /// Output plan directory. Defaults to `<plan_dir>/<YYYY-MM-DD>-<id>`.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

/// `apply` arguments.
#[derive(Args, Debug)]
pub struct ApplyArgs {
    /// Path to a plan directory written by `pgevolve plan`.
    pub plan_dir: PathBuf,
    /// Environment name.
    #[arg(long)]
    pub db: String,
    /// Override the resolved DSN.
    #[arg(long)]
    pub url: Option<String>,
    /// Skip the target-identity check (use only when re-targeting intentionally).
    #[arg(long)]
    pub allow_different_target: bool,
    /// Skip the drift recheck (use only when re-applying after out-of-band changes).
    #[arg(long)]
    pub allow_drift: bool,
}

/// `status` arguments.
#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Environment name.
    #[arg(long)]
    pub db: String,
    /// Override the resolved DSN.
    #[arg(long)]
    pub url: Option<String>,
    /// Show per-step detail for one apply by id (UUID).
    #[arg(long)]
    pub apply_id: Option<String>,
    /// Limit the recent-applies list (default 10).
    #[arg(long, default_value_t = 10)]
    pub limit: u32,
}

/// `dump` arguments. (v0.1.1.)
#[derive(Args, Debug)]
pub struct DumpArgs {
    /// Environment name.
    #[arg(long)]
    pub db: String,
    /// Override the resolved DSN.
    #[arg(long)]
    pub url: Option<String>,
    /// Output directory.
    #[arg(short, long)]
    pub output: PathBuf,
}

/// `bootstrap` arguments.
#[derive(Args, Debug)]
pub struct BootstrapArgs {
    /// Environment name.
    #[arg(long)]
    pub db: String,
    /// Override the resolved DSN.
    #[arg(long)]
    pub url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_diff_command() {
        let cli = Cli::try_parse_from(["pgevolve", "diff", "--db", "dev"]).unwrap();
        match cli.cmd {
            Command::Diff(a) => assert_eq!(a.db, "dev"),
            _ => panic!("expected diff"),
        }
    }

    #[test]
    fn parses_apply_with_plan_dir() {
        let cli = Cli::try_parse_from(["pgevolve", "apply", "/tmp/plan", "--db", "dev"]).unwrap();
        match cli.cmd {
            Command::Apply(a) => {
                assert_eq!(a.plan_dir, PathBuf::from("/tmp/plan"));
                assert_eq!(a.db, "dev");
                assert!(!a.allow_drift);
            }
            _ => panic!("expected apply"),
        }
    }

    #[test]
    fn rejects_missing_db_argument() {
        assert!(Cli::try_parse_from(["pgevolve", "diff"]).is_err());
    }

    #[test]
    fn parses_global_format_flag() {
        let cli =
            Cli::try_parse_from(["pgevolve", "--format", "json", "diff", "--db", "x"]).unwrap();
        assert_eq!(cli.format, OutputFormat::Json);
    }

    #[test]
    fn parses_verbosity_count() {
        let cli = Cli::try_parse_from(["pgevolve", "-vv", "diff", "--db", "x"]).unwrap();
        assert_eq!(cli.verbose, 2);
    }
}
