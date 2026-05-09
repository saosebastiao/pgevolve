# Phase 10 — Layout-profile linter

**Goal:** Land the lint engine: universal rules that always apply, four built-in layout profiles (schema-mirror, kind-grouped, feature-grouped, free-form), and a custom-profile mechanism using regex+assertion rules.

**Spec coverage:** §12.

**Depends on:** Phase 2 (parser; lint operates over Source IR + per-file paths).

**Exit criteria:**

- `pgevolve_core::lint::run(source: &SourceTree, managed: &ManagedConfig, profile: &Profile) -> Vec<Finding>`.
- Each universal rule from spec §12.1 has a test.
- Each built-in profile has at least one passing fixture and one failing fixture.
- Custom profile loaded from a TOML file works.
- `pgevolve lint` invokes the engine and prints findings cleanly.

---

## File structure

```
crates/pgevolve-core/src/
└── lint/
    ├── mod.rs            # public re-exports + run()
    ├── finding.rs        # Finding, Severity
    ├── source_tree.rs    # SourceTree wrapping parse output + per-file metadata
    ├── universal.rs      # universal rules
    └── profile/
        ├── mod.rs
        ├── loader.rs     # built-in / custom dispatcher
        ├── schema_mirror.rs
        ├── kind_grouped.rs
        ├── feature_grouped.rs
        ├── free_form.rs
        └── custom.rs
```

---

### Task 10.1: `Finding`, `Severity`, and `SourceTree`

**Files:**
- `crates/pgevolve-core/src/lint/finding.rs`
- `crates/pgevolve-core/src/lint/source_tree.rs`

```rust
pub enum Severity { Error, Warning }

pub struct Finding {
    pub severity: Severity,
    pub rule: &'static str,
    pub message: String,
    pub location: Option<SourceLocation>,
}

pub struct SourceTree {
    pub catalog: Catalog,
    /// Maps each object's qname to the file it was defined in.
    pub object_locations: HashMap<ObjectKey, SourceLocation>,
}

pub enum ObjectKey {
    Schema(QualifiedName),
    Table(QualifiedName),
    Index(QualifiedName),
    Sequence(QualifiedName),
}
```

Phase 2's `parse_directory` is enhanced this phase to return `SourceTree` instead of bare `Catalog` (preserving `object_locations`). Adjust phase-2 callers accordingly.

Commit: `feat(core): SourceTree + Finding + Severity`

---

### Task 10.2: Universal rules

**File:** `crates/pgevolve-core/src/lint/universal.rs`

For each rule in spec §12.1:

```rust
pub fn check_universal(
    tree: &SourceTree,
    managed: &ManagedConfig,
) -> Vec<Finding> {
    let mut out = Vec::new();
    out.extend(rule_managed_schemas_match(tree, managed));
    out.extend(rule_no_duplicate_qnames(tree));
    out.extend(rule_no_unsupported_kinds(tree));
    out.extend(rule_no_alter_outside_whitelist(tree));
    out.extend(rule_closed_world_references(tree));
    out
}
```

Each `rule_*` function tests one spec rule:

- **`rule_managed_schemas_match`** — every `managed.schemas` entry has a matching `Schema` in `tree.catalog`; every `Schema` in the tree is in `managed.schemas`. Two-way check; emits one finding per mismatch.

- **`rule_no_duplicate_qnames`** — already enforced by `parse_directory` (which emits `DuplicateObject` errors). If the parser caught duplicates we won't reach this; but for safety, double-check via `HashSet`.

- **`rule_no_unsupported_kinds`** — parser already errors on these. No-op here; placeholder for non-error variants we may want to demote to warnings later.

- **`rule_no_alter_outside_whitelist`** — also parser-enforced; placeholder.

- **`rule_closed_world_references`** — every FK target table exists in the source IR; every column referenced by an index/constraint exists in its table; every type reference (UserDefined) — for v0.1 this rule is informational only since custom types are out-of-scope.

Schema qualification rule (spec §6.1) is handled by the parser, not here.

Tests cover each rule with passing and failing inputs.

Commit: `feat(core): universal lint rules`

---

### Task 10.3: `Profile` enum and dispatcher

**File:** `crates/pgevolve-core/src/lint/profile/mod.rs`

```rust
pub enum Profile {
    SchemaMirror,
    KindGrouped,
    FeatureGrouped,
    FreeForm,
    Custom(CustomProfile),
}

pub fn check_profile(profile: &Profile, tree: &SourceTree, schema_dir: &Path) -> Vec<Finding> {
    match profile {
        Profile::SchemaMirror   => schema_mirror::check(tree, schema_dir),
        Profile::KindGrouped    => kind_grouped::check(tree, schema_dir),
        Profile::FeatureGrouped => feature_grouped::check(tree, schema_dir),
        Profile::FreeForm       => Vec::new(),  // no path constraints
        Profile::Custom(c)      => custom::check(c, tree, schema_dir),
    }
}
```

