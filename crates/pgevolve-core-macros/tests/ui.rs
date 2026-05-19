//! `trybuild` UI tests for `#[derive(Diff)]`. Each `tests/ui/*.rs` is a
//! file that MUST fail to compile; the expected stderr lives next to it
//! as `*.stderr`. To regenerate stderr files after intentional message
//! changes, set TRYBUILD=overwrite in the env.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
