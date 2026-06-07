-- @pgevolve plan id=1f060fa9832c73ef version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_column_type destructive=false targets=app.events
ALTER TABLE app.events ALTER COLUMN count TYPE bigint;
COMMIT;

