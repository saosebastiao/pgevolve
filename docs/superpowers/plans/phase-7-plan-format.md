# Phase 7 — Plan format (serialize / deserialize)

**Goal:** Land the `Plan` data type, `PlanId` hashing, and the round-trip serialize/deserialize machinery for `plan.sql` + `intent.toml` + `manifest.toml`.

**Spec coverage:** §6.6, §7.

**Depends on:** Phase 6.

**Exit criteria:**

- `Plan::from_grouped(groups, target_catalog, source_catalog, version) -> Plan` populates `id`, `intents`, `metadata`.
- `Plan::write_to_dir(&self, dir: &Path) -> Result<(), PlanIoError>` produces `plan.sql`, `intent.toml`, `manifest.toml`.
- `Plan::read_from_dir(dir: &Path) -> Result<Plan, PlanIoError>` parses the directory back into an equivalent `Plan`.
- Round-trip property: `Plan::write_to_dir(p, dir); Plan::read_from_dir(dir) == p`.
- `PlanId` is deterministic: identical `(target, source, pgevolve_version, planner_ruleset_version)` yields byte-identical `id`.

---

## File structure

```
crates/pgevolve-core/src/
└── plan/
    ├── plan.rs            # Plan struct + PlanId + DestructiveIntent + PlanMetadata
    ├── serialize.rs       # Plan → plan.sql, intent.toml, manifest.toml
    ├── deserialize.rs     # plan.sql + intent.toml + manifest.toml → Plan
    └── io_error.rs        # PlanIoError
```

---

### Task 7.1: `Plan` struct + `PlanId` hashing

**File:** `crates/pgevolve-core/src/plan/plan.rs`

```rust
pub struct Plan {
    pub id: PlanId,
    pub groups: Vec<TransactionGroup>,
    pub intents: Vec<DestructiveIntent>,
    pub metadata: PlanMetadata,
}

pub struct PlanId(pub [u8; 32]);

impl PlanId {
    pub fn short(&self) -> String {
        // First 8 bytes hex-encoded → 16 chars
        hex::encode(&self.0[..8])
    }
}

impl std::fmt::Display for PlanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

pub struct DestructiveIntent {
    pub id: u32,
    pub step: u32,                // step number in the plan
    pub kind: String,             // human kind name, e.g., "drop_column"
    pub target: String,           // rendered target qname
    pub reason: String,           // human reason
}

pub struct PlanMetadata {
    pub pgevolve_version: String,
    pub planner_ruleset_version: u32,
    pub source_rev: Option<String>,    // git rev when known
    pub target_identity: String,       // hash of (host, port, dbname, system_identifier)
    pub target_snapshot: Catalog,      // pre-image used for drift recheck
    pub created_at: time::OffsetDateTime,
}
```

`PlanId::compute(...)`: BLAKE3 over a deterministic byte stream:

```rust
impl PlanId {
    pub fn compute(
        source: &Catalog,
        target: &Catalog,
        pgevolve_version: &str,
        planner_ruleset_version: u32,
    ) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(b"pgevolve-plan-id-v1\n");
        h.update(pgevolve_version.as_bytes());
        h.update(&[0]);
        h.update(&planner_ruleset_version.to_be_bytes());
        h.update(&[0]);
        // Canonical YAML serialization sorts maps by key.
        let source_yaml = serde_yaml::to_string(source).expect("serialize source");
        let target_yaml = serde_yaml::to_string(target).expect("serialize target");
        h.update(source_yaml.as_bytes());
        h.update(&[0]);
        h.update(target_yaml.as_bytes());
        Self(*h.finalize().as_bytes())
    }
}
```

> Note: `serde_yaml::to_string` does NOT guarantee key-order stability across versions. v0.1 needs a deterministic canonicalization. Either:
> - Use `serde_yaml`'s `Mapping` and sort keys explicitly before serialization.
> - Use `bincode` or `postcard` for hashing (binary, deterministic, compact).
>
> **Recommended for v0.1:** use `bincode` for hashing with `bincode::serde::encode_to_vec`. The hash payload doesn't have to be human-readable. It's an implementation detail behind `PlanId::compute`.

