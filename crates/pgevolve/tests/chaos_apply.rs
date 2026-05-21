//! Chaos-apply harness: abort an apply midway through and recover by
//! re-planning against the partial live state.
//!
//! The abort is a clean abort (the executor sees `abort_after_step` and
//! returns `ApplyError::AbortedAfterStep`) rather than SIGKILL — the
//! recovery semantics we want to validate are identical, and a clean abort
//! is cheaper to reproduce reliably than a child-process SIGKILL.
//!
//! Skipped when Docker is unavailable.
//!
//! All tests in this file are #[ignore]'d for CI. Run with
//! `cargo test --test chaos_apply -- --ignored` locally, or via the
//! property-tests.yml workflow.

#![allow(clippy::items_after_statements)]

mod common;

use anyhow::Result;
use pgevolve::executor::ApplyError;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

use common::{apply_diff, connect_and_bootstrap, introspect, schemas_of};

/// Apply `final_` to a fresh DB but abort after `abort_step`. Then re-plan
/// from the partial live state and apply to completion. Verify the live
/// state matches `final_`.
async fn run_chaos(final_: &Catalog, abort_step: u32) -> Result<()> {
    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let managed = schemas_of(final_);

    let mut client = connect_and_bootstrap(&pg).await?;

    // First apply: aborted midway.
    let first = apply_diff(
        &mut client,
        &Catalog::empty(),
        final_,
        &managed,
        Some(abort_step),
    )
    .await?;
    match first {
        Err(ApplyError::AbortedAfterStep { step_no }) => {
            assert_eq!(step_no, abort_step);
        }
        Err(other) => return Err(anyhow::anyhow!("expected AbortedAfterStep, got {other:?}")),
        Ok(_) => {
            // The abort_step was past the last step; the plan ran to
            // completion. Skip the recovery half of the test.
            return Ok(());
        }
    }

    // The partial live state — diff from this to final_.
    let partial = introspect(&pg, &managed).await?;

    // Second apply: from partial → final, no abort.
    let second = apply_diff(&mut client, &partial, final_, &managed, None).await?;
    second.map_err(|e| anyhow::anyhow!("recovery apply failed: {e}"))?;

    let live = introspect(&pg, &managed).await?;
    pgevolve_testkit::assert_canonical_eq(final_, &live)
}

#[ignore = "property test — run via property-tests workflow or `cargo test -- --ignored`"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborted_mid_apply_recovers_after_replan() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }

    use pgevolve_core::identifier::{Identifier, QualifiedName};
    use pgevolve_core::ir::column::Column;
    use pgevolve_core::ir::column_type::ColumnType;
    use pgevolve_core::ir::constraint::{Constraint, ConstraintKind, Deferrable};
    use pgevolve_core::ir::schema::Schema;
    use pgevolve_core::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    // Build a plan with 3 explicit steps: CREATE SCHEMA + CREATE TABLE x2.
    let mut catalog = Catalog::empty();
    catalog.schemas.push(Schema::new(id("chaos")));
    for t in ["alpha", "beta"] {
        catalog.tables.push(Table {
            qname: QualifiedName::new(id("chaos"), id(t)),
            columns: vec![Column {
                name: id("id"),
                ty: ColumnType::BigInt,
                nullable: false,
                default: None,
                identity: None,
                generated: None,
                collation: None,
                comment: None,
            }],
            constraints: vec![Constraint {
                qname: QualifiedName::new(id("chaos"), id(&format!("{t}_pkey"))),
                kind: ConstraintKind::PrimaryKey {
                    columns: vec![id("id")],
                    include: vec![],
                },
                deferrable: Deferrable::NotDeferrable,
                comment: None,
            }],
            partition_by: None,
            partition_of: None,
            comment: None,
        });
    }
    let canonical = catalog.canonicalize().unwrap();

    // Abort after step 2 (i.e., after creating one of the tables) and
    // verify recovery brings us to the full final state.
    run_chaos(&canonical, 2).await.expect("chaos recovery");
}
