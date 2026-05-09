# Phase 9 — CLI commands

**Goal:** Wire the full CLI surface in the `pgevolve` binary: argument parsing, `pgevolve.toml` loading, connection precedence, and all nine v0.1 commands. By the end of this phase, `pgevolve` is a usable end-to-end tool.

**Spec coverage:** §10, §11, §13.

**Depends on:** Phase 8 (executor).

**Exit criteria:**

- `pgevolve --help` lists all nine commands.
- `pgevolve.toml` is loaded with full validation; bad config produces clean errors.
- Connection precedence matches `psql`'s rules.
- Each command works end-to-end against `EphemeralPostgres`.
- `--format=json` output has a documented stable schema.
- Logging via `tracing-subscriber` works with `RUST_LOG` and pgevolve's own `-v / -vv / --quiet` flags.
- All exit codes (0 / 1 / 2 / 3 / 4) match spec §13.

---

## File structure

```
crates/pgevolve/src/
├── main.rs                # tokio runtime + dispatch
├── cli.rs                 # clap definitions
├── config/
│   ├── mod.rs             # PgevolveConfig + loader
│   ├── error.rs           # ConfigError
│   ├── managed.rs         # [managed] section
│   ├── planner.rs         # [planner] section
│   ├── environment.rs     # [environments.<name>] section
│   └── shadow.rs          # [shadow] section
├── connection.rs          # DSN resolution
├── output/
│   ├── mod.rs
│   ├── human.rs           # human-readable (color-on-tty)
│   └── json.rs            # JSON output schemas
└── commands/
    ├── mod.rs
    ├── init.rs
    ├── lint.rs            # phase 10 wires the lint logic; this is the command driver
    ├── validate.rs
    ├── diff.rs
    ├── plan.rs
    ├── apply.rs
    ├── status.rs
    ├── dump.rs
    └── bootstrap.rs
```

---

### Task 9.1: clap CLI scaffold

**File:** `crates/pgevolve/src/cli.rs`

```rust
#[derive(Parser, Debug)]
#[command(name = "pgevolve", version, about = "Postgres declarative schema management")]
pub struct Cli {
    /// Path to pgevolve.toml (defaults to ./pgevolve.toml).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, global = true, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,
    /// Verbosity (-v, -vv).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Quiet mode.
    #[arg(short, long, global = true)]
    pub quiet: bool,
    #[command(subcommand)]
    pub cmd: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Init(InitArgs),
    Lint(LintArgs),
    Validate(ValidateArgs),
    Diff(DiffArgs),
    Plan(PlanArgs),
    Apply(ApplyArgs),
    Status(StatusArgs),
    Dump(DumpArgs),
    Bootstrap(BootstrapArgs),
}

#[derive(Args, Debug)] pub struct InitArgs { /* ... */ }
#[derive(Args, Debug)] pub struct LintArgs { /* ... */ }
// ... per command
```

Each subcommand struct holds its specific args (e.g., `DiffArgs { db: String }`, `PlanArgs { db: String, output: Option<PathBuf> }`).

Tests: `Cli::try_parse_from(["pgevolve", "diff", "--db", "dev"]).is_ok()`. Reject ill-formed inputs.

Commit: `feat(cli): clap scaffold for all v0.1 commands`

---

### Task 9.2: `PgevolveConfig` loader

**File:** `crates/pgevolve/src/config/mod.rs`

```rust
#[derive(Debug, Deserialize)]
pub struct PgevolveConfig {
    pub project: ProjectConfig,
    #[serde(default)]
    pub managed: ManagedConfig,
    #[serde(default)]
    pub planner: PlannerConfig,
    #[serde(default)]
    pub environments: HashMap<String, EnvironmentConfig>,
    #[serde(default)]
    pub shadow: Option<ShadowConfig>,
}

pub fn load(path: &Path) -> Result<PgevolveConfig, ConfigError> {
    let bytes = std::fs::read_to_string(path).map_err(|e| ConfigError::Io(path.into(), e))?;
    let cfg: PgevolveConfig = toml::from_str(&bytes)?;
    cfg.validate()?;
    Ok(cfg)
}
```

`PgevolveConfig::validate()`: checks that:
- Every `managed.schemas` entry is a valid `Identifier`.
- `managed.ignore_objects` globs compile.
- `planner.strategy` is `"atomic"` or `"online"`.
- Each `environments.<name>.url` is non-empty XOR `url_env` is non-empty.

