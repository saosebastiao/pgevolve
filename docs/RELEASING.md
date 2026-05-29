# Releasing pgevolve

This runbook applies the [Constitution §9](./CONSTITUTION.md#9-cicd-and-release-discipline)
release discipline + the [CLAUDE.md §11](../CLAUDE.md) "never `cargo
publish` until CI is green" rule (added 2026-05-28 after the v0.3.8
disaster).

## Canonical executable form

For a normal release, run:

```sh
scripts/release.sh X.Y.Z
```

The script walks every step below, gates on the pre-flight verify,
waits for CI green on both the push and the tag, and prompts before
each irreversible action (publish, push, tag). If you bail at any
prompt, nothing irrecoverable has happened.

The prose runbook below documents what the script is doing, for
when something goes wrong and you need to step through manually.

---

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

## Push main + WAIT FOR CI GREEN

Push the release commit:
```sh
git push origin main
```

Then **wait for the per-push CI run to finish green across all 5 PG
majors**. Per CLAUDE.md §11, this is non-negotiable — today's v0.3.8
disaster happened because the maintainer published immediately after
the tag push while CI was still running. CI then failed on PG 15 + 16
(broken ICU SQL); the buggy v0.3.8 was already on crates.io.

```sh
COMMIT=$(git rev-parse HEAD)
RUN_ID=$(gh run list --branch main --commit "$COMMIT" --limit 1 --json databaseId --jq '.[0].databaseId')
gh run watch "$RUN_ID" --exit-status
```

`--exit-status` makes the command return non-zero if any job fails;
do not proceed past this step until it returns 0.

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

## Push tag

```sh
git push origin vX.Y.Z
```

The tag push does not trigger CI by itself in the current workflow
setup — CI already ran on the underlying commit when you pushed
main above. If for some reason the tag is on a different commit
than main's HEAD, wait for CI green on that commit too before
publishing.

## Publish to crates.io

When the release is ready for crates.io, publish in dependency order:

```sh
cargo publish -p pgevolve-core-macros
# Wait ~30 seconds for the index to sync, then:
cargo publish -p pgevolve-core
# Wait ~30 seconds again, then:
cargo publish -p pgevolve
```

`pgevolve-core-macros` is a proc-macro crate that's only published so
`pgevolve-core` resolves on crates.io — it's not a stable public API.
Bumping its version follows the same workspace-bump cadence; lock it
in lockstep with `pgevolve-core` to avoid version-skew surprises.

`pgevolve-conformance`, `pgevolve-testkit`, and `xtask` are all
`publish = false` and stay local.

For pre-publish sanity:
```sh
cargo publish --dry-run -p pgevolve-core-macros
cargo publish --dry-run -p pgevolve-core
cargo publish --dry-run -p pgevolve
```

## Yank a prior version (if shipping a fix)

If the version you just published replaces a buggy prior version
(v0.3.9 replacing the broken v0.3.8 is the canonical example), yank
the prior so cargo prefers the new version:

```sh
cargo yank --version <prior> pgevolve-core
cargo yank --version <prior> pgevolve
```

The CHANGELOG entry for the fix release should call out the yank
explicitly in a `### Yanked` section. Yanking does not remove the
prior version from crates.io — cargo can still resolve it if pinned —
but it stops new installs from picking it up.

## Post-release

- Push the tag (already done above; this is the reminder bullet).
- Verify the badge updates: README's `[![crates.io]` and `[![Soak]`
  badges refresh within a few minutes.
- If this release closes a v1.0-checklist row (per
  [`v1.md`](./v1.md) §4), flip the row's status in
  [`spec/objects.md`](./spec/objects.md) and remove it from
  [`spec/roadmap.md`](./spec/roadmap.md)'s Active matrix in a
  follow-up docs commit.