`profile_for(cfg)` (in `pgevolve` binary): looks at `cfg.project.layout_profile`. If it's a known name, return the built-in; if it's a path ending in `.toml`, load `CustomProfile` from disk.

Commit: `feat(core): Profile dispatcher`

---

### Task 10.4: `schema-mirror` profile

**File:** `crates/pgevolve-core/src/lint/profile/schema_mirror.rs`

For each object in `tree.object_locations`:
- Required path: `<schema_dir>/<schema>/<kind>/<name>.sql`.
- `kind` is one of `tables`, `indexes`, `sequences`, `schemas` (the special schema-only file).
- Schema files: `<schema_dir>/<schema>/_schema.sql` (Schema goes here; conventionally, this is also where you put `CREATE SCHEMA`).
- One object per file: count the objects parsed from each file path and emit `Finding::Error` if > 1.

Tests:
- Compliant tree → 0 findings.
- Table at wrong path → 1 finding.
- Two tables in one file → 1 finding.

Commit: `feat(core): schema-mirror lint profile`

---

### Task 10.5: `kind-grouped` profile

**File:** `crates/pgevolve-core/src/lint/profile/kind_grouped.rs`

For each object: required path is `<schema_dir>/<kind>/<schema>.<name>.sql`. Schema declarations in `<schema_dir>/schemas/<name>.sql`. One object per file enforced.

Commit: `feat(core): kind-grouped lint profile`

---

### Task 10.6: `feature-grouped` profile

**File:** `crates/pgevolve-core/src/lint/profile/feature_grouped.rs`

Less strict: every object's file path must start with `<schema_dir>/<some-feature-dir>/`. Multiple objects per file are allowed; cross-feature overlap is forbidden — i.e., if file `app/billing/invoices.sql` defines `app.invoices`, no other file in any other feature dir may define `app.invoices_idx_*` or other related objects (use prefix check on object name).

Actually that's hard to define rigorously. Simpler v0.1 rule: each feature dir is a black box; lint just checks "every file is under some `<schema_dir>/<dir>/`". The cross-overlap rule is dropped from v0.1 — document as a known limitation.

Commit: `feat(core): feature-grouped lint profile (path-shape only)`

---

### Task 10.7: `free-form` profile

**File:** `crates/pgevolve-core/src/lint/profile/free_form.rs`

`pub fn check(_tree: &SourceTree, _dir: &Path) -> Vec<Finding> { Vec::new() }`

Commit: `feat(core): free-form profile (no path rules)`

---

### Task 10.8: Custom profile (regex+assertion)

**File:** `crates/pgevolve-core/src/lint/profile/custom.rs`

```rust
#[derive(Debug, Deserialize)]
pub struct CustomProfile {
    pub patterns: Vec<PathPattern>,
}

#[derive(Debug, Deserialize)]
pub struct PathPattern {
    pub regex: String,                // with named captures: schema, kind, name
    pub assertions: Vec<Assertion>,   // e.g., "captured.schema == object.schema"
}

pub enum Assertion {
    SchemaMatchesCapture,           // qname.schema == captures.schema
    NameMatchesCapture,             // qname.name == captures.name
    KindMatchesCapture { allowed_values: HashMap<String, ObjectKindName> },
    OneObjectPerFile,
}

pub fn check(profile: &CustomProfile, tree: &SourceTree, dir: &Path) -> Vec<Finding> {
    // For each object: find the first pattern whose regex matches its file path; run assertions.
    // If no pattern matches, emit a finding ("file path doesn't match any custom pattern").
    // If a pattern matches but assertions fail, emit a finding per failed assertion.
}
```

A custom profile TOML file looks like:

```toml
[[patterns]]
regex = "^schema/(?P<schema>[^/]+)/(?P<kind>tables|indexes)/(?P<name>[^/]+)\\.sql$"
assertions = ["schema_matches_capture", "name_matches_capture", { kind_matches_capture = { tables = "table", indexes = "index" } }, "one_object_per_file"]
```

Tests:
- A custom profile that mirrors `schema-mirror` behaves identically.
- Bad assertion fails the rule.

Commit: `feat(core): custom layout profile with regex+assertion rules`

---

### Task 10.9: Wire `lint` into the `pgevolve` binary

Replace the phase-9 stub in `crates/pgevolve/src/commands/lint.rs` with the full driver: load profile via `profile_for(cfg)`, run universal + profile rules, format findings, exit with appropriate code.

Tests via `assert_cmd`: a complete tree with violations → exit 1, stderr lists each finding.

Commit: `feat(cli): wire lint command to phase-10 lint engine`

---

### Task 10.10: Phase 10 self-review

- Spec §12 walkthrough: every documented rule has a test.
- Each built-in profile has at least one passing and one failing fixture.
- `cargo test --workspace` passes; clippy clean.

Phase 10 complete.
