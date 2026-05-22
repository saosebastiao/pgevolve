-- @pgevolve plan id=f059b6cc871d9631 version=0.2.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_column_type destructive=false targets=app.events
ALTER TABLE app.events ALTER COLUMN count TYPE bigint;
COMMIT;

