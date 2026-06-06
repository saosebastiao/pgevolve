-- @pgevolve plan id=d42d8325c01c0cac version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_table_storage destructive=false targets=app.child_us
ALTER TABLE app.child_us SET (fillfactor = 80);
COMMIT;

