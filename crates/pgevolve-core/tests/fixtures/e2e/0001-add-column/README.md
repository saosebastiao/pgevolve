# Fixture 0001 — add column

Smallest non-trivial migration: starting from a single-table schema, the
final source adds one nullable column (`display_name text`) between
existing columns.

Expected plan: one `ALTER TABLE app.users ADD COLUMN display_name text;`
step in a single transactional group; no destructive intents.

This fixture's full plan-and-apply harness lands in Phase 9 once the CLI
has a `plan` command. Phase 8 ships the fixture format and one seed.
