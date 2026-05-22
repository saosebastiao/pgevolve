-- @pgevolve plan id=17b409a8b1b023ce version=0.2.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_column_default destructive=false targets=app.items
ALTER TABLE app.items ALTER COLUMN priority SET DEFAULT 0;
COMMIT;

