//! `NormalizedBody` equivalence tests.
//!
//! Two bodies that differ only in whitespace, qualifier presence, or
//! redundant parens must produce equal `NormalizedBody`.

use pgevolve_core::parse::normalize_body::NormalizedBody;

#[test]
fn whitespace_differences_are_equivalent() {
    let a = NormalizedBody::from_sql("SELECT id FROM app.users").unwrap();
    let b = NormalizedBody::from_sql("SELECT    id\nFROM   app.users").unwrap();
    assert_eq!(a.canonical_text(), b.canonical_text());
    assert_eq!(a.canonical_hash(), b.canonical_hash());
}

#[test]
fn redundant_parens_are_equivalent() {
    let a = NormalizedBody::from_sql("SELECT (id) FROM app.users").unwrap();
    let b = NormalizedBody::from_sql("SELECT id FROM app.users").unwrap();
    assert_eq!(a.canonical_text(), b.canonical_text());
}

#[test]
fn materially_different_bodies_differ() {
    let a = NormalizedBody::from_sql("SELECT id FROM app.users").unwrap();
    let b = NormalizedBody::from_sql("SELECT id FROM app.products").unwrap();
    assert_ne!(a.canonical_text(), b.canonical_text());
    assert_ne!(a.canonical_hash(), b.canonical_hash());
}

#[test]
fn invalid_sql_returns_error() {
    let err = NormalizedBody::from_sql("SELECT FROM WHERE").unwrap_err();
    let msg = err.to_string();
    assert!(!msg.is_empty(), "error should have a non-empty message");
}
