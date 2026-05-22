//! `pgevolve` CLI entry point.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::{EnvFilter, fmt};

use pgevolve::cli::{Cli, ClusterCommand, Command, OutputFormat};
use pgevolve::commands;
use pgevolve::config;

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose, cli.quiet);

    let exit = match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    };
    ExitCode::from(exit)
}

fn run(cli: Cli) -> Result<u8, anyhow::Error> {
    // `init` doesn't need a config (it creates one).
    if let Command::Init(args) = cli.cmd {
        return Ok(u8::try_from(commands::init::run(args)?).unwrap_or(0));
    }

    // `cluster` commands use `pgevolve-cluster.toml`, not `pgevolve.toml`.
    if let Command::Cluster(args) = cli.cmd {
        let cfg_path = args
            .config
            .unwrap_or_else(|| PathBuf::from("pgevolve-cluster.toml"));
        let cluster_cmd = args.cmd;

        // `cluster init` scaffolds the config; it doesn't need one to exist.
        if let ClusterCommand::Init { path } = cluster_cmd {
            return Ok(u8::try_from(commands::cluster::init::run(path)?).unwrap_or(0));
        }

        let cfg = match pgevolve::cluster_config::load(&cfg_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("cluster config error: {e}");
                return Ok(4);
            }
        };
        let project_root = cfg_path
            .parent()
            .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let code: i32 = runtime.block_on(async move {
            match cluster_cmd {
                ClusterCommand::Init { .. } => unreachable!("handled above"),
                ClusterCommand::Diff => commands::cluster::diff::run(&project_root, &cfg).await,
                ClusterCommand::Plan => commands::cluster::plan::run(&project_root, &cfg).await,
                ClusterCommand::Apply { plan_id } => {
                    commands::cluster::apply::run(&project_root, &cfg, plan_id.as_deref()).await
                }
                ClusterCommand::Status => commands::cluster::status::run(&project_root, &cfg)
                    .map_err(|e| anyhow::anyhow!("{e}")),
            }
        })?;
        return Ok(u8::try_from(code).unwrap_or(1));
    }

    // All remaining commands require `pgevolve.toml`.
    let cfg_path = cli
        .config
        .clone()
        .unwrap_or_else(|| PathBuf::from("pgevolve.toml"));
    let cfg = match config::load(&cfg_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return Ok(4);
        }
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let code: i32 = runtime.block_on(async move {
        match cli.cmd {
            Command::Init(_) | Command::Cluster(_) => unreachable!("handled above"),
            Command::Lint(args) => commands::lint::run(args, &cfg, cli.format),
            Command::Validate(args) => commands::validate::run(&args, &cfg).await,
            Command::Diff(args) => commands::diff::run(args, &cfg, cli.format).await,
            Command::Plan(args) => commands::plan::run(args, &cfg).await,
            Command::Apply(args) => commands::apply::run(args, &cfg).await,
            Command::Status(args) => commands::status::run(args, &cfg, cli.format).await,
            Command::Dump(args) => commands::dump::run(args, &cfg).await,
            Command::Bootstrap(args) => commands::bootstrap::run(args, &cfg).await,
            Command::Doctor { db, url } => commands::doctor::run(&cfg, &db, url.as_deref()).await,
            Command::RewriteTable {
                qname,
                db,
                url: _,
                confirm_rewrite,
            } => commands::rewrite_table::run(&qname, &db, confirm_rewrite),
            Command::Graph {
                graph_format,
                out,
                plan,
            } => commands::graph::run(&cfg, graph_format, out, plan.as_ref()),
        }
    })?;

    Ok(u8::try_from(code).unwrap_or(1))
}

fn init_tracing(verbose: u8, quiet: bool) {
    let level = if quiet {
        "error"
    } else {
        match verbose {
            0 => "info",
            1 => "debug",
            _ => "trace",
        }
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("pgevolve={level},pgevolve_core={level}")));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
    let _ = OutputFormat::Human; // keep import compiled
}
