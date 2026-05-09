//! `pgevolve-testkit` — internal test infrastructure for the pgevolve workspace.
//!
//! Consumed only as a `dev-dependency`. Provides ephemeral Postgres
//! provisioning, IR generators, equivalence asserters, and end-to-end
//! harnesses for property and chaos testing.
#![warn(missing_docs)]
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {}
}