Add `bincode` to workspace dependencies; use it here.

Tests:
- `compute(c, c, ...)` produces stable hash across runs.
- Different `target` → different hash.
- `id.short()` is 16 hex chars.

Commit: `feat(core): Plan, PlanId (bincode-hashed), DestructiveIntent, PlanMetadata`

---

### Task 7.2: `Plan::from_grouped`

**File:** `crates/pgevolve-core/src/plan/plan.rs`

Walks `groups` to:
1. Assign a 1-indexed `step_no` to each `RawStep` (across all groups).
2. For each step with `destructive == true`, allocate an `intent_id` and create a `DestructiveIntent` row.
3. Assemble `Plan { id, groups, intents, metadata }`.

`metadata.target_snapshot` is the catalog IR fed into the differ — copied here verbatim. **It includes only managed schemas + objects after filtering.**

`metadata.target_identity`: computed by the binary's apply path; the planner accepts it as a parameter.

```rust
impl Plan {
    pub fn from_grouped(
        groups: Vec<TransactionGroup>,
        source: &Catalog,
        target: &Catalog,
        target_identity: String,
        source_rev: Option<String>,
        pgevolve_version: &str,
        planner_ruleset_version: u32,
    ) -> Self {
        let id = PlanId::compute(source, target, pgevolve_version, planner_ruleset_version);
        let (groups_with_steps, intents) = assign_steps_and_intents(groups);
        let metadata = PlanMetadata {
            pgevolve_version: pgevolve_version.to_string(),
            planner_ruleset_version,
            source_rev,
            target_identity,
            target_snapshot: target.clone(),
            created_at: time::OffsetDateTime::now_utc(),
        };
        Self { id, groups: groups_with_steps, intents, metadata }
    }
}
```

Tests: small plan with 3 steps, one destructive → `intents.len() == 1`; step numbers are 1-indexed and contiguous.

Commit: `feat(core): Plan::from_grouped assigns step numbers and intent ids`

---

### Task 7.3: `plan.sql` writer

**File:** `crates/pgevolve-core/src/plan/serialize.rs`

```rust
pub fn write_plan_sql(plan: &Plan, w: &mut dyn Write) -> std::io::Result<()> {
    // Header
    writeln!(w, "-- @pgevolve plan id={} version={} ruleset={} created={}",
        plan.id.short(),
        plan.metadata.pgevolve_version,
        plan.metadata.planner_ruleset_version,
        plan.metadata.created_at.format(&time::format_description::well_known::Rfc3339).unwrap())?;
    if let Some(rev) = &plan.metadata.source_rev {
        writeln!(w, "-- @pgevolve source_rev={rev}")?;
    }
    writeln!(w, "-- @pgevolve target={}", plan.metadata.target_identity)?;
    writeln!(w, "-- @pgevolve intents_required={}", plan.intents.len())?;
    writeln!(w)?;

    for group in &plan.groups {
        writeln!(w, "-- @pgevolve group id={} transactional={}", group.id, group.transactional)?;
        if group.transactional {
            writeln!(w, "BEGIN;")?;
        }
        for step in &group.steps {
            write_step_directive(w, step)?;
            writeln!(w, "{}", step.sql)?;
            if !step.sql.ends_with(';') {
                writeln!(w, ";")?;
            }
        }
        if group.transactional {
            writeln!(w, "COMMIT;")?;
        }
        writeln!(w)?;
    }
    Ok(())
}

fn write_step_directive(w: &mut dyn Write, s: &RawStep) -> std::io::Result<()> {
    write!(w, "-- @pgevolve step={} kind={} destructive={}",
        s.step_no, kind_name(&s.kind), s.destructive)?;
    if let Some(intent_id) = s.intent_id {
        write!(w, " intent_id={intent_id}")?;
    }
    write!(w, " targets=")?;
    let mut first = true;
    for t in &s.targets {
        if !first { write!(w, ",")?; }
        first = false;
        write!(w, "{}", t)?;
    }
    writeln!(w)?;
    Ok(())
}
```

