# Cluster-level surface

pgevolve manages cluster-level state — roles (v0.3.0), with tablespaces,
cluster settings, foreign servers, and user mappings planned — through
a parallel project type and command family separate from per-database
projects.

## Project shape

```
my-cluster/
  pgevolve-cluster.toml
  roles/
    app.sql
    ops.sql
```

## Commands

- `pgevolve cluster init [path]` — scaffold a new cluster project
- `pgevolve cluster diff` — show diff between source and live cluster
- `pgevolve cluster plan` — write a cluster plan directory
- `pgevolve cluster apply [<plan_id>]` — apply a cluster plan
- `pgevolve cluster status` — list applied/pending plans

## Currently managed

| Object | Status |
|---|---|
| Roles (CREATE/ALTER/DROP ROLE, CREATE USER) | ✅ v0.3.0 |
| Role membership (GRANT role TO target) | ✅ v0.3.0 |
| Tablespaces | 🔮 Future |
| Cluster GUCs (postgresql.conf) | 🔮 Future |
| Foreign servers / user mappings | 🔮 Future |
| Databases list | 🔮 Future |

## Passwords

Passwords are **not stored in source**. The catalog reader skips
`rolpassword`; the source parser drops `PASSWORD '…'` clauses
silently. Set passwords out-of-band (`psql`, secret manager, etc.).

## Bootstrap roles

The `[bootstrap].roles` list in `pgevolve-cluster.toml` names roles
that pgevolve treats as PG-owned and never diffs in or out. Defaults
to `["postgres"]`. Cloud Postgres (RDS, Cloud SQL, etc.) typically
needs additional entries (e.g. `["postgres", "cloudsqlsuperuser"]`).

## Linking from per-DB projects

Per-DB projects can lint-check grantee role names against the cluster
project by setting `[cluster].project = "../my-cluster"` in
`pgevolve.toml`. See `docs/spec/grants.md` for details.

## Known limitations (v0.3.0)

- Cluster apply does not yet write to a per-DB-style `pgevolve.apply_log`.
  The `pgevolve cluster status` command lists plan directories rather
  than reading applied state from the DB. Will be addressed when the
  cluster executor reaches feature-parity with the per-DB one.
- DROP ROLE steps are marked `destructive: true` in the emit pipeline,
  but cluster apply does not yet read `intent.toml` to gate them —
  it executes whatever is in `plan.sql`. Operators should review
  `cluster-plans/<id>/plan.sql` before running `pgevolve cluster apply`.
- No advisory lock is taken during cluster apply; concurrent applies
  against the same cluster are not protected.
- Object-level GRANT/REVOKE (e.g. `GRANT SELECT ON TABLE`) is per-DB,
  not cluster. It ships in v0.3.1.
- Row-level security policies ship in v0.3.2.
