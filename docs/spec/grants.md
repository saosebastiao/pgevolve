# Object grants + ownership + default privileges

pgevolve manages Postgres object permissions declaratively. Every
grantable IR object (Schema, Sequence, Table, View, MaterializedView,
Function, Procedure, UserType) carries:

- `owner: Option<Identifier>` — opt-in object ownership.
- `grants: Vec<Grant>` — per-object ACL entries.

Plus the top-level `Catalog.default_privileges: Vec<DefaultPrivilegeRule>`
mirroring `pg_default_acl`.

## Source surface

```sql
ALTER TABLE app.t OWNER TO app_owner;
GRANT SELECT ON app.t TO readers;
GRANT INSERT (name) ON app.t TO readers;
GRANT EXECUTE ON FUNCTION app.foo(int) TO readers;
ALTER DEFAULT PRIVILEGES FOR ROLE app_owner IN SCHEMA app GRANT SELECT ON TABLES TO readers;
```

REVOKE statements are **rejected in source** — revokes come from the
diff against the catalog.

## Drift policy

Catalog grants to roles **not declared in source** are tolerated and
surface as a `grants-to-unmanaged-role` warning, never silently
revoked. This protects out-of-band workflows (e.g., temp consultant
grants) while still surfacing the drift so operators can decide.

## Cluster-link option

Optional `[cluster]` block in `pgevolve.toml`:

```toml
[cluster]
project = "../my-cluster"
```

When set, the `grant-references-unknown-role` lint cross-checks
every grantee role name (in grants, owners, default-priv targets)
against the linked cluster project's `roles/*.sql`. Missing role →
Error severity, catching typos pre-apply.

When absent, per-DB independence preserved.

## Ownership semantics

- `owner: None` — unmanaged. The differ ignores ownership for this
  object; shadow-validate also ignores any catalog-side owner.
- `owner: Some(role)` — managed. Diff emits
  `ALTER <KIND> ... OWNER TO role` when catalog disagrees.

Source authors opt in per-object by writing an `ALTER ... OWNER TO`
statement. Omitting it leaves the object unmanaged.

## Passwords

Passwords are **not modeled**. v0.3.0's cluster surface already says
this for roles; this sub-spec inherits the same stance for object-level
permissions.

## Lint rules

- `grants-to-unmanaged-role` (warning, waivable) — catalog has grants
  to roles not in source.
- `revoke-from-owner` (error, non-waivable) — diff would REVOKE from
  the object's owner. PG silently rejects such REVOKEs; we pre-empt.
- `grant-references-unknown-role` (error, when `[cluster].project` is set)
  — grantee not declared in the linked cluster source.

## Out of scope

- DATABASE, TABLESPACE, LANGUAGE, FOREIGN TABLE grants — cluster-level
  or unmanaged.
- LARGE OBJECT grants — not declarative.
- Row-level security policies — v0.3.2.