> Note: `RawStep` doesn't currently carry `step_no` — add a small wrapper struct `Step` that owns the rendered `RawStep` plus its assigned `step_no` and `intent_id`. Adjust phase 6's groups/steps to use `Step` instead of `RawStep` (or keep `RawStep` and put the per-plan numbering on the group). Cleanest: rename `RawStep` → `StepBody` and have a `Step { step_no, intent_id, body: StepBody }`.

Tests: hand-author a `Plan` with two groups; serialize; assert directive lines exist as expected.

Commit: `feat(core): write_plan_sql with directive header and per-step metadata`

---

### Task 7.4: `intent.toml` writer

**File:** `crates/pgevolve-core/src/plan/serialize.rs`

```rust
#[derive(Serialize)]
struct IntentDoc<'a> {
    plan_id: String,
    intent: Vec<IntentRow<'a>>,
}

#[derive(Serialize)]
struct IntentRow<'a> {
    id: u32,
    step: u32,
    kind: &'a str,
    target: &'a str,
    reason: &'a str,
    approved: bool,
}

pub fn write_intent_toml(plan: &Plan, w: &mut dyn Write) -> std::io::Result<()> {
    let doc = IntentDoc {
        plan_id: plan.id.short(),
        intent: plan.intents.iter().map(|i| IntentRow {
            id: i.id, step: i.step, kind: &i.kind,
            target: &i.target, reason: &i.reason,
            approved: false,  // user must explicitly flip to true
        }).collect(),
    };
    let s = toml::to_string_pretty(&doc).expect("intent toml");
    w.write_all(s.as_bytes())?;
    Ok(())
}
```

Tests: a plan with 2 intents → produces a TOML doc with `[[intent]]` entries.

Commit: `feat(core): write_intent_toml`

---

### Task 7.5: `manifest.toml` writer

**File:** `crates/pgevolve-core/src/plan/serialize.rs`

```rust
#[derive(Serialize)]
struct ManifestDoc<'a> {
    plan_id: &'a str,
    plan_hash: String,
    pgevolve_version: &'a str,
    planner_ruleset_version: u32,
    source_rev: Option<&'a str>,
    target_identity: &'a str,
    created_at: String,
    target_snapshot_yaml: String,  // multi-line YAML, embedded as string
}
```

The `target_snapshot_yaml` field is the catalog-IR pre-image, serialized via `serde_yaml`. Yes, it's verbose; for a typical small schema it's a few KB. For very large schemas it could be hundreds of KB — acceptable for v0.1.

Alternative considered: store the snapshot as a separate file `manifest_snapshot.yaml`. Rejected because it splits the manifest into two pieces; keeping it inside `manifest.toml` makes the directory simpler.

Tests: round-trip embedded YAML; verify it parses back into a `Catalog`.

Commit: `feat(core): write_manifest_toml with embedded catalog snapshot`

---

### Task 7.6: `Plan::write_to_dir`

**File:** `crates/pgevolve-core/src/plan/serialize.rs`

```rust
pub fn write_plan_dir(plan: &Plan, dir: &Path) -> Result<(), PlanIoError> {
    std::fs::create_dir_all(dir).map_err(|e| PlanIoError::Io(dir.into(), e))?;
    let mut sql = std::fs::File::create(dir.join("plan.sql"))?;
    write_plan_sql(plan, &mut sql)?;
    let mut intent = std::fs::File::create(dir.join("intent.toml"))?;
    write_intent_toml(plan, &mut intent)?;
    let mut manifest = std::fs::File::create(dir.join("manifest.toml"))?;
    write_manifest_toml(plan, &mut manifest)?;
    Ok(())
}
```

Tests: `tempdir`, write plan, list files, verify all three exist.

Commit: `feat(core): Plan::write_to_dir orchestrates the three writers`

---

### Task 7.7: `plan.sql` reader

