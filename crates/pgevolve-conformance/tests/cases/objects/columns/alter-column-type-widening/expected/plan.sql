-- @pgevolve plan id=b9f521050a5b0c2e version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_column_type destructive=false targets=app.events
ALTER TABLE app.events ALTER COLUMN count TYPE bigint;
COMMIT;

