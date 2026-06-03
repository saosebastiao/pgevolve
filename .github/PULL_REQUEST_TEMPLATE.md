## Summary

<!-- 1-3 sentences: what changes, and why. -->

## Related issue

<!-- Most PRs should link an issue. Use `Closes #N` or `Fixes #N` so
the issue auto-closes on merge. -->

Closes #

## Verify gate

Run locally before pushing:

- [ ] `cargo fmt --all -- --check` is clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean
- [ ] `cargo test --workspace --all-targets` passes
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` is clean
- [ ] `cargo deny check` passes
- [ ] `CHANGELOG.md` `[Unreleased]` section updated for any user-visible change

## Notes for the reviewer

<!-- Optional: things worth calling out (deliberate trade-offs, areas
that need extra attention, follow-ups deferred to a separate PR). -->