**File:** `crates/pgevolve-core/src/plan/deserialize.rs`

Parses the structured `-- @pgevolve` directives. Each directive line is a key=value list. The reader emits a sequence of high-level events (header, group_begin, step, group_end) and assembles a partial `Plan` with steps and groups but no intents/metadata.

Implementation: line-based scanner (no need to involve pg_query — the SQL between directives is just opaque text per step).

```rust
pub fn read_plan_sql(s: &str) -> Result<PartialPlan, PlanIoError> { ... }

pub struct PartialPlan {
    pub id_short: String,
    pub pgevolve_version: String,
    pub planner_ruleset_version: u32,
    pub source_rev: Option<String>,
    pub target_identity: String,
    pub intents_required: u32,
    pub groups: Vec<TransactionGroup>,
    pub created_at: time::OffsetDateTime,
}
```

Each step's SQL body: lines between this step's directive and the next directive (or COMMIT or end-of-file). Trim trailing semicolon and whitespace.

Tests: round-trip — write a plan, read it back, assert structurally equal (modulo `target_snapshot` which is in `manifest.toml`).

Commit: `feat(core): read_plan_sql parser for directives + per-step SQL bodies`

---

### Task 7.8: `intent.toml` and `manifest.toml` readers

**File:** `crates/pgevolve-core/src/plan/deserialize.rs`

Use `toml::from_str` to deserialize both files. For `manifest.toml`, parse `target_snapshot_yaml` via `serde_yaml::from_str` to recover the `Catalog`.

Tests: round-trip.

Commit: `feat(core): read_intent_toml + read_manifest_toml`

---

### Task 7.9: `Plan::read_from_dir`

**File:** `crates/pgevolve-core/src/plan/deserialize.rs`

```rust
pub fn read_plan_dir(dir: &Path) -> Result<Plan, PlanIoError> {
    let sql_path      = dir.join("plan.sql");
    let intent_path   = dir.join("intent.toml");
    let manifest_path = dir.join("manifest.toml");

    let sql = std::fs::read_to_string(&sql_path)?;
    let intent_str = std::fs::read_to_string(&intent_path)?;
    let manifest_str = std::fs::read_to_string(&manifest_path)?;

    let partial = read_plan_sql(&sql)?;
    let intent = read_intent_toml(&intent_str)?;
    let manifest = read_manifest_toml(&manifest_str)?;

    // Cross-check: plan_id consistency
    if partial.id_short != intent.plan_id || partial.id_short != manifest.plan_id {
        return Err(PlanIoError::PlanIdMismatch {
            sql: partial.id_short,
            intent: intent.plan_id,
            manifest: manifest.plan_id,
        });
    }

    let plan_id = PlanId::from_full_hex(&manifest.plan_hash)?;

    Ok(Plan {
        id: plan_id,
        groups: partial.groups,
        intents: intent.into_intents(),
        metadata: manifest.into_metadata(),
    })
}
```

Tests: full round-trip property — write a plan to a tempdir, read it back, assert `==` (after defining `PartialEq` for `Plan` for tests only).

Commit: `feat(core): Plan::read_from_dir round-trips plan directories`

---

### Task 7.10: `PlanIoError`

**File:** `crates/pgevolve-core/src/plan/io_error.rs`

```rust
#[derive(Debug, thiserror::Error)]
pub enum PlanIoError {
    #[error("I/O error on {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("malformed directive: {0}")]
    MalformedDirective(String),
    #[error("plan id mismatch: sql={sql} intent={intent} manifest={manifest}")]
    PlanIdMismatch { sql: String, intent: String, manifest: String },
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("yaml parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("invalid plan hash: {0}")]
    InvalidPlanHash(String),
}
```

Commit: `feat(core): PlanIoError type`

---

### Task 7.11: Phase 7 self-review

- Round-trip property test: random small plans round-trip exactly.
- `PlanId::compute(c, c, "0.1.0", 1)` is stable across runs and across machines.
- `cargo test -p pgevolve-core` passes; clippy clean.

Phase 7 complete.
