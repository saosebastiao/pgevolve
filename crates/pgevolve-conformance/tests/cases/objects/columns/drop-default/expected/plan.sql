-- @pgevolve plan id=24e9aec83539e562 version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_column_default destructive=false targets=app.items
ALTER TABLE app.items ALTER COLUMN priority DROP DEFAULT;
COMMIT;

