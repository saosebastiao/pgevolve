-- @pgevolve plan id=42ec0ff20d0f5680 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_column_type destructive=true intent_id=1 targets=app.events
ALTER TABLE app.events ALTER COLUMN count TYPE integer;
COMMIT;

