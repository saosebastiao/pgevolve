# Security Policy

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security vulnerabilities.

Instead, report the issue privately via GitHub's [security advisory form](https://github.com/saosebastiao/pgevolve/security/advisories/new). Include:

- A description of the issue and its potential impact.
- Steps to reproduce (proof-of-concept code is ideal).
- Affected versions and configurations.
- Whether you intend to publish your findings, and on what timeline.

We acknowledge reports within **7 days**. We aim to provide a fix or mitigation within **30 days** for medium-severity issues, and faster for criticals at the maintainer's discretion.

After a fix is available we publish the issue with a CVE where applicable. Credit is given to the reporter unless they ask to remain anonymous.

We do not pursue legal action against good-faith security researchers acting in accordance with this policy.

## Supported versions

| Version | Supported                              |
|---------|----------------------------------------|
| 0.3.x   | ✅ Active                              |
| 0.2.x   | ⚠️  Security fixes only (until v1.0)   |
| 0.1.x   | ❌ Unsupported (please upgrade)        |

## Scope

In scope:

- The `pgevolve` CLI and the `pgevolve-core` library.
- The plan-format on-disk artifacts (`plan.sql`, `intent.toml`, `manifest.toml`).
- Catalog-introspection SQL run by `pgevolve` against managed Postgres instances.
- The shadow-validation path.

Out of scope:

- Vulnerabilities in third-party dependencies — please report those upstream. We track advisories via `cargo deny check` (see `deny.toml`); when an upstream advisory affects pgevolve, we follow the same disclosure timeline.
- Self-inflicted misuse (e.g., running `pgevolve apply` against a database without backups).

## Reference

This policy implements §10 of [`docs/CONSTITUTION.md`](../docs/CONSTITUTION.md). The constitution is the authoritative source for project security values; this file is the operational reporting channel.
