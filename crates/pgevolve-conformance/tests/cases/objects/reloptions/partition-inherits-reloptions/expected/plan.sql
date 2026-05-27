-- @pgevolve plan id=c13d4fa27a1a7fb7 version=0.3.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_table_storage destructive=false targets=app.child_us
ALTER TABLE app.child_us SET (fillfactor = 80);
COMMIT;

