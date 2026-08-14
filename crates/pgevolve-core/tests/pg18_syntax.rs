//! What PG 18-only syntax does, end to end.
//!
//! The vendored parser is PG 18, so the grammar now *accepts* constructs
//! pgevolve does not model. That is the dangerous state: a construct that parses
//! but lowers to the wrong IR is worse than one that fails, because the failure
//! shows up later as DDL that silently changes the author's schema.
//!
//! Two of those were live the moment the parser bumped from 17 to 18:
//!
//! - `GENERATED ... VIRTUAL` was read as `STORED`, because the builder hardcoded
//!   the kind — safe under the PG 17 grammar, which could not produce anything
//!   else.
//! - `NOT ENFORCED` was dropped by the constraint decoder's catch-all arm, so
//!   the constraint was modelled as enforced.
//!
//! This file pins the outcome for each PG 18 feature: it either lowers to the
//! right IR, or it fails loudly. Never in between.
//!
//! `VIRTUAL` and `NOT ENFORCED` have both since moved from the second column to
//! the first — they are modelled now, and their assertions here flipped from
//! "refused" to "round-trips". Temporal keys are still refused; when they are
//! implemented, that assertion flips the same way.

// Integration tests are separate compilation units; the crate-level allow
// doesn't propagate. See crates/pgevolve-core/src/lib.rs for rationale.
#![allow(clippy::result_large_err)]

use pgevolve_core::parse::{ParseError, parse_directory};

/// Parse one SQL file's worth of DDL through the production entry point.
fn parse(sql: &str) -> Result<pgevolve_core::ir::catalog::Catalog, ParseError> {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(tmp.path().join("f.sql"), sql).expect("write fixture");
    parse_directory(tmp.path(), &[])
}

/// Assert the DDL is refused, and that the message names the feature.
#[track_caller]
fn refused(sql: &str, expected_substring: &str) {
    let err = parse(sql).expect_err("expected this PG 18 feature to be refused");
    let message = err.to_string();
    assert!(
        message.contains(expected_substring),
        "refused, but not for the expected reason.\n  wanted substring: \
         {expected_substring:?}\n  got: {message}"
    );
}

#[track_caller]
fn accepted(sql: &str) {
    parse(sql).expect("expected this DDL to parse and lower cleanly");
}

// ---- refused: parses under PG 18, not modelled by pgevolve ----

#[test]
fn temporal_key_is_refused() {
    refused(
        "CREATE SCHEMA app;\n\
         CREATE TABLE app.t (id int, valid daterange, \
         CONSTRAINT pk PRIMARY KEY (id, valid WITHOUT OVERLAPS));",
        "temporal constraints",
    );
}

// ---- accepted: the ordinary forms these refusals must not catch ----
//
// The `is_enforced` flag is only meaningful for CHECK and FOREIGN KEY; Postgres
// leaves it false on PRIMARY KEY and UNIQUE. Testing it unconditionally rejects
// every primary key, which is what the first version of the check did.

#[test]
fn stored_generated_column_still_works() {
    accepted(
        "CREATE SCHEMA app;\n\
         CREATE TABLE app.t (a int, b int GENERATED ALWAYS AS (a * 2) STORED);",
    );
}

/// The kind must survive into the IR, not merely parse.
///
/// This is the assertion that would have caught the original bug: the DDL
/// parsed fine while `VIRTUAL` was being read as `STORED`, so only an IR-level
/// check distinguishes "supported" from "silently mis-modelled".
#[test]
fn generated_kind_round_trips_into_the_ir() {
    use pgevolve_core::ir::column::GeneratedKind;

    for (sql_kind, expected) in [
        ("VIRTUAL", GeneratedKind::Virtual),
        ("STORED", GeneratedKind::Stored),
    ] {
        let catalog = parse(&format!(
            "CREATE SCHEMA app;\n\
             CREATE TABLE app.t (a int, b int GENERATED ALWAYS AS (a * 2) {sql_kind});"
        ))
        .expect("parses and lowers");

        let table = catalog
            .tables
            .iter()
            .find(|t| t.qname.name.as_str() == "t")
            .expect("table t");
        let column = table
            .columns
            .iter()
            .find(|c| c.name.as_str() == "b")
            .expect("column b");
        let generated = column
            .generated
            .as_ref()
            .unwrap_or_else(|| panic!("{sql_kind} column should be generated"));

        assert_eq!(
            generated.kind, expected,
            "{sql_kind} lowered to the wrong GeneratedKind"
        );
    }
}