Each error variant in `ConfigError` carries the key path (e.g., `"managed.schemas[1]"`) so users get clean diagnostics.

Tests: minimal valid config; missing project section; invalid strategy; managed schemas with reserved names.

Commit: `feat(cli): pgevolve.toml loader with validation`

---

### Task 9.3: Connection precedence

**File:** `crates/pgevolve/src/connection.rs`

```rust
pub struct DbOptions {
    pub dsn: String,
    pub managed_schemas: Vec<Identifier>,
    pub ignore_objects: Vec<String>,
    pub strategy: Strategy,
}

pub fn resolve_db(
    cfg: &PgevolveConfig,
    env_name: &str,
    cli_url: Option<&str>,
) -> Result<DbOptions, ConfigError> {
    let env = cfg.environments.get(env_name).ok_or_else(|| ConfigError::UnknownEnvironment(env_name.into()))?;
    let dsn = if let Some(u) = cli_url { u.to_string() }
              else if let Some(u) = &env.url { u.clone() }
              else if let Some(var) = &env.url_env {
                  std::env::var(var).map_err(|_| ConfigError::EnvVarMissing(var.clone()))?
              } else if let Ok(u) = std::env::var("PGEVOLVE_DATABASE_URL") { u }
              else {
                  // Fall back to libpq env vars by leaving DSN empty —
                  // tokio_postgres::Config::from_str("") + per-field setters
                  // OR use postgres-libpq-style env discovery (PGHOST, etc.).
                  build_dsn_from_libpq_env()?
              };

    let strategy = env.strategy.unwrap_or(cfg.planner.strategy);
    Ok(DbOptions { dsn, managed_schemas: cfg.managed.schemas.clone(), ignore_objects: cfg.managed.ignore_objects.clone(), strategy })
}
```

Tests: each precedence rule exercised; missing config → clean error.

Commit: `feat(cli): connection precedence resolver matching psql rules`

---

### Task 9.4: `init` command

**File:** `crates/pgevolve/src/commands/init.rs`

```rust
pub async fn run(args: InitArgs) -> anyhow::Result<()> {
    let dir = args.dir.unwrap_or_else(|| PathBuf::from("."));
    if dir.join("pgevolve.toml").exists() && !args.force {
        anyhow::bail!("pgevolve.toml already exists in {}", dir.display());
    }
    std::fs::create_dir_all(dir.join("schema"))?;
    std::fs::create_dir_all(dir.join("plans"))?;
    std::fs::write(dir.join("pgevolve.toml"), DEFAULT_CONFIG)?;
    let gitignore = dir.join(".gitignore");
    append_gitignore(&gitignore)?;
    println!("Initialized pgevolve project in {}", dir.display());
    Ok(())
}

const DEFAULT_CONFIG: &str = r#"
[project]
name           = "myproject"
schema_dir     = "schema"
plan_dir       = "plans"
layout_profile = "schema-mirror"

[managed]
schemas        = []
ignore_objects = []

[planner]
strategy = "online"

[environments.dev]
url = "postgres://localhost/myproject_dev"
"#;
```

Tests: `init` in a tempdir → all files exist; second invocation fails without `--force`.

Commit: `feat(cli): init command scaffolds project files`

---

### Task 9.5: `lint` command (driver only — rules land in phase 10)

**File:** `crates/pgevolve/src/commands/lint.rs`

```rust
pub async fn run(args: LintArgs, cfg: &PgevolveConfig) -> anyhow::Result<i32> {
    let source = parse_directory(&cfg.project.schema_dir_resolved(), &[])?;
    // Phase 10 implements profile selection + rule execution; for now stub:
    let findings: Vec<LintFinding> = pgevolve_core::lint::run(&source, &cfg.managed, &profile_for(&cfg))?;
    if findings.is_empty() {
        println!("No lint findings.");
        Ok(0)
    } else {
        for f in &findings { println!("{}: {}", f.severity, f.message); }
        Ok(1)
    }
}
```

Commit: `feat(cli): lint command driver (phase 10 wires the rules)`

---

### Task 9.6: `validate` command

**File:** `crates/pgevolve/src/commands/validate.rs`

Without `--shadow`: parses source, builds IR, runs lint rules. Same as `lint` but stricter — non-zero on any error-severity finding.

With `--shadow`: defers to phase 12.

Commit: `feat(cli): validate command (shadow mode lands in phase 12)`

