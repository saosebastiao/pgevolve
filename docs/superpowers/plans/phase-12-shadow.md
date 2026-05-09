# Phase 12 — `validate --shadow` (shadow Postgres round-trip)

**Goal:** Implement the `validate --shadow` mode: spin up an ephemeral Postgres of the configured version, apply the source SQL to it, introspect it back into Catalog IR, and assert that the round-trip preserves equivalence to the source-derived IR. This is the optional verification mode promised in spec §6 (the C-mode hook from the brainstorm).

**Spec coverage:** §10 (`validate` row); future-work pointer in §16.

**Depends on:** Phases 2 (parser), 3 (catalog reader, EphemeralPostgres), 11 (testkit harnesses).

**Exit criteria:**

- `pgevolve validate --shadow` works end-to-end against a `[shadow]`-configured project.
- The shadow PG version respects `[shadow].postgres_version`.
- Discrepancies between source IR and shadow-introspected IR are reported as `Finding`s.
- Tests cover at least: a clean round-trip, a deliberately tricky default expression, an FK forward-reference cycle, and a SERIAL desugar.

---

## File structure

```
crates/pgevolve/src/
└── commands/
    └── validate.rs          # extended to handle --shadow
```

(No new files in `pgevolve-core`; the binary owns shadow because it requires Docker / process control.)

---

### Task 12.1: Extend `ValidateArgs`

**File:** `crates/pgevolve/src/cli.rs`

```rust
#[derive(Args, Debug)]
pub struct ValidateArgs {
    /// Round-trip source through an ephemeral Postgres of the configured version.
    #[arg(long)]
    pub shadow: bool,
}
```

Tests: `Cli::try_parse_from(["pgevolve", "validate", "--shadow"]).is_ok()`.

Commit: `feat(cli): add --shadow flag to validate`

---

### Task 12.2: Wire shadow run

**File:** `crates/pgevolve/src/commands/validate.rs`

```rust
pub async fn run(args: ValidateArgs, cfg: &PgevolveConfig) -> anyhow::Result<i32> {
    let source_tree = parse_directory(&cfg.project.schema_dir_resolved(), &[])?;
    let mut findings = pgevolve_core::lint::run(&source_tree, &cfg.managed, &profile_for(cfg))?;

    if args.shadow {
        findings.extend(run_shadow_validation(&source_tree, cfg).await?);
    }

    let exit_code = if findings.iter().any(|f| f.severity == Severity::Error) { 1 } else { 0 };
    print_findings(&findings);
    Ok(exit_code)
}

async fn run_shadow_validation(
    source_tree: &SourceTree,
    cfg: &PgevolveConfig,
) -> anyhow::Result<Vec<Finding>> {
    let shadow_cfg = cfg.shadow.as_ref()
        .ok_or_else(|| anyhow::anyhow!("--shadow requires [shadow] section in pgevolve.toml"))?;
    let pg_version = parse_pg_version(&shadow_cfg.postgres_version)?;

    let shadow = EphemeralPostgres::start(pg_version).await?;

    // Apply source SQL by emitting CREATE statements for the source IR
    // (we don't have an idempotent applier in v0.1, so we render the source
    // IR as SQL and execute it).
    apply_source_to_shadow(source_tree, &shadow).await?;

    // Introspect the shadow.
    let querier = PgCatalogQuerier::connect_with(shadow.dsn()).await?;
    let filter = CatalogFilter::new(cfg.managed.schemas.clone(), cfg.managed.ignore_objects.clone())?;
    let shadow_catalog = pgevolve_core::catalog::read_catalog(&querier, &filter)?;

    // Compare.
    let diffs = source_tree.catalog.diff(&shadow_catalog);
    Ok(diffs.into_iter().map(|d| Finding {
        severity: Severity::Error,
        rule: "shadow-roundtrip-mismatch",
        message: format!("source vs shadow at {}: {} → {}", d.path, d.from, d.to),
        location: None,
    }).collect())
}
```

`apply_source_to_shadow`: render each source object as SQL via the same SQL emitters used by the planner (`emit_steps_for` from phase 6) but as a single `CREATE`-only sequence. v0.1 acceptable: render to SQL using `Plan` machinery and run as one-shot.

Tests:
- Clean round-trip → 0 findings.
- A `DEFAULT 'foo'::text` literal → 0 findings (the shadow catalog returns `'foo'::text`, but our normalization already strips the redundant cast → equivalent).
- Forward-FK cycle → 0 findings (apply uses NOT VALID/VALIDATE pattern).
- SERIAL desugar → 0 findings.

Commit: `feat(cli): validate --shadow round-trips source through ephemeral Postgres`

---

### Task 12.3: Phase 12 self-review

- `validate --shadow` runs in CI on the `pg-matrix` job for at least one fixture.
- Spec §10 row for `validate` reflects the implemented behavior.
- `cargo test --workspace --features pg-tests` passes.

Phase 12 complete.

---

## v0.1 Release Checklist

After phase 12:

- [ ] All 13 phase plans implemented and merged.
- [ ] `cargo test --workspace --features pg-tests` green on PG 14, 15, 16, 17.
- [ ] Soak job (weekly) green for one full run.
- [ ] README updated with installation, quickstart, and links to spec/plan.
- [ ] CHANGELOG.md added with v0.1.0 entry.
- [ ] Crate metadata (description, keywords, categories) populated for crates.io.
- [ ] `cargo publish --dry-run` clean for `pgevolve-core` and `pgevolve` (skip `pgevolve-testkit` per its `publish = false`).
- [ ] Tag `v0.1.0` on `main` and create a GitHub release with the binary attached for at least Linux x86_64 and macOS arm64.

v0.1 complete.
