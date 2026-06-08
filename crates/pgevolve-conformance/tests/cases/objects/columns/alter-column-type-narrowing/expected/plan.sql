-- @pgevolve plan id=399ec579ed1e990e version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_column_type destructive=true intent_id=1 targets=app.events
ALTER TABLE app.events ALTER COLUMN count TYPE integer;
COMMIT;