---

### Task 9.7: `diff` command

**File:** `crates/pgevolve/src/commands/diff.rs`

```rust
pub async fn run(args: DiffArgs, cfg: &PgevolveConfig, format: OutputFormat) -> anyhow::Result<i32> {
    let db = resolve_db(cfg, &args.db, args.url.as_deref())?;
    let source = parse_directory(&cfg.project.schema_dir_resolved(), &[])?;
    let (client, _runtime) = connect(&db).await?;
    let querier = PgCatalogQuerier::new(client);
    let filter = CatalogFilter::new(db.managed_schemas, db.ignore_objects)?;
    let target = read_catalog(&querier, &filter)?;
    let changes = diff(&target, &source);

    match format {
        OutputFormat::Human => print_diff_human(&changes),
        OutputFormat::Json  => print_diff_json(&changes),
        OutputFormat::Sql   => print_diff_sql(&changes), // naive ALTER SQL, no rewrites
    }
    Ok(if changes.is_empty() { 0 } else { 0 }) // diff itself is informational; never non-zero
}
```

Tests: a populated DB → introspect → diff vs equivalent source → empty. Diff vs different source → expected entries.

Commit: `feat(cli): diff command with human/json/sql output formats`

---

### Task 9.8: `plan` command

**File:** `crates/pgevolve/src/commands/plan.rs`

```rust
pub async fn run(args: PlanArgs, cfg: &PgevolveConfig) -> anyhow::Result<i32> {
    let db = resolve_db(cfg, &args.db, args.url.as_deref())?;
    let source = parse_directory(&cfg.project.schema_dir_resolved(), &[])?;
    let (client, _runtime) = connect(&db).await?;
    let querier = PgCatalogQuerier::new(client);
    let filter = CatalogFilter::new(db.managed_schemas, db.ignore_objects)?;
    let target = read_catalog(&querier, &filter)?;
    let target_identity = compute_target_identity(&querier.client()).await?;

    let changes = diff(&target, &source);
    let ordered = pgevolve_core::plan::order(&target, &source, changes)?;

    let policy = PlannerPolicy { strategy: db.strategy, ..Default::default() };
    let raw_steps = pgevolve_core::plan::rewrite(ordered, &target, &policy);
    let groups = pgevolve_core::plan::group_steps(raw_steps);
    let plan = Plan::from_grouped(
        groups, &source, &target,
        target_identity,
        detect_git_rev().ok(),
        pgevolve_core::VERSION,
        policy.planner_ruleset_version,
    );

    let out_dir = args.output.unwrap_or_else(|| {
        let date = time::OffsetDateTime::now_utc().date();
        cfg.project.plan_dir_resolved().join(format!("{date}-{}", plan.id.short()))
    });
    pgevolve_core::plan::write_plan_dir(&plan, &out_dir)?;
    println!("Wrote plan to {}", out_dir.display());
    Ok(0)
}

fn detect_git_rev() -> Result<String, std::io::Error> {
    let out = std::process::Command::new("git").args(["rev-parse", "HEAD"]).output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(std::io::Error::other("git rev-parse failed"))
    }
}
```

Tests: end-to-end against `EphemeralPostgres`.

Commit: `feat(cli): plan command writes plan directory`

---

### Task 9.9: `apply` command

**File:** `crates/pgevolve/src/commands/apply.rs`

```rust
pub async fn run(args: ApplyArgs, cfg: &PgevolveConfig) -> anyhow::Result<i32> {
    let db = resolve_db(cfg, &args.db, args.url.as_deref())?;
    let overrides = ApplyOverrides {
        allow_different_target: args.allow_different_target,
        allow_drift: args.allow_drift,
    };
    match executor::apply(&args.plan_dir, db, overrides).await {
        Ok(_) => Ok(0),
        Err(ApplyError::TargetIdentityMismatch { .. }) => Ok(2),
        Err(ApplyError::DriftDetected { .. })          => Ok(2),
        Err(ApplyError::UnapprovedIntents(_))          => Ok(2),
        Err(ApplyError::LockHeld)                       => Ok(3),
        Err(ApplyError::StepFailed { .. })             => Ok(3),
        Err(other) => Err(other.into()),
    }
}
```

Tests: each branch.

Commit: `feat(cli): apply command with proper exit-code mapping`

---

### Task 9.10: `status` command

**File:** `crates/pgevolve/src/commands/status.rs`

