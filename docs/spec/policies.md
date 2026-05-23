# Row-level security policies

pgevolve manages Postgres RLS declaratively. Tables carry:

- `rls_enabled: bool` — `ALTER TABLE t ENABLE/DISABLE ROW LEVEL SECURITY`.
- `rls_forced: bool` — `ALTER TABLE t FORCE/NO FORCE ROW LEVEL SECURITY`
  (applies even to the table owner).
- `policies: Vec<Policy>` — embedded; policies can't exist orphan.

## Source surface

```sql
CREATE POLICY author_only ON app.docs
    AS PERMISSIVE              -- (default)
    FOR ALL                    -- (default)
    TO public                  -- (default)
    USING (author = current_user);

ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
ALTER TABLE app.docs FORCE ROW LEVEL SECURITY;
```

`ALTER POLICY` and `DROP POLICY` are **rejected in source** — both
come from the diff against the catalog.

## Command-kind changes recreate

PG's `ALTER POLICY` can change roles, USING, and WITH CHECK but
NOT the command kind. If source changes a policy from `FOR SELECT`
to `FOR INSERT`, pgevolve emits `DROP POLICY` + `CREATE POLICY` as
two separate plan steps.

## Cross-cluster role validation

Policy `TO` clauses reference roles. The v0.3.1 cross-cluster lint
`grant-references-unknown-role` extends to policy roles when
`[cluster].project` is set in `pgevolve.toml`.

## FORCE without policies = denial

PG's behavior: `FORCE ROW LEVEL SECURITY` with no policies defined
denies every row, including for the table owner. Almost always a
configuration mistake. The `force-rls-without-policies` lint warns
on this state.

## WITH CHECK validity

`WITH CHECK` is invalid on `FOR SELECT` and `FOR DELETE` policies
(PG rejects). The source parser pre-empts with a clear error.

## Out of scope

- `ALTER POLICY ... RENAME TO ...` in source — rejected.
  Operators can drop+create.
- `SECURITY LABEL` — not planned.
- `leakproof` / `security_barrier` on views — future.
