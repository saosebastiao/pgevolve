-- @pgevolve plan id=4960131e4896ceff version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_column_default destructive=false targets=app.items
ALTER TABLE app.items ALTER COLUMN priority SET DEFAULT 0;
COMMIT;