Prints recent applies (apply_id, plan_id, status, started_at, finished_at) plus, if `--apply-id <id>` is given, the per-step detail table.

`--format=json` emits a stable JSON schema documented in module docs.

Tests: round-trip against `EphemeralPostgres` after a successful apply.

Commit: `feat(cli): status command with recent applies and per-step detail`

---

### Task 9.11: `dump` command

**File:** `crates/pgevolve/src/commands/dump.rs`

```rust
pub async fn run(args: DumpArgs, cfg: &PgevolveConfig) -> anyhow::Result<i32> {
    let db = resolve_db(cfg, &args.db, args.url.as_deref())?;
    let (client, _runtime) = connect(&db).await?;
    let querier = PgCatalogQuerier::new(client);
    let filter = CatalogFilter::new(db.managed_schemas, db.ignore_objects)?;
    let catalog = read_catalog(&querier, &filter)?;
    let layout = profile_for(cfg);
    write_catalog_as_source(&catalog, &args.output, layout)?;
    Ok(0)
}
```

`write_catalog_as_source(catalog, dir, layout)` emits SQL files in the layout's expected file structure. Use `Catalog`'s deterministic ordering. Each table emits `CREATE TABLE ...; (constraints inline). Each non-PK/FK index gets its own file. Each sequence gets its own file. Each schema gets a `_schema.sql`.

Tests: dump from a populated DB; re-parse the dumped tree; assert it round-trips to the same IR.

Commit: `feat(cli): dump command writes catalog as layout-formatted source`

---

### Task 9.12: `bootstrap` command

**File:** `crates/pgevolve/src/commands/bootstrap.rs`

Calls `executor::bootstrap_metadata` directly. Useful for users wanting an explicit "create the pgevolve schema right now" without a plan.

Commit: `feat(cli): explicit bootstrap command`

---

### Task 9.13: Output formatters

**Files:** `crates/pgevolve/src/output/{human,json}.rs`

Human formatter: hierarchical with color (`anstream` or `termcolor` crate). Color disabled when stdout isn't a TTY.

JSON formatter: stable schemas — version each output's top-level object with a `schema_version` field. Document the schemas in `crates/pgevolve/src/output/json.rs` doc comments.

Tests: snapshot-based via `insta` for human output; schema validation for JSON.

Commit: `feat(cli): output formatters for human and json modes`

---

### Task 9.14: `main.rs` dispatch and exit codes

**File:** `crates/pgevolve/src/main.rs`

```rust
fn main() {
    let cli = Cli::parse();
    init_tracing(cli.verbose, cli.quiet);
    let cfg_path = cli.config.clone().unwrap_or_else(|| PathBuf::from("pgevolve.toml"));
    let cfg = match commands::needs_config(&cli.cmd) {
        true  => Some(config::load(&cfg_path).unwrap_or_else(|e| { eprintln!("config error: {e}"); std::process::exit(4); })),
        false => None,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let exit_code = runtime.block_on(async {
        match cli.cmd {
            Command::Init(a) => commands::init::run(a).await.map(|()| 0),
            Command::Lint(a) => commands::lint::run(a, cfg.as_ref().unwrap()).await,
            Command::Validate(a) => commands::validate::run(a, cfg.as_ref().unwrap()).await,
            Command::Diff(a) => commands::diff::run(a, cfg.as_ref().unwrap(), cli.format).await,
            Command::Plan(a) => commands::plan::run(a, cfg.as_ref().unwrap()).await,
            Command::Apply(a) => commands::apply::run(a, cfg.as_ref().unwrap()).await,
            Command::Status(a) => commands::status::run(a, cfg.as_ref().unwrap(), cli.format).await,
            Command::Dump(a) => commands::dump::run(a, cfg.as_ref().unwrap()).await,
            Command::Bootstrap(a) => commands::bootstrap::run(a, cfg.as_ref().unwrap()).await,
        }
    });
    std::process::exit(exit_code.unwrap_or_else(|e| { eprintln!("error: {e:#}"); 1 }));
}
```

Tests: `assert_cmd` driving the binary against `EphemeralPostgres`.

Commit: `feat(cli): main dispatch with proper exit codes`

---

### Task 9.15: Phase 9 self-review

- All nine commands invoked end-to-end against an ephemeral PG.
- `pgevolve --help` lists everything.
- Exit codes match spec §13.
- `cargo test --workspace` passes; clippy clean.

Phase 9 complete.