#[test]
fn ordinary_constraints_are_not_caught_by_the_pg18_refusals() {
    for (label, sql) in [
        (
            "primary key",
            "CREATE TABLE app.t (id int, CONSTRAINT pk PRIMARY KEY (id));",
        ),
        (
            "unique",
            "CREATE TABLE app.t (a int, CONSTRAINT u UNIQUE (a));",
        ),
        (
            "check",
            "CREATE TABLE app.t (a int, CONSTRAINT c CHECK (a > 0));",
        ),
        (
            "foreign key",
            "CREATE TABLE app.p (id int PRIMARY KEY);\n\
             CREATE TABLE app.t (a int, CONSTRAINT f FOREIGN KEY (a) REFERENCES app.p(id));",
        ),
    ] {
        let full = format!("CREATE SCHEMA app;\n{sql}");
        assert!(
            parse(&full).is_ok(),
            "the PG 18 refusals wrongly rejected an ordinary {label} constraint"
        );
    }
}

/// Enforcement must survive into the IR, not merely parse.
///
/// The original bug was the mirror of the VIRTUAL one: `NOT ENFORCED` parsed
/// cleanly and was dropped by the constraint decoder's catch-all arm, so the
/// constraint was modelled as *enforced* — and pgevolve would have emitted DDL
/// that starts checking data the author declared unchecked.
#[test]
fn constraint_enforcement_round_trips_into_the_ir() {
    use pgevolve_core::ir::constraint::Enforcement;

    for (clause, expected) in [
        (" NOT ENFORCED", Enforcement::NotEnforced),
        ("", Enforcement::Enforced),
    ] {
        let catalog = parse(&format!(
            "CREATE SCHEMA app;\n\
             CREATE TABLE app.t (a int, CONSTRAINT c CHECK (a > 0){clause});"
        ))
        .expect("parses and lowers");

        let table = catalog
            .tables
            .iter()
            .find(|t| t.qname.name.as_str() == "t")
            .expect("table t");
        let constraint = table
            .constraints
            .iter()
            .find(|c| c.qname.name.as_str() == "c")
            .expect("constraint c");

        assert_eq!(
            constraint.enforcement, expected,
            "CHECK{clause:?} lowered to the wrong Enforcement"
        );
    }
}

/// `is_enforced` is false on PK and UNIQUE, where the concept does not apply.
/// Reading it unconditionally would mark every primary key NOT ENFORCED.
#[test]
fn primary_and_unique_keys_are_always_enforced() {
    use pgevolve_core::ir::constraint::Enforcement;

    let catalog = parse(
        "CREATE SCHEMA app;\n\
         CREATE TABLE app.t (id int, a int, CONSTRAINT pk PRIMARY KEY (id), \
         CONSTRAINT u UNIQUE (a));",
    )
    .expect("parses and lowers");

    let table = &catalog.tables[0];
    assert!(!table.constraints.is_empty(), "expected constraints");
    for constraint in &table.constraints {
        assert_eq!(
            constraint.enforcement,
            Enforcement::Enforced,
            "{} should be enforced",
            constraint.qname.name
        );
    }
}

#[test]
fn the_vendored_parser_is_the_major_we_claim() {
    // If this fails, every refusal above is testing the wrong grammar.
    assert_eq!(pgevolve_pgquery::VENDORED_PG_MAJOR, 18);
}
