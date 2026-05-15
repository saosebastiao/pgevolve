//! Determinism harness.
//!
//! The conformance suite (see docs/superpowers/specs/2026-05-11-conformance-test-suite-design.md)
//! relies on plan-SQL goldens. Plan-SQL goldens require the in-process
//! planner pipeline to produce byte-identical output for the same input
//! across runs. This test runs the pipeline 100 times and asserts byte
//! equality; any flake here is a determinism bug to fix at the source.

use pgevolve_core::diff::diff;
use pgevolve_core::parse::parse_directory;
use pgevolve_core::plan::{
    Plan, PlannerPolicy, Strategy, group_steps, order, rewrite, write_plan_sql,
};

const BEFORE_SQL: &str = "\
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orgs (
  id bigint NOT NULL,
  CONSTRAINT orgs_pkey PRIMARY KEY (id)
);
CREATE TABLE app.users (
  id bigint NOT NULL,
  org_id bigint NOT NULL,
  CONSTRAINT users_pkey PRIMARY KEY (id),
  CONSTRAINT users_org_fk FOREIGN KEY (org_id) REFERENCES app.orgs(id)
);
CREATE INDEX users_org_idx ON app.users (org_id);
";

const AFTER_SQL: &str = "\
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orgs (
  id bigint NOT NULL,
  name text NOT NULL,
  CONSTRAINT orgs_pkey PRIMARY KEY (id)
);
CREATE TABLE app.users (
  id bigint NOT NULL,
  org_id bigint NOT NULL,
  email text NOT NULL,
  CONSTRAINT users_pkey PRIMARY KEY (id),
  CONSTRAINT users_org_fk FOREIGN KEY (org_id) REFERENCES app.orgs(id)
);
CREATE INDEX users_org_idx ON app.users (org_id);
CREATE INDEX users_email_idx ON app.users (email);
";

fn render_plan_once() -> String {
    let before_dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(before_dir.path().join("before.sql"), BEFORE_SQL).unwrap();
    let target = parse_directory(before_dir.path(), &[]).expect("parse before");

    let after_dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(after_dir.path().join("after.sql"), AFTER_SQL).unwrap();
    let source = parse_directory(after_dir.path(), &[]).expect("parse after");

    let changes = diff(&target, &source, &pgevolve_core::catalog::DriftReport::default());
    let ordered = order(&target, &source, changes).expect("order");
    let policy = PlannerPolicy {
        strategy: Strategy::Online,
        ..PlannerPolicy::default()
    };
    let steps = rewrite(ordered, &target, &policy);
    let groups = group_steps(steps);
    // Fixed target identity so plan IDs are reproducible. Any stable string
    // works; we just need it identical across iterations.
    let plan = Plan::from_grouped(
        groups,
        &source,
        &target,
        "determinism-test-target".to_string(),
        None,
        pgevolve_core::VERSION,
        policy.planner_ruleset_version,
    );

    let mut buf = Vec::new();
    write_plan_sql(&plan, &mut buf).expect("render plan.sql");
    let raw = String::from_utf8(buf).expect("plan.sql is utf-8");

    // Strip the `created=<timestamp>` field from the plan header. The
    // timestamp is intentionally set to `now()` on each call to
    // `Plan::from_grouped` and is therefore legitimately non-identical
    // across runs. Everything else in plan.sql must be byte-identical.
    normalize_plan_sql(&raw)
}

/// Remove the `created=<rfc3339>` token from the plan header line so the
/// timestamp does not interfere with byte-identity comparison.
fn normalize_plan_sql(s: &str) -> String {
    s.lines()
        .map(|line| {
            if line.starts_with("-- @pgevolve plan ") {
                // Strip ` created=<value>` — the value has no spaces, so we
                // find the token and remove everything from " created=" to the
                // next space (or end of line).
                line.find(" created=").map_or_else(
                    || line.to_string(),
                    |start| {
                        let after_prefix = &line[start + " created=".len()..];
                        let end = after_prefix.find(' ').unwrap_or(after_prefix.len());
                        format!("{}{}", &line[..start], &after_prefix[end..])
                    },
                )
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[test]
fn planner_pipeline_is_byte_deterministic() {
    let baseline = render_plan_once();
    for i in 0..99 {
        let next = render_plan_once();
        if next != baseline {
            let dir = tempfile::tempdir().expect("tempdir");
            std::fs::write(dir.path().join("baseline.sql"), &baseline).unwrap();
            std::fs::write(dir.path().join(format!("run-{i}.sql")), &next).unwrap();
            panic!(
                "planner output differed on iteration {i}. baseline written to {} for inspection.",
                dir.path().display()
            );
        }
    }
}
