# Releasing pgevolve

This runbook applies the [Constitution §9](./CONSTITUTION.md#9-cicd-and-release-discipline) release discipline.

## Pre-flight

Before starting the release commit, the following must be true:

- [ ] All open PRs targeting the release have merged.
- [ ] `cargo test --workspace --all-targets` is green locally.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is green.
- [ ] `cargo fmt --all -- --check` is clean.
- [ ] `cargo doc --workspace --no-deps` builds with zero warnings (run with `RUSTDOCFLAGS=-D warnings`).
- [ ] `cargo deny check` passes. Install if missing: `cargo install cargo-deny`.
- [ ] The `[Unreleased]` section of `CHANGELOG.md` accurately describes everything in the release.
- [ ] The previous release tag's signature verifies (`git verify-tag <prev-tag>`) — proves your signing key is configured.

## Release commit

1. **Update versions** — three locations must agree:
   - `Cargo.toml` → `[workspace.package].version`
   - `crates/pgevolve-core-macros/Cargo.toml` → `[package].version` (not workspace-inherited because it's a proc-macro crate with its own version literal)
   - The next CHANGELOG section header

2. **Date-stamp the CHANGELOG entry**:
   ```markdown
   ## [X.Y.Z] — YYYY-MM-DD
   ```
   The CI `changelog` job (in `.github/workflows/ci.yml`) verifies that the `Cargo.toml` version has a matching `## [X.Y.Z] — DATE` line.

3. **Rebuild so `Cargo.lock` picks up the new version**:
   ```sh
   cargo build --workspace
   ```

4. **Commit**:
   ```sh
   git add Cargo.toml Cargo.lock CHANGELOG.md crates/pgevolve-core-macros/Cargo.toml
   git commit -m "release: vX.Y.Z"
   ```

## Tag

**Tags must be signed.** Per Constitution §9, an unsigned release tag is not a valid release.

```sh
git tag -s vX.Y.Z -m "pgevolve vX.Y.Z

<short release summary, 2-3 lines>"
```

Verify locally:
```sh
git verify-tag vX.Y.Z
```

If `git tag -s` complains, your signing key isn't configured. See:
- `git config user.signingkey <KEY-ID>`
- `git config gpg.format ssh` (for SSH signing — recommended in 2026)
- `git config gpg.ssh.allowedSignersFile <path>` (for verification)

The first time you sign, also enable signing-by-default for safety:
```sh
git config commit.gpgsign true
git config tag.gpgsign true
```

## Push

```sh
git push origin main
git push origin vX.Y.Z
```

CI will run on `main`. The tag does not trigger CI by itself in the current workflow setup.

## Publish to crates.io (optional)

When the release is ready for crates.io:

```sh
cargo publish -p pgevolve-core
# Wait ~30 seconds for the index to sync, then:
cargo publish -p pgevolve
```

`pgevolve-core-macros`, `pgevolve-conformance`, `pgevolve-testkit`, and `xtask` are all `publish = false` and stay local.

For pre-publish sanity:
```sh
cargo publish --dry-run -p pgevolve-core
cargo publish --dry-run -p pgevolve
```

## Post-release

1. Open a new `[Unreleased]` section at the top of `CHANGELOG.md`.
2. Optionally bump the workspace version to `X.(Y+1).0-dev` to make accidental crates.io uploads of a stale version impossible.
3. Create a GitHub release from the new tag with the CHANGELOG section as the body.

## Historical notes

- **v0.1.0 (`adb0177` parent commit)** and **v0.2.0 (`3087a5b`)** were tagged with annotated, **unsigned** tags. The 2026-05-21 constitution audit flagged this as a §9 violation. From v0.3.0 onward, every release tag is signed; the two historical tags are documented as exceptions and will not be re-signed because rewriting tag history breaks any consumer who has already fetched them.

## Why this exists

Constitution §9 mandates: "Release tags are signed. The `[workspace.package].version` field and the `CHANGELOG.md` entry must agree at the time of tagging." This file is the operational checklist that makes those guarantees mechanical.
