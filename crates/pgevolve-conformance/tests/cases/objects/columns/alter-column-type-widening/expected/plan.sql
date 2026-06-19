-- @pgevolve plan id=9cc6d2cf1fb67cb8 version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_column_type destructive=false targets=app.events
ALTER TABLE app.events ALTER COLUMN count TYPE bigint;
COMMIT;

