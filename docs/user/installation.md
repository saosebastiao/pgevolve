# Installation

v0.3.x is pre-release; the only supported install is from source.

## Prerequisites

- **Rust 1.95 or newer.** The repo pins `1.95` in `rust-toolchain.toml`,
  so `rustup` will pick it up automatically.
- **Docker** (optional). Required only for `pgevolve validate --shadow`,
  for the tier-3/4/5 test suites, and for any local property-test runs.
- **Postgres 14–17.** pgevolve introspects through `pg_catalog`; major
  versions outside this range are not tested.

## Build the binary

```sh
git clone https://github.com/saosebastiao/pgevolve.git
cd pgevolve
cargo build --release -p pgevolve
```

The release binary lands at `target/release/pgevolve`. Add it to your
`PATH` (or copy it to `~/.local/bin/`, `/usr/local/bin/`, etc.):

```sh
install -m 0755 target/release/pgevolve ~/.local/bin/pgevolve
pgevolve --version
```

## Verify the install

A no-Postgres smoke check:

```sh
pgevolve --help
pgevolve init --dir /tmp/pgevolve-smoke
ls /tmp/pgevolve-smoke
# → pgevolve.toml  schema/  plans/  .gitignore
```

If you have Docker available and want to verify the shadow path works
end-to-end:

```sh
cd /tmp/pgevolve-smoke
echo '[shadow]
backend          = "testcontainers"
postgres_version = "16"' >> pgevolve.toml
mkdir -p schema/app
cat > schema/app/0001-init.sql <<'SQL'
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
    id    bigint NOT NULL,
    email text   NOT NULL,
    CONSTRAINT users_pkey PRIMARY KEY (id)
);
SQL
pgevolve validate --shadow
# → pgevolve validate --shadow: round-trip matched (1 object(s))
```

## Upgrading

Until a stable release ships, upgrade by pulling and rebuilding:

```sh
cd pgevolve
git pull
cargo build --release -p pgevolve
install -m 0755 target/release/pgevolve ~/.local/bin/pgevolve
```

The `pgevolve` metadata schema is upgraded idempotently by every command
that touches the DB, so there's no separate migration step on upgrade.

## Pre-built binaries / package managers

Not yet published. The first stable release will ship GitHub-release
binaries for at least Linux x86_64 and macOS arm64. Homebrew, Cargo
(`cargo install pgevolve`), and a Docker image are planned but not
yet committed.
